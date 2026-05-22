# QBWebUIHelper — Developer Notes

Hard-won lessons and non-obvious decisions from the Tauri v2 port. Read this before touching the window management or WebView2 code.

---

## 1. Window Creation Must Happen in `setup()`

**Problem**: Creating a `WebviewWindow` from an IPC command thread (or via `run_on_main_thread`) deadlocks or produces a blank/unresponsive window.

**Root cause**: `WebviewWindowBuilder::build()` internally calls WebView2's async initialisation, which posts callbacks to the Win32 message queue expecting the pump to be running. When called from:
- an IPC command thread → `build()` dispatches to the main thread and waits, but the main thread is processing the IPC request → stall → blank window
- `run_on_main_thread(|| build())` → the closure blocks the message pump; WebView2 can't deliver its own init callbacks back → deadlock, `build()` never returns

**Tray menu works** because its callback is invoked *by* the message pump (re-entrantly), so WebView2 callbacks can be delivered while `build()` runs.

**Fix**: Pre-create ALL secondary windows (settings, about) in `setup()` with `.visible(false)`. `setup()` runs on the main thread before the event loop fully starts, so `build()` completes cleanly. From that point on, `open_settings()` / `open_about()` only call `w.show()` + `w.set_focus()`, which are safe from any thread.

```rust
// setup() — main thread, pump not yet blocking
if let Ok(w) = WebviewWindowBuilder::new(app, "settings", ...).visible(false).build() {
    let _ = w.hide(); // override window-state plugin restoration (see §3)
}

// open_settings() — safe from any thread
fn open_settings(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}
```

---

## 2. qBittorrent "Unauthorized" on First Navigation

**Problem**: Navigating from `https://tauri.localhost/index.html` to the qBittorrent WebUI returns plain-text `Unauthorized` immediately.

**Root cause**: qBittorrent's CSRF protection checks the `Referer` header. When JavaScript calls `window.location.replace(url)`, the browser includes `Referer: https://tauri.localhost/` in the outgoing request. qBittorrent sees a foreign origin and rejects it.

**Fix**: Use `win.navigate(parsed_url)` (Rust API) instead of JS `window.location.replace()`. A programmatic Rust-level navigation does not attach a `Referer` header — there is no "current page" in the host application's context. qBittorrent receives a referer-less request and allows it through.

```rust
// WRONG — carries Referer: https://tauri.localhost/
let _ = win.eval(&format!("window.location.replace('{}')", url));

// CORRECT — no Referer header, works with qBittorrent CSRF protection
if let Ok(parsed) = url.parse::<url::Url>() {
    let _ = win.navigate(parsed);
}
```

The same issue appears in the original userscript (`GoApp/test.js`) which used an `about:blank` intermediate step for the same reason — null origin = no Referer.

---

## 3. `tauri-plugin-window-state` Restores Hidden Windows as Visible

**Problem**: Even with `.visible(false)` in the builder, settings and about windows appear at startup if they were previously visible (e.g. during development). The plugin auto-restores the last-saved visibility state for all labelled windows.

**Fix**: Explicitly call `w.hide()` immediately after `build()`. This runs after any plugin callback and always wins.

```rust
if let Ok(w) = WebviewWindowBuilder::new(...).visible(false).build() {
    let _ = w.hide(); // force-hide after plugin restoration
}
```

---

## 4. Closing the Main Window Orphans the Tray Icon

**Problem**: When the user clicks X on the main window (with close-to-tray disabled), the window closes but the tray icon stays. The process is still running because Tauri does not exit while any window exists — the hidden settings and about windows keep it alive.

**Fix**: In the main window's `CloseRequested` handler, call `app.exit(0)` explicitly when close-to-tray is disabled. This forces a clean process exit regardless of hidden windows.

```rust
"main" => {
    if config.close_to_tray {
        window.hide(); api.prevent_close();
    } else {
        window.app_handle().exit(0); // kill process, removes tray icon
    }
}
```

---

## 5. Settings / About Windows Must Use `prevent_close()`

**Problem**: After the user closes the settings window, `get_webview_window("settings")` returns `None` — the window was destroyed. Subsequent `open_settings()` calls silently do nothing.

**Fix**: Intercept `CloseRequested` for settings and about windows and hide them instead of allowing destruction.

```rust
"settings" | "about" => {
    let _ = window.hide();
    api.prevent_close();
}
```

The window lives for the entire app lifetime. `open_settings()` / `open_about()` can always find and re-show it.

---

## 6. Logging Architecture

A dedicated channel-based logger is kept alive for the app's lifetime. `log()` is a non-blocking send — it never stalls the caller, even if a deadlock is in progress elsewhere.

```rust
static LOG_TX: OnceLock<mpsc::Sender<String>> = OnceLock::new();

fn init_logger() { /* spawn thread, open log.txt, loop on rx */ }
fn log(msg: &str) { LOG_TX.get().map(|tx| tx.send(msg.into())); }
```

Written in UTC with human-readable timestamps. Log is wiped (not rotated) when it exceeds 5 MB.

### File locations — platform reference

| File | Windows | macOS |
|------|---------|-------|
| `log.txt` | `%LOCALAPPDATA%\com.kritblade.qbwebuihelper\log.txt` | `~/Library/Application Support/com.kritblade.qbwebuihelper/log.txt` |
| `config.json` | `%LOCALAPPDATA%\com.kritblade.qbwebuihelper\config.json` | `~/Library/Application Support/com.kritblade.qbwebuihelper/config.json` |

#### Tail the log live (macOS)

```bash
tail -f ~/Library/Application\ Support/com.kritblade.qbwebuihelper/log.txt
```

#### Tail the log live (Windows PowerShell)

```powershell
Get-Content "$env:LOCALAPPDATA\com.kritblade.qbwebuihelper\log.txt" -Wait -Tail 50
```

### Full reset — delete all app data

Completely removes config, log, and cached window state. The next launch behaves as a first run (shows Settings automatically).

**macOS:**
```bash
rm -rf ~/Library/Application\ Support/com.kritblade.qbwebuihelper/
```

**Windows (PowerShell):**
```powershell
Remove-Item "$env:LOCALAPPDATA\com.kritblade.qbwebuihelper" -Recurse -Force
```

> ⚠️ If you have file associations set as default on macOS, click **Restore Previous Default** in Settings *before* deleting the config — the backup of your previous handler (e.g. Transmission) lives in `config.json`. Deleting it first means you can no longer restore automatically.

---

## 7. Windows Registry Changes — Register / Unregister

All writes are under `HKEY_CURRENT_USER` (no admin rights). The exe path is captured via `std::env::current_exe()` at the moment the Register button is clicked.

### Why we do NOT touch `HKCU\Software\Classes\.torrent` or `magnet`

Windows 10/11 resolves file/URL handlers in this priority order:
1. `HKCU\...\UserChoice` ← **active default**, written by Default Apps UI with a Microsoft-protected hash
2. `HKCU\Software\Classes\<ext>`
3. `HKLM\Software\Classes\<ext>` (set by installers)

Writing `HKCU\Software\Classes\.torrent` only takes effect if **no UserChoice exists**, and a UserChoice already exists on practically every user system (it's auto-set whenever the user opens the file type once). So claiming the extension that way is a no-op in real-world scenarios.

Instead, we follow the **Deluge / Chrome / Firefox pattern**: register only our own ProgIDs and `Capabilities`, then guide the user to Default Apps where Windows writes the valid UserChoice for us.

### What Register writes (matches Deluge's Group C + D exactly)

| Registry key | Value name | Set to |
|---|---|---|
| `Software\Classes\QBWebUIHelper.Torrent` | `(Default)` | `"Torrent File"` |
| `Software\Classes\QBWebUIHelper.Torrent\DefaultIcon` | `(Default)` | `"<exe>",0` |
| `Software\Classes\QBWebUIHelper.Torrent\shell\open\command` | `(Default)` | `"<exe>" "%1"` |
| `Software\Classes\QBWebUIHelper.Magnet` | `(Default)` | `"Magnet Link"` |
| `Software\Classes\QBWebUIHelper.Magnet\DefaultIcon` | `(Default)` | `"<exe>",0` |
| `Software\Classes\QBWebUIHelper.Magnet\shell\open\command` | `(Default)` | `"<exe>" "%1"` |
| `Software\QBWebUIHelper\Capabilities` | `ApplicationIcon` | `"<exe>",0` |
| `Software\QBWebUIHelper\Capabilities` | `ApplicationName` | `"QBWebUIHelper"` |
| `Software\QBWebUIHelper\Capabilities` | `ApplicationDescription` | `"qBittorrent WebUI Desktop Wrapper"` |
| `Software\QBWebUIHelper\Capabilities\FileAssociations` | `.torrent` | `"QBWebUIHelper.Torrent"` |
| `Software\QBWebUIHelper\Capabilities\URLAssociations` | `magnet` | `"QBWebUIHelper.Magnet"` |
| `Software\QBWebUIHelper\shell\open\command` | `(Default)` | `"<exe>"` (no `%1` — launch from picker) |
| `Software\RegisteredApplications` | `QBWebUIHelper` | `"Software\QBWebUIHelper\Capabilities"` |

After these writes, QBWebUIHelper appears in **Settings → Apps → Default Apps**. The user must explicitly click QBWebUIHelper in the picker for `.torrent` / `magnet` — at that point Windows writes a valid `UserChoice` entry with the protected hash, and we become the actual active handler.

### Transaction log — exact reversal

Before applying ANY mutation, `register()` reads the previous value of each (path, value-name) pair and saves the full list to `config.json` as `reg_backup`:

```json
"reg_backup": [
  { "path": "Software\\Classes\\QBWebUIHelper.Torrent", "name": "", "prev": null },
  { "path": "Software\\Classes\\QBWebUIHelper.Torrent\\DefaultIcon", "name": "", "prev": null },
  ...
  { "path": "Software\\RegisteredApplications", "name": "QBWebUIHelper", "prev": null }
]
```

`prev: null` means the value did not exist before. `prev: "..."` means a previous value will be restored on Unregister.

`Unregister()` walks the backup in reverse:
- `prev: Some(v)` → write `v` back to that (path, name)
- `prev: None` → delete that specific value (not the whole key)

Then a final sweep walks the subkeys we may have created (deepest first) and tries a non-recursive `delete_subkey` on each. Non-recursive deletes fail safely if the subkey still has any content, so we never accidentally nuke data added by another app.

`SHChangeNotify(SHCNE_ASSOCCHANGED)` fires at the end of both Register and Unregister so the shell picks up changes without a reboot.

### Failure handling

If a registry write fails mid-Register, the backup log is still persisted to `config.json` first (`cmd_register` saves backup before returning the error). Clicking Unregister then cleans up any partial writes.

### Warning: moving the exe after Register

The exe path is baked into `shell\open\command` at the time Register is clicked. If the `.exe` is moved, magnet/torrent opens will silently fail. **Always Unregister before moving the exe, then Register again from the new location.**

---

## 8. Icon Embedding — Cargo Incremental Build Trap

### The problem

Changing `icon.ico` (or any resource file) and running `cargo build --release` does **not** re-embed the new icon. Cargo caches the output of `build.rs` and only reruns it when the build script itself changes. The exe gets recompiled but keeps the icon from the previous build.

This creates a confusing state: the app window shows the new icon (loaded from `icon.png` at runtime via Tauri), but the file icon shown in Windows Explorer still shows the old one (extracted from the ICO resource embedded in the exe at compile time).

### Why the two icons differ

| Icon | Source file | When applied |
|---|---|---|
| App window / taskbar | `icons/icon.png` | Loaded at runtime by Tauri |
| File icon in Explorer | `icons/icon.ico` | Embedded into the exe by `tauri-build` at compile time |

Only the ICO embedding is affected by the cache problem.

### Fix

Touch `build.rs` to force its rerun, then rebuild:

```bat
copy /b "src-tauri\build.rs"+,, "src-tauri\build.rs" >nul
cargo build --release --manifest-path src-tauri\Cargo.toml
```

`buildme.bat` already does this automatically.

### Verifying the embedded icon

Extract and inspect the icon actually baked into an exe (PowerShell):

```powershell
Add-Type -AssemblyName System.Drawing
$icon = [System.Drawing.Icon]::ExtractAssociatedIcon("path\to\app.exe")
$icon.ToBitmap().Save("$env:TEMP\extracted.png", [System.Drawing.Imaging.ImageFormat]::Png)
# Open $env:TEMP\extracted.png to confirm
```

### Windows Explorer icon cache

Even after rebuilding with the correct icon, Explorer may still display the old one from its icon cache. To clear it:

```powershell
Stop-Process -Name explorer -Force
Remove-Item "$env:LOCALAPPDATA\Microsoft\Windows\Explorer\iconcache*" -Force -ErrorAction SilentlyContinue
Remove-Item "$env:LOCALAPPDATA\Microsoft\Windows\Explorer\thumbcache*" -Force -ErrorAction SilentlyContinue
Start-Process explorer
```

A full reboot also clears the cache. This is a Windows shell issue — unrelated to the build.

---

## 9. Platform Notes for macOS

The connection landing page (`index.html`) and TCP check are platform-neutral Rust — no changes needed. The `win.navigate(url)` fix (§2) applies to WebKit on macOS identically. Window pre-creation (§1) is the same.

### macOS LaunchServices — symbol names gotcha

The correct CoreServices symbols for content-type (UTI) handler management are:

```
LSCopyDefaultRoleHandlerForContentType   ← note "Role" in the name
LSSetDefaultRoleHandlerForContentType    ← note "Role" in the name
```

The URL-scheme variants are named differently (no "Role"):

```
LSCopyDefaultHandlerForURLScheme
LSSetDefaultHandlerForURLScheme
```

Using the wrong names causes an `undefined symbol` linker error on macOS — magnet: links will work but `.torrent` will fail at link time.

### macOS Info.plist requirements for file association claiming

Three keys are required for `LSSetDefaultRoleHandlerForContentType` to accept the claim:

1. **`CFBundleDocumentTypes`** must include an entry with `LSItemContentTypes: ["com.bittorrent.torrent"]`. Tauri's `fileAssociations` in `tauri.conf.json` generates `CFBundleTypeExtensions` but NOT `LSItemContentTypes`. Override via custom `src-tauri/Info.plist`.

2. **`UTImportedTypeDeclarations`** — declares `com.bittorrent.torrent` to Launch Services. Without this, if no BitTorrent client (Transmission, etc.) is installed, the UTI is unknown and the LS call returns `kLSUnknownTypeErr (-10809)`. Add to custom `Info.plist`.

3. **`NSLocalNetworkUsageDescription`** — macOS may block TCP connections to LAN IPs without this key in the bundle. Add to custom `Info.plist`.

See `src-tauri/Info.plist` for the current complete definition of all three.

### macOS global app menu

On macOS, `WebviewWindowBuilder::menu()` does NOT replace the system app menu. Use `app.set_menu()` in `setup()` with a full menu structure (app submenu → About, Settings ⌘,, Services, Hide, Quit; plus Edit, View, Window submenus). See `build_mac_app_menu()` in `lib.rs`.

### macOS association confirmation dialog

When `LSSetDefaultRoleHandlerForContentType` is called and another app currently owns the type, macOS shows a system dialog asynchronously (e.g. "Do you want .torrent to open with QBWebUIHelper or keep using Free Download Manager?"). The LS call returns success **before** the user dismisses the dialog. Poll `cmd_is_registered` after calling `cmd_register` rather than checking status immediately — the settings UI does this with `pollUntilRegistered(15)` in `settings.html`.
