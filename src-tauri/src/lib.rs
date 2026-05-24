mod associations;
mod config;

use serde::{Deserialize, Serialize};
use tauri::Manager;

static LOG_TX: std::sync::OnceLock<std::sync::mpsc::Sender<String>> = std::sync::OnceLock::new();
static PENDING_ACTION: std::sync::Mutex<Option<PendingAction>> = std::sync::Mutex::new(None);
static DEBUG_LOG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Proleptic Gregorian calendar from Unix epoch (1970-01-01)
    let mut d = days + 719468;
    let era = d / 146097;
    let doe = d % 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    (y, mo, d)
}

fn init_logger(log_path: std::path::PathBuf) {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    LOG_TX.set(tx).ok();
    std::thread::spawn(move || {
        use std::io::Write;
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let over_limit = std::fs::metadata(&log_path)
            .map(|m| m.len() > 5 * 1024 * 1024)
            .unwrap_or(false);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(!over_limit)
            .write(over_limit)
            .truncate(over_limit)
            .open(&log_path)
        {
            for msg in rx {
                let now = std::time::SystemTime::now();
                let secs = now
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default().as_secs();
                let s = secs % 60;
                let m = (secs / 60) % 60;
                let h = (secs / 3600) % 24;
                let days = secs / 86400;
                let (y, mo, d) = days_to_ymd(days);
                let _ = writeln!(f, "[{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02} UTC] {msg}");
                let _ = f.flush();
            }
        }
    });
}

fn log(msg: &str) {
    if let Some(tx) = LOG_TX.get() {
        let _ = tx.send(msg.to_string());
    }
}

/// Gated by the Debug Logging setting. Drop verbose diagnostics here so the
/// log doesn't bloat in steady-state use. Toggle from Settings UI.
fn debug_log(msg: &str) {
    if DEBUG_LOG.load(std::sync::atomic::Ordering::Relaxed) {
        log(msg);
    }
}

static ACTION_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn next_action_id() -> u64 {
    ACTION_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct PendingAction {
    id: u64,
    #[serde(rename = "type")]
    action_type: String,
    url: Option<String>,
    filename: Option<String>,
    data: Option<String>,
}

fn build_action(arg: &str) -> Option<PendingAction> {
    if arg.starts_with("magnet:") {
        return Some(PendingAction {
            id: next_action_id(),
            action_type: "magnet".into(),
            url: Some(arg.to_string()),
            filename: None,
            data: None,
        });
    }
    // macOS deep-link plugin sends .torrent file opens as file:// URLs.
    let path_str = if arg.starts_with("file://") {
        url::Url::parse(arg).ok()
            .and_then(|u| u.to_file_path().ok())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| arg.to_string())
    } else {
        arg.to_string()
    };
    debug_log(&format!("build_action: reading torrent file {}", path_str));
    match std::fs::read(&path_str) {
        Ok(bytes) => {
            let filename = std::path::Path::new(&path_str)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file.torrent")
                .to_string();
            debug_log(&format!("build_action: read {} bytes from {}", bytes.len(), filename));
            use base64::Engine;
            Some(PendingAction {
                id: next_action_id(),
                action_type: "torrent".into(),
                url: None,
                filename: Some(filename),
                data: Some(base64::engine::general_purpose::STANDARD.encode(&bytes)),
            })
        }
        Err(e) => {
            log(&format!("build_action: failed to read {}: {}", path_str, e));
            None
        }
    }
}

fn inject_action(window: &tauri::WebviewWindow, action: &PendingAction) {
    let url = window.url().map(|u| u.to_string()).unwrap_or_else(|_| "?".to_string());
    debug_log(&format!("inject_action: id={} into {}", action.id, url));
    if let Ok(json) = serde_json::to_string(action) {
        if let Err(e) = window.eval(&format!("if (typeof window.__qbHelper_handle === 'function') window.__qbHelper_handle({})", json)) {
            log(&format!("inject_action: eval error {}", e));
        }
    }
}

/// Inject a single time, then retry a few times in case the WebUI is still
/// loading. The JS dedupes by action.id so duplicate injections are harmless.
fn inject_with_retries(win: tauri::WebviewWindow, action: PendingAction) {
    std::thread::spawn(move || {
        inject_action(&win, &action);
        for &delay_ms in &[500u64, 1500, 3500] {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            inject_action(&win, &action);
        }
    });
}

/// Dedupe deep-link URLs ONLY during the cold-start window. On macOS, the
/// deep-link plugin delivers the launch URL via two paths (on_open_url AND
/// get_current()) — both represent the same user click. Once get_current()'s
/// polling thread has finished, only on_open_url can fire, so every URL after
/// that is a new user action and must NOT be deduped (otherwise repeat-clicks
/// of the same .torrent/magnet are silently dropped).
#[cfg(target_os = "macos")]
static COLDSTART_URLS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
#[cfg(target_os = "macos")]
static COLDSTART_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(target_os = "macos")]
fn dispatch_url(app_handle: &tauri::AppHandle, dl_url: &str) {
    if !COLDSTART_DONE.load(std::sync::atomic::Ordering::Relaxed) {
        let mut seen = COLDSTART_URLS.lock().unwrap();
        if seen.iter().any(|u| u == dl_url) {
            log(&format!("deep-link (cold-start dup, skipped): {}", dl_url));
            return;
        }
        seen.push(dl_url.to_string());
    }
    log(&format!("deep-link: {}", dl_url));
    if let Some(action) = build_action(dl_url) {
        debug_log(&format!("dispatch_url: built action id={} type={}", action.id, action.action_type));
        // Store so JS can pull it once the WebUI is ready (handles cold-start
        // where the WebUI hasn't finished loading when the URL arrives).
        if let Ok(mut g) = PENDING_ACTION.lock() {
            *g = Some(action.clone());
            debug_log("dispatch_url: stored pending action");
        }
        if let Some(w) = app_handle.get_webview_window("main") {
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
            // Direct injection covers the app-already-open case fast.
            // JS dedupes by id, so the polling path won't double-process.
            debug_log("dispatch_url: triggering inject_with_retries");
            inject_with_retries(w, action);
        } else {
            log("dispatch_url: main window not found yet");
        }
    } else {
        log(&format!("dispatch_url: build_action returned None for {}", dl_url));
    }
}

fn check_tcp_connection(url: &str) -> bool {
    (|| -> Option<bool> {
        let parsed = url.parse::<url::Url>().ok()?;
        let host = parsed.host_str()?.to_string();
        let port = parsed.port_or_known_default()?;
        let addr = format!("{}:{}", host, port);
        let sock_addr: std::net::SocketAddr = addr.parse().ok()?;
        Some(
            std::net::TcpStream::connect_timeout(
                &sock_addr,
                std::time::Duration::from_secs(5),
            )
            .is_ok(),
        )
    })()
    .unwrap_or(false)
}

fn config_exists(app: &tauri::AppHandle) -> bool {
    app.path()
        .app_data_dir()
        .map(|d| d.join("config.json").exists())
        .unwrap_or(false)
}

fn connect_flow(win: &tauri::WebviewWindow, url: &str, startup_action: Option<PendingAction>) {
    let escaped = url.replace('\'', "\\'");
    let _ = win.eval(&format!("setConnecting('{}')", escaped));
    if check_tcp_connection(url) {
        if let Ok(parsed) = url.parse::<url::Url>() {
            let _ = win.navigate(parsed);
        }
        if let Some(action) = startup_action {
            std::thread::sleep(std::time::Duration::from_millis(500));
            inject_action(win, &action);
        }
    } else {
        let _ = win.eval(&format!("showError('{}')", escaped));
    }
}

fn trigger_connect(win: tauri::WebviewWindow, url: String, startup_action: Option<PendingAction>) {
    #[cfg(target_os = "windows")]
    let _ = win.eval("window.location.replace('https://tauri.localhost/index.html')");
    #[cfg(not(target_os = "windows"))]
    let _ = win.eval("window.location.replace('tauri://localhost/index.html')");

    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        connect_flow(&win, &url, startup_action);
    });
}

fn toggle_main_window(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        if w.is_visible().unwrap_or(false) {
            let _ = w.hide();
        } else {
            let _ = w.show();
            let _ = w.set_focus();
        }
    }
}

fn open_settings(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn open_about(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("about") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

#[cfg(target_os = "macos")]
fn build_mac_app_menu(app: &tauri::App) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};

    let about_item = MenuItemBuilder::with_id("menu_about", "About QBWebUIHelper").build(app)?;
    let settings_item = MenuItemBuilder::with_id("menu_settings", "Settings…")
        .accelerator("Cmd+,")
        .build(app)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let sep3 = PredefinedMenuItem::separator(app)?;
    let sep4 = PredefinedMenuItem::separator(app)?;
    let services = PredefinedMenuItem::services(app, None)?;
    let hide = PredefinedMenuItem::hide(app, None)?;
    let hide_others = PredefinedMenuItem::hide_others(app, None)?;
    let show_all = PredefinedMenuItem::show_all(app, None)?;
    let quit = PredefinedMenuItem::quit(app, None)?;

    let app_submenu = SubmenuBuilder::new(app, "QBWebUIHelper")
        .items(&[
            &about_item,
            &sep1,
            &settings_item,
            &sep2,
            &services,
            &sep3,
            &hide,
            &hide_others,
            &show_all,
            &sep4,
            &quit,
        ])
        .build()?;

    let edit_undo = PredefinedMenuItem::undo(app, None)?;
    let edit_redo = PredefinedMenuItem::redo(app, None)?;
    let edit_sep = PredefinedMenuItem::separator(app)?;
    let edit_cut = PredefinedMenuItem::cut(app, None)?;
    let edit_copy = PredefinedMenuItem::copy(app, None)?;
    let edit_paste = PredefinedMenuItem::paste(app, None)?;
    let edit_select_all = PredefinedMenuItem::select_all(app, None)?;
    let edit_submenu = SubmenuBuilder::new(app, "Edit")
        .items(&[&edit_undo, &edit_redo, &edit_sep, &edit_cut, &edit_copy, &edit_paste, &edit_select_all])
        .build()?;

    let view_fullscreen = PredefinedMenuItem::fullscreen(app, None)?;
    let view_submenu = SubmenuBuilder::new(app, "View")
        .items(&[&view_fullscreen])
        .build()?;

    let win_minimize = PredefinedMenuItem::minimize(app, None)?;
    let win_maximize = PredefinedMenuItem::maximize(app, None)?;
    let win_submenu = SubmenuBuilder::new(app, "Window")
        .items(&[&win_minimize, &win_maximize])
        .build()?;

    MenuBuilder::new(app)
        .items(&[&app_submenu, &edit_submenu, &view_submenu, &win_submenu])
        .build()
}

fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem};
    use tauri::tray::TrayIconBuilder;

    let show_hide = MenuItemBuilder::with_id("show_hide", "Show / Hide Window").build(app)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let settings = MenuItemBuilder::with_id("settings", "Settings...").build(app)?;
    let about = MenuItemBuilder::with_id("about", "About").build(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;

    let menu = MenuBuilder::new(app)
        .items(&[&show_hide, &sep1, &settings, &about, &sep2, &quit])
        .build()?;

    // On macOS the menu bar icon is click-to-open-menu (native convention).
    // On Windows left-click toggles the window; the menu appears on right-click.
    #[cfg(target_os = "macos")]
    TrayIconBuilder::new()
        .icon(app.default_window_icon().unwrap().clone())
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show_hide" => toggle_main_window(app),
            "settings"  => open_settings(app),
            "about"     => open_about(app),
            "quit"      => app.exit(0),
            _ => {}
        })
        .build(app)?;

    #[cfg(not(target_os = "macos"))]
    TrayIconBuilder::new()
        .icon(app.default_window_icon().unwrap().clone())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show_hide" => toggle_main_window(app),
            "settings"  => open_settings(app),
            "about"     => open_about(app),
            "quit"      => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                toggle_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

// ── IPC commands ────────────────────────────────────────────────────────────

#[tauri::command]
fn cmd_get_config(app: tauri::AppHandle) -> config::Config {
    config::load(&app)
}

#[tauri::command]
fn cmd_save_url(app: tauri::AppHandle, url: String) -> bool {
    let mut cfg = config::load(&app);
    cfg.webui_url = url.clone();
    config::save(&app, &cfg);
    let ok = check_tcp_connection(&url);
    if let Some(w) = app.get_webview_window("main") {
        trigger_connect(w, url, None);
    }
    if ok {
        if let Some(w) = app.get_webview_window("settings") {
            let _ = w.hide();
        }
    }
    ok
}

#[tauri::command]
fn cmd_set_close_to_tray(app: tauri::AppHandle, enabled: bool) {
    let mut cfg = config::load(&app);
    cfg.close_to_tray = enabled;
    config::save(&app, &cfg);
}

#[tauri::command]
fn cmd_open_settings(app: tauri::AppHandle) {
    open_settings(&app);
}

#[tauri::command]
fn cmd_retry(app: tauri::AppHandle) {
    let cfg = config::load(&app);
    let url = cfg.webui_url.clone();
    if let Some(w) = app.get_webview_window("main") {
        std::thread::spawn(move || {
            connect_flow(&w, &url, None);
        });
    }
}

#[tauri::command]
fn cmd_get_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[tauri::command]
fn cmd_is_registered() -> bool {
    associations::is_registered()
}

#[tauri::command]
fn cmd_register(app: tauri::AppHandle) -> Result<(), String> {
    platform_register(&app)
}

#[cfg(target_os = "windows")]
fn platform_register(app: &tauri::AppHandle) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe_str = exe.to_str().ok_or("invalid exe path")?;
    let (backup, result) = associations::register(exe_str);
    let mut cfg = config::load(app);
    if cfg.reg_backup.is_empty() {
        cfg.reg_backup = backup;
    }
    config::save(app, &cfg);
    result
}

#[cfg(target_os = "macos")]
fn platform_register(app: &tauri::AppHandle) -> Result<(), String> {
    let (new_backup, result) = associations::register();
    let mut cfg = config::load(app);
    // Only preserve the backup on the first registration — subsequent clicks
    // would otherwise snapshot our own bundle ID as the "previous" handler.
    if !cfg.mac_backup.has_any() {
        cfg.mac_backup = new_backup;
    }
    config::save(app, &cfg);
    result
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn platform_register(_app: &tauri::AppHandle) -> Result<(), String> {
    Err("Not supported on this platform".to_string())
}

#[tauri::command]
fn cmd_unregister(app: tauri::AppHandle) {
    platform_unregister(&app);
}

#[cfg(target_os = "windows")]
fn platform_unregister(app: &tauri::AppHandle) {
    let cfg = config::load(app);
    associations::unregister(&cfg.reg_backup);
    let mut cfg = config::load(app);
    cfg.reg_backup.clear();
    config::save(app, &cfg);
}

#[cfg(target_os = "macos")]
fn platform_unregister(app: &tauri::AppHandle) {
    let cfg = config::load(app);
    associations::unregister(&cfg.mac_backup);
    let mut cfg = config::load(app);
    cfg.mac_backup = config::MacBackup::default();
    config::save(app, &cfg);
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn platform_unregister(_app: &tauri::AppHandle) {}

#[tauri::command]
fn cmd_open_default_apps() {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/c", "start", "ms-settings:defaultapps"])
        .spawn();
}

/// Returns "windows", "macos", or "other" so the settings UI can show the
/// correct platform-specific file-association controls.
#[tauri::command]
fn cmd_get_platform() -> &'static str {
    #[cfg(target_os = "windows")] { "windows" }
    #[cfg(target_os = "macos")]   { "macos"   }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))] { "other" }
}

/// Returns true when a macOS backup exists in config (so "Restore Previous
/// Default" button should be shown in Settings).
#[tauri::command]
fn cmd_has_mac_backup(app: tauri::AppHandle) -> bool {
    config::load(&app).mac_backup.has_any()
}

#[tauri::command]
fn cmd_js_log(msg: String) {
    debug_log(&format!("[JS] {}", msg));
}

#[tauri::command]
fn cmd_get_pending_action() -> Option<PendingAction> {
    let action = PENDING_ACTION.lock().ok().and_then(|mut g| g.take());
    debug_log(&format!("cmd_get_pending_action → {}",
        match &action {
            Some(a) => format!("Some(id={}, type={})", a.id, a.action_type),
            None => "None".to_string(),
        }));
    action
}

#[tauri::command]
fn cmd_set_debug_logging(app: tauri::AppHandle, enabled: bool) {
    DEBUG_LOG.store(enabled, std::sync::atomic::Ordering::Relaxed);
    let mut cfg = config::load(&app);
    cfg.debug_logging = enabled;
    config::save(&app, &cfg);
    log(&format!("debug_logging set to {}", enabled));
}

// ── App entry ───────────────────────────────────────────────────────────────

pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
                if let Some(arg) = args.get(1) {
                    if let Some(action) = build_action(arg) {
                        inject_with_retries(w, action);
                    }
                }
            }
        }))
        .plugin(tauri_plugin_window_state::Builder::default().build());

    // Deep-link plugin handles magnet: URL events and file:// .torrent opens on macOS.
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_plugin_deep_link::init());

    builder.setup(|app| {
            // Logger goes to app_data_dir/log.txt (platform-correct, writable on macOS).
            let log_path = app.path()
                .app_data_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
                .join("log.txt");
            init_logger(log_path);
            log("--- app start ---");

            let startup_action = std::env::args().nth(1).as_deref().and_then(build_action);
            let is_first_run = !config_exists(app.handle());
            let cfg = config::load(app.handle());
            let url = cfg.webui_url.clone();
            DEBUG_LOG.store(cfg.debug_logging, std::sync::atomic::Ordering::Relaxed);

            // Wire up the deep-link handler (macOS: receives magnet: URLs and
            // file:// paths for .torrent files opened from Finder).
            #[cfg(target_os = "macos")]
            {
                use tauri_plugin_deep_link::DeepLinkExt;

                // Runtime URLs (app already open).
                let app_handle_rt = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for dl_url in event.urls() {
                        dispatch_url(&app_handle_rt, dl_url.as_str());
                    }
                });

                // Cold-start URL: on_open_url does NOT reliably fire for the
                // URL the app was launched with on macOS. Poll get_current() —
                // the plugin populates it once tauri::RunEvent::Opened arrives.
                // When the loop exits (found something or timed out), flip
                // COLDSTART_DONE so dispatch_url stops deduping. After that,
                // every URL coming through on_open_url is a new user click.
                let app_handle_cs = app.handle().clone();
                std::thread::spawn(move || {
                    for &delay_ms in &[100u64, 300, 700, 1500, 3000] {
                        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                        if let Ok(Some(urls)) = app_handle_cs.deep_link().get_current() {
                            debug_log(&format!("get_current returned {} url(s)", urls.len()));
                            for url in urls {
                                dispatch_url(&app_handle_cs, url.as_str());
                            }
                            break;
                        }
                    }
                    COLDSTART_DONE.store(true, std::sync::atomic::Ordering::Relaxed);
                    debug_log("cold-start window closed (dedup disabled)");
                });
            }

            // Windows: window-level title-bar menu (Settings > Settings… | About).
            // macOS:   global app menu with About/Settings inside the app submenu.
            #[cfg(target_os = "macos")]
            {
                let menu = build_mac_app_menu(app)?;
                app.set_menu(menu)?;
                app.on_menu_event(|app, event| match event.id().as_ref() {
                    "menu_settings" => open_settings(app),
                    "menu_about"    => open_about(app),
                    _ => {}
                });
            }

            #[cfg(target_os = "windows")]
            let win_menu = {
                use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
                let settings_item = MenuItemBuilder::with_id("menu_settings", "Settings...").build(app)?;
                let about_item    = MenuItemBuilder::with_id("menu_about",    "About").build(app)?;
                let sep           = PredefinedMenuItem::separator(app)?;
                let submenu = SubmenuBuilder::new(app, "Settings")
                    .items(&[&settings_item, &sep, &about_item])
                    .build()?;
                MenuBuilder::new(app).items(&[&submenu]).build()?
            };

            let win_builder = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title(&format!("QBWebUIHelper {}", env!("CARGO_PKG_VERSION")))
            .inner_size(1600.0, 900.0)
            .initialization_script(helper_js())
            .visible(false);

            #[cfg(target_os = "windows")]
            let win_builder = win_builder.menu(win_menu);

            let win = win_builder.build()?;

            #[cfg(target_os = "windows")]
            win.on_menu_event(|win, event| match event.id().as_ref() {
                "menu_settings" => open_settings(win.app_handle()),
                "menu_about"    => open_about(win.app_handle()),
                _ => {}
            });

            use tauri_plugin_window_state::WindowExt;
            let _ = win.restore_state(tauri_plugin_window_state::StateFlags::all());

            win.show()?;

            // If a deep-link arrived before the main window existed (common for
            // .torrent file:// URLs on macOS — they're delivered very early in
            // the launch sequence), dispatch_url stored the action but couldn't
            // start inject_with_retries. Drain it now.
            if let Some(action) = PENDING_ACTION.lock().ok().and_then(|g| g.clone()) {
                debug_log(&format!("drain early pending action id={} type={}", action.id, action.action_type));
                inject_with_retries(win.clone(), action);
            }

            if let Ok(w) = tauri::WebviewWindowBuilder::new(
                app, "settings",
                tauri::WebviewUrl::App("settings.html".into()),
            )
            .title("Settings — QBWebUIHelper")
            .inner_size(500.0, 480.0)
            .resizable(false).maximizable(false)
            .visible(false)
            .build() {
                let _ = w.hide();
            }

            if let Ok(w) = tauri::WebviewWindowBuilder::new(
                app, "about",
                tauri::WebviewUrl::App("about.html".into()),
            )
            .title("About QBWebUIHelper")
            .inner_size(380.0, 420.0)
            .resizable(false).maximizable(false)
            .visible(false)
            .build() {
                let _ = w.hide();
            }

            let app_handle = app.handle().clone();
            let win_clone = win.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(300));
                if is_first_run {
                    let _ = win_clone.eval("showFirstRun()");
                    open_settings(&app_handle);
                } else {
                    connect_flow(&win_clone, &url, None);
                    if let Some(action) = startup_action {
                        if let Ok(mut g) = PENDING_ACTION.lock() {
                            *g = Some(action.clone());
                        }
                        inject_with_retries(win_clone, action);
                    }
                }
            });

            setup_tray(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            let label = window.label().to_string();
            match event {
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    match label.as_str() {
                        "main" => {
                            if config::load(window.app_handle()).close_to_tray {
                                let _ = window.hide();
                                api.prevent_close();
                            } else {
                                window.app_handle().exit(0);
                            }
                        }
                        "settings" | "about" => {
                            let _ = window.hide();
                            api.prevent_close();
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            cmd_get_config,
            cmd_save_url,
            cmd_set_close_to_tray,
            cmd_open_settings,
            cmd_retry,
            cmd_is_registered,
            cmd_register,
            cmd_unregister,
            cmd_open_default_apps,
            cmd_get_platform,
            cmd_has_mac_backup,
            cmd_get_version,
            cmd_js_log,
            cmd_get_pending_action,
            cmd_set_debug_logging,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ── Injected JavaScript ─────────────────────────────────────────────────────

fn helper_js() -> &'static str {
    r#"
function __qbLog(msg) {
    if (window.__TAURI__) window.__TAURI__.core.invoke('cmd_js_log', { msg: msg });
}

// Wait for the qBittorrent WebUI's Client object to appear, then invoke cb(client).
// Used by both magnet and .torrent paths — the only thing that differs is the
// API call inside the callback. 30 attempts × 300ms = 9-second budget.
function __qbWaitForClient(label, cb) {
    function attempt(n) {
        if (n <= 0) { __qbLog(label + ': qBittorrent.Client not available after retries'); return; }
        var client = window.qBittorrent && window.qBittorrent.Client;
        if (client) {
            __qbLog(label + ': client ready on attempt ' + (31 - n));
            try { cb(client); } catch (e) { __qbLog(label + ': error ' + e); }
        } else {
            if (n === 30 || n === 15) __qbLog(label + ': waiting for qBittorrent.Client (attempt ' + (31 - n) + ')');
            setTimeout(function() { attempt(n - 1); }, 300);
        }
    }
    attempt(30);
}

window.__qbHelper_handle = function(action) {
    // Dedupe by action id across all delivery paths (direct inject + polling).
    if (window.__qbHelper_lastId === action.id) {
        __qbLog('__qbHelper_handle: duplicate id=' + action.id + ' ignored');
        return;
    }
    window.__qbHelper_lastId = action.id;
    __qbLog('__qbHelper_handle id=' + action.id + ' type=' + action.type);

    if (action.type === 'magnet') {
        __qbWaitForClient('addMagnet', function(client) {
            if (typeof client.createAddTorrentWindow === 'function') {
                client.createAddTorrentWindow('Magnet link', action.url);
            } else if (typeof showDownloadPage === 'function') {
                __qbLog('addMagnet: createAddTorrentWindow missing, fallback to showDownloadPage');
                showDownloadPage([action.url]);
            } else {
                __qbLog('addMagnet: no API available — giving up');
            }
        });
    } else if (action.type === 'torrent') {
        __qbLog('addTorrent: ' + action.filename + ' (' + (action.data ? action.data.length : 0) + ' base64 chars)');
        __qbWaitForClient('addTorrent', function(client) {
            if (typeof client.uploadTorrentFiles !== 'function') {
                __qbLog('addTorrent: uploadTorrentFiles missing on Client — giving up');
                return;
            }
            var binary = atob(action.data);
            var bytes = new Uint8Array(binary.length);
            for (var i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
            var file = new File([bytes], action.filename, { type: 'application/x-bittorrent' });
            client.uploadTorrentFiles([file]);
            __qbLog('addTorrent: uploadTorrentFiles called (file size=' + bytes.length + ')');
        });
    }
};

// On every page load, wait for the qBittorrent WebUI to be ready, then pull
// any pending action from Rust. This is the primary delivery path for cold
// starts — direct injection races the WebUI load and often loses.
window.__qbCheckPending = function() {
    __qbLog('__qbCheckPending started on ' + window.location.href);
    var startTime = Date.now();
    function attempt(n) {
        if (n <= 0) { __qbLog('__qbCheckPending: timed out after ' + (Date.now() - startTime) + 'ms'); return; }
        var client = window.qBittorrent && window.qBittorrent.Client;
        if (client && typeof client.createAddTorrentWindow === 'function') {
            __qbLog('__qbCheckPending: WebUI ready after ' + (Date.now() - startTime) + 'ms');
            if (!window.__TAURI__) { __qbLog('__qbCheckPending: no Tauri IPC available'); return; }
            window.__TAURI__.core.invoke('cmd_get_pending_action').then(function(action) {
                if (action) {
                    __qbLog('__qbCheckPending: got action id=' + action.id);
                    window.__qbHelper_handle(action);
                } else {
                    __qbLog('__qbCheckPending: no pending action');
                }
            }).catch(function(e) {
                __qbLog('__qbCheckPending: invoke error ' + e);
            });
        } else {
            if (n === 60 || n === 30) __qbLog('__qbCheckPending: waiting (attempt ' + (61 - n) + ')');
            setTimeout(function() { attempt(n - 1); }, 300);
        }
    }
    attempt(60); // 60 × 300ms = 18 seconds budget for WebUI load
};

__qbLog('helper_js loaded on ' + window.location.href);
window.__qbCheckPending();
"#
}
