#!/bin/sh
# Regenerate the WebAssembly bundle embedded in sdo-server.
set -eu
cd "$(dirname "$0")/../app"
cargo build --release --locked --target wasm32-unknown-unknown --lib -p sdo-web -p sdo-arrowgene
wasm-bindgen --target web --no-typescript --out-dir ../web/pkg \
  "${CARGO_TARGET_DIR:-target}/wasm32-unknown-unknown/release/sdo_web.wasm"
wasm-bindgen --target web --no-typescript --out-dir ../web/pkg \
  "${CARGO_TARGET_DIR:-target}/wasm32-unknown-unknown/release/sdo_arrowgene.wasm"
