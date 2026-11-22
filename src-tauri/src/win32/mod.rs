pub mod accessibility;
pub mod capslock;
pub mod coordinator;
pub mod detector;
pub mod hook;
pub mod language;
pub mod minimizer;
pub mod startup;

pub(crate) fn decode_utf16le_reg_string(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    let trimmed = match units.iter().position(|&unit| unit == 0) {
        Some(pos) => &units[..pos],
        None => &units[..],
    };
    String::from_utf16_lossy(trimmed)
}

#[cfg(windows)]
pub(crate) fn to_wide_null(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
pub(crate) struct OwnedHkey(pub windows::Win32::System::Registry::HKEY);

#[cfg(windows)]
impl Drop for OwnedHkey {
    fn drop(&mut self) {
        use windows::Win32::System::Registry::RegCloseKey;

        if !self.0.is_invalid() {
            // SAFETY: `self.0` is a valid registry key handle owned exclusively by this guard.
            let _ = unsafe { RegCloseKey(self.0) };
        }
    }
}

#[cfg(windows)]
pub(crate) fn query_reg_string(
    hkey: windows::Win32::System::Registry::HKEY,
    value_name: &str,
) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        RegQueryValueExW, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE,
    };

    let wide_name = to_wide_null(value_name);
    let mut value_type = REG_VALUE_TYPE::default();
    let mut byte_len: u32 = 0;

    // SAFETY: `hkey` is an open registry key and `wide_name` is NUL-terminated UTF-16.
    let status = unsafe {
        RegQueryValueExW(
            hkey,
            PCWSTR(wide_name.as_ptr()),
            None,
            Some(&mut value_type),
            None,
            Some(&mut byte_len),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    if value_type != REG_SZ && value_type != REG_EXPAND_SZ {
        return None;
    }
    if byte_len == 0 {
        return Some(String::new());
    }

    for _ in 0..3 {
        let mut buf = vec![0u8; byte_len as usize];
        let mut actual_len = byte_len;
        // SAFETY: `buf` has capacity `actual_len` bytes for `RegQueryValueExW` output.
        let read_status = unsafe {
            RegQueryValueExW(
                hkey,
                PCWSTR(wide_name.as_ptr()),
                None,
                Some(&mut value_type),
                Some(buf.as_mut_ptr()),
                Some(&mut actual_len),
            )
        };

        if read_status == ERROR_SUCCESS {
            if value_type != REG_SZ && value_type != REG_EXPAND_SZ {
                return None;
            }
            let valid_len = (actual_len as usize).min(buf.len());
            return Some(decode_utf16le_reg_string(&buf[..valid_len]));
        }

        if read_status == ERROR_MORE_DATA && actual_len > byte_len {
            byte_len = actual_len;
            continue;
        }

        return None;
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_utf16le_reg_string_handles_nul_and_trailing_bytes() {
        let with_nul: Vec<u8> = "4\0ignored"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        assert_eq!(decode_utf16le_reg_string(&with_nul), "4");

        let without_nul: Vec<u8> = "PUBG"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        assert_eq!(decode_utf16le_reg_string(&without_nul), "PUBG");

        // Odd trailing byte does not panic
        let mut odd = without_nul;
        odd.push(0xAA);
        assert_eq!(decode_utf16le_reg_string(&odd), "PUBG");
    }
}
