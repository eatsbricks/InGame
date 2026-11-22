use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const MOD_CTRL: u32 = 0x01;
pub const MOD_ALT: u32 = 0x02;
pub const MOD_SHIFT: u32 = 0x04;

const VALID_MODIFIERS_MASK: u32 = MOD_CTRL | MOD_ALT | MOD_SHIFT;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EscapeKey {
    pub vk: u32,
    pub modifiers: u32,
    pub label: String,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub enabled: bool,
    pub lock_language: bool,
    pub block_minimize: bool,
    #[serde(default = "default_true")]
    pub block_caps_lock: bool,
    pub escape_key: EscapeKey,
    pub launch_at_startup: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            lock_language: true,
            block_minimize: true,
            block_caps_lock: true,
            escape_key: EscapeKey {
                vk: 0x79, // VK_F10
                modifiers: 0,
                label: "F10".to_string(),
            },
            launch_at_startup: false,
        }
    }
}

fn is_mouse_vk(vk: u32) -> bool {
    matches!(vk, 0x01..=0x06)
}

fn is_modifier_vk(vk: u32) -> bool {
    matches!(vk, 0x10..=0x12 | 0xA0..=0xA5 | 0x5B | 0x5C)
}

fn is_safe_bare_key(vk: u32) -> bool {
    matches!(
        vk,
        0x70..=0x7B // F1..=F12
            | 0x13  // VK_PAUSE
            | 0x91  // VK_SCROLL (ScrollLock)
            | 0x2D  // VK_INSERT
            | 0x24  // VK_HOME
            | 0x23  // VK_END
            | 0x21  // VK_PRIOR (PageUp)
            | 0x22 // VK_NEXT (PageDown)
    )
}

const VK_PAUSE: u32 = 0x13;
const VK_DELETE: u32 = 0x2E;

pub fn validate_escape_key(key: &EscapeKey) -> Result<(), String> {
    if key.label.trim().is_empty() {
        return Err("Escape key label cannot be empty".to_string());
    }

    if key.modifiers & !VALID_MODIFIERS_MASK != 0 {
        return Err("Unsupported modifier flags for escape key".to_string());
    }

    if key.vk == 0 || key.vk > 0xFE {
        return Err("Invalid virtual key code".to_string());
    }

    if is_mouse_vk(key.vk) {
        return Err("Mouse buttons cannot be used as escape key".to_string());
    }

    if is_modifier_vk(key.vk) {
        return Err("Modifier keys cannot be used as the primary escape key".to_string());
    }

    if key.vk == VK_PAUSE && (key.modifiers & MOD_CTRL) != 0 {
        return Err(
            "Ctrl+Pause translates to VK_CANCEL on Windows and cannot be used as escape key"
                .to_string(),
        );
    }

    if key.vk == VK_DELETE && (key.modifiers & (MOD_CTRL | MOD_ALT)) == (MOD_CTRL | MOD_ALT) {
        return Err(
            "Ctrl+Alt+Del is reserved by Windows and cannot be used as escape key".to_string(),
        );
    }

    if key.modifiers == 0 && !is_safe_bare_key(key.vk) {
        return Err(
            "Bare key is not allowed; use F1-F12, Pause, ScrollLock, Insert/Home/End/PageUp/PageDown, or add a modifier"
                .to_string(),
        );
    }

    Ok(())
}

pub fn settings_path() -> PathBuf {
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) {
        return PathBuf::from(local_app_data)
            .join("InGame")
            .join("settings.json");
    }

    if let Some(user_profile) = std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()) {
        return PathBuf::from(user_profile)
            .join("AppData")
            .join("Local")
            .join("InGame")
            .join("settings.json");
    }

    PathBuf::from("InGame").join("settings.json")
}

pub fn load_settings_from(path: &Path) -> Settings {
    let Ok(contents) = fs::read_to_string(path) else {
        return Settings::default();
    };

    let Ok(settings) = serde_json::from_str::<Settings>(&contents) else {
        return Settings::default();
    };

    if validate_escape_key(&settings.escape_key).is_err() {
        return Settings::default();
    }

    settings
}

fn temp_path_for(path: &Path) -> PathBuf {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    PathBuf::from(tmp)
}

#[cfg(windows)]
fn atomic_replace(tmp_path: &Path, dest_path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let src: Vec<u16> = tmp_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let dst: Vec<u16> = dest_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    // SAFETY: `src` and `dst` are valid NUL-terminated UTF-16 buffers that outlive the call.
    unsafe {
        MoveFileExW(
            PCWSTR(src.as_ptr()),
            PCWSTR(dst.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|e| format!("Failed to atomically replace settings file: {e}"))
}

#[cfg(not(windows))]
fn atomic_replace(tmp_path: &Path, dest_path: &Path) -> Result<(), String> {
    fs::rename(tmp_path, dest_path)
        .map_err(|e| format!("Failed to rename temporary settings file: {e}"))
}

pub fn save_settings_to(path: &Path, settings: &Settings) -> Result<(), String> {
    validate_escape_key(&settings.escape_key)?;

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create settings directory: {e}"))?;
        }
    }

    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("Failed to serialize settings: {e}"))?;

    let tmp_path = temp_path_for(path);

    let write_result: Result<(), String> = (|| {
        let mut file = File::create(&tmp_path)
            .map_err(|e| format!("Failed to create temporary settings file: {e}"))?;
        file.write_all(json.as_bytes())
            .map_err(|e| format!("Failed to write temporary settings file: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("Failed to flush temporary settings file: {e}"))?;
        Ok(())
    })();

    if let Err(err) = write_result {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }

    if let Err(err) = atomic_replace(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_DIR_ID: AtomicU64 = AtomicU64::new(1);

    struct TempDirGuard {
        path: PathBuf,
    }

    impl TempDirGuard {
        fn new(name: &str) -> Self {
            let id = NEXT_TEST_DIR_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "pubg_helper_config_test_{}_{}_{}",
                std::process::id(),
                name,
                id
            ));
            fs::create_dir_all(&path).expect("create temp test dir");
            Self { path }
        }
    }

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn default_settings_match_spec() {
        let settings = Settings::default();
        assert!(settings.enabled);
        assert!(settings.lock_language);
        assert!(settings.block_minimize);
        assert!(settings.block_caps_lock);
        assert!(!settings.launch_at_startup);
        assert_eq!(
            settings.escape_key,
            EscapeKey {
                vk: 0x79,
                modifiers: 0,
                label: "F10".into(),
            }
        );
    }

    #[test]
    fn roundtrip_atomic_save_and_load() {
        let dir = TempDirGuard::new("roundtrip");
        let path = dir.path.join("nested").join("settings.json");

        let initial = Settings {
            enabled: false,
            lock_language: true,
            block_minimize: false,
            block_caps_lock: false,
            escape_key: EscapeKey {
                vk: 0x09,
                modifiers: MOD_CTRL | MOD_ALT,
                label: "Ctrl+Alt+Tab".into(),
            },
            launch_at_startup: true,
        };

        save_settings_to(&path, &initial).expect("initial save should succeed");
        assert_eq!(load_settings_from(&path), initial);

        // Overwrite existing file atomically and confirm no .tmp remnants remain.
        let updated = Settings {
            enabled: true,
            lock_language: false,
            block_minimize: true,
            block_caps_lock: true,
            escape_key: EscapeKey {
                vk: 0x78,
                modifiers: 0,
                label: "F9".into(),
            },
            launch_at_startup: false,
        };

        save_settings_to(&path, &updated).expect("overwrite save should succeed");
        assert_eq!(load_settings_from(&path), updated);

        let parent = path.parent().expect("parent dir");
        let tmp_files: Vec<_> = fs::read_dir(parent)
            .expect("read parent dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            tmp_files.is_empty(),
            "expected no .tmp files left behind, found: {tmp_files:?}"
        );
    }

    #[test]
    fn save_rejects_invalid_escape_key_without_writing_files() {
        let dir = TempDirGuard::new("reject_invalid_save");
        let path = dir.path.join("settings.json");

        let invalid = Settings {
            escape_key: EscapeKey {
                vk: 0x57, // bare W
                modifiers: 0,
                label: "W".into(),
            },
            ..Settings::default()
        };

        assert!(save_settings_to(&path, &invalid).is_err());
        assert!(!path.exists());
        assert!(!temp_path_for(&path).exists());
    }

    #[test]
    fn corrupt_or_truncated_json_falls_back_to_defaults() {
        let dir = TempDirGuard::new("corrupt");

        // Truncated JSON
        let corrupt_path = dir.path.join("settings.json");
        fs::write(&corrupt_path, "{ \"enabled\": true").expect("write truncated json");
        assert_eq!(load_settings_from(&corrupt_path), Settings::default());

        // Missing file
        let missing_path = dir.path.join("does_not_exist.json");
        assert_eq!(load_settings_from(&missing_path), Settings::default());

        // Valid JSON shape but dangerous invalid escape key (bare W)
        let invalid_key_path = dir.path.join("invalid_key.json");
        let bad_json = r#"{
            "enabled": false,
            "lock_language": false,
            "block_minimize": false,
            "escape_key": { "vk": 87, "modifiers": 0, "label": "W" },
            "launch_at_startup": true
        }"#;
        fs::write(&invalid_key_path, bad_json).expect("write invalid escape_key json");
        assert_eq!(load_settings_from(&invalid_key_path), Settings::default());

        // Hand-edited settings.json with untriggerable Ctrl+Pause falls back to defaults
        let ctrl_pause_path = dir.path.join("ctrl_pause.json");
        let ctrl_pause_json = r#"{
            "enabled": false,
            "lock_language": false,
            "block_minimize": false,
            "escape_key": { "vk": 19, "modifiers": 1, "label": "Ctrl+Pause" },
            "launch_at_startup": true
        }"#;
        fs::write(&ctrl_pause_path, ctrl_pause_json).expect("write Ctrl+Pause json");
        assert_eq!(load_settings_from(&ctrl_pause_path), Settings::default());

        // Hand-edited settings.json with Ctrl+Alt+Del falls back to defaults
        let cad_path = dir.path.join("ctrl_alt_del.json");
        let cad_json = r#"{
            "enabled": false,
            "lock_language": false,
            "block_minimize": false,
            "escape_key": { "vk": 46, "modifiers": 3, "label": "Ctrl+Alt+Del" },
            "launch_at_startup": true
        }"#;
        fs::write(&cad_path, cad_json).expect("write Ctrl+Alt+Del json");
        assert_eq!(load_settings_from(&cad_path), Settings::default());

        // Hand-edited settings.json with empty or whitespace-only label falls back to defaults
        let empty_label_path = dir.path.join("empty_label.json");
        let empty_label_json = r#"{
            "enabled": false,
            "lock_language": false,
            "block_minimize": false,
            "escape_key": { "vk": 121, "modifiers": 0, "label": "   " },
            "launch_at_startup": true
        }"#;
        fs::write(&empty_label_path, empty_label_json).expect("write empty label json");
        assert_eq!(load_settings_from(&empty_label_path), Settings::default());
    }

    #[test]
    fn validate_escape_key_allows_safe_keys_and_combos() {
        // Bare F1-F12 (0x70..=0x7B)
        for vk in 0x70..=0x7B {
            let key = EscapeKey {
                vk,
                modifiers: 0,
                label: format!("F{}", vk - 0x6F),
            };
            assert!(
                validate_escape_key(&key).is_ok(),
                "expected bare F-key vk=0x{vk:02X} to be accepted"
            );
        }

        // Bare Pause, ScrollLock, Insert, Home, End, PageUp, PageDown
        let safe_bare = [
            (0x13, "Pause"),
            (0x91, "ScrollLock"),
            (0x2D, "Insert"),
            (0x24, "Home"),
            (0x23, "End"),
            (0x21, "PageUp"),
            (0x22, "PageDown"),
        ];
        for (vk, label) in safe_bare {
            let key = EscapeKey {
                vk,
                modifiers: 0,
                label: label.into(),
            };
            assert!(
                validate_escape_key(&key).is_ok(),
                "expected bare {label} (0x{vk:02X}) to be accepted"
            );
        }

        // Modifier + key combos
        let combos = [
            EscapeKey {
                vk: 0x09,
                modifiers: MOD_CTRL | MOD_ALT,
                label: "Ctrl+Alt+Tab".into(),
            },
            EscapeKey {
                vk: 0x57,
                modifiers: MOD_CTRL | MOD_SHIFT,
                label: "Ctrl+Shift+W".into(),
            },
            EscapeKey {
                vk: 0x79,
                modifiers: MOD_ALT,
                label: "Alt+F10".into(),
            },
        ];
        for key in combos {
            assert!(
                validate_escape_key(&key).is_ok(),
                "expected combo {:?} to be accepted",
                key
            );
        }
    }

    #[test]
    fn validate_escape_key_rejects_dangerous_bare_keys() {
        // Bare W/A/S/D, Space, Tab, Esc
        for (vk, label) in [
            (0x57, "W"),
            (0x41, "A"),
            (0x53, "S"),
            (0x44, "D"),
            (0x20, "Space"),
            (0x09, "Tab"),
            (0x1B, "Esc"),
        ] {
            let key = EscapeKey {
                vk,
                modifiers: 0,
                label: label.into(),
            };
            assert!(
                validate_escape_key(&key).is_err(),
                "expected bare {label} (0x{vk:02X}) to be rejected"
            );
        }

        // Bare modifiers (0x10..=0x12, 0xA0..=0xA5, 0x5B, 0x5C)
        let modifier_vks: Vec<u32> = (0x10..=0x12)
            .chain(0xA0..=0xA5)
            .chain([0x5B, 0x5C])
            .collect();
        for vk in modifier_vks {
            let bare = EscapeKey {
                vk,
                modifiers: 0,
                label: format!("Mod0x{vk:02X}"),
            };
            assert!(
                validate_escape_key(&bare).is_err(),
                "expected bare modifier vk=0x{vk:02X} to be rejected"
            );

            // Modifier VK as primary key even when another modifier is held
            let combo = EscapeKey {
                vk,
                modifiers: MOD_CTRL,
                label: format!("Ctrl+0x{vk:02X}"),
            };
            assert!(
                validate_escape_key(&combo).is_err(),
                "expected modifier vk=0x{vk:02X} with MOD_CTRL to be rejected"
            );
        }

        // Mouse buttons (0x01..=0x06) bare and with modifiers
        for vk in 0x01..=0x06 {
            let bare = EscapeKey {
                vk,
                modifiers: 0,
                label: format!("Mouse0x{vk:02X}"),
            };
            assert!(
                validate_escape_key(&bare).is_err(),
                "expected mouse button vk=0x{vk:02X} to be rejected"
            );

            let combo = EscapeKey {
                vk,
                modifiers: MOD_ALT,
                label: format!("Alt+Mouse0x{vk:02X}"),
            };
            assert!(
                validate_escape_key(&combo).is_err(),
                "expected mouse button vk=0x{vk:02X} with modifier to be rejected"
            );
        }

        // vk == 0 and out-of-range vk
        for vk in [0x00, 0xFF, 0x100] {
            let key = EscapeKey {
                vk,
                modifiers: 0,
                label: "Invalid".into(),
            };
            assert!(
                validate_escape_key(&key).is_err(),
                "expected vk=0x{vk:02X} to be rejected"
            );
        }

        // Unknown modifier flags
        let bad_mod = EscapeKey {
            vk: 0x79,
            modifiers: 0x08,
            label: "Win+F10".into(),
        };
        assert!(validate_escape_key(&bad_mod).is_err());

        // Ctrl+Pause (and any Ctrl+*+Pause) translates to VK_CANCEL on Windows
        for modifiers in [
            MOD_CTRL,
            MOD_CTRL | MOD_ALT,
            MOD_CTRL | MOD_SHIFT,
            MOD_CTRL | MOD_ALT | MOD_SHIFT,
        ] {
            let key = EscapeKey {
                vk: 0x13,
                modifiers,
                label: "Ctrl+Pause".into(),
            };
            assert!(
                validate_escape_key(&key).is_err(),
                "expected Ctrl+Pause (modifiers=0x{modifiers:02X}) to be rejected"
            );
        }

        // Alt+Pause and Shift+Pause remain valid
        for (modifiers, label) in [(MOD_ALT, "Alt+Pause"), (MOD_SHIFT, "Shift+Pause")] {
            let key = EscapeKey {
                vk: 0x13,
                modifiers,
                label: label.into(),
            };
            assert!(
                validate_escape_key(&key).is_ok(),
                "expected {label} to remain accepted"
            );
        }

        // Ctrl+Alt+Del (Windows SAS) is rejected, while Ctrl+Del stays allowed
        for modifiers in [MOD_CTRL | MOD_ALT, MOD_CTRL | MOD_ALT | MOD_SHIFT] {
            let key = EscapeKey {
                vk: 0x2E,
                modifiers,
                label: "Ctrl+Alt+Del".into(),
            };
            assert!(
                validate_escape_key(&key).is_err(),
                "expected Ctrl+Alt+Del (modifiers=0x{modifiers:02X}) to be rejected"
            );
        }
        assert!(validate_escape_key(&EscapeKey {
            vk: 0x2E,
            modifiers: MOD_CTRL,
            label: "Ctrl+Del".into(),
        })
        .is_ok());

        // Empty or whitespace-only label is rejected even for an otherwise valid key
        for label in ["", " ", "\t\n"] {
            let key = EscapeKey {
                vk: 0x79,
                modifiers: 0,
                label: label.into(),
            };
            assert!(
                validate_escape_key(&key).is_err(),
                "expected empty/whitespace label {label:?} to be rejected"
            );
        }
    }

    #[test]
    fn settings_serde_uses_default_snake_case_fields() {
        let json = serde_json::to_value(Settings::default()).expect("serialize settings");
        let obj = json.as_object().expect("json object");
        assert!(obj.contains_key("enabled"));
        assert!(obj.contains_key("lock_language"));
        assert!(obj.contains_key("block_minimize"));
        assert!(obj.contains_key("block_caps_lock"));
        assert!(obj.contains_key("escape_key"));
        assert!(obj.contains_key("launch_at_startup"));

        let ek = obj["escape_key"].as_object().expect("escape_key object");
        assert!(ek.contains_key("vk"));
        assert!(ek.contains_key("modifiers"));
        assert!(ek.contains_key("label"));
    }

    #[test]
    fn v01_settings_json_without_block_caps_lock_migrates_preserving_fields() {
        let dir = TempDirGuard::new("v01_migration");
        let path = dir.path.join("settings.json");

        let v01_json = r#"{
            "enabled": false,
            "lock_language": true,
            "block_minimize": false,
            "escape_key": { "vk": 120, "modifiers": 1, "label": "Ctrl+F9" },
            "launch_at_startup": true
        }"#;
        fs::write(&path, v01_json).expect("write v0.1.0 json");

        let loaded = load_settings_from(&path);
        assert!(!loaded.enabled);
        assert!(loaded.lock_language);
        assert!(!loaded.block_minimize);
        assert!(loaded.block_caps_lock);
        assert_eq!(
            loaded.escape_key,
            EscapeKey {
                vk: 120,
                modifiers: MOD_CTRL,
                label: "Ctrl+F9".into(),
            }
        );
        assert!(loaded.launch_at_startup);
    }

    #[test]
    fn settings_path_ends_with_expected_app_file() {
        let path = settings_path();
        assert_eq!(
            path.file_name().and_then(|s| s.to_str()),
            Some("settings.json")
        );
        assert_eq!(
            path.parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str()),
            Some("InGame")
        );
    }
}
