@echo off
rem Windows task runner for Bridle. No WSL is needed for any task.
rem
rem   scripts\dev.cmd fmt            format the code
rem   scripts\dev.cmd clippy         lint, warnings are errors
rem   scripts\dev.cmd test           the whole test suite
rem   scripts\dev.cmd gate           the three above, as CI runs them
rem   scripts\dev.cmd build          debug build of bridle and bridle-cli
rem   scripts\dev.cmd release        release build of bridle
rem   scripts\dev.cmd e2e [--skip-model]   the end-to-end suite on the real window (builds debug first)
rem   scripts\dev.cmd vendor PROXY   re-vendor dependencies natively into a new folder (see below)
rem   scripts\dev.cmd install        install the release exe for this user (see scripts\install-bridle.cmd)
rem   scripts\dev.cmd bundle [OUT]   release build, then the release zip in OUT (default: your Downloads folder)
rem
rem Settings (environment variables, with defaults for the tested machine):
rem   CARGO_TARGET_DIR       where builds go            (D:\cargo-target\harness)
rem   BRIDLE_VENDOR_CONFIG   cargo source replacement   (D:\cargo-vendor\bridle-desktop-source.toml)
rem
rem Dependencies are vendored, so builds run --offline. To refresh them, `vendor` needs the proxy on networks that only let
rem traffic out through one, for example:  scripts\dev.cmd vendor http://172.31.2.4:8080
rem The result goes to <vendor dir>.new; compare it with the current folder, then swap the folders yourself.
setlocal
set "ROOT=%~dp0.."
if not defined CARGO_TARGET_DIR set "CARGO_TARGET_DIR=D:\cargo-target\harness"
if not defined BRIDLE_VENDOR_CONFIG set "BRIDLE_VENDOR_CONFIG=D:\cargo-vendor\bridle-desktop-source.toml"
set "HTTP_PROXY="
set "HTTPS_PROXY="
set "http_proxy="
set "https_proxy="
set "CARGO_BUILD_JOBS=4"
set "VCVARS=C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"
if exist "%VCVARS%" call "%VCVARS%" >nul
cd /d "%ROOT%"
if "%~1"=="" goto usage
goto %~1

:fmt
cargo fmt --all
exit /b %ERRORLEVEL%

:clippy
cargo clippy --workspace --all-targets --locked --offline --config "%BRIDLE_VENDOR_CONFIG%" -- -D warnings
exit /b %ERRORLEVEL%

:test
cargo --config "%BRIDLE_VENDOR_CONFIG%" test --workspace --locked --offline --no-fail-fast
exit /b %ERRORLEVEL%

:gate
cargo fmt --all -- --check || exit /b 1
call "%~f0" clippy || exit /b 1
call "%~f0" test
exit /b %ERRORLEVEL%

:build
cargo --config "%BRIDLE_VENDOR_CONFIG%" build -p bridle-desktop -p harness-cli --locked --offline
exit /b %ERRORLEVEL%

:release
cargo --config "%BRIDLE_VENDOR_CONFIG%" build --release -p bridle-desktop --locked --offline
exit /b %ERRORLEVEL%

:e2e
call "%~f0" build || exit /b 1
py -3 tools\e2e\run.py --exe "%CARGO_TARGET_DIR%\debug\bridle.exe" %2 %3
exit /b %ERRORLEVEL%

:vendor
if "%~2"=="" (
  echo usage: %~nx0 vendor PROXY   e.g. http://172.31.2.4:8080
  exit /b 2
)
set "CARGO_HTTP_PROXY=%~2"
for %%I in ("%BRIDLE_VENDOR_CONFIG%") do set "VENDOR_PARENT=%%~dpI"
set "NEWDIR=%VENDOR_PARENT%bridle-desktop.new"
cargo vendor --locked "%NEWDIR%"
echo vendored into %NEWDIR%
exit /b %ERRORLEVEL%

:install
call "%~f0" release || exit /b 1
call scripts\install-bridle.cmd "%CARGO_TARGET_DIR%\release\bridle.exe"
exit /b %ERRORLEVEL%

:bundle
call "%~f0" release || exit /b 1
py -3 scripts\make_bundle.py --exe "%CARGO_TARGET_DIR%\release\bridle.exe" %2 %3
exit /b %ERRORLEVEL%

:usage
echo usage: %~nx0 fmt ^| clippy ^| test ^| gate ^| build ^| release ^| e2e [--skip-model] ^| vendor PROXY ^| install ^| bundle
exit /b 2
