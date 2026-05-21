@echo off
set PATH=%PATH%;%USERPROFILE%\.cargo\bin

REM Force tauri-build's build script to re-run so icon.ico changes get re-embedded.
REM Without this, cargo's incremental cache keeps the previously embedded icon.
copy /b "%~dp0src-tauri\build.rs"+,, "%~dp0src-tauri\build.rs" >nul

cargo build --release --manifest-path "%~dp0src-tauri\Cargo.toml"
