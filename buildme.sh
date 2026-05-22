#!/usr/bin/env bash
set -euo pipefail

# Force tauri-build's build script to re-run so icon.icns changes get re-embedded.
# Without this, cargo's incremental cache keeps the previously embedded icon.
touch "$(dirname "$0")/src-tauri/build.rs"

cargo build --release --manifest-path "$(dirname "$0")/src-tauri/Cargo.toml"
