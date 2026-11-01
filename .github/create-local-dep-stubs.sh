#!/usr/bin/env bash
# CI helper for local path dependencies.
#
# - obrain-* and vite-rs: minimal STUBS, so cargo can resolve Cargo.toml. Their
#   features are never activated in CI (--features all,wami, not --all-features).
# - wami (Lsh0x/wami, followed on `main`, ADR D3): a REAL sparse clone, because
#   the `wami` feature IS tested in CI and a stub cannot provide its API. The
#   75 MB coverage.json tracked upstream is excluded by the blob filter.

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

# wami (public, followed on main; local worktree in dev at ../../obrain/wami-main)
WAMI_DIR="$REPO_ROOT/../../obrain/wami-main"
if [ ! -d "$WAMI_DIR/crates/wami" ]; then
  mkdir -p "$(dirname "$WAMI_DIR")"
  if git clone --depth 1 --branch main --filter=blob:none --sparse \
       https://github.com/Lsh0x/wami.git "$WAMI_DIR"; then
    git -C "$WAMI_DIR" sparse-checkout set crates Cargo.toml Cargo.lock
  else
    echo "sparse clone failed, falling back to a plain shallow clone"
    rm -rf "$WAMI_DIR"
    git clone --depth 1 --branch main https://github.com/Lsh0x/wami.git "$WAMI_DIR"
  fi
  echo "  clone: wami main @ $(git -C "$WAMI_DIR" rev-parse --short HEAD)"
fi

echo "Local dep stubs ready."
