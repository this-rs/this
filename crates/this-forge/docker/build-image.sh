#!/bin/sh
# Construit this-forge-builder:1.93 avec le SDK courant (source unique : crates/this-module-sdk).
# Usage : crates/this-forge/docker/build-image.sh [tag]
set -e
here=$(cd "$(dirname "$0")" && pwd)
sdk="$here/../../this-module-sdk"
rm -rf "$here/sdk"; mkdir -p "$here/sdk"
cp "$sdk/Cargo.toml" "$here/sdk/"; cp -R "$sdk/src" "$here/sdk/src"
docker build -t "${1:-this-forge-builder:1.93}" -f "$here/Dockerfile" "$here"
