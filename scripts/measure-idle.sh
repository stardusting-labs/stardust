#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

cargo build --release --locked --bin stardust

workdir=$(mktemp -d)
trap 'kill "$pid" 2>/dev/null || true; rm -rf "$workdir"' EXIT
config="$workdir/config.json"
bin="$root/target/release/stardust"

"$bin" --config "$config" schedule set --interval 24h --timezone UTC >/dev/null
"$bin" --config "$config" character hide >/dev/null
"$bin" --config "$config" character position 32 32 >/dev/null

stderr="$workdir/stderr.txt"
if command -v dbus-run-session >/dev/null 2>&1; then
  dbus-run-session -- "$bin" --config "$config" >"$workdir/stdout.txt" 2>"$stderr" &
else
  "$bin" --config "$config" >"$workdir/stdout.txt" 2>"$stderr" &
fi
pid=$!
sleep 2

child=$(pgrep -n -f "$bin --config $config" || true)
if [[ -z "$child" ]] || ! kill -0 "$child" 2>/dev/null; then
  echo "stardust exited before the idle sample" >&2
  cat "$stderr" >&2 || true
  exit 1
fi

samples=()
for _ in 1 2 3 4 5; do
  rss=$(awk '/VmRSS:/ { print $2 }' "/proc/$child/status")
  samples+=("$rss")
  sleep 0.4
done

printf 'pid %s\n' "$child"
tr '\0' ' ' <"/proc/$child/cmdline"
printf '\n'
printf 'rss_kb'
for sample in "${samples[@]}"; do
  printf ' %s' "$sample"
done
printf '\n'
printf '%s\n' "${samples[@]}" | sort -n | awk 'NR==3 { printf "median_rss_kb %s\nmedian_rss_mib %.1f\n", $1, $1/1024 }'
echo "--- stderr ---"
cat "$stderr" || true
