#!/usr/bin/env bash
# Build the Cera playground into web/pkg (deploy ./web with Cloudflare Pages).
# Needs: rustup target add wasm32-unknown-unknown ; cargo install wasm-bindgen-cli
set -euo pipefail
cd "$(dirname "$0")"
cargo build -p cera-wasm --target wasm32-unknown-unknown --release
wasm-bindgen target/wasm32-unknown-unknown/release/cera_wasm.wasm \
  --target web --out-dir web/pkg --no-typescript
if command -v wasm-opt >/dev/null; then
  wasm-opt -Oz web/pkg/cera_wasm_bg.wasm -o web/pkg/cera_wasm_bg.wasm
fi
# Bundle the example programs for the playground's picker.
python3 - <<'PY'
import json, pathlib
ex = {p.stem: p.read_text() for p in sorted(pathlib.Path("examples").glob("*.cera"))
      if p.stem not in ("broken", "kilo", "wordcount")}
pathlib.Path("web/examples.json").write_text(json.dumps(ex))
PY
ls -la web/pkg
python3 tools/bundle-playground.py
