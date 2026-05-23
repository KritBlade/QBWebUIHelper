use crate::config::RegMutation;

// ── Windows ──────────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
mod win {
    use super::RegMutation;
    use winreg::enums::*;
    use winreg::RegKey;

    pub fn is_registered() -> bool {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        hkcu.open_subkey(r"Software\RegisteredApplications")
            .ok()
            .and_then(|k| k.get_value::<String, _>("QBWebUIHelper").ok())
            .is_some()
    }

    fn planned_mutations(exe_path: &str) -> Vec<(&'static str, &'static str, String)> {
        let cmd = format!("\"{}\" \"%1\"", exe_path);
        let cmd_noargs = format!("\"{}\"", exe_path);
        let icon = format!("\"{}\",0", exe_path);

        vec![
            // ---------- Group C: our .torrent ProgID ----------
            (r"Software\Classes\QBWebUIHelper.Torrent", "", "Torrent File".to_string()),
            (r"Software\Classes\QBWebUIHelper.Torrent\DefaultIcon", "", icon.clone()),
            (r"Software\Classes\QBWebUIHelper.Torrent\shell\open\command", "", cmd.clone()),

            // ---------- Group C: our magnet ProgID ----------
            (r"Software\Classes\QBWebUIHelper.Magnet", "", "Magnet Link".to_string()),
            (r"Software\Classes\QBWebUIHelper.Magnet\DefaultIcon", "", icon.clone()),
            (r"Software\Classes\QBWebUIHelper.Magnet\shell\open\command", "", cmd.clone()),

            // ---------- Group D: Capabilities ----------
            (r"Software\QBWebUIHelper\Capabilities", "ApplicationIcon", icon.clone()),
            (r"Software\QBWebUIHelper\Capabilities", "ApplicationName", "QBWebUIHelper".to_string()),
            (r"Software\QBWebUIHelper\Capabilities", "ApplicationDescription", "qBittorrent WebUI Desktop Wrapper".to_string()),
            (r"Software\QBWebUIHelper\Capabilities\FileAssociations", ".torrent", "QBWebUIHelper.Torrent".to_string()),
            (r"Software\QBWebUIHelper\Capabilities\URLAssociations", "magnet", "QBWebUIHelper.Magnet".to_string()),
            (r"Software\QBWebUIHelper\shell\open\command", "", cmd_noargs),

            // ---------- Group D: RegisteredApplications entry ----------
            (r"Software\RegisteredApplications", "QBWebUIHelper", r"Software\QBWebUIHelper\Capabilities".to_string()),
        ]
    }

    /// Reads the previous values for every planned mutation, then applies the writes.
    /// Returns (backup_log, write_result). The backup is always populated, even on
    /// write failure, so the caller can persist it and call `unregister` to clean up.
    pub fn register(exe_path: &str) -> (Vec<RegMutation>, Result<(), String>) {
        let mutations = planned_mutations(exe_path);
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);

        // Phase 1: snapshot previous values (None = key/value did not exist).
        // Filter out values that already point to our own ProgID or exe so that a
        // second Register call doesn't overwrite the real previous-handler backup
        // with a self-reference that would loop back to QBWebUIHelper on unregister.
        let is_our_value = |v: &str| v.to_ascii_lowercase().contains("qbwebuihelper");
        let backup: Vec<RegMutation> = mutations.iter().map(|(path, name, _)| {
            let prev = hkcu.open_subkey(path)
                .ok()
                .and_then(|k| k.get_value::<String, _>(*name).ok())
                .filter(|v| !is_our_value(v));
            RegMutation {
                path: path.to_string(),
                name: name.to_string(),
                prev,
            }
        }).collect();

        // Phase 2: apply writes.
        let result = (|| -> Result<(), String> {
            for (path, name, value) in &mutations {
                let (key, _) = hkcu.create_subkey(path)
                    .map_err(|e| format!("create_subkey {}: {}", path, e))?;
                key.set_value(*name, value)
                    .map_err(|e| format!("set_value {}/{}: {}", path, name, e))?;
            }
            notify_assoc_changed();
            Ok(())
        })();

        (backup, result)
    }

    /// Walks the backup log in reverse, restoring each value or deleting it if it
    /// did not exist previously. Then sweeps any empty subkeys we created.
    pub fn unregister(backup: &[RegMutation]) {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);

        // Phase 1: restore each value.
        for entry in backup.iter().rev() {
            match &entry.prev {
                Some(v) => {
                    if let Ok((key, _)) = hkcu.create_subkey(&entry.path) {
                        let _ = key.set_value(&entry.name, &v.to_string());
                    }
                }
                None => {
                    if let Ok(key) = hkcu.open_subkey_with_flags(&entry.path, KEY_WRITE) {
                        let _ = key.delete_value(&entry.name);
                    }
                }
            }
        }

        // Phase 2: sweep subkeys we may have created. delete_subkey is non-recursive
        // and fails harmlessly if the key still has values or children.
        let sweep_paths = [
            r"Software\Classes\QBWebUIHelper.Torrent\shell\open\command",
            r"Software\Classes\QBWebUIHelper.Torrent\shell\open",
            r"Software\Classes\QBWebUIHelper.Torrent\shell",
            r"Software\Classes\QBWebUIHelper.Torrent\DefaultIcon",
            r"Software\Classes\QBWebUIHelper.Torrent",
            r"Software\Classes\QBWebUIHelper.Magnet\shell\open\command",
            r"Software\Classes\QBWebUIHelper.Magnet\shell\open",
            r"Software\Classes\QBWebUIHelper.Magnet\shell",
            r"Software\Classes\QBWebUIHelper.Magnet\DefaultIcon",
            r"Software\Classes\QBWebUIHelper.Magnet",
            r"Software\QBWebUIHelper\Capabilities\FileAssociations",
            r"Software\QBWebUIHelper\Capabilities\URLAssociations",
            r"Software\QBWebUIHelper\Capabilities",
            r"Software\QBWebUIHelper\shell\open\command",
            r"Software\QBWebUIHelper\shell\open",
            r"Software\QBWebUIHelper\shell",
            r"Software\QBWebUIHelper",
        ];
        for path in &sweep_paths {
            let _ = hkcu.delete_subkey(path);
        }

        // Phase 3: clean up entries Windows auto-created (not in our backup).
        cleanup_windows_tracking();

        notify_assoc_changed();
    }

    /// Removes traces Windows itself adds when the user opens a .torrent with our
    /// exe via the "Open With" picker. The backup-restore logic above only undoes
    /// what `register` wrote, so without this step QBWebUIHelper.exe lingers in the
    /// Default Apps picker as a "Suggested app".
    fn cleanup_windows_tracking() {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);

        let exe_name = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "qbwebuihelper.exe".to_string());

        // OpenWithList holds single-letter slots (a, b, c, ...) pointing to exe
        // filenames, plus MRUList ordering them. Remove slots that point at us and
        // patch MRUList so it no longer references the removed letters.
        let list_path = r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.torrent\OpenWithList";
        if let Ok(key) = hkcu.open_subkey_with_flags(list_path, KEY_READ | KEY_WRITE) {
            let mut slots_to_remove: Vec<String> = Vec::new();
            for entry in key.enum_values() {
                if let Ok((name, _)) = entry {
                    if name == "MRUList" { continue; }
                    if let Ok(data) = key.get_value::<String, _>(&name) {
                        if data.eq_ignore_ascii_case(&exe_name) {
                            slots_to_remove.push(name);
                        }
                    }
                }
            }
            for name in &slots_to_remove {
                let _ = key.delete_value(name);
            }
            if !slots_to_remove.is_empty() {
                if let Ok(mru) = key.get_value::<String, _>("MRUList") {
                    let new_mru: String = mru.chars()
                        .filter(|c| !slots_to_remove.iter().any(|s| s.chars().next() == Some(*c)))
                        .collect();
                    let _ = key.set_value("MRUList", &new_mru);
                }
            }
        }

        // OpenWithProgids: drop our ProgID if Windows recorded it here.
        let progids_path = r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.torrent\OpenWithProgids";
        if let Ok(key) = hkcu.open_subkey_with_flags(progids_path, KEY_WRITE) {
            let _ = key.delete_value("QBWebUIHelper.Torrent");
        }

        // HKCU\Software\Classes\Applications\<exe>: auto-created when the user picks
        // our exe in Open With. delete_subkey_all is recursive.
        let app_path = format!(r"Software\Classes\Applications\{}", exe_name);
        let _ = hkcu.delete_subkey_all(&app_path);
    }

    fn notify_assoc_changed() {
        use std::os::raw::c_void;
        #[link(name = "shell32")]
        extern "system" {
            fn SHChangeNotify(wEventId: i32, uFlags: u32, dwItem1: *const c_void, dwItem2: *const c_void);
        }
        unsafe {
            SHChangeNotify(0x08000000, 0, std::ptr::null(), std::ptr::null());
        }
    }
}

#[cfg(target_os = "windows")]
pub use win::{is_registered, register, unregister};

// ── macOS ─────────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
mod mac {
    use crate::config::MacBackup;
    use core_foundation::base::TCFType;
    use core_foundation::string::{CFString, CFStringRef};

    // LSRolesMask — all roles (viewer, editor, shell)
    const LS_ROLES_ALL: u32 = 0xFFFFFFFF;

    // Bundle identifier from tauri.conf.json
    const OUR_BUNDLE_ID: &str = "com.kritblade.qbwebuihelper";

    // UTI for .torrent files registered by most BitTorrent clients (Transmission et al.)
    const TORRENT_UTI: &str = "com.bittorrent.torrent";

    #[link(name = "CoreServices", kind = "framework")]
    extern "C" {
        fn LSSetDefaultHandlerForURLScheme(scheme: CFStringRef, bundle_id: CFStringRef) -> i32;
        fn LSCopyDefaultHandlerForURLScheme(scheme: CFStringRef) -> CFStringRef;
        fn LSSetDefaultRoleHandlerForContentType(
            content_type: CFStringRef,
            role: u32,
            bundle_id: CFStringRef,
        ) -> i32;
        fn LSCopyDefaultRoleHandlerForContentType(
            content_type: CFStringRef,
            role: u32,
        ) -> CFStringRef;
    }

    fn get_url_handler(scheme: &str) -> Option<String> {
        let cf = CFString::new(scheme);
        let raw = unsafe { LSCopyDefaultHandlerForURLScheme(cf.as_concrete_TypeRef()) };
        if raw.is_null() {
            return None;
        }
        // LSCopy* returns a +1 retained object — wrap_under_create_rule releases it on drop.
        let result = unsafe { CFString::wrap_under_create_rule(raw) };
        Some(result.to_string())
    }

    fn set_url_handler(scheme: &str, bundle_id: &str) -> bool {
        let cf_scheme = CFString::new(scheme);
        let cf_bundle = CFString::new(bundle_id);
        let status = unsafe {
            LSSetDefaultHandlerForURLScheme(
                cf_scheme.as_concrete_TypeRef(),
                cf_bundle.as_concrete_TypeRef(),
            )
        };
        status == 0
    }

    fn get_content_handler(uti: &str) -> Option<String> {
        let cf = CFString::new(uti);
        let raw = unsafe { LSCopyDefaultRoleHandlerForContentType(cf.as_concrete_TypeRef(), LS_ROLES_ALL) };
        if raw.is_null() {
            return None;
        }
        let result = unsafe { CFString::wrap_under_create_rule(raw) };
        Some(result.to_string())
    }

    fn set_content_handler(uti: &str, bundle_id: &str) -> Result<(), i32> {
        let cf_uti = CFString::new(uti);
        let cf_bundle = CFString::new(bundle_id);
        let status = unsafe {
            LSSetDefaultRoleHandlerForContentType(
                cf_uti.as_concrete_TypeRef(),
                LS_ROLES_ALL,
                cf_bundle.as_concrete_TypeRef(),
            )
        };
        if status == 0 { Ok(()) } else { Err(status) }
    }

    pub fn is_registered() -> bool {
        let magnet_ok = get_url_handler("magnet")
            .map(|h| h.eq_ignore_ascii_case(OUR_BUNDLE_ID))
            .unwrap_or(false);
        let torrent_ok = get_content_handler(TORRENT_UTI)
            .map(|h| h.eq_ignore_ascii_case(OUR_BUNDLE_ID))
            .unwrap_or(false);
        magnet_ok && torrent_ok
    }

    /// Saves existing default handlers, then claims both magnet: and .torrent for
    /// this app. Returns (backup, result). Backup is always populated so the caller
    /// can persist it even if the write partially fails.
    pub fn register() -> (MacBackup, Result<(), String>) {
        // Only snapshot a handler as "previous" if it belongs to someone else.
        // If it's already us (or missing), store None so we don't create a
        // circular restore that points back to QBWebUIHelper itself.
        let filter = |h: Option<String>| -> Option<String> {
            h.filter(|id| !id.eq_ignore_ascii_case(OUR_BUNDLE_ID))
        };
        let backup = MacBackup {
            prev_magnet_handler: filter(get_url_handler("magnet")),
            prev_torrent_handler: filter(get_content_handler(TORRENT_UTI)),
        };

        let magnet_ok = set_url_handler("magnet", OUR_BUNDLE_ID);
        let torrent_result = set_content_handler(TORRENT_UTI, OUR_BUNDLE_ID);

        let result = match (&magnet_ok, &torrent_result) {
            (true, Ok(())) => Ok(()),
            _ => Err(format!(
                "Failed to set default handler(s): magnet={} torrent={}",
                magnet_ok,
                torrent_result.as_ref().err().map(|e| format!("err({})", e)).unwrap_or_else(|| "ok".into())
            )),
        };

        (backup, result)
    }

    /// Restores previously saved handlers. If a slot was empty (no prior handler),
    /// the association is not touched — LaunchServices will self-heal over time.
    pub fn unregister(backup: &MacBackup) {
        if let Some(ref prev) = backup.prev_magnet_handler {
            if !prev.is_empty() {
                let _ = set_url_handler("magnet", prev);
            }
        }
        if let Some(ref prev) = backup.prev_torrent_handler {
            if !prev.is_empty() {
                let _ = set_content_handler(TORRENT_UTI, prev);
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac::{is_registered, register, unregister};

// ── Unsupported platforms ─────────────────────────────────────────────────────

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn is_registered() -> bool { false }

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn register(_exe_path: &str) -> (Vec<RegMutation>, Result<(), String>) {
    (Vec::new(), Err("Not supported on this platform".to_string()))
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn unregister(_backup: &[RegMutation]) {}
