$ErrorActionPreference = "Stop"

$Root = $PSScriptRoot

Write-Host "==> Building embedded frontend..."
Set-Location "$Root\frontend"

npm ci
npm run build

if (-not (Test-Path "$Root\frontend\dist\index.html")) {
    throw "frontend/dist/index.html was not generated"
}

Write-Host "==> Building Rain executable..."
Set-Location "$Root\backend"

cargo fmt --check
cargo test --locked
cargo build --release --locked

$OutputDir = "$Root\release"
New-Item -ItemType Directory -Force $OutputDir | Out-Null

Copy-Item `
    "$Root\backend\target\release\backend.exe" `
    "$OutputDir\Rain.exe" `
    -Force

Write-Host ""
Write-Host "Build completed:"
Write-Host "$OutputDir\Rain.exe"
