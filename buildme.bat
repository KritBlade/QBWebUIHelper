@echo off
set PATH=%PATH%;%USERPROFILE%\.cargo\bin

set /p VERSION=<"%~dp0VERSION"
echo Building version %VERSION%...

REM Stamp version into tauri.conf.json and Cargo.toml
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\stamp.ps1" -Version "%VERSION%"

REM Force tauri-build's build script to re-run so icon.ico changes get re-embedded.
REM Without this, cargo's incremental cache keeps the previously embedded icon.
copy /b "%~dp0src-tauri\build.rs"+,, "%~dp0src-tauri\build.rs" >nul

cargo build --release --manifest-path "%~dp0src-tauri\Cargo.toml"
