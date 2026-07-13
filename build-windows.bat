@echo off
setlocal

set "ROOT=%~dp0"
if "%ROOT:~-1%"=="\" set "ROOT=%ROOT:~0,-1%"

echo ==^> Building embedded frontend...
pushd "%ROOT%\frontend" || exit /b 1
call npm ci || exit /b 1
call npm run build || exit /b 1
if not exist "%ROOT%\frontend\dist\index.html" (
  echo frontend\dist\index.html was not generated
  exit /b 1
)
popd

echo ==^> Building Rain executable...
pushd "%ROOT%\backend" || exit /b 1
cargo fmt --check || exit /b 1
cargo test --locked || exit /b 1
cargo build --release --locked || exit /b 1
popd

if not exist "%ROOT%\release" mkdir "%ROOT%\release"
copy /Y "%ROOT%\backend\target\release\backend.exe" "%ROOT%\release\Rain.exe" >nul || exit /b 1
copy /Y "%ROOT%\backend\.env.example" "%ROOT%\release\.env" >nul || exit /b 1

echo.
echo Build completed. Keep both files together:
echo %ROOT%\release\Rain.exe
echo %ROOT%\release\.env
