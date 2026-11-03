@echo off
rem Installs the Bridle desktop app for the current Windows user.
rem
rem   install-bridle.cmd [PATH\TO\bridle.exe]
rem
rem Copies bridle.exe to %LOCALAPPDATA%\Bridle\bin and adds that folder to the user PATH, so `bridle` works in any folder of
rem a terminal opened afterwards. With no argument it installs the bridle.exe that sits next to this file (as in the release
rem bundle). Nothing else on the machine is changed. Build the exe with:
rem   cargo build --release -p bridle-desktop     (the result is target\release\bridle.exe)
setlocal
set "SRC=%~1"
if "%SRC%"=="" set "SRC=%~dp0bridle.exe"
if not exist "%SRC%" (
  echo not found: %SRC%
  echo usage: %~nx0 [PATH\TO\bridle.exe]
  exit /b 2
)
set "DEST=%LOCALAPPDATA%\Bridle\bin"
if not exist "%DEST%" mkdir "%DEST%"
copy /y "%SRC%" "%DEST%\bridle.exe" >nul || exit /b 1
powershell -NoProfile -Command "$d='%DEST%'; $p=[Environment]::GetEnvironmentVariable('Path','User'); if (($p -split ';') -notcontains $d) { [Environment]::SetEnvironmentVariable('Path', ($p.TrimEnd(';') + ';' + $d), 'User'); 'added to the user PATH: ' + $d } else { 'already on the user PATH: ' + $d }"
echo installed %DEST%\bridle.exe
echo Open a new terminal, go to a project folder and run: bridle
endlocal
