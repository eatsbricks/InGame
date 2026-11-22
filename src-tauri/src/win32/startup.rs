use std::path::Path;

#[cfg(windows)]
const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const VALUE_NAME: &str = "InGame";
#[cfg(windows)]
const LEGACY_VALUE_NAME: &str = "PUBGHelper";

pub(crate) fn format_startup_command(exe_path: &Path) -> String {
    format!("\"{}\" --minimized", exe_path.display())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(windows, test))]
enum RegOpError {
    NotFound,
    Other(u32),
}

#[cfg(any(windows, test))]
fn apply_startup_registry<K, FCreate, FOpen, FSet, FDelete>(
    enabled: bool,
    create_key: FCreate,
    open_key: FOpen,
    set_value: FSet,
    delete_value: FDelete,
) -> Result<(), String>
where
    FCreate: FnOnce() -> Result<K, u32>,
    FOpen: FnOnce() -> Result<K, RegOpError>,
    FSet: FnOnce(&K) -> Result<(), String>,
    FDelete: FnOnce(&K) -> Result<(), RegOpError>,
{
    if enabled {
        let key = create_key().map_err(|code| {
            format!("Failed to open or create startup registry key (error {code})")
        })?;
        set_value(&key)
    } else {
        let key = match open_key() {
            Ok(key) => key,
            Err(RegOpError::NotFound) => return Ok(()),
            Err(RegOpError::Other(code)) => {
                return Err(format!(
                    "Failed to open startup registry key (error {code})"
                ));
            }
        };
        match delete_value(&key) {
            Ok(()) | Err(RegOpError::NotFound) => Ok(()),
            Err(RegOpError::Other(code)) => Err(format!(
                "Failed to remove startup registry value (error {code})"
            )),
        }
    }
}

#[cfg(windows)]
pub fn set_launch_at_startup(enabled: bool) -> Result<(), String> {
    use super::{to_wide_null, OwnedHkey};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    let subkey = to_wide_null(RUN_SUBKEY);
    let value_name = to_wide_null(VALUE_NAME);
    let legacy_value_name = to_wide_null(LEGACY_VALUE_NAME);

    apply_startup_registry(
        enabled,
        || {
            let mut raw_key = HKEY::default();
            // SAFETY: `subkey` is NUL-terminated UTF-16 and `raw_key` is a valid out pointer.
            let create_err = unsafe {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    PCWSTR(subkey.as_ptr()),
                    Some(0),
                    PCWSTR::null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_SET_VALUE,
                    None,
                    &mut raw_key,
                    None,
                )
            };
            if create_err != ERROR_SUCCESS {
                Err(create_err.0)
            } else {
                Ok(OwnedHkey(raw_key))
            }
        },
        || {
            let mut raw_key = HKEY::default();
            // SAFETY: `subkey` is NUL-terminated UTF-16 and `raw_key` is a valid out pointer.
            let open_err = unsafe {
                RegOpenKeyExW(
                    HKEY_CURRENT_USER,
                    PCWSTR(subkey.as_ptr()),
                    Some(0),
                    KEY_SET_VALUE,
                    &mut raw_key,
                )
            };
            if open_err == ERROR_SUCCESS {
                Ok(OwnedHkey(raw_key))
            } else if open_err == ERROR_FILE_NOT_FOUND {
                Err(RegOpError::NotFound)
            } else {
                Err(RegOpError::Other(open_err.0))
            }
        },
        |key| {
            let exe_path = std::env::current_exe()
                .map_err(|e| format!("Failed to resolve current executable path: {e}"))?;
            let command = format_startup_command(&exe_path);
            let wide_cmd = to_wide_null(&command);
            let bytes: Vec<u8> = wide_cmd
                .iter()
                .flat_map(|unit| unit.to_le_bytes())
                .collect();

            // SAFETY: `key.0` is an open HKCU Run key, `value_name` is NUL-terminated UTF-16,
            // and `bytes` is a NUL-terminated UTF-16LE REG_SZ value.
            let set_err = unsafe {
                RegSetValueExW(
                    key.0,
                    PCWSTR(value_name.as_ptr()),
                    Some(0),
                    REG_SZ,
                    Some(&bytes),
                )
            };
            if set_err != ERROR_SUCCESS {
                return Err(format!(
                    "Failed to write startup registry value (error {})",
                    set_err.0
                ));
            }
            // Best-effort cleanup of legacy v0.1.0/pre-rebrand Run entry so startup isn't duplicated.
            // SAFETY: `key.0` is an open HKCU Run key and `legacy_value_name` is NUL-terminated UTF-16.
            let _ = unsafe { RegDeleteValueW(key.0, PCWSTR(legacy_value_name.as_ptr())) };
            Ok(())
        },
        |key| {
            // Best-effort cleanup of legacy pre-rebrand Run entry as well.
            // SAFETY: `key.0` is an open HKCU Run key and `legacy_value_name` is NUL-terminated UTF-16.
            let _ = unsafe { RegDeleteValueW(key.0, PCWSTR(legacy_value_name.as_ptr())) };
            // SAFETY: `key.0` is an open HKCU Run key and `value_name` is NUL-terminated UTF-16.
            let del_err = unsafe { RegDeleteValueW(key.0, PCWSTR(value_name.as_ptr())) };
            if del_err == ERROR_SUCCESS {
                Ok(())
            } else if del_err == ERROR_FILE_NOT_FOUND {
                Err(RegOpError::NotFound)
            } else {
                Err(RegOpError::Other(del_err.0))
            }
        },
    )
}

#[cfg(not(windows))]
pub fn set_launch_at_startup(_enabled: bool) -> Result<(), String> {
    Err("Startup registration is only supported on Windows".to_string())
}

#[cfg(windows)]
pub fn is_launch_at_startup_enabled() -> bool {
    use super::{query_reg_string, to_wide_null, OwnedHkey};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ};

    let subkey = to_wide_null(RUN_SUBKEY);
    let mut raw_key = HKEY::default();
    // SAFETY: `subkey` is NUL-terminated UTF-16 and `raw_key` is a valid out pointer.
    let open_err = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            Some(0),
            KEY_READ,
            &mut raw_key,
        )
    };
    if open_err != ERROR_SUCCESS {
        return false;
    }

    let key = OwnedHkey(raw_key);
    query_reg_string(key.0, VALUE_NAME)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

#[cfg(not(windows))]
pub fn is_launch_at_startup_enabled() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_startup_command_quotes_exe_path_with_minimized_flag() {
        let path = Path::new(r"C:\Users\Player\InGame\ingame.exe");
        assert_eq!(
            format_startup_command(path),
            "\"C:\\Users\\Player\\InGame\\ingame.exe\" --minimized"
        );
    }

    #[test]
    fn apply_startup_registry_creates_key_when_enabling_and_tolerates_missing_when_disabling() {
        // Enabling uses RegCreateKeyExW path (works on fresh profiles without HKCU\...\Run).
        let mut created = false;
        let mut wrote_value = false;
        let enable_res = apply_startup_registry(
            true,
            || {
                created = true;
                Ok(42usize)
            },
            || panic!("open_key must not be used when enabled == true"),
            |&k| {
                assert_eq!(k, 42);
                wrote_value = true;
                Ok(())
            },
            |_| panic!("delete_value must not be called when enabled == true"),
        );
        assert!(enable_res.is_ok());
        assert!(created && wrote_value);

        // Disabling when HKCU\...\Run does not exist returns Ok(()) without creating the key.
        let mut opened = false;
        let disable_missing_key = apply_startup_registry(
            false,
            || -> Result<usize, u32> {
                panic!("create_key must not be used when enabled == false")
            },
            || {
                opened = true;
                Err(RegOpError::NotFound)
            },
            |_| panic!("set_value must not be called when enabled == false"),
            |_| panic!("delete_value must not be called when key is missing"),
        );
        assert!(disable_missing_key.is_ok());
        assert!(opened);

        // Disabling when key exists but value is missing also returns Ok(()).
        let disable_missing_val = apply_startup_registry(
            false,
            || -> Result<usize, u32> { panic!("create_key must not be used") },
            || Ok(7usize),
            |_| panic!("set_value must not be called"),
            |&k| {
                assert_eq!(k, 7);
                Err(RegOpError::NotFound)
            },
        );
        assert!(disable_missing_val.is_ok());

        // Real registry errors propagate cleanly.
        let create_err = apply_startup_registry(
            true,
            || -> Result<usize, u32> { Err(5) },
            || unreachable!(),
            |_| unreachable!(),
            |_| unreachable!(),
        );
        assert!(create_err.is_err());

        let open_err = apply_startup_registry(
            false,
            || -> Result<usize, u32> { unreachable!() },
            || Err(RegOpError::Other(5)),
            |_| unreachable!(),
            |_| unreachable!(),
        );
        assert!(open_err.is_err());
    }
}
