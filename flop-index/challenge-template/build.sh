#!/bin/sh
# Build every example rule to out/<name>.wasm.
# Needs: rustup target add wasm32-unknown-unknown
set -eu
cd "$(dirname "$0")"
cargo build --release --locked --target wasm32-unknown-unknown --examples
mkdir -p out
for f in target/wasm32-unknown-unknown/release/examples/*.wasm; do
  case "$(basename "$f")" in *-*) continue ;; esac   # skip cargo's hashed copies
  cp "$f" out/
done
ls -l out/*.wasm
