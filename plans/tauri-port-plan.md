# QBWebUIHelper — Tauri v2 Port Plan

## Goal

Port QBWebUIHelper from a Windows-only Go/WebView2 app to a cross-platform (Windows + macOS) Tauri v2 app while preserving all existing functionality and adding new features.

## Requirements

| # | Requirement | Status |
|---|-------------|--------|
| 1 | Display qBittorrent WebUI in a native window | Tauri built-in |
| 2 | Window resize + remember size/position across launches | `tauri-plugin-window-state` |
| 3 | Settings and About menus | Built-in Menu API |
| 4 | Close-to-tray option (configurable: tray vs quit) | Built-in tray + event interception |
| 5 | Status icon (Windows: system tray, macOS: menu bar) with context menu | Built-in `tray-icon` feature |
| 6 | Register/unregister as .torrent and magnet: handler | `tauri-plugin-deep-link` + bundler `fileAssociations` |
| 7 | Log file rotation — cap `log.txt` at 5 MB | `std::fs::metadata` size check on startup |

## Log File Management

`log.txt` lives next to the exe (Windows) or in the app bundle's working directory (macOS). On every startup, `init_logger()` checks its size before opening it:

- **≤ 5 MB** → open in append mode (normal operation)
- **> 5 MB** → open with `truncate(true)` to wipe it before writing the first entry of the new session

This is implemented in pure Rust (`std::fs::metadata` + `OpenOptions`) and is fully cross-platform — the same code runs unchanged on macOS.

No rolling/archiving needed; a clean wipe is sufficient for a helper app.

## Feasibility Assessment

### Fully cross-platform (no hacks needed)

- **Requirements 1-5**: Tauri v2 handles all of these with built-in APIs or official plugins. Window state, menus, tray icons, and close-to-tray all work identically on Windows and macOS.

### Platform differences (requirement 6)

| Platform | .torrent files | magnet: links |
|----------|---------------|---------------|
| **Windows** | Registry keys written by installer. Can also register at runtime via `reg` commands. | `tauri-plugin-deep-link` `register("magnet")` writes registry at runtime. Unregister = restore previous handler from saved config. |
| **macOS** | `Info.plist` `CFBundleDocumentTypes` — declared at build time. Registered with Launch Services when app is first opened. | `Info.plist` `CFBundleURLSchemes` — declared at build time. Claimed as active default via `LSSetDefaultHandlerForURLScheme` at runtime (first launch prompt). Unregister = restore previous handler via `LSSetDefaultHandlerForURLScheme`. |

### Previous handler backup — critical safety rule

**Before claiming any association, save the current default handler. Restore it on unregister.**

If QBWebUIHelper is uninstalled without restoring the previous handler, magnet: links and .torrent files become orphaned — the OS tries to open an app that no longer exists.

**macOS**:
- Before registering: call `LSCopyDefaultHandlerForURLScheme("magnet")` to get the current handler bundle ID (e.g., `org.m0k.transmission`)
- Save that bundle ID to `config.json` as `"prev_magnet_handler": "org.m0k.transmission"`
- On unregister: call `LSSetDefaultHandlerForURLScheme("magnet", prevBundleId)` to restore it
- Same pattern for `.torrent` file type using `LSCopyDefaultHandlerForContentType` / `LSSetDefaultHandlerForContentType`

**Windows**:
- Before registering `.torrent`: read and save the current value of `HKCU\Software\Classes\.torrent` (e.g., `Bittorrent`) to config
- Before registering `magnet`: read and save the current `HKCU\Software\Classes\magnet\shell\open\command` to config
- On unregister: write those saved values back to the registry instead of just deleting the keys

**If no previous handler exists** (nil / empty): on unregister, simply remove the association rather than trying to restore — which is the safe default.

**Implication for Settings UI** — Windows and macOS are handled completely differently:

**Windows**:
- Show **Register / Unregister / Default Apps** buttons.
- After a successful Register, show a modal dialog prompting the user to manually set QBWebUIHelper as the default in Windows Settings (Default Apps), because a system-level HKLM entry from another app may override the HKCU registration we write.
  - Modal steps: (1) Open Default Apps, (2) search `.torrent` → select QBWebUIHelper, (3) search `magnet` → select QBWebUIHelper.
  - Modal also reminds the user to click **Unregister** before uninstalling.
- Unregister cleans up: the HKCU ProgID keys, `RegisteredApplications` entry, `OpenWithList` slots (including MRUList), `OpenWithProgids`, and the `HKCU\Software\Classes\Applications\qbwebuihelper.exe` key Windows auto-creates via Open With.

**macOS**:
- Never auto-claim on launch. User must explicitly click **Set as Default** in Settings.
- Before claiming, show a prominent modal warning (see §macOS "Set as Default" warning below).
- On confirm: save previous handler bundle ID to config via `LSCopyDefaultHandlerForURLScheme` / `LSCopyDefaultHandlerForContentType`, then call `LSSetDefaultHandlerForURLScheme` / `LSSetDefaultHandlerForContentType` to claim.
- Show a **Restore Previous Default** button only when a backup exists in config.
- The Register/Unregister/OpenWithList approach used on Windows does not apply on macOS at all.

### macOS "Set as Default" warning — UX requirement

When the user clicks "Set as Default" on macOS, show a **prominent modal warning before proceeding**:

> ⚠️ **Before you uninstall QBWebUIHelper**
>
> macOS has no automatic uninstaller. If you drag QBWebUIHelper to the Trash without clicking **Restore Previous Default** first, your previous magnet: and .torrent handler (e.g. Transmission) will not be automatically restored.
>
> **Always open Settings → Restore Previous Default before uninstalling.**
>
> [ Cancel ]  [ I understand, Set as Default ]

This warning must appear every time "Set as Default" is clicked, not just once — the user may have forgotten. macOS will eventually self-heal via Launch Services after the app is deleted, but the timing is unpredictable (may require a reboot), so the warning is the primary safeguard.

This is not a hack — browsers (Chrome, Firefox) use the exact same backup/restore pattern when claiming or releasing the default browser role.

## Architecture

```
QBWebUIHelper/
├── src-tauri/           # Rust backend
│   ├── src/
│   │   ├── main.rs      # Entry point, tray, menu, window events
│   │   ├── config.rs    # Config load/save (JSON, same format as Go version)
│   │   └── lib.rs       # Tauri command handlers (IPC)
│   ├── Cargo.toml
│   ├── tauri.conf.json  # Window config, file associations, deep-link schemes
│   ├── icons/           # App icons (.ico for Windows, .icns for macOS)
│   └── capabilities/    # Tauri v2 permissions
├── src/                 # Frontend (HTML/JS/CSS)
│   ├── index.html       # Minimal shell — redirects to WebUI URL
│   ├── settings.html    # Settings page (or modal overlay)
│   └── styles.css       # Settings/About styling
├── GoApp/               # Previous Go implementation (archived)
└── plans/               # This file
```

### How it works

1. **App launch**: Tauri opens the main window at the local `index.html` landing page first, then performs a TCP connection check before navigating to the WebUI. See §Landing Page below.

2. **Single instance**: `tauri-plugin-single-instance` ensures only one window. If a second instance launches with a .torrent or magnet: argument, it forwards the argument to the running instance via the plugin's callback.

3. **Torrent/magnet handling**: When the app receives a file or URL (via single-instance forwarding, deep-link event, or file-open event), the Rust backend:
   - For `.torrent` files: reads the file, base64-encodes it, sends to the webview JS
   - For `magnet:` links: sends the URL directly to the webview JS
   - The webview JS calls `showDownloadPage([url])` or `uploadTorrentFiles([file])` — same as the current Go implementation

4. **Tray / menu bar icon**: Tauri's `TrayIcon` API maps to the platform-native status area:
   - **Windows**: System tray icon (bottom-right notification area). Right-click opens context menu. Left-click shows/hides the window.
   - **macOS**: Menu bar icon (top-right status area, next to Wi-Fi/battery). Click opens a dropdown context menu.
   
   Context menu on both: Show/Hide, Settings, About, Quit.

5. **Close behavior**: Configurable in Settings. If "close to tray" is enabled, clicking X hides the window instead of quitting. The tray icon stays. If disabled, X quits the app.

6. **Window state**: `tauri-plugin-window-state` automatically saves/restores window size, position, and maximized state. No manual DPR calculations needed — Tauri handles DPI scaling internally.

7. **Settings/About**: Native menu bar with Settings and About items. Clicking Settings opens a small Tauri window with the settings form. Clicking About opens a dialog or small window. Alternatively, these could be injected as an overlay into the WebUI page (like the current Go implementation's gear icon).

## Landing Page & Connection Check

The main window always starts at `src/index.html` (a local Tauri asset), never directly at the external WebUI URL. This provides three states:

| State | Trigger | UI |
|-------|---------|-----|
| `s-connecting` | Default on launch | Spinner + URL being checked |
| `s-error` | TCP check fails (5 s timeout) | Warning + URL + Retry + Open Settings |
| `s-firstrun` | No `config.json` exists | Welcome + Open Settings |

### Connection flow (Rust)

```
setup()
  └── spawn thread
        ├── no config → eval showFirstRun() + open_settings()
        └── config exists → connect_flow(win, url)
              ├── eval setConnecting(url)
              ├── TcpStream::connect_timeout(host:port, 5s)
              ├── success → win.navigate(parsed_url)   ← Rust API, no Referer header
              └── failure → eval showError(url)

cmd_retry / cmd_save_url → trigger_connect()
  └── navigate main window back to index.html
        └── after 400 ms → connect_flow()
```

### Why `win.navigate()` instead of `window.location.replace()`

qBittorrent's CSRF protection checks the `Referer` header. A JS-initiated navigation from `https://tauri.localhost/` sends `Referer: https://tauri.localhost/`, which qBittorrent rejects with plain-text `Unauthorized`. Rust's `win.navigate(url::Url)` performs a host-level navigation with no `Referer` header at all — the request arrives clean.

### macOS applicability

The entire landing page flow is platform-neutral:
- `index.html`, `setConnecting()`, `showError()`, `showFirstRun()` — pure HTML/JS, unchanged
- `TcpStream::connect_timeout` — standard Rust, works on macOS
- `win.navigate(url)` — Tauri API, maps to WebKit navigation on macOS (same no-Referer behaviour)

No macOS-specific changes needed for the landing page.

## Key Decisions

### Settings UI: Separate local window (decided)

Settings and About are **separate Tauri windows** loading local HTML files bundled with the app. The main window only ever loads the remote WebUI URL and is never touched.

```
src/
├── settings.html   ← Settings window (local, bundled)
└── about.html      ← About window (local, bundled)
```

```
┌─────────────────────────────────┐    ┌──────────────────────┐
│  Main Window                    │    │  Settings Window     │
│  webview → http://10.0.1.249    │    │  webview → local     │
│  (qBittorrent WebUI)            │    │  settings.html       │
└────────────────┬────────────────┘    └──────────┬───────────┘
                 │ Tauri IPC                       │ Tauri IPC
                 └──────────────┬─────────────────┘
                          Rust backend
                     (config, tray, handlers)
```

Keeping Settings as a separate window means more features can be added to it over time without any risk of conflicting with the qBittorrent WebUI's own CSS or JavaScript.

### Menu bar approach

- **macOS**: Native menu bar (App name > About, Preferences/Settings). This is expected macOS behavior.
- **Windows**: Menu bar on the window frame, or skip the menu bar and use only the tray icon's context menu + a keyboard shortcut (Ctrl+,) for settings.

**Recommendation**: Native menu bar on both platforms. Tauri's Menu API handles the platform differences automatically.

## Plugins Required

| Plugin | Purpose |
|--------|---------|
| `tauri-plugin-window-state` | Save/restore window size and position |
| `tauri-plugin-single-instance` | Single instance with argument forwarding |
| `tauri-plugin-deep-link` | magnet: URL protocol handling |
| `tauri-plugin-shell` | Open Default Apps settings on Windows |

## Migration from Go

| Go code | Tauri equivalent |
|---------|-----------------|
| `helperJS()` — torrent/magnet JS injection | Same JS, injected via Tauri's `initialization_scripts` in config or `webview.eval()` |
| `settingsJS()` — overlay UI | Replaced by separate settings window |
| `platform_windows.go` — DPI, registry, tray | Tauri built-in (DPI automatic, tray API, deep-link plugin) |
| `platform_other.go` — stubs | No longer needed — Tauri is cross-platform |
| `main.go` — IPC, config, webview | `main.rs` — Tauri commands, config, event handlers |
| `torrent.go` — bencode parser | Port to Rust, or use `lava_torrent`/`bendy` crate |
| TCP IPC on port 47683 | `tauri-plugin-single-instance` handles this |
| `config.json` in `%LOCALAPPDATA%` | Tauri's `app_data_dir()` (platform-aware: `%LOCALAPPDATA%` on Win, `~/Library/Application Support/` on macOS) |

## Build & Distribution

| Platform | Build command | Output |
|----------|--------------|--------|
| Windows | `cargo tauri build` | `.exe` + `.msi` or `.nsi` installer (~5-8 MB) |
| macOS | `cargo tauri build` | `.app` bundle + `.dmg` installer (~3-5 MB) |

Cross-compilation is possible but not trivial. Best to build on each target platform (or use CI with GitHub Actions — Tauri has an official action).

## Prerequisites

- **Rust** toolchain (`rustup`)
- **Node.js** (for frontend build tooling, even if minimal)
- **Windows**: WebView2 runtime (pre-installed on Windows 10/11)
- **macOS**: Xcode Command Line Tools (for WebKit framework linking)

## Pre-macOS Checklist

Windows is confirmed working. Fix these before starting the macOS port — they are gaps that will break or complicate the Mac build if left as-is.

| # | Issue | Status | Detail |
|---|-------|--------|--------|
| 1 | **`log.txt` next to the exe** | ✅ Done | `init_logger` now takes a `PathBuf`; called from `setup()` with `app.path().app_data_dir().join("log.txt")`. Works on both platforms. |
| 2 | **`tauri.conf.json` missing `magnet:` in `fileAssociations`** | ✅ Done | `tauri-plugin-deep-link` added with `plugins.deep-link.schemes: ["magnet"]`. The plugin's build script injects `CFBundleURLTypes` into the macOS `Info.plist` automatically. |
| 3 | **`buildme.bat` is Windows-only** | ✅ Done | `buildme.sh` created with the same `touch build.rs` trick using `$(dirname "$0")`. |
| 4 | **`about.html` claims macOS support** | ⬜ Pending | Update once macOS build is tested and associations confirmed working. |
| 5 | **`tauri.conf.json` targets `"all"`** | N/A | `targets` accepts installer format names (e.g. `"nsis"`, `"dmg"`), not OS names. `"all"` means all formats for the current build OS — Linux formats will not build unless on Linux. Left as `"all"`. |

## macOS-only items still needed on a Mac

| # | Task | Detail |
|---|------|--------|
| 1 | **`icons/icon.icns` generated** | ✅ Done — `npx tauri icon src-tauri/icons/icon.png` was run on Windows; `icon.icns` is committed. |
| 2 | **First Mac build** | Run `./buildme.sh` from the repo root. Fix any compile errors (see §Mac Build Notes below). |
| 3 | **Test basic functionality** | App launches, connects to qBittorrent WebUI, settings window opens, tray/menu-bar icon shows. |
| 4 | **Test associations** | In Settings, click **Set as Default**. Confirm warning modal appears. Confirm. Click a `magnet:` link in Safari → should open in qBittorrent. Open a `.torrent` from Finder → should open in qBittorrent. |
| 5 | **Test "Restore Previous Default"** | After Set as Default, verify "Restore Previous Default" button appears. Click it; verify the previous app (e.g. Transmission) is restored. |
| 6 | **Test deep-link on cold launch** | With app NOT running, open a `.torrent` from Finder. App should launch and pass the file to qBittorrent. |
| 7 | **Update `about.html`** | Change copy to reflect macOS support once tests pass. |
| 8 | **DMG installer** | `cargo tauri build` produces a `.dmg`. Test installing from DMG and verifying associations in `Info.plist`. |

---

## macOS Session Fixes (2026-05-22) — What Changed & What Needs Windows Backport

### Fixes that apply to both platforms (already in the shared code)

| Fix | File | Detail |
|-----|------|--------|
| **Settings auto-close on successful save** | `lib.rs` `cmd_save_url`, `settings.html` | `cmd_save_url` now returns `bool` (TCP check result). On success, the Rust side calls `w.hide()` on the settings window. On failure, the JS side shows an error message. Save button is disabled during the check. Cross-platform — no separate Windows work needed. |
| **Set as Default / Register button greyed when already registered** | `settings.html` `refreshRegStatus()` | Added `btn-set-default.disabled = registered` for macOS branch. **Windows backport needed**: same line should be added for the `btn-register` button in the Windows branch (see §Windows backport below). |
| **Backup overwrite guard** | `lib.rs` `platform_register` (macOS) | `cfg.mac_backup` is only written if `!cfg.mac_backup.has_any()` — prevents a second "Set as Default" click from overwriting the original handler backup with our own bundle ID. **Windows backport needed**: `platform_register` (Windows) has the same bug — it always overwrites `cfg.reg_backup`. |
| **Backup self-reference filter** | `associations.rs` `mac::register()` | Before snapshotting the current handler as "previous", the code now filters out our own bundle ID. Prevents `mac_backup` from ever pointing to `com.kritblade.qbwebuihelper` itself. **Windows backport needed**: Windows `register()` should similarly skip writing backup entries whose existing value already points to our own ProgID/exe. |

### macOS-only fixes (no Windows equivalent needed)

| Fix | File | Detail |
|-----|------|--------|
| **Wrong LS symbol names** | `associations.rs` | `LSSetDefaultHandlerForContentType` / `LSCopyDefaultHandlerForContentType` do not exist. Correct names are `LSSetDefaultRoleHandlerForContentType` / `LSCopyDefaultRoleHandlerForContentType`. The URL-scheme variants (`LSSetDefaultHandlerForURLScheme`) are named differently — no "Role" — which is why magnet worked but torrent didn't. |
| **Missing `LSItemContentTypes` in `CFBundleDocumentTypes`** | `src-tauri/Info.plist` | Tauri's `fileAssociations` generates `CFBundleTypeExtensions` but not `LSItemContentTypes`. Without this key, `LSSetDefaultRoleHandlerForContentType` rejects the claim (`kLSUnknownTypeErr`). Fixed via custom `Info.plist` that overrides `CFBundleDocumentTypes` to add `LSItemContentTypes: ["com.bittorrent.torrent"]`. |
| **Missing `UTImportedTypeDeclarations`** | `src-tauri/Info.plist` | If Transmission (or another app that owns `com.bittorrent.torrent`) is not installed, the UTI is unknown to the system. Declaring it via `UTImportedTypeDeclarations` with extension/MIME tag tells Launch Services the UTI exists. |
| **Missing `NSLocalNetworkUsageDescription`** | `src-tauri/Info.plist` | macOS may block TCP connections to LAN IPs without this key. Added to custom `Info.plist`. |
| **Global app menu** | `lib.rs` `build_mac_app_menu()` | On macOS the per-window `.menu()` approach used for Windows doesn't replace the default app menu. Fixed by building a proper macOS menu (app submenu with About, Settings ⌘,, Services, Hide, Quit + Edit/View/Window submenus) and calling `app.set_menu()` globally. The Windows path still uses `WebviewWindowBuilder::menu()` unchanged. |
| **`set_content_handler` error code** | `associations.rs` | Changed return type from `bool` to `Result<(), i32>` so the actual `OSStatus` code is surfaced in the error message (e.g. `torrent=err(-10809)` for unknown type). Helps diagnose future LS failures. |

### Windows backport checklist

These are bugs found during macOS testing that exist on Windows too. Fix on Windows before next release.

| # | What to fix | Where | Detail |
|---|------------|-------|--------|
| 1 | **Register button disabled when already registered** | `settings.html` `refreshRegStatus()` | In the `if (currentPlatform === 'windows')` branch, add `document.getElementById('btn-register').disabled = registered;` after setting `reg-status-win`. |
| 2 | **Backup overwrite guard** | `lib.rs` `platform_register` (Windows, lines ~368-378) | Wrap `cfg.reg_backup = backup;` in `if cfg.reg_backup.is_empty() { ... }`. Need to add `is_empty()` to `Vec<RegMutation>` check or use `.is_empty()` directly since it's a Vec. |
| 3 | **Backup self-reference filter** | `associations.rs` `win::register()` | In the backup snapshot phase, check if the existing registry value already points to our own ProgID (`QBWebUIHelper.Torrent` / `QBWebUIHelper.Magnet`). If so, store `None` / empty string instead of writing it as the backup, so unregister doesn't restore to ourselves. |

---

## Mac Handoff — Read This First on the Mac

> **This section is written for a fresh AI session on macOS that has no history of this project.**

### What this project is

QBWebUIHelper is a Tauri v2 desktop app that wraps qBittorrent's web UI in a native window and registers as the default handler for `.torrent` files and `magnet:` links. It was originally Windows-only (Go + WebView2). The Tauri port has been completed on Windows and is now being brought up on macOS.

### What is already done (do NOT redo these)

All Rust source code is written and compiles cleanly on Windows. The Mac session only needs to **build and test** — not write new code (unless the build reveals compile errors).

| What | Where | Notes |
|------|-------|-------|
| macOS LaunchServices associations | [src-tauri/src/associations.rs](../src-tauri/src/associations.rs) | `mac` module, uses `core-foundation` crate + raw `CoreServices.framework` FFI. Calls `LSSet/LSCopyDefaultHandlerForURLScheme` and `LSSet/LSCopyDefaultHandlerForContentType` (UTI: `com.bittorrent.torrent`). |
| macOS backup/restore in config | [src-tauri/src/config.rs](../src-tauri/src/config.rs) | `MacBackup` struct with `prev_magnet_handler` / `prev_torrent_handler` (bundle IDs). Stored in `config.json` under `mac_backup`. |
| Deep-link plugin (macOS only) | [src-tauri/src/lib.rs](../src-tauri/src/lib.rs) | `tauri-plugin-deep-link` registered in the builder only on `#[cfg(target_os = "macos")]`. `on_open_url` callback handles both `magnet:` URLs and `file://` paths for .torrent files. |
| log.txt path | [src-tauri/src/lib.rs](../src-tauri/src/lib.rs) | `init_logger` takes a `PathBuf` from `app.path().app_data_dir()` → `~/Library/Application Support/com.kritblade.qbwebuihelper/log.txt`. |
| Tray icon | [src-tauri/src/lib.rs](../src-tauri/src/lib.rs) | `show_menu_on_left_click(true)` on macOS (click opens menu), `false` + left-click toggle on Windows. |
| Settings UI | [src/settings.html](../src/settings.html) | Calls `cmd_get_platform` IPC → shows macOS section (Set as Default, Restore Previous Default + warning modal) or Windows section (Register/Unregister/Default Apps). |
| magnet: scheme in bundle | [src-tauri/tauri.conf.json](../src-tauri/tauri.conf.json) | `plugins.deep-link.schemes: ["magnet"]` — the plugin's build script injects `CFBundleURLTypes` into `Info.plist` automatically. |
| icon.icns | [src-tauri/icons/icon.icns](../src-tauri/icons/icon.icns) | Already generated from `icon.png`. |

### Mac prerequisites

```bash
# 1. Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup update

# 2. Node.js (for Tauri CLI)
# Install via https://nodejs.org or brew install node

# 3. Xcode Command Line Tools (WebKit linker)
xcode-select --install
```

### First build

```bash
cd /path/to/QBWebUIHelper
chmod +x buildme.sh
./buildme.sh
# or equivalently:
# cargo build --release --manifest-path src-tauri/Cargo.toml
```

The binary lands at `src-tauri/target/release/qbwebuihelper`.

### Expected build issues and fixes

These are LIKELY issues based on the code written on Windows. Fix them on the Mac if they occur:

**1. `core-foundation` version mismatch**
If `core_foundation::string::CFString` or `TCFType` paths don't exist, the `core-foundation` crate version may have changed the module layout. Check `cargo doc --open` for the actual path, then update [src-tauri/src/associations.rs](../src-tauri/src/associations.rs) line ~9.

**2. `LSCopyDefaultHandlerForURLScheme` / `LSSetDefaultHandlerForContentType` not found**
These are declared in `CoreServices.framework`. If the linker can't find them, verify the `#[link(name = "CoreServices", kind = "framework")]` attribute is on the correct `extern "C"` block in `associations.rs`. On macOS 12+ these APIs are deprecated (but still functional) — deprecation warnings are OK.

**3. `tauri_plugin_deep_link::DeepLinkExt` not in scope**
If `app.deep_link()` fails, check the `tauri-plugin-deep-link` version in `Cargo.toml`. The `DeepLinkExt` trait must be in scope. The import is `use tauri_plugin_deep_link::DeepLinkExt;` inside the `#[cfg(target_os = "macos")]` block in `lib.rs`.

**4. `on_open_url` API changed**
If the deep-link plugin's API is different (e.g. uses `register` instead of `on_open_url`), check the plugin docs for v2 and update the handler in `lib.rs` inside the `#[cfg(target_os = "macos")]` setup block.

**5. Window shows blank / WebUI unreachable**
The default WebUI URL is `http://10.0.1.249:9865`. On Mac, open Settings and enter the correct URL for the qBittorrent instance you're testing against.

### Key architectural facts

- **Config location**: `~/Library/Application Support/com.kritblade.qbwebuihelper/config.json`
- **Log location**: `~/Library/Application Support/com.kritblade.qbwebuihelper/log.txt`
- **Bundle ID**: `com.kritblade.qbwebuihelper` (set in `tauri.conf.json`)
- **Settings window**: separate Tauri window loading local `settings.html`, not injected into the WebUI
- **No JS IPC calls from frontend to deep-link plugin** — all deep-link handling is pure Rust-side; `deep-link:default` is NOT in `capabilities/default.json` intentionally
- **CSRF**: `win.navigate(url)` (Rust API) is used to navigate to the WebUI, not JS `window.location` — this avoids qBittorrent's Referer-based CSRF rejection
- **Tray icon**: macOS uses `show_menu_on_left_click(true)` so clicking the menu bar icon shows the context menu (native macOS convention)

## Implementation Order

1. **Scaffold** — `cargo create-tauri-app`, minimal config
2. **Core** — Load WebUI URL in window, config load/save
3. **Torrent/magnet handling** — JS injection, file reading, base64 encoding
4. **Single instance** — Plugin setup, argument forwarding
5. **Status icon** — Tray icon (Windows) / menu bar icon (macOS), context menu, show/hide toggle
6. **Close-to-tray** — Event interception, config toggle
7. **Settings window** — WebUI URL, close behavior, file associations (Windows only)
8. **About window** — Version, author, license
9. **Menu bar** — Wire up Settings/About menu items
10. **File associations** — `tauri.conf.json` bundler config + deep-link plugin
11. **Window state** — Plugin setup (should be near-zero code)
12. **Icons** — App icon for both platforms
13. **Test on macOS** — Verify all features work
14. **Installers** — NSIS for Windows, DMG for macOS
