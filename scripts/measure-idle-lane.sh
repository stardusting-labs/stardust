#!/usr/bin/env bash
# Idle RSS of a baseline build (BEFORE) and the working tree (AFTER), tray only.
#
# Usage: scripts/measure-idle-lane.sh [BASELINE_REV]
#
# BASELINE_REV defaults to 023d33e. It is unpacked with git archive into a
# temp dir and built there. The working tree is built in place. Every run
# gets a fresh config file and XDG_CONFIG_HOME, hides the character at
# 32,32, uses a 24h interval unless the scenario says otherwise, idles
# IDLE_SECONDS (default 60), and keeps the median of five VmRSS samples
# taken one second apart.
#
#   signed-out      sign-out first; STARDUST_SERVER_URL is a closed port
#   signed-in-mock  device sign-in against scripts/mock_orbit.rs, 1m checks
#   after-flags-on  AFTER only; signed in when possible, with auto-sign-in,
#                   jittered-backoff, and routing turned on
#   demo-1m         --demo with 1m checks: sample items every minute through
#                   the same check, bubble, and routing code, no keychain.
#                   Stands in for signed-in-mock where no keychain exists.
#   demo-1m-flags   AFTER only; demo-1m with the three features on, compared
#                   with the demo-1m BEFORE run
#
# Each scenario runs RUNS times (default 3), each with a fresh config, and
# keeps the median of the run medians: one run can land a few hundred KiB
# off, depending on which threads made their first allocation first.
#
# A scenario fails when AFTER uses more than 1024 KiB or 10 % more than
# BEFORE. The script prints OVERALL PASS or OVERALL FAIL and exits 0.
#
# sign-in and sign-out use the keychain entry a real Stardust sign-in uses,
# so an existing ORBIT sign-in on this machine is removed. Signed-in runs
# start on the current session bus so the app reads the token that sign-in
# stored; the other runs use dbus-run-session when it is installed.
# The mock token lasts 5 minutes, so keep IDLE_SECONDS under about 280.
# The baseline build goes under TMPDIR (default /tmp). Linux only (/proc).
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

if (( $# > 1 )) || [[ "${1:-}" == -h || "${1:-}" == --help ]]; then
  echo "usage: scripts/measure-idle-lane.sh [BASELINE_REV]" >&2
  exit 2
fi
baseline=${1:-023d33e}
idle_seconds=${IDLE_SECONDS:-60}
runs=${RUNS:-3}
if [[ ! "$idle_seconds" =~ ^[0-9]+$ ]]; then
  echo "IDLE_SECONDS must be a whole number of seconds" >&2
  exit 2
fi
if [[ ! "$runs" =~ ^[1-9][0-9]*$ ]]; then
  echo "RUNS must be a positive whole number" >&2
  exit 2
fi
if [[ ! -r /proc/self/status ]]; then
  echo "this script reads /proc and runs on Linux only" >&2
  exit 2
fi
if ! git rev-parse --verify --quiet "${baseline}^{commit}" >/dev/null; then
  echo "unknown baseline revision $baseline" >&2
  exit 2
fi

workdir=$(mktemp -d)
mock_bin="$workdir/mock_orbit"
before_bin="$workdir/before-target/release/stardust"
after_bin="$workdir/after/stardust"
closed_server="http://127.0.0.1:9"

app_wrapper=""
app_child=""
mock_pid=""
mock_port=""
signed_in_bin=""
run_extra=()
run_seq=0

stop_app() {
  if [[ -n "$app_child" ]]; then
    kill "$app_child" 2>/dev/null || true
    local i
    for ((i = 0; i < 25; i++)); do
      if ! kill -0 "$app_child" 2>/dev/null; then
        break
      fi
      sleep 0.2
    done
    if kill -0 "$app_child" 2>/dev/null; then
      kill -9 "$app_child" 2>/dev/null || true
    fi
  fi
  if [[ -n "$app_wrapper" ]]; then
    kill "$app_wrapper" 2>/dev/null || true
    wait "$app_wrapper" 2>/dev/null || true
  fi
  app_child=""
  app_wrapper=""
}

stop_mock() {
  if [[ -n "$mock_pid" ]]; then
    kill "$mock_pid" 2>/dev/null || true
    wait "$mock_pid" 2>/dev/null || true
  fi
  mock_pid=""
  mock_port=""
}

cleanup() {
  stop_app
  stop_mock
  if [[ -n "$signed_in_bin" ]]; then
    "$signed_in_bin" sign-out >/dev/null 2>&1 || true
  fi
  rm -rf "$workdir"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

echo "baseline $baseline ($(git rev-parse --short "${baseline}^{commit}"))"
echo "idle ${idle_seconds}s per run, five VmRSS samples one second apart, $runs runs per scenario"
echo "note: sign-in and sign-out use the system keychain; an existing Stardust sign-in is removed"

echo
echo "== build mock ORBIT"
rustc --edition 2021 -O "$root/scripts/mock_orbit.rs" -o "$mock_bin"

echo "== build AFTER (working tree)"
cargo build --release --locked --bin stardust
after_target=${CARGO_TARGET_DIR:-target}
if [[ "$after_target" != /* ]]; then
  after_target="$root/$after_target"
fi
# A private copy, so a rebuild in target/ during the run changes nothing.
mkdir -p "$workdir/after"
cp "$after_target/release/stardust" "$after_bin"

echo "== build BEFORE ($baseline)"
mkdir -p "$workdir/before"
git archive "$baseline" | tar -x -C "$workdir/before"
(cd "$workdir/before" && CARGO_TARGET_DIR="$workdir/before-target" cargo build --release --locked --bin stardust)

# Escape a literal string for pgrep's extended regex.
regex_quote() {
  printf '%s' "$1" | sed 's/[][\.*^$+?(){}|]/\\&/g'
}

# The helpers below read run_bin, run_dir, run_config, run_xdg, and
# run_server from the measure call that is in progress.
run_cli() {
  XDG_CONFIG_HOME="$run_xdg" STARDUST_SERVER_URL="$run_server" \
    "$run_bin" --config "$run_config" "$@"
}

sign_out_quietly() {
  if ! run_cli sign-out >/dev/null 2>&1; then
    echo "note: sign-out could not use the keychain; continuing"
  fi
}

sign_in() {
  local limit=()
  if command -v timeout >/dev/null 2>&1; then
    limit=(timeout 30)
  fi
  XDG_CONFIG_HOME="$run_xdg" STARDUST_SERVER_URL="$run_server" \
    ${limit[@]+"${limit[@]}"} "$run_bin" --config "$run_config" sign-in \
    >"$run_dir/sign-in.txt" 2>&1
}

start_mock() {
  local ready='^listening 127\.0\.0\.1:([0-9]+)$' line i
  "$mock_bin" 0 >"$run_dir/mock.out" 2>"$run_dir/mock.log" &
  mock_pid=$!
  for ((i = 0; i < 50; i++)); do
    # Wait for a whole line, newline included.
    if [[ -s "$run_dir/mock.out" ]] && (( $(wc -l <"$run_dir/mock.out") >= 1 )); then
      line=$(head -n 1 "$run_dir/mock.out")
      if [[ "$line" =~ $ready ]]; then
        mock_port=${BASH_REMATCH[1]}
        return 0
      fi
      break
    fi
    if ! kill -0 "$mock_pid" 2>/dev/null; then
      break
    fi
    sleep 0.1
  done
  stop_mock
  return 1
}

count_requests() {
  grep -c -x -F "$1" "$run_dir/mock.log" 2>/dev/null || true
}

# private: dbus-run-session when installed. current: the caller's session bus.
launch() {
  if [[ "$1" == private ]] && command -v dbus-run-session >/dev/null 2>&1; then
    XDG_CONFIG_HOME="$run_xdg" STARDUST_SERVER_URL="$run_server" \
      dbus-run-session -- "$run_bin" --config "$run_config" ${run_extra[@]+"${run_extra[@]}"} \
      >"$run_dir/stdout.txt" 2>"$run_dir/stderr.txt" &
  else
    XDG_CONFIG_HOME="$run_xdg" STARDUST_SERVER_URL="$run_server" \
      "$run_bin" --config "$run_config" ${run_extra[@]+"${run_extra[@]}"} \
      >"$run_dir/stdout.txt" 2>"$run_dir/stderr.txt" &
  fi
  app_wrapper=$!
}

# The app itself, not the dbus-run-session wrapper whose command line also
# contains "$run_bin --config $run_config".
find_child() {
  local pattern i
  pattern="^$(regex_quote "$run_bin --config $run_config")"
  for ((i = 0; i < 50; i++)); do
    app_child=$(pgrep -n -f "$pattern" || true)
    if [[ -n "$app_child" ]]; then
      return 0
    fi
    if ! kill -0 "$app_wrapper" 2>/dev/null; then
      return 1
    fi
    sleep 0.2
  done
  return 1
}

show_stderr() {
  echo "--- stardust stderr (last 20 lines) ---" >&2
  tail -n 20 "$run_dir/stderr.txt" 2>/dev/null \
    | sed -E 's/(Bearer )[^[:space:]]+/\1<redacted>/g' >&2 || true
}

finish_run() {
  stop_app
  if [[ "$1" == yes ]]; then
    if ! run_cli sign-out >/dev/null 2>&1; then
      echo "note: sign-out after the run failed"
    fi
    signed_in_bin=""
  fi
  stop_mock
}

# measure SCENARIO SIDE BIN MODE
#   MODE is signed-out, signed-in, flags-on, demo, or demo-flags.
# Sets measured_kib (empty when there is no sample), measured_state (ok,
# skipped, or failed), and measured_signed_in (yes or no).
measure() {
  local scenario=$1 side=$2 run_bin=$3 mode=$4
  run_seq=$((run_seq + 1))
  local run_dir="$workdir/run-$scenario-$side-$run_seq"
  local run_config="$run_dir/config.json"
  local run_xdg="$run_dir/xdg"
  local run_server=$closed_server
  local signed_in=no bus=private reason
  measured_kib=""
  measured_state=failed
  measured_signed_in=no
  run_extra=()
  mkdir -p "$run_xdg"
  printf '\n== %s %s\n' "$scenario" "$side"

  if [[ "$mode" == signed-out || "$mode" == demo* ]]; then
    sign_out_quietly
  fi
  run_cli schedule set --interval 24h --timezone UTC >/dev/null
  run_cli character hide >/dev/null
  run_cli character position 32 32 >/dev/null
  if [[ "$mode" == demo* ]]; then
    run_extra=(--demo)
    # Sample items every minute, so a check lands in the idle window.
    run_cli schedule set --interval 1m >/dev/null
  fi

  if [[ "$mode" == signed-in || "$mode" == flags-on ]]; then
    if start_mock; then
      run_server="http://127.0.0.1:$mock_port"
      if sign_in; then
        signed_in=yes
      else
        reason=$(tail -n 1 "$run_dir/sign-in.txt" 2>/dev/null || true)
        if [[ "$mode" == signed-in ]]; then
          echo "keychain unavailable: scenario B skipped"
        else
          echo "keychain unavailable: scenario C runs signed out"
        fi
        if [[ -n "$reason" ]]; then
          printf '  sign-in: %s\n' "$reason"
        fi
      fi
    elif [[ "$mode" == signed-in ]]; then
      echo "mock ORBIT did not start: scenario B skipped"
    else
      echo "mock ORBIT did not start: scenario C runs signed out"
    fi

    if [[ "$signed_in" == yes ]]; then
      signed_in_bin=$run_bin
      bus=current
      # Checks every minute, so some land in the idle window.
      run_cli schedule set --interval 1m >/dev/null
    else
      stop_mock
      run_server=$closed_server
      if [[ "$mode" == signed-in ]]; then
        measured_state=skipped
        return 0
      fi
      sign_out_quietly
    fi
  fi
  measured_signed_in=$signed_in

  if [[ "$mode" == flags-on || "$mode" == demo-flags ]]; then
    local flag
    for flag in auto-sign-in jittered-backoff routing; do
      if ! run_cli feature "$flag" on >"$run_dir/feature-$flag.txt" 2>&1; then
        printf 'note: feature %s on failed, continuing: %s\n' \
          "$flag" "$(tail -n 1 "$run_dir/feature-$flag.txt" 2>/dev/null || true)"
      fi
    done
  fi

  # A fresh config opens the first-run settings window. Mark it seen so the
  # run stays tray only.
  if grep -q '"onboarded": false' "$run_config"; then
    sed -i 's/"onboarded": false/"onboarded": true/' "$run_config"
  fi

  if [[ "$bus" == current ]]; then
    echo "signed in; launched on the current session bus"
  elif [[ "$mode" == demo* ]]; then
    echo "demo items every minute; launched under dbus-run-session when installed"
  elif command -v dbus-run-session >/dev/null 2>&1; then
    echo "signed out; launched under dbus-run-session"
  else
    echo "signed out; launched without dbus-run-session"
  fi

  launch "$bus"
  if ! find_child; then
    echo "stardust exited before the idle sample" >&2
    show_stderr
    finish_run "$signed_in"
    return 0
  fi
  sleep "$idle_seconds"
  if ! kill -0 "$app_child" 2>/dev/null; then
    echo "stardust exited before the idle sample" >&2
    show_stderr
    finish_run "$signed_in"
    return 0
  fi

  local samples=() rss i
  for ((i = 1; i <= 5; i++)); do
    rss=$(awk '/^VmRSS:/ { print $2 }' "/proc/$app_child/status" 2>/dev/null || true)
    if [[ -z "$rss" ]]; then
      break
    fi
    samples+=("$rss")
    if (( i < 5 )); then
      sleep 1
    fi
  done
  if (( ${#samples[@]} < 5 )); then
    echo "stardust exited during sampling" >&2
    show_stderr
    finish_run "$signed_in"
    return 0
  fi

  measured_kib=$(printf '%s\n' "${samples[@]}" | sort -n | awk 'NR == 3 { print $1 }')
  measured_state=ok
  printf 'pid %s\n' "$app_child"
  printf 'rss_kib %s\n' "${samples[*]}"
  # Heap and stacks against mapped files (the binary and libraries), to tell
  # allocations from a larger or differently laid out binary.
  awk '/^(RssAnon|RssFile|RssShmem):/ { printf "%s %s ", tolower($1), $2 } END { print "" }' \
    "/proc/$app_child/status" 2>/dev/null || true
  printf 'binary_bytes %s\n' "$(stat -c %s "$run_bin" 2>/dev/null || echo -)"
  awk -v kib="$measured_kib" \
    'BEGIN { printf "median_rss_kib %d\nmedian_rss_mib %.1f\n", kib, kib / 1024 }'
  if [[ "$signed_in" == yes ]]; then
    printf 'mock requests: status %s, alerts %s\n' \
      "$(count_requests "GET /api/pqc/v1/desktop/status 200")" \
      "$(count_requests "GET /api/pqc/v1/desktop/alerts 200")"
  fi
  finish_run "$signed_in"
  return 0
}

# measure_runs SCENARIO SIDE BIN MODE: measure RUNS times and keep the
# median of the run medians in measured_kib.
measure_runs() {
  local values=() i
  for ((i = 1; i <= runs; i++)); do
    measure "$1" "$2" "$3" "$4"
    if [[ "$measured_state" == skipped ]]; then
      return 0
    fi
    if [[ "$measured_state" == ok ]]; then
      values+=("$measured_kib")
    fi
  done
  if (( ${#values[@]} == 0 )); then
    measured_kib=""
    measured_state=failed
    return 0
  fi
  measured_kib=$(printf '%s\n' "${values[@]}" | sort -n \
    | awk '{ v[NR] = $1 } END { print v[int((NR + 1) / 2)] }')
  measured_state=ok
  printf '%s %s: run medians %s KiB, median %s KiB\n' "$1" "$2" "${values[*]}" "$measured_kib"
}

declare -A result_kib=() result_state=()
record() {
  result_kib["$1/$2"]=$measured_kib
  result_state["$1/$2"]=$measured_state
}

measure_runs signed-out before "$before_bin" signed-out
record signed-out before
measure_runs signed-out after "$after_bin" signed-out
record signed-out after
measure_runs signed-in-mock before "$before_bin" signed-in
record signed-in-mock before
measure_runs signed-in-mock after "$after_bin" signed-in
record signed-in-mock after
measure_runs after-flags-on after "$after_bin" flags-on
record after-flags-on after
flags_signed_in=$measured_signed_in
measure_runs demo-1m before "$before_bin" demo
record demo-1m before
measure_runs demo-1m after "$after_bin" demo
record demo-1m after
measure_runs demo-1m-flags after "$after_bin" demo-flags
record demo-1m-flags after

# after-flags-on is compared with the signed-in BEFORE run when it signed in
# and that run has a sample, and with the signed-out BEFORE run otherwise.
flags_baseline=signed-out
if [[ "$flags_signed_in" == yes && "${result_state[signed-in-mock/before]:-}" == ok ]]; then
  flags_baseline=signed-in-mock
fi

overall=PASS
row_format='%-16s %10s %10s %10s %9s  %s\n'

table_row() {
  local scenario=$1 before_key=$2 after_key=$3
  local before=${result_kib[$before_key]:-} after=${result_kib[$after_key]:-}
  local before_state=${result_state[$before_key]:-failed}
  local after_state=${result_state[$after_key]:-failed}
  local delta pct verdict
  if [[ "$before_state" == skipped || "$after_state" == skipped ]]; then
    # shellcheck disable=SC2059
    printf "$row_format" "$scenario" "${before:--}" "${after:--}" - - "SKIP (not signed in)"
    return 0
  fi
  if [[ -z "$before" || -z "$after" ]]; then
    # shellcheck disable=SC2059
    printf "$row_format" "$scenario" "${before:--}" "${after:--}" - - "FAIL (no sample)"
    overall=FAIL
    return 0
  fi
  read -r delta pct verdict < <(awk -v b="$before" -v a="$after" 'BEGIN {
    d = a - b
    p = (b > 0) ? d * 100 / b : 0
    v = (d > 1024 || p > 10) ? "FAIL" : "PASS"
    printf "%+d %+.1f%% %s\n", d, p, v
  }')
  # shellcheck disable=SC2059
  printf "$row_format" "$scenario" "$before" "$after" "$delta" "$pct" "$verdict"
  if [[ "$verdict" == FAIL ]]; then
    overall=FAIL
  fi
}

echo
# shellcheck disable=SC2059
printf "$row_format" scenario before_kib after_kib delta_kib delta_pct verdict
table_row signed-out signed-out/before signed-out/after
table_row signed-in-mock signed-in-mock/before signed-in-mock/after
table_row after-flags-on "$flags_baseline/before" after-flags-on/after
table_row demo-1m demo-1m/before demo-1m/after
table_row demo-1m-flags demo-1m/before demo-1m-flags/after
echo
echo "after-flags-on before_kib is the $flags_baseline BEFORE run."
echo "demo-1m-flags before_kib is the demo-1m BEFORE run."
echo "OVERALL $overall"
exit 0
