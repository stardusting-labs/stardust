#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
"$root/scripts/bootstrap-ci-tools.sh"
export PATH="$root/.ci-tools/bin:$PATH"
gitleaks detect --source . --config .gitleaks.toml --redact
