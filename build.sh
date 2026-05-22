#!/usr/bin/env bash
# Build the static site into ./dist. Requires `wasm-pack` and `tailwindcss` in PATH.
set -euo pipefail

cd "$(dirname "$0")"

OUT=dist
mkdir -p "$OUT/pkg"

echo ">>> wasm-pack build (release)"
wasm-pack build --release --target web --out-dir "$OUT/pkg" --no-typescript --no-pack

echo ">>> tailwindcss"
tailwindcss -i web/style.css -o "$OUT/style.css" --minify

echo ">>> copy static assets"
cp web/index.html web/app.js "$OUT/"
cp -r web/vendor "$OUT/"

# Strip wasm-pack's gitignore so we can deploy the build artifact tree.
rm -f "$OUT/pkg/.gitignore"

echo
echo "Built $OUT/"
ls -lh "$OUT"
