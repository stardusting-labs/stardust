#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

if ! pkg-config --exists xkbcommon; then
  sudo apt-get update
  sudo apt-get install -y pkg-config libxkbcommon-dev libwayland-dev libx11-dev
fi

"$root/scripts/bootstrap-ci-tools.sh"
export PATH="$root/.ci-tools/bin:$PATH"

cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo deny check
"$root/scripts/secret-scan.sh"
