@echo off
setlocal
pushd "%~dp0" || exit /b 1

cargo build --release --locked --target-dir target
set "build_exit=%errorlevel%"
if "%build_exit%"=="0" if exist "target\release\linebot_rust.exe" echo Executable "%CD%\target\release\linebot_rust.exe"

popd
exit /b %build_exit%
