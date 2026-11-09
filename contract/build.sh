#!/usr/bin/env bash
# Builds the Axiom Pay contract to contract/target/near/axiom_pay.wasm (no cargo-near needed).
set -euo pipefail
cd "$(dirname "$0")"
rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true
# `--cfg near` is what cargo-near sets: without it near-sdk turns contract panics into bare wasm traps.
RUSTFLAGS="--cfg near -C link-arg=-s" cargo build --target wasm32-unknown-unknown --release
mkdir -p target/near
cp target/wasm32-unknown-unknown/release/axiom_pay.wasm target/near/axiom_pay.wasm
ls -l target/near/axiom_pay.wasm
