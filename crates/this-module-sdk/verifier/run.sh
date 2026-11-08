#!/bin/sh
# Compile l'exemple (ou le crate donné) en wasm puis le contrôle contre l'ABI v1.
# Usage : crates/this-module-sdk/verifier/run.sh [crate]   (défaut : this-module-taches)
set -e
cd "$(dirname "$0")/../../.."
crate="${1:-this-module-taches}"
cargo build --release --target wasm32-unknown-unknown -p "$crate"
node crates/this-module-sdk/verifier/verify.mjs "target/wasm32-unknown-unknown/release/$(echo "$crate" | tr - _).wasm"
