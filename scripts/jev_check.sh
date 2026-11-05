#!/usr/bin/env bash
# Advisory Jev check for coding agents. Fail closed. Never print JEV_TYPESAFE_KEY.
#
# xtask rust-only rejects tracked Go, TypeScript, and Python source. A Rust
# HTTPS client would be a new crate; under apps/ or crates/ it would enter the
# licence workflow and would still leave xtask/src/main.rs untouched only if it
# lived elsewhere. This shell entrypoint is the form that gate allows without
# editing xtask/src/main.rs or deny.toml. Node encodes and decodes JSON because
# repository-boundary CI already requires it. curl performs the request.
#
# Usage: scripts/jev_check.sh --stage pre-plan|pre-pr < summary.txt
# Exit 0 verdict ok. Exit 3 verdict escalate (score >= 0.5, any error, missing key).
#
# JEV_TYPESAFE_URL default: https://api.typesafe.ai/v1/systemone
# JEV_TYPESAFE_MODEL default: jev-1.13.0
# Those defaults are the 2026-10-21 client notes. This checkout has no live key
# to confirm them. Override both with the environment variables above.
# Loopback http://127.0.0.1:PORT/ or http://localhost:PORT/ is accepted so
# offline tests can stub the API. Every other URL must be https.
# ORBIT_JEV_VERDICT_FILE default: /tmp/orbit-jev-verdict.json
# ORBIT_JEV_CLAIMS_FILE default: scripts/lib/forbidden-claims.mjs
set -euo pipefail

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
stage=""
verdict_file="${ORBIT_JEV_VERDICT_FILE:-/tmp/orbit-jev-verdict.json}"
claims_file="${ORBIT_JEV_CLAIMS_FILE:-$root/scripts/lib/forbidden-claims.mjs}"
summary_file=""
body_file=""
resp_file=""
err_file=""
printed=0

cleanup() {
  if [[ -n "$summary_file" ]]; then rm -f -- "$summary_file"; fi
  if [[ -n "$body_file" ]]; then rm -f -- "$body_file"; fi
  if [[ -n "$resp_file" ]]; then rm -f -- "$resp_file"; fi
  if [[ -n "$err_file" ]]; then rm -f -- "$err_file"; fi
  return 0
}
on_exit() {
  local status=$?
  cleanup
  if [[ "$printed" -eq 0 && "$status" -ne 0 ]]; then
    printf '{"verdict":"escalate","stage":"%s","scores":{"reason":"jev-error:internal"}}\n' "$stage"
    exit 3
  fi
  return 0
}
trap on_exit EXIT

emit() {
  local verdict="$1" scores="$2" line dir tmp
  printed=1
  line=$(printf '{"verdict":"%s","stage":"%s","scores":%s}\n' "$verdict" "$stage" "$scores")
  printf '%s' "$line"
  dir=$(dirname -- "$verdict_file")
  mkdir -p -- "$dir"
  tmp=$(mktemp "${dir}/orbit-jev-verdict.XXXXXX")
  chmod 600 "$tmp"
  printf '%s' "$line" >"$tmp"
  mv -f -- "$tmp" "$verdict_file"
  if [[ "$verdict" == "ok" ]]; then
    exit 0
  fi
  exit 3
}

usage() {
  cat <<'EOF'
usage: scripts/jev_check.sh --stage pre-plan|pre-pr < summary.txt

Reads a plain-text summary from stdin (1500 characters maximum).
Prints one JSON object. Exit 0 is ok. Exit 3 is escalate.
Does not print JEV_TYPESAFE_KEY.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --help | -h)
      usage
      exit 0
      ;;
    --stage)
      [[ $# -ge 2 ]] || emit escalate '{"reason":"input-rejected"}'
      stage=$2
      shift 2
      ;;
    *)
      emit escalate '{"reason":"input-rejected"}'
      ;;
  esac
done

case "$stage" in
  pre-plan | pre-pr) ;;
  *) emit escalate '{"reason":"input-rejected"}' ;;
esac

umask 077
summary_file=$(mktemp)
body_file=$(mktemp)
resp_file=$(mktemp)
err_file=$(mktemp)
cat >"$summary_file"

raw_bytes=$(wc -c <"$summary_file")
text_bytes=$(tr -d '\000' <"$summary_file" | wc -c)
[[ "$raw_bytes" -eq "$text_bytes" ]] || emit escalate '{"reason":"input-rejected"}'

summary=$(<"$summary_file")
summary=${summary//$'\r'/}
summary="${summary#"${summary%%[![:space:]]*}"}"
summary="${summary%"${summary##*[![:space:]]}"}"
if [[ -z "${summary//[[:space:]]/}" ]]; then
  emit escalate '{"reason":"input-rejected"}'
fi

if ! char_count=$(printf '%s' "$summary" | LC_ALL=C.UTF-8 wc -m 2>/dev/null); then
  char_count=$(printf '%s' "$summary" | LC_ALL=C wc -c)
fi
char_count=${char_count//[[:space:]]/}
[[ "$char_count" =~ ^[0-9]+$ ]] || emit escalate '{"reason":"input-rejected"}'
((char_count <= 1500)) || emit escalate '{"reason":"input-rejected"}'

printf '%s' "$summary" >"$summary_file"
if grep -q $'[\001-\010\013\014\016-\037]' "$summary_file"; then
  emit escalate '{"reason":"input-rejected"}'
fi

# Prefixes are concatenated so this source file does not contain a token shape.
live_prefix='sk_'"live_"
gh_prefix='gh'"p_"
pat_prefix='github_'"pat_"
if grep -Eq "^(diff --git|@@ |\\+\\+\\+ |--- a/)|BEGIN [A-Z ]*PRIVATE KEY|${live_prefix}|${gh_prefix}|${pat_prefix}|xox[baprs]-" "$summary_file"; then
  emit escalate '{"reason":"input-rejected"}'
fi

key=${JEV_TYPESAFE_KEY:-}
if [[ -z "${key//[[:space:]]/}" ]]; then
  key=""
fi
if [[ -n "$key" ]] && printf '%s' "$summary" | grep -Fq -- "$key"; then
  emit escalate '{"reason":"input-rejected"}'
fi
if [[ -z "$key" ]]; then
  emit escalate '{"reason":"key-unavailable"}'
fi

url=${JEV_TYPESAFE_URL:-https://api.typesafe.ai/v1/systemone}
model=${JEV_TYPESAFE_MODEL:-jev-1.13.0}
[[ "$model" =~ ^[A-Za-z0-9._-]{1,64}$ ]] || emit escalate '{"reason":"jev-error:model"}'

curl_proto=()
if [[ "$url" =~ ^https://[^[:space:]]+$ ]]; then
  curl_proto=(--proto '=https' --tlsv1.2)
elif [[ "$url" =~ ^http://(127\.0\.0\.1|localhost):[0-9]+/[^[:space:]]*$ ]]; then
  curl_proto=(--proto '=http')
else
  emit escalate '{"reason":"jev-error:url"}'
fi

[[ -f "$claims_file" ]] || emit escalate '{"reason":"claim-gate-unavailable"}'
mapfile -t claim_ids < <(grep -oE '\["[a-z0-9-]+"' "$claims_file" | sed -E 's/\["//; s/"$//' || true)
((${#claim_ids[@]} > 0)) || emit escalate '{"reason":"claim-gate-unavailable"}'
claim_csv=$(IFS=,; printf '%s' "${claim_ids[*]}")

command -v node >/dev/null 2>&1 || emit escalate '{"reason":"parser-unavailable"}'
command -v curl >/dev/null 2>&1 || emit escalate '{"reason":"jev-error:curl"}'

ORBIT_JEV_SUMMARY_FILE="$summary_file" \
  ORBIT_JEV_MODEL="$model" \
  ORBIT_JEV_CLAIM_IDS="$claim_csv" \
  ORBIT_JEV_BODY_FILE="$body_file" \
  node <<'NODE'
const fs = require("fs");
const summary = fs.readFileSync(process.env.ORBIT_JEV_SUMMARY_FILE, "utf8");
const model = process.env.ORBIT_JEV_MODEL;
const ids = process.env.ORBIT_JEV_CLAIM_IDS;
const suffix = " Answer only from the summary text; if the summary does not say, lean no.";
const questions = {
  security:
    "Does the summary describe a security or design risk a reviewer should resolve before merge?" +
    suffix,
  wording:
    "Does the summary describe the product using a forbidden claim rejected by scripts/check-forbidden-claims.sh (pattern ids: " +
    ids +
    ")?" +
    suffix,
  licence:
    "Does the summary indicate an AGPL, unlicensed, or other dependency that would fail the deny.toml licences allow-list in shipped code?" +
    suffix,
  block: "Should this draft be blocked from merging as described?" + suffix,
};
const body = {
  model,
  state: summary,
  questions: Object.fromEntries(
    Object.entries(questions).map(([id, instructions]) => [id, { type: "noul", instructions }]),
  ),
};
fs.writeFileSync(process.env.ORBIT_JEV_BODY_FILE, JSON.stringify(body));
NODE

if ! curl --silent --show-error --fail --max-time 60 \
  "${curl_proto[@]}" \
  -H "Authorization: Bearer ${key}" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json" \
  --data-binary @"$body_file" \
  -o "$resp_file" \
  "$url" 2>"$err_file"; then
  rm -f -- "$resp_file" "$err_file" "$body_file"
  resp_file=""
  err_file=""
  body_file=""
  emit escalate '{"reason":"jev-error:curl"}'
fi

set +e
parsed=$(
  ORBIT_JEV_RESPONSE_FILE="$resp_file" node <<'NODE'
const fs = require("fs");
let data;
try {
  data = JSON.parse(fs.readFileSync(process.env.ORBIT_JEV_RESPONSE_FILE, "utf8"));
} catch {
  process.exit(2);
}
const ids = ["security", "wording", "licence", "block"];
const scores = {};
for (const id of ids) {
  const answer = data && data.answers && data.answers[id];
  const noul = answer && answer.noul;
  if (typeof noul !== "number" || !Number.isFinite(noul) || noul < 0 || noul > 1) {
    process.exit(2);
  }
  scores[id] = noul;
}
process.stdout.write(JSON.stringify(scores));
NODE
)
parse_status=$?
set -e
[[ "$parse_status" -eq 0 ]] || emit escalate '{"reason":"jev-error:parse"}'

set +e
decision=$(node -e 'const scores = JSON.parse(process.argv[1]); const max = Math.max(...Object.values(scores)); if (!Number.isFinite(max)) process.exit(2); process.stdout.write(max >= 0.5 ? "escalate" : "ok");' "$parsed")
decision_status=$?
set -e
[[ "$decision_status" -eq 0 ]] || emit escalate '{"reason":"jev-error:parse"}'

if [[ "$decision" == "ok" ]]; then
  emit ok "$parsed"
fi
emit escalate "$parsed"
