#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"

VERSION="$(tr -d '[:space:]' < "$ROOT/VERSION")"
echo "Building version $VERSION..."

# Stamp version into tauri.conf.json
sed -i '' -E "s/\"version\": \"[0-9]+\.[0-9]+\.[0-9]+\"/\"version\": \"$VERSION\"/" \
    "$ROOT/src-tauri/tauri.conf.json"

# Stamp version into Cargo.toml — package version only (first matching line).
# Dependency versions use { version = "x" } syntax so this safely targets only [package].
awk -v ver="$VERSION" \
    '!done && /^version = "/ { print "version = \"" ver "\""; done=1; next } 1' \
    "$ROOT/src-tauri/Cargo.toml" > "$ROOT/src-tauri/Cargo.toml.tmp"
mv "$ROOT/src-tauri/Cargo.toml.tmp" "$ROOT/src-tauri/Cargo.toml"

echo "Stamped version $VERSION into tauri.conf.json and Cargo.toml"

# Force tauri-build's build script to re-run so icon.icns changes get re-embedded.
# Without this, cargo's incremental cache keeps the previously embedded icon.
touch "$ROOT/src-tauri/build.rs"

cargo build --release --manifest-path "$ROOT/src-tauri/Cargo.toml"
