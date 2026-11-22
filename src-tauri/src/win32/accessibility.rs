use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const SKF_STICKYKEYSON: u32 = 0x0000_0001;
pub const SKF_HOTKEYACTIVE: u32 = 0x0000_0004;
pub const SKF_CONFIRMHOTKEY: u32 = 0x0000_0008;

pub const TKF_TOGGLEKEYSON: u32 = 0x0000_0001;
pub const TKF_HOTKEYACTIVE: u32 = 0x0000_0004;
pub const TKF_CONFIRMHOTKEY: u32 = 0x0000_0008;

pub const FKF_FILTERKEYSON: u32 = 0x0000_0001;
pub const FKF_HOTKEYACTIVE: u32 = 0x0000_0004;
pub const FKF_CONFIRMHOTKEY: u32 = 0x0000_0008;

#[cfg(windows)]
const _: () = {
    use windows::Win32::UI::Accessibility as acc;
    use windows::Win32::UI::WindowsAndMessaging as wm;
    assert!(SKF_STICKYKEYSON == acc::SKF_STICKYKEYSON.0);
    assert!(SKF_HOTKEYACTIVE == acc::SKF_HOTKEYACTIVE.0);
    assert!(SKF_CONFIRMHOTKEY == acc::SKF_CONFIRMHOTKEY.0);
    assert!(TKF_TOGGLEKEYSON == wm::TKF_TOGGLEKEYSON);
    assert!(TKF_HOTKEYACTIVE == wm::TKF_HOTKEYACTIVE);
    assert!(TKF_CONFIRMHOTKEY == wm::TKF_CONFIRMHOTKEY);
    assert!(FKF_FILTERKEYSON == wm::FKF_FILTERKEYSON);
    assert!(FKF_HOTKEYACTIVE == wm::FKF_HOTKEYACTIVE);
    assert!(FKF_CONFIRMHOTKEY == wm::FKF_CONFIRMHOTKEY);
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessibilitySnapshot {
    pub sticky_flags: u32,
    pub toggle_flags: u32,
    pub filter_flags: u32,
}

#[inline]
fn suppress_feature_flags(flags: u32, on_bit: u32, clear_mask: u32) -> u32 {
    if (flags & on_bit) != 0 {
        flags
    } else {
        flags & !clear_mask
    }
}

pub fn compute_suppressed_flags(orig: AccessibilitySnapshot) -> AccessibilitySnapshot {
    AccessibilitySnapshot {
        sticky_flags: suppress_feature_flags(
            orig.sticky_flags,
            SKF_STICKYKEYSON,
            SKF_HOTKEYACTIVE | SKF_CONFIRMHOTKEY,
        ),
        toggle_flags: suppress_feature_flags(
            orig.toggle_flags,
            TKF_TOGGLEKEYSON,
            TKF_HOTKEYACTIVE | TKF_CONFIRMHOTKEY,
        ),
        filter_flags: suppress_feature_flags(
            orig.filter_flags,
            FKF_FILTERKEYSON,
            FKF_HOTKEYACTIVE | FKF_CONFIRMHOTKEY,
        ),
    }
}

pub fn recovery_path() -> PathBuf {
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) {
        return PathBuf::from(local_app_data)
            .join("InGame")
            .join("recovery.json");
    }

    if let Some(user_profile) = std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()) {
        return PathBuf::from(user_profile)
            .join("AppData")
            .join("Local")
            .join("InGame")
            .join("recovery.json");
    }

    PathBuf::from("InGame").join("recovery.json")
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
    .map_err(|e| format!("Failed to atomically replace recovery file: {e}"))
}

#[cfg(not(windows))]
fn atomic_replace(tmp_path: &Path, dest_path: &Path) -> Result<(), String> {
    fs::rename(tmp_path, dest_path)
        .map_err(|e| format!("Failed to rename temporary recovery file: {e}"))
}

pub(crate) fn write_recovery_snapshot(
    path: &Path,
    snapshot: AccessibilitySnapshot,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create recovery directory: {e}"))?;
        }
    }

    let json = serde_json::to_string_pretty(&snapshot)
        .map_err(|e| format!("Failed to serialize recovery snapshot: {e}"))?;

    let tmp_path = temp_path_for(path);

    let write_result: Result<(), String> = (|| {
        let mut file = File::create(&tmp_path)
            .map_err(|e| format!("Failed to create temporary recovery file: {e}"))?;
        file.write_all(json.as_bytes())
            .map_err(|e| format!("Failed to write temporary recovery file: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("Failed to flush temporary recovery file: {e}"))?;
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

pub(crate) fn load_recovery_snapshot(path: &Path) -> Option<AccessibilitySnapshot> {
    let contents = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => {
            let _ = fs::remove_file(path);
            return None;
        }
    };

    match serde_json::from_str::<AccessibilitySnapshot>(&contents) {
        Ok(snapshot) => Some(snapshot),
        Err(_) => {
            let _ = fs::remove_file(path);
            None
        }
    }
}

#[cfg(windows)]
fn read_current_snapshot() -> Option<AccessibilitySnapshot> {
    use std::ffi::c_void;
    use std::mem::size_of;
    use windows::Win32::UI::Accessibility::{FILTERKEYS, STICKYKEYS, TOGGLEKEYS};
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETFILTERKEYS, SPI_GETSTICKYKEYS, SPI_GETTOGGLEKEYS,
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };

    const IN_MEMORY_ONLY: SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS =
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0);

    let mut sk = STICKYKEYS {
        cbSize: size_of::<STICKYKEYS>() as u32,
        ..Default::default()
    };
    let mut tk = TOGGLEKEYS {
        cbSize: size_of::<TOGGLEKEYS>() as u32,
        ..Default::default()
    };
    let mut fk = FILTERKEYS {
        cbSize: size_of::<FILTERKEYS>() as u32,
        ..Default::default()
    };

    // SAFETY: Each accessibility struct has `cbSize` set to its exact byte size and is passed
    // with matching `uiparam` and `fWinIni = 0`.
    unsafe {
        SystemParametersInfoW(
            SPI_GETSTICKYKEYS,
            sk.cbSize,
            Some((&mut sk as *mut STICKYKEYS).cast::<c_void>()),
            IN_MEMORY_ONLY,
        )
        .ok()?;
        SystemParametersInfoW(
            SPI_GETTOGGLEKEYS,
            tk.cbSize,
            Some((&mut tk as *mut TOGGLEKEYS).cast::<c_void>()),
            IN_MEMORY_ONLY,
        )
        .ok()?;
        SystemParametersInfoW(
            SPI_GETFILTERKEYS,
            fk.cbSize,
            Some((&mut fk as *mut FILTERKEYS).cast::<c_void>()),
            IN_MEMORY_ONLY,
        )
        .ok()?;
    }

    Some(AccessibilitySnapshot {
        sticky_flags: sk.dwFlags.0,
        toggle_flags: tk.dwFlags,
        filter_flags: fk.dwFlags,
    })
}

#[cfg(not(windows))]
fn read_current_snapshot() -> Option<AccessibilitySnapshot> {
    None
}

#[cfg(windows)]
fn apply_snapshot(target: AccessibilitySnapshot) {
    use std::ffi::c_void;
    use std::mem::size_of;
    use windows::Win32::UI::Accessibility::{FILTERKEYS, STICKYKEYS, STICKYKEYS_FLAGS, TOGGLEKEYS};
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETFILTERKEYS, SPI_GETSTICKYKEYS, SPI_GETTOGGLEKEYS,
        SPI_SETFILTERKEYS, SPI_SETSTICKYKEYS, SPI_SETTOGGLEKEYS,
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };

    const IN_MEMORY_ONLY: SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS =
        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0);

    let mut sk = STICKYKEYS {
        cbSize: size_of::<STICKYKEYS>() as u32,
        ..Default::default()
    };
    // SAFETY: `sk` is properly sized; `fWinIni` is strictly 0 (in-memory only).
    unsafe {
        if SystemParametersInfoW(
            SPI_GETSTICKYKEYS,
            sk.cbSize,
            Some((&mut sk as *mut STICKYKEYS).cast::<c_void>()),
            IN_MEMORY_ONLY,
        )
        .is_ok()
            && sk.dwFlags.0 != target.sticky_flags
        {
            sk.dwFlags = STICKYKEYS_FLAGS(target.sticky_flags);
            let _ = SystemParametersInfoW(
                SPI_SETSTICKYKEYS,
                sk.cbSize,
                Some((&mut sk as *mut STICKYKEYS).cast::<c_void>()),
                IN_MEMORY_ONLY,
            );
        }
    }

    let mut tk = TOGGLEKEYS {
        cbSize: size_of::<TOGGLEKEYS>() as u32,
        ..Default::default()
    };
    // SAFETY: `tk` is properly sized; `fWinIni` is strictly 0 (in-memory only).
    unsafe {
        if SystemParametersInfoW(
            SPI_GETTOGGLEKEYS,
            tk.cbSize,
            Some((&mut tk as *mut TOGGLEKEYS).cast::<c_void>()),
            IN_MEMORY_ONLY,
        )
        .is_ok()
            && tk.dwFlags != target.toggle_flags
        {
            tk.dwFlags = target.toggle_flags;
            let _ = SystemParametersInfoW(
                SPI_SETTOGGLEKEYS,
                tk.cbSize,
                Some((&mut tk as *mut TOGGLEKEYS).cast::<c_void>()),
                IN_MEMORY_ONLY,
            );
        }
    }

    let mut fk = FILTERKEYS {
        cbSize: size_of::<FILTERKEYS>() as u32,
        ..Default::default()
    };
    // SAFETY: `fk` retains existing timing fields from `SPI_GETFILTERKEYS` and updates only
    // `dwFlags` with `fWinIni = 0` (in-memory only).
    unsafe {
        if SystemParametersInfoW(
            SPI_GETFILTERKEYS,
            fk.cbSize,
            Some((&mut fk as *mut FILTERKEYS).cast::<c_void>()),
            IN_MEMORY_ONLY,
        )
        .is_ok()
            && fk.dwFlags != target.filter_flags
        {
            fk.dwFlags = target.filter_flags;
            let _ = SystemParametersInfoW(
                SPI_SETFILTERKEYS,
                fk.cbSize,
                Some((&mut fk as *mut FILTERKEYS).cast::<c_void>()),
                IN_MEMORY_ONLY,
            );
        }
    }
}

#[cfg(not(windows))]
fn apply_snapshot(_target: AccessibilitySnapshot) {}

#[derive(Debug)]
pub struct AccessibilityGuard {
    recovery_file: PathBuf,
    original: Option<AccessibilitySnapshot>,
}

impl Default for AccessibilityGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl AccessibilityGuard {
    pub fn new() -> Self {
        Self::with_recovery_path(recovery_path())
    }

    pub fn with_recovery_path(path: PathBuf) -> Self {
        Self {
            recovery_file: path,
            original: None,
        }
    }

    pub fn recover_from_crash_file(path: &Path) {
        Self::recover_from_crash_file_with(path, apply_snapshot);
    }

    fn recover_from_crash_file_with<F>(path: &Path, apply_fn: F)
    where
        F: FnOnce(AccessibilitySnapshot),
    {
        if let Some(snapshot) = load_recovery_snapshot(path) {
            apply_fn(snapshot);
            let _ = fs::remove_file(path);
        }
    }

    pub fn suppress(&mut self) {
        if self.original.is_some() {
            return;
        }

        let Some(orig) = read_current_snapshot() else {
            return;
        };

        let suppressed = compute_suppressed_flags(orig);
        if suppressed == orig {
            return;
        }

        if write_recovery_snapshot(&self.recovery_file, orig).is_err() {
            return;
        }

        self.original = Some(orig);
        apply_snapshot(suppressed);
    }

    pub fn restore(&mut self) {
        if let Some(orig) = self.original.take() {
            apply_snapshot(orig);
            let _ = fs::remove_file(&self.recovery_file);
        }
    }
}

impl Drop for AccessibilityGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    const ON_BIT: u32 = 0x01;
    const AVAILABLE_BIT: u32 = 0x02;
    const HOTKEY_ACTIVE: u32 = 0x04;
    const CONFIRM_HOTKEY: u32 = 0x08;
    const HOTKEY_SOUND: u32 = 0x10;

    static NEXT_TEST_DIR_ID: AtomicU64 = AtomicU64::new(1);

    struct TempDirGuard {
        path: PathBuf,
    }

    impl TempDirGuard {
        fn new(name: &str) -> Self {
            let id = NEXT_TEST_DIR_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "pubg_helper_accessibility_test_{}_{}_{}",
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
    fn compute_suppressed_flags_preserves_enabled_accessibility_and_clears_hotkeys() {
        // All *_ON == 0: clears HOTKEYACTIVE (0x04) | CONFIRMHOTKEY (0x08), keeps other bits
        let off_orig = AccessibilitySnapshot {
            sticky_flags: AVAILABLE_BIT | HOTKEY_ACTIVE | CONFIRM_HOTKEY | HOTKEY_SOUND,
            toggle_flags: AVAILABLE_BIT | HOTKEY_ACTIVE | CONFIRM_HOTKEY,
            filter_flags: HOTKEY_ACTIVE | CONFIRM_HOTKEY | HOTKEY_SOUND,
        };
        let off_suppressed = compute_suppressed_flags(off_orig);
        assert_eq!(off_suppressed.sticky_flags, AVAILABLE_BIT | HOTKEY_SOUND);
        assert_eq!(off_suppressed.toggle_flags, AVAILABLE_BIT);
        assert_eq!(off_suppressed.filter_flags, HOTKEY_SOUND);

        // All *_ON == 1: leaves every feature's flags completely untouched
        let on_orig = AccessibilitySnapshot {
            sticky_flags: ON_BIT | AVAILABLE_BIT | HOTKEY_ACTIVE | CONFIRM_HOTKEY | HOTKEY_SOUND,
            toggle_flags: ON_BIT | HOTKEY_ACTIVE | CONFIRM_HOTKEY,
            filter_flags: ON_BIT | AVAILABLE_BIT | HOTKEY_ACTIVE | CONFIRM_HOTKEY,
        };
        assert_eq!(compute_suppressed_flags(on_orig), on_orig);

        // Mixed: sticky ON (preserved), toggle OFF (cleared), filter ON (preserved)
        let mixed = AccessibilitySnapshot {
            sticky_flags: ON_BIT | HOTKEY_ACTIVE | CONFIRM_HOTKEY,
            toggle_flags: AVAILABLE_BIT | HOTKEY_ACTIVE | CONFIRM_HOTKEY,
            filter_flags: ON_BIT | HOTKEY_ACTIVE,
        };
        let mixed_suppressed = compute_suppressed_flags(mixed);
        assert_eq!(mixed_suppressed.sticky_flags, mixed.sticky_flags);
        assert_eq!(mixed_suppressed.toggle_flags, AVAILABLE_BIT);
        assert_eq!(mixed_suppressed.filter_flags, mixed.filter_flags);
    }

    #[test]
    fn recovery_file_roundtrip_and_corrupt_cleanup() {
        let dir = TempDirGuard::new("recovery");
        let path = dir.path.join("nested").join("recovery.json");

        let snapshot = AccessibilitySnapshot {
            sticky_flags: AVAILABLE_BIT | HOTKEY_ACTIVE | CONFIRM_HOTKEY,
            toggle_flags: AVAILABLE_BIT | HOTKEY_ACTIVE,
            filter_flags: HOTKEY_ACTIVE | CONFIRM_HOTKEY | HOTKEY_SOUND,
        };

        // Roundtrip write & read
        write_recovery_snapshot(&path, snapshot).expect("write recovery snapshot");
        assert!(path.exists());
        assert_eq!(load_recovery_snapshot(&path), Some(snapshot));

        // Overwrite atomically with updated snapshot
        let updated = AccessibilitySnapshot {
            sticky_flags: AVAILABLE_BIT | HOTKEY_ACTIVE,
            toggle_flags: HOTKEY_ACTIVE | CONFIRM_HOTKEY,
            filter_flags: AVAILABLE_BIT | HOTKEY_SOUND,
        };
        write_recovery_snapshot(&path, updated).expect("overwrite recovery snapshot");
        assert_eq!(load_recovery_snapshot(&path), Some(updated));

        // Verify no leftover .tmp files
        let parent = path.parent().expect("parent dir");
        let tmp_files: Vec<_> = fs::read_dir(parent)
            .expect("read parent dir")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(tmp_files.is_empty(), "leftover .tmp files: {tmp_files:?}");

        // Recovery flow applies snapshot and removes recovery.json
        let mut applied = None;
        AccessibilityGuard::recover_from_crash_file_with(&path, |snap| {
            applied = Some(snap);
        });
        assert_eq!(applied, Some(updated));
        assert!(
            !path.exists(),
            "recovery file should be removed after apply"
        );

        // Missing file returns None cleanly without panicking or applying
        let mut called_missing = false;
        AccessibilityGuard::recover_from_crash_file_with(&path, |_| {
            called_missing = true;
        });
        assert!(!called_missing);
        assert_eq!(load_recovery_snapshot(&path), None);

        // Corrupt/truncated JSON returns None and removes corrupt file without panicking
        fs::write(&path, "{ \"sticky_flags\": 12").expect("write corrupt json");
        assert!(path.exists());
        assert_eq!(load_recovery_snapshot(&path), None);
        assert!(
            !path.exists(),
            "corrupt recovery file must be deleted cleanly"
        );

        // Corrupt JSON via public recover_from_crash_file cleans up without panicking
        // (and never calls SystemParametersInfoW because JSON fails to parse)
        fs::write(&path, "not valid json").expect("write corrupt json 2");
        AccessibilityGuard::recover_from_crash_file(&path);
        assert!(!path.exists());

        // Non-UTF-8 corrupt bytes also cleaned up without panicking
        fs::write(&path, [0xFFu8, 0xFE, 0x00, 0x80]).expect("write invalid utf8");
        assert_eq!(load_recovery_snapshot(&path), None);
        assert!(!path.exists());
    }

    #[test]
    fn recovery_path_ends_with_expected_app_file() {
        let path = recovery_path();
        assert_eq!(
            path.file_name().and_then(|s| s.to_str()),
            Some("recovery.json")
        );
        assert_eq!(
            path.parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str()),
            Some("InGame")
        );
    }
}
