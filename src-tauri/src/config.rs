use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct RegMutation {
    pub path: String,
    pub name: String,
    pub prev: Option<String>,
}

/// Saved previous default handlers on macOS, so we can restore them on unregister.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct MacBackup {
    #[serde(default)]
    pub prev_magnet_handler: Option<String>,
    #[serde(default)]
    pub prev_torrent_handler: Option<String>,
}

impl MacBackup {
    pub fn has_any(&self) -> bool {
        self.prev_magnet_handler.is_some() || self.prev_torrent_handler.is_some()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    #[serde(default = "default_url")]
    pub webui_url: String,
    #[serde(default)]
    pub close_to_tray: bool,
    /// When true, verbose deep-link / inject diagnostics are written to log.txt.
    /// Off by default so the log stays small in steady-state use.
    #[serde(default)]
    pub debug_logging: bool,
    /// Windows registry backup (populated only on Windows after Register).
    #[serde(default)]
    pub reg_backup: Vec<RegMutation>,
    /// macOS LaunchServices backup (populated only on macOS after Set as Default).
    #[serde(default)]
    pub mac_backup: MacBackup,
}

fn default_url() -> String { "http://localhost:8080".to_string() }

impl Default for Config {
    fn default() -> Self {
        Config {
            webui_url: default_url(),
            close_to_tray: false,
            debug_logging: false,
            reg_backup: Vec::new(),
            mac_backup: MacBackup::default(),
        }
    }
}

fn config_path(app: &AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .expect("failed to get app data dir")
        .join("config.json")
}

pub fn load(app: &AppHandle) -> Config {
    let path = config_path(app);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(app: &AppHandle, cfg: &Config) {
    let path = config_path(app);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(path, json);
    }
}
