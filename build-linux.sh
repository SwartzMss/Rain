#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RAIN_RELEASE_VERSION="${RAIN_RELEASE_VERSION:-v0.0.1}"
export RAIN_RELEASE_VERSION
export VITE_APP_VERSION="${VITE_APP_VERSION:-$RAIN_RELEASE_VERSION}"

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
printf '%s\n' "$RAIN_RELEASE_VERSION" > "$ROOT/release/VERSION"
chmod +x "$ROOT/release/rain"

echo
echo "Build completed. Keep both files together:"
echo "$ROOT/release/rain"
echo "$ROOT/release/.env"
echo "$ROOT/release/VERSION"
