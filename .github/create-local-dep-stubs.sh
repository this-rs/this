#!/usr/bin/env bash
# CI helper: create minimal stubs for local-only path dependencies
# (obrain-*, vite-rs) so cargo can resolve Cargo.toml without them
# being checked out. These features are NEVER activated in CI
# (we use --features all,wami, not --all-features).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# obrain crates (private, local-only)
OBRAIN_DIR="$REPO_ROOT/../banc-obrain-j4/crates"
for crate in obrain-engine obrain-common obrain-core obrain-substrate obrain-adapters obrain-reactive; do
  dir="$OBRAIN_DIR/$crate"
  if [ ! -d "$dir" ]; then
    mkdir -p "$dir/src"
    cat > "$dir/Cargo.toml" <<EOF
[package]
name = "$crate"
version = "0.0.1"
edition = "2021"

[features]
default = []
algos = []
cognitive-full = []
EOF
    touch "$dir/src/lib.rs"
    echo "  stub: $crate"
  fi
done

# vite-rs (public, but local checkout in dev)
VITE_DIR="$REPO_ROOT/../vite-rs"
if [ ! -d "$VITE_DIR" ]; then
  mkdir -p "$VITE_DIR/src"
  cat > "$VITE_DIR/Cargo.toml" <<EOF
[package]
name = "this-vite"
version = "0.0.1"
edition = "2021"
EOF
  touch "$VITE_DIR/src/lib.rs"
  echo "  stub: this-vite"
fi

echo "Local dep stubs ready."
