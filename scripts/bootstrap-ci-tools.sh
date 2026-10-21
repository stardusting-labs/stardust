#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
bindir="$root/.ci-tools/bin"
mkdir -p "$bindir"

install_gitleaks() {
  local version="8.30.1"
  local sha="551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb"
  if [[ -x "$bindir/gitleaks" ]] && "$bindir/gitleaks" version 2>/dev/null | grep -q "$version"; then
    return
  fi
  local tmp
  tmp=$(mktemp -d)
  curl -fsSL -o "$tmp/gitleaks.tar.gz" \
    "https://github.com/gitleaks/gitleaks/releases/download/v${version}/gitleaks_${version}_linux_x64.tar.gz"
  echo "$sha  $tmp/gitleaks.tar.gz" | sha256sum -c -
  tar -xzf "$tmp/gitleaks.tar.gz" -C "$tmp"
  install -m 0755 "$tmp/gitleaks" "$bindir/gitleaks"
  rm -rf "$tmp"
}

install_cargo_deny() {
  local version="0.20.2"
  local sha="9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f"
  if [[ -x "$bindir/cargo-deny" ]] && "$bindir/cargo-deny" --version 2>/dev/null | grep -q "$version"; then
    return
  fi
  local tmp
  tmp=$(mktemp -d)
  curl -fsSL -o "$tmp/cargo-deny.tar.gz" \
    "https://github.com/EmbarkStudios/cargo-deny/releases/download/${version}/cargo-deny-${version}-x86_64-unknown-linux-musl.tar.gz"
  echo "$sha  $tmp/cargo-deny.tar.gz" | sha256sum -c -
  tar -xzf "$tmp/cargo-deny.tar.gz" -C "$tmp"
  install -m 0755 "$tmp/cargo-deny-${version}-x86_64-unknown-linux-musl/cargo-deny" "$bindir/cargo-deny"
  rm -rf "$tmp"
}

install_gitleaks
install_cargo_deny
