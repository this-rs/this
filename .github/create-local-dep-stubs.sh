#!/usr/bin/env bash
# CI helper for local path dependencies.
#
# - vite-rs: NO stub any more. `this-vite` is a pinned git dependency of the public
#   this-rs/vite-rs repository (Cargo.toml), fetched by cargo like any other crate.
# - wami (Lsh0x/wami, followed on `main`, ADR D3): a REAL shallow clone, because
#   the `wami` feature IS tested in CI and a stub cannot provide its API.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# wami (public, followed on main; local worktree in dev at ../../obrain/wami-main)
# REPO_ROOT is the repository's PARENT (see above): Cargo.toml's `../../obrain/wami-main`
# from the repo is `$REPO_ROOT/../obrain/wami-main`, like `$REPO_ROOT/../banc-obrain-j4`.
WAMI_DIR="$REPO_ROOT/../obrain/wami-main"
if [ ! -f "$WAMI_DIR/crates/wami/Cargo.toml" ]; then
  mkdir -p "$(dirname "$WAMI_DIR")"
  rm -rf "$WAMI_DIR"
  # A plain shallow clone: the sparse --no-cone variant left crates/ empty under
  # Git for Windows. The 75 MB coverage.json upstream is the price, once per job.
  git clone --depth 1 --branch main https://github.com/Lsh0x/wami.git "$WAMI_DIR"
  echo "  clone: wami main @ $(git -C "$WAMI_DIR" rev-parse --short HEAD)"
fi
# Fail HERE, with a readable message, rather than in cargo's manifest loader.
if [ ! -f "$WAMI_DIR/crates/wami/Cargo.toml" ]; then
  echo "wami clone incomplete: $WAMI_DIR/crates/wami/Cargo.toml missing" >&2
  exit 1
fi

echo "Local dep stubs ready."
