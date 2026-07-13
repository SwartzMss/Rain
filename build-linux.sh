#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "==> Building embedded frontend..."
cd "$ROOT/frontend"
npm ci
npm run build

if [ ! -f "$ROOT/frontend/dist/index.html" ]; then
  echo "frontend/dist/index.html was not generated" >&2
  exit 1
fi

echo "==> Building Rain executable..."
cd "$ROOT/backend"
cargo fmt --check
cargo test --locked
cargo build --release --locked

mkdir -p "$ROOT/release"
cp "$ROOT/backend/target/release/backend" "$ROOT/release/rain"
cp "$ROOT/backend/.env.example" "$ROOT/release/.env"
chmod +x "$ROOT/release/rain"

echo
echo "Build completed. Keep both files together:"
echo "$ROOT/release/rain"
echo "$ROOT/release/.env"
