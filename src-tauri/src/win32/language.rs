const LANG_ENGLISH: u16 = 0x09;
const LANGID_EN_US: u16 = 0x0409;
#[cfg(any(windows, test))]
const PROFILE_TYPE_KEYBOARDLAYOUT: u32 = 2;

#[cfg(windows)]
const KEYBOARD_TOGGLE_SUBKEY: &str = r"Keyboard Layout\Toggle";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForceLangResult {
    Applied,
    SkippedNoEnglish,
    AccessDeniedElevation,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedProfile {
    pub profile_type: u32,
    pub lang_id: u16,
    pub clsid: [u8; 16],
    pub guid_profile: [u8; 16],
    pub hkl: isize,
}

impl SavedProfile {
    fn from_hkl(hkl: isize) -> Self {
        Self {
            profile_type: PROFILE_TYPE_KEYBOARDLAYOUT,
            lang_id: hkl_lang_id(hkl),
            clsid: [0; 16],
            guid_profile: [0; 16],
            hkl,
        }
    }
}

fn clean_registry_hotkey(value: Option<&str>) -> Option<&str> {
    value
        .map(|v| v.trim_matches(|c: char| c == '\0' || c.is_whitespace()))
        .filter(|v| !v.is_empty())
}

pub fn parse_grave_accent_toggle(language_hotkey: Option<&str>, hotkey: Option<&str>) -> bool {
    let effective =
        clean_registry_hotkey(language_hotkey).or_else(|| clean_registry_hotkey(hotkey));
    effective == Some("4")
}

#[cfg(windows)]
pub fn is_grave_accent_os_toggle() -> bool {
    use super::{query_reg_string, to_wide_null, OwnedHkey};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ};

    let subkey = to_wide_null(KEYBOARD_TOGGLE_SUBKEY);
    let mut raw_key = HKEY::default();
    // SAFETY: `subkey` is a NUL-terminated UTF-16 string and `raw_key` is a valid out pointer.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            Some(0),
            KEY_READ,
            &mut raw_key,
        )
    };
    if status != ERROR_SUCCESS {
        return false;
    }

    let key = OwnedHkey(raw_key);
    let language_hotkey = query_reg_string(key.0, "Language Hotkey");
    let hotkey = query_reg_string(key.0, "Hotkey");
    parse_grave_accent_toggle(language_hotkey.as_deref(), hotkey.as_deref())
}

#[cfg(not(windows))]
pub fn is_grave_accent_os_toggle() -> bool {
    false
}

#[inline]
fn hkl_lang_id(hkl: isize) -> u16 {
    (hkl as usize & 0xFFFF) as u16
}

#[inline]
fn primary_lang_id(lang_id: u16) -> u16 {
    lang_id & 0x03FF
}

#[inline]
pub fn is_english_lang_id(lang_id: u16) -> bool {
    lang_id != 0 && primary_lang_id(lang_id) == LANG_ENGLISH
}

pub fn is_english_hkl(hkl: isize) -> bool {
    hkl != 0 && is_english_lang_id(hkl_lang_id(hkl))
}

pub fn select_english_hkl(layouts: &[isize]) -> Option<isize> {
    if let Some(&en_us) = layouts
        .iter()
        .find(|&&hkl| hkl != 0 && hkl_lang_id(hkl) == LANGID_EN_US)
    {
        return Some(en_us);
    }

    layouts.iter().copied().find(|&hkl| is_english_hkl(hkl))
}

#[cfg(windows)]
fn query_installed_english_hkl() -> Option<isize> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayoutList, HKL};

    // SAFETY: Passing `None` queries the count of installed keyboard layouts.
    let count = unsafe { GetKeyboardLayoutList(None) };
    if count <= 0 {
        return None;
    }

    let mut layouts = vec![HKL::default(); count as usize];
    // SAFETY: `layouts` is a valid writable slice of length `count`.
    let written = unsafe { GetKeyboardLayoutList(Some(&mut layouts)) };
    if written <= 0 {
        return None;
    }

    let valid_len = (written as usize).min(layouts.len());
    let raw: Vec<isize> = layouts[..valid_len]
        .iter()
        .map(|hkl| hkl.0 as isize)
        .collect();
    select_english_hkl(&raw)
}

#[cfg(not(windows))]
fn query_installed_english_hkl() -> Option<isize> {
    None
}

#[cfg(windows)]
fn get_window_hkl(hwnd: isize) -> isize {
    use std::ffi::c_void;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

    if hwnd == 0 {
        return 0;
    }

    // SAFETY: `GetWindowThreadProcessId` safely validates `hwnd` and returns 0 if invalid,
    // without opening any process handle (`OpenProcess` is never called).
    let tid = unsafe { GetWindowThreadProcessId(HWND(hwnd as *mut c_void), None) };
    if tid == 0 {
        return 0;
    }

    // SAFETY: `tid` is a live thread ID retrieved from `GetWindowThreadProcessId`.
    let hkl = unsafe { GetKeyboardLayout(tid) };
    hkl.0 as isize
}

#[cfg(not(windows))]
fn get_window_hkl(_hwnd: isize) -> isize {
    0
}

#[cfg(windows)]
fn is_per_thread_input_settings() -> bool {
    use std::ffi::c_void;
    use windows::core::BOOL;
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETTHREADLOCALINPUTSETTINGS, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };

    let mut per_thread = BOOL(0);
    // SAFETY: `per_thread` is a valid `BOOL` output buffer for `SPI_GETTHREADLOCALINPUTSETTINGS`.
    let res = unsafe {
        SystemParametersInfoW(
            SPI_GETTHREADLOCALINPUTSETTINGS,
            0,
            Some((&mut per_thread as *mut BOOL).cast::<c_void>()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    res.is_ok() && per_thread.as_bool()
}

#[cfg(not(windows))]
fn is_per_thread_input_settings() -> bool {
    false
}

#[cfg(windows)]
fn post_input_lang_change(hwnd: isize, hkl: isize) -> ForceLangResult {
    use std::ffi::c_void;
    use windows::core::HRESULT;
    use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_INPUTLANGCHANGEREQUEST};

    // SAFETY: `PostMessageW` posts an asynchronous `WM_INPUTLANGCHANGEREQUEST` without opening
    // any process handle or dereferencing memory in the target window's process.
    let res = unsafe {
        PostMessageW(
            Some(HWND(hwnd as *mut c_void)),
            WM_INPUTLANGCHANGEREQUEST,
            WPARAM(0),
            LPARAM(hkl),
        )
    };

    match res {
        Ok(()) => ForceLangResult::Applied,
        Err(err) if err.code() == HRESULT::from_win32(ERROR_ACCESS_DENIED.0) => {
            ForceLangResult::AccessDeniedElevation
        }
        Err(_) => ForceLangResult::Failed,
    }
}

#[cfg(not(windows))]
fn post_input_lang_change(_hwnd: isize, _hkl: isize) -> ForceLangResult {
    ForceLangResult::Applied
}

#[cfg(windows)]
struct ComApartmentGuard {
    should_uninitialize: bool,
}

#[cfg(windows)]
impl ComApartmentGuard {
    fn acquire() -> Option<Self> {
        use windows::Win32::Foundation::{RPC_E_CHANGED_MODE, S_FALSE, S_OK};
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};

        // SAFETY: Initializes COM STA on this thread when not yet initialized.
        // Only `S_OK` and `S_FALSE` increment the COM refcount and must call `CoUninitialize`.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if hr == S_OK || hr == S_FALSE {
            Some(Self {
                should_uninitialize: true,
            })
        } else if hr == RPC_E_CHANGED_MODE {
            Some(Self {
                should_uninitialize: false,
            })
        } else {
            None
        }
    }
}

#[cfg(windows)]
impl Drop for ComApartmentGuard {
    fn drop(&mut self) {
        if self.should_uninitialize {
            use windows::Win32::System::Com::CoUninitialize;

            // SAFETY: Balances the matching `S_OK`/`S_FALSE` `CoInitializeEx` on this thread.
            unsafe { CoUninitialize() };
        }
    }
}

#[cfg(windows)]
fn create_tsf_profile_mgr() -> Option<(
    ComApartmentGuard,
    windows::Win32::UI::TextServices::ITfInputProcessorProfileMgr,
)> {
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::UI::TextServices::{
        CLSID_TF_InputProcessorProfiles, ITfInputProcessorProfileMgr,
    };

    let com = ComApartmentGuard::acquire()?;
    // SAFETY: `CLSID_TF_InputProcessorProfiles` is an in-process Windows TSF system class
    // implementing `ITfInputProcessorProfileMgr`. `com` is returned after `mgr` in tuple order,
    // wait — Rust drops struct/tuple fields in declaration order (field 0 before field 1)!
    // Wait: return `(mgr, com)` so `mgr` is ALWAYS dropped before `com` (`CoUninitialize`)!
    let mgr: ITfInputProcessorProfileMgr =
        unsafe { CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER) }
            .ok()?;
    Some((com, mgr))
}

#[cfg(windows)]
fn with_tsf_profile_mgr<R>(
    f: impl FnOnce(&windows::Win32::UI::TextServices::ITfInputProcessorProfileMgr) -> R,
) -> Option<R> {
    let (_com, mgr) = create_tsf_profile_mgr()?;
    // Ensure `mgr` is dropped BEFORE `_com` (`CoUninitialize`), regardless of tuple field order.
    let result = f(&mgr);
    drop(mgr);
    Some(result)
}

#[cfg(windows)]
fn tsf_get_active_keyboard_profile() -> Option<SavedProfile> {
    use windows::Win32::UI::TextServices::{GUID_TFCAT_TIP_KEYBOARD, TF_INPUTPROCESSORPROFILE};

    with_tsf_profile_mgr(|mgr| {
        let mut profile = TF_INPUTPROCESSORPROFILE::default();
        // SAFETY: `GUID_TFCAT_TIP_KEYBOARD` and `profile` are valid pointers for the call.
        unsafe { mgr.GetActiveProfile(&GUID_TFCAT_TIP_KEYBOARD, &mut profile) }.ok()?;
        if profile.langid == 0 {
            return None;
        }
        Some(SavedProfile {
            profile_type: profile.dwProfileType,
            lang_id: profile.langid,
            clsid: profile.clsid.to_u128().to_le_bytes(),
            guid_profile: profile.guidProfile.to_u128().to_le_bytes(),
            hkl: profile.hkl.0 as isize,
        })
    })
    .flatten()
}

#[cfg(not(windows))]
fn tsf_get_active_keyboard_profile() -> Option<SavedProfile> {
    None
}

#[cfg(windows)]
fn tsf_activate_keyboard_layout(hkl: isize) -> bool {
    use std::ffi::c_void;
    use windows::core::GUID;
    use windows::Win32::UI::Input::KeyboardAndMouse::HKL;
    use windows::Win32::UI::TextServices::{
        TF_IPPMF_DONTCARECURRENTINPUTLANGUAGE, TF_IPPMF_FORSESSION, TF_PROFILETYPE_KEYBOARDLAYOUT,
    };

    if hkl == 0 {
        return false;
    }

    with_tsf_profile_mgr(|mgr| {
        let zero = GUID::zeroed();
        // SAFETY: Activates the specified keyboard layout for the current user session via TSF
        // without opening or injecting into any target process.
        unsafe {
            mgr.ActivateProfile(
                TF_PROFILETYPE_KEYBOARDLAYOUT,
                hkl_lang_id(hkl),
                &zero,
                &zero,
                HKL(hkl as *mut c_void),
                TF_IPPMF_FORSESSION | TF_IPPMF_DONTCARECURRENTINPUTLANGUAGE,
            )
        }
        .is_ok()
    })
    .unwrap_or(false)
}

#[cfg(not(windows))]
fn tsf_activate_keyboard_layout(_hkl: isize) -> bool {
    false
}

#[cfg(windows)]
fn tsf_restore_profile(profile: SavedProfile) -> bool {
    use std::ffi::c_void;
    use windows::core::GUID;
    use windows::Win32::UI::Input::KeyboardAndMouse::HKL;
    use windows::Win32::UI::TextServices::{
        TF_IPPMF_DONTCARECURRENTINPUTLANGUAGE, TF_IPPMF_FORSESSION,
    };

    with_tsf_profile_mgr(|mgr| {
        let clsid = GUID::from_u128(u128::from_le_bytes(profile.clsid));
        let guid_profile = GUID::from_u128(u128::from_le_bytes(profile.guid_profile));
        // SAFETY: Restores the previously captured TSF layout/IME profile for the user session.
        unsafe {
            mgr.ActivateProfile(
                profile.profile_type,
                profile.lang_id,
                &clsid,
                &guid_profile,
                HKL(profile.hkl as *mut c_void),
                TF_IPPMF_FORSESSION | TF_IPPMF_DONTCARECURRENTINPUTLANGUAGE,
            )
        }
        .is_ok()
    })
    .unwrap_or(false)
}

#[cfg(not(windows))]
fn tsf_restore_profile(_profile: SavedProfile) -> bool {
    false
}

#[derive(Debug)]
pub struct LanguageGuard {
    english_hkl: Option<isize>,
    saved_prev_profile: Option<SavedProfile>,
}

impl Default for LanguageGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguageGuard {
    pub fn new() -> Self {
        Self {
            english_hkl: query_installed_english_hkl(),
            saved_prev_profile: None,
        }
    }

    pub fn has_english_layout(&self) -> bool {
        self.english_hkl.is_some()
    }

    pub fn force_english_on(&mut self, hwnd: isize, prev_hwnd: Option<isize>) -> ForceLangResult {
        self.force_english_on_with(
            hwnd,
            prev_hwnd,
            query_installed_english_hkl,
            is_per_thread_input_settings,
            get_window_hkl,
            tsf_get_active_keyboard_profile,
            post_input_lang_change,
            tsf_activate_keyboard_layout,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn force_english_on_with<FQueryEn, FPerThread, FGetHkl, FTsfGet, FPost, FTsfActivateEn>(
        &mut self,
        hwnd: isize,
        prev_hwnd: Option<isize>,
        query_en: FQueryEn,
        is_per_thread: FPerThread,
        mut get_hkl: FGetHkl,
        tsf_get_active: FTsfGet,
        post_change: FPost,
        tsf_activate_en: FTsfActivateEn,
    ) -> ForceLangResult
    where
        FQueryEn: FnOnce() -> Option<isize>,
        FPerThread: FnOnce() -> bool,
        FGetHkl: FnMut(isize) -> isize,
        FTsfGet: FnOnce() -> Option<SavedProfile>,
        FPost: FnOnce(isize, isize) -> ForceLangResult,
        FTsfActivateEn: FnOnce(isize) -> bool,
    {
        self.english_hkl = query_en();
        let Some(en_hkl) = self.english_hkl else {
            self.saved_prev_profile = None;
            return ForceLangResult::SkippedNoEnglish;
        };

        if hwnd == 0 {
            return ForceLangResult::Applied;
        }

        let per_thread = is_per_thread();
        let prev_to_save = if !per_thread {
            if let Some(tsf_profile) =
                tsf_get_active().filter(|p| p.lang_id != 0 && !is_english_lang_id(p.lang_id))
            {
                Some(tsf_profile)
            } else {
                let pubg_hkl = get_hkl(hwnd);
                if pubg_hkl != 0 && !is_english_hkl(pubg_hkl) {
                    Some(SavedProfile::from_hkl(pubg_hkl))
                } else {
                    prev_hwnd
                        .filter(|&prev| prev != 0 && prev != hwnd)
                        .map(&mut get_hkl)
                        .filter(|&hkl| hkl != 0 && !is_english_hkl(hkl))
                        .map(SavedProfile::from_hkl)
                }
            }
        } else {
            None
        };

        let post_result = post_change(hwnd, en_hkl);
        let tsf_applied = if !per_thread {
            tsf_activate_en(en_hkl)
        } else {
            false
        };

        match post_result {
            ForceLangResult::AccessDeniedElevation => {
                self.saved_prev_profile = None;
                ForceLangResult::AccessDeniedElevation
            }
            ForceLangResult::Applied => {
                if per_thread {
                    self.saved_prev_profile = None;
                } else if prev_to_save.is_some() {
                    self.saved_prev_profile = prev_to_save;
                }
                ForceLangResult::Applied
            }
            ForceLangResult::Failed if tsf_applied => {
                if prev_to_save.is_some() {
                    self.saved_prev_profile = prev_to_save;
                }
                ForceLangResult::Applied
            }
            ForceLangResult::SkippedNoEnglish | ForceLangResult::Failed => {
                self.saved_prev_profile = None;
                post_result
            }
        }
    }

    pub fn ensure_english_on(&self, hwnd: isize) -> ForceLangResult {
        self.ensure_english_on_with(
            hwnd,
            is_per_thread_input_settings,
            get_window_hkl,
            tsf_get_active_keyboard_profile,
            post_input_lang_change,
            tsf_activate_keyboard_layout,
        )
    }

    fn ensure_english_on_with<FPerThread, FGetHkl, FTsfGet, FPost, FTsfActivateEn>(
        &self,
        hwnd: isize,
        is_per_thread: FPerThread,
        get_hkl: FGetHkl,
        tsf_get_active: FTsfGet,
        post_change: FPost,
        tsf_activate_en: FTsfActivateEn,
    ) -> ForceLangResult
    where
        FPerThread: FnOnce() -> bool,
        FGetHkl: FnOnce(isize) -> isize,
        FTsfGet: FnOnce() -> Option<SavedProfile>,
        FPost: FnOnce(isize, isize) -> ForceLangResult,
        FTsfActivateEn: FnOnce(isize) -> bool,
    {
        let Some(en_hkl) = self.english_hkl else {
            return ForceLangResult::SkippedNoEnglish;
        };

        if hwnd == 0 {
            return ForceLangResult::Applied;
        }

        let current_hkl = get_hkl(hwnd);
        let hkl_non_english = current_hkl != 0 && !is_english_hkl(current_hkl);
        let per_thread = is_per_thread();
        let tsf_non_english = if !per_thread {
            tsf_get_active()
                .map(|p| p.lang_id != 0 && !is_english_lang_id(p.lang_id))
                .unwrap_or(false)
        } else {
            false
        };

        if !hkl_non_english && !tsf_non_english {
            return ForceLangResult::Applied;
        }

        let post_result = post_change(hwnd, en_hkl);
        let tsf_applied = if !per_thread {
            tsf_activate_en(en_hkl)
        } else {
            false
        };

        match post_result {
            ForceLangResult::AccessDeniedElevation => ForceLangResult::AccessDeniedElevation,
            ForceLangResult::Applied => ForceLangResult::Applied,
            ForceLangResult::Failed if tsf_applied => ForceLangResult::Applied,
            other => other,
        }
    }

    pub fn restore_if_needed(&mut self, next_hwnd: isize) {
        self.restore_if_needed_with(
            next_hwnd,
            is_per_thread_input_settings,
            tsf_restore_profile,
            post_input_lang_change,
        );
    }

    fn restore_if_needed_with<FPerThread, FTsfRestore, FPost>(
        &mut self,
        next_hwnd: isize,
        is_per_thread: FPerThread,
        tsf_restore: FTsfRestore,
        post_change: FPost,
    ) where
        FPerThread: FnOnce() -> bool,
        FTsfRestore: FnOnce(SavedProfile) -> bool,
        FPost: FnOnce(isize, isize) -> ForceLangResult,
    {
        let Some(prev) = self.saved_prev_profile.take() else {
            return;
        };

        if next_hwnd == 0 || is_per_thread() {
            return;
        }

        let _ = tsf_restore(prev);
        if prev.hkl != 0 {
            let _ = post_change(next_hwnd, prev.hkl);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EN_US_HKL: isize = 0x0409_0409;
    const THAI_HKL: isize = 0x041E_041E;
    const PROFILE_TYPE_INPUTPROCESSOR: u32 = 1;

    #[test]
    fn parse_grave_accent_registry_precedence_and_values() {
        assert!(parse_grave_accent_toggle(Some("4"), Some("1")));
        assert!(!parse_grave_accent_toggle(Some("1"), Some("4")));
        assert!(parse_grave_accent_toggle(None, Some("4")));
        assert!(!parse_grave_accent_toggle(Some("3"), None));
        assert!(!parse_grave_accent_toggle(None, Some("3")));
        assert!(!parse_grave_accent_toggle(None, None));
        assert!(!parse_grave_accent_toggle(Some("2"), Some("4")));
        assert!(parse_grave_accent_toggle(Some("4\0"), None));
        assert!(parse_grave_accent_toggle(Some(" 4 "), Some("1")));
        assert!(parse_grave_accent_toggle(Some(""), Some("4")));
    }

    #[test]
    fn select_english_hkl_prefers_en_us_then_other_english_then_none() {
        assert_eq!(
            select_english_hkl(&[THAI_HKL, 0x0809_0809, EN_US_HKL]),
            Some(EN_US_HKL)
        );
        assert_eq!(
            select_english_hkl(&[THAI_HKL, 0x0809_0809]),
            Some(0x0809_0809)
        );
        assert_eq!(select_english_hkl(&[THAI_HKL]), None);
        assert_eq!(select_english_hkl(&[]), None);
        assert_eq!(select_english_hkl(&[0]), None);

        assert!(is_english_hkl(EN_US_HKL));
        assert!(is_english_hkl(0x0809_0809));
        assert!(!is_english_hkl(THAI_HKL));
        assert!(!is_english_hkl(0));

        // High-bit HKL (e.g. sign-extended US-Dvorak layout handle 0xF0020409)
        let dvorak_en_us = 0xF002_0409_u32 as i32 as isize;
        assert!(is_english_hkl(dvorak_en_us));
        assert_eq!(
            select_english_hkl(&[THAI_HKL, 0x0809_0809, dvorak_en_us]),
            Some(dvorak_en_us)
        );
    }

    #[test]
    fn global_mode_falls_back_to_prev_hwnd_non_english_hkl_on_repeat_pubg_entry() {
        let mut guard = LanguageGuard {
            english_hkl: Some(EN_US_HKL),
            saved_prev_profile: None,
        };

        // Repeat PUBG entry: PUBG's thread still reports EN_US_HKL from the previous lock,
        // TSF reports None (unavailable), while previous window (Discord, hwnd 200) is in Thai.
        let mut posted = Vec::new();
        let mut tsf_activated = Vec::new();
        let res = guard.force_english_on_with(
            100,
            Some(200),
            || Some(EN_US_HKL),
            || false,
            |hwnd| match hwnd {
                100 => EN_US_HKL,
                200 => THAI_HKL,
                _ => 0,
            },
            || None,
            |hwnd, hkl| {
                posted.push((hwnd, hkl));
                ForceLangResult::Applied
            },
            |hkl| {
                tsf_activated.push(hkl);
                true
            },
        );
        assert_eq!(res, ForceLangResult::Applied);
        assert_eq!(posted, vec![(100, EN_US_HKL)]);
        assert_eq!(tsf_activated, vec![EN_US_HKL]);
        assert_eq!(
            guard.saved_prev_profile,
            Some(SavedProfile::from_hkl(THAI_HKL))
        );

        // Leaving PUBG back to Discord restores Thai via TSF + PostMessageW and consumes saved profile.
        posted.clear();
        let mut tsf_restored = Vec::new();
        guard.restore_if_needed_with(
            200,
            || false,
            |profile| {
                tsf_restored.push(profile);
                true
            },
            |hwnd, hkl| {
                posted.push((hwnd, hkl));
                ForceLangResult::Applied
            },
        );
        assert_eq!(tsf_restored, vec![SavedProfile::from_hkl(THAI_HKL)]);
        assert_eq!(posted, vec![(200, THAI_HKL)]);
        assert_eq!(guard.saved_prev_profile, None);

        // Also falls back to prev_hwnd when get_window_hkl(pubg_hwnd) returns 0.
        let res_zero = guard.force_english_on_with(
            100,
            Some(200),
            || Some(EN_US_HKL),
            || false,
            |hwnd| match hwnd {
                100 => 0,
                200 => THAI_HKL,
                _ => 0,
            },
            || None,
            |_, _| ForceLangResult::Applied,
            |_| true,
        );
        assert_eq!(res_zero, ForceLangResult::Applied);
        assert_eq!(
            guard.saved_prev_profile,
            Some(SavedProfile::from_hkl(THAI_HKL))
        );
    }

    #[test]
    fn global_mode_prefers_tsf_profile_for_thai_and_japanese_ime_and_restores_on_exit() {
        let mut guard = LanguageGuard {
            english_hkl: Some(EN_US_HKL),
            saved_prev_profile: None,
        };

        // 1. Thai layout via TSF when PUBG's own HKL still says EN (common on Win10/11)
        let thai_profile = SavedProfile::from_hkl(THAI_HKL);
        let mut tsf_activated = Vec::new();
        let mut posted = Vec::new();
        let res = guard.force_english_on_with(
            100,
            Some(200),
            || Some(EN_US_HKL),
            || false,
            |_| EN_US_HKL,
            || Some(thai_profile),
            |hwnd, hkl| {
                posted.push((hwnd, hkl));
                ForceLangResult::Applied
            },
            |hkl| {
                tsf_activated.push(hkl);
                true
            },
        );
        assert_eq!(res, ForceLangResult::Applied);
        assert_eq!(posted, vec![(100, EN_US_HKL)]);
        assert_eq!(tsf_activated, vec![EN_US_HKL]);
        assert_eq!(guard.saved_prev_profile, Some(thai_profile));

        // 2. Japanese IME (TF_PROFILETYPE_INPUTPROCESSOR with hkl == 0)
        let jp_ime = SavedProfile {
            profile_type: PROFILE_TYPE_INPUTPROCESSOR,
            lang_id: 0x0411,
            clsid: [1; 16],
            guid_profile: [2; 16],
            hkl: 0,
        };
        guard.saved_prev_profile = None;
        let res_jp = guard.force_english_on_with(
            100,
            Some(200),
            || Some(EN_US_HKL),
            || false,
            |_| EN_US_HKL,
            || Some(jp_ime),
            |_, _| ForceLangResult::Applied,
            |_| true,
        );
        assert_eq!(res_jp, ForceLangResult::Applied);
        assert_eq!(guard.saved_prev_profile, Some(jp_ime));

        // Restoring JP IME restores via TSF without posting hkl == 0 via PostMessageW
        posted.clear();
        let mut tsf_restored = Vec::new();
        guard.restore_if_needed_with(
            200,
            || false,
            |profile| {
                tsf_restored.push(profile);
                true
            },
            |hwnd, hkl| {
                posted.push((hwnd, hkl));
                ForceLangResult::Applied
            },
        );
        assert_eq!(tsf_restored, vec![jp_ime]);
        assert!(posted.is_empty());
        assert_eq!(guard.saved_prev_profile, None);
    }

    #[test]
    fn global_mode_succeeds_when_post_message_fails_if_tsf_succeeds_and_per_thread_skips_tsf() {
        let mut guard = LanguageGuard {
            english_hkl: Some(EN_US_HKL),
            saved_prev_profile: None,
        };

        // PostMessageW fails (e.g., target window ignores/drops it) but TSF activates EN -> Applied + saved
        let res = guard.force_english_on_with(
            100,
            Some(200),
            || Some(EN_US_HKL),
            || false,
            |hwnd| if hwnd == 200 { THAI_HKL } else { EN_US_HKL },
            || None,
            |_, _| ForceLangResult::Failed,
            |_| true,
        );
        assert_eq!(res, ForceLangResult::Applied);
        assert_eq!(
            guard.saved_prev_profile,
            Some(SavedProfile::from_hkl(THAI_HKL))
        );

        // In per-thread input settings mode, NEVER call session-wide TSF (neither get, activate, nor restore)
        guard.saved_prev_profile = Some(SavedProfile::from_hkl(THAI_HKL));
        let mut tsf_get_called = false;
        let mut tsf_activate_called = false;
        let res_pt = guard.force_english_on_with(
            100,
            Some(200),
            || Some(EN_US_HKL),
            || true,
            |_| THAI_HKL,
            || {
                tsf_get_called = true;
                Some(SavedProfile::from_hkl(THAI_HKL))
            },
            |_, _| ForceLangResult::Applied,
            |_| {
                tsf_activate_called = true;
                true
            },
        );
        assert_eq!(res_pt, ForceLangResult::Applied);
        assert!(!tsf_get_called);
        assert!(!tsf_activate_called);
        assert_eq!(guard.saved_prev_profile, None);

        // Restore in per-thread mode also skips TSF and PostMessageW
        guard.saved_prev_profile = Some(SavedProfile::from_hkl(THAI_HKL));
        let mut tsf_restore_called = false;
        let mut post_restore_called = false;
        guard.restore_if_needed_with(
            200,
            || true,
            |_| {
                tsf_restore_called = true;
                true
            },
            |_, _| {
                post_restore_called = true;
                ForceLangResult::Applied
            },
        );
        assert!(!tsf_restore_called);
        assert!(!post_restore_called);
        assert_eq!(guard.saved_prev_profile, None);
    }

    #[test]
    fn failed_both_or_access_denied_post_message_never_records_saved_profile() {
        let mut guard = LanguageGuard {
            english_hkl: Some(EN_US_HKL),
            saved_prev_profile: None,
        };

        // Both PostMessageW and TSF fail -> Failed, nothing saved
        let failed = guard.force_english_on_with(
            100,
            Some(200),
            || Some(EN_US_HKL),
            || false,
            |hwnd| if hwnd == 200 { THAI_HKL } else { EN_US_HKL },
            || None,
            |_, _| ForceLangResult::Failed,
            |_| false,
        );
        assert_eq!(failed, ForceLangResult::Failed);
        assert_eq!(guard.saved_prev_profile, None);

        // Restore must be a no-op since nothing was saved.
        let mut tsf_restore_called = false;
        let mut post_restore_called = false;
        guard.restore_if_needed_with(
            200,
            || false,
            |_| {
                tsf_restore_called = true;
                true
            },
            |_, _| {
                post_restore_called = true;
                ForceLangResult::Applied
            },
        );
        assert!(!tsf_restore_called);
        assert!(!post_restore_called);

        // AccessDeniedElevation clears any prior saved_prev_profile and propagates warning
        guard.saved_prev_profile = Some(SavedProfile::from_hkl(THAI_HKL));
        let denied = guard.force_english_on_with(
            100,
            Some(200),
            || Some(EN_US_HKL),
            || false,
            |_| THAI_HKL,
            || Some(SavedProfile::from_hkl(THAI_HKL)),
            |_, _| ForceLangResult::AccessDeniedElevation,
            |_| true,
        );
        assert_eq!(denied, ForceLangResult::AccessDeniedElevation);
        assert_eq!(guard.saved_prev_profile, None);
    }

    #[test]
    fn ensure_english_on_detects_non_english_tsf_profile_and_reapplies_both() {
        let guard = LanguageGuard {
            english_hkl: Some(EN_US_HKL),
            saved_prev_profile: None,
        };

        // Both HKL and TSF are English -> no-op
        let mut post_calls = 0usize;
        let mut tsf_calls = 0usize;
        let ok = guard.ensure_english_on_with(
            100,
            || false,
            |_| EN_US_HKL,
            || Some(SavedProfile::from_hkl(EN_US_HKL)),
            |_, _| {
                post_calls += 1;
                ForceLangResult::Applied
            },
            |_| {
                tsf_calls += 1;
                true
            },
        );
        assert_eq!(ok, ForceLangResult::Applied);
        assert_eq!(post_calls, 0);
        assert_eq!(tsf_calls, 0);

        // Window HKL claims English, but global TSF active profile switched to Thai/JP -> re-applies both!
        let mut posted = Vec::new();
        let mut tsf_activated = Vec::new();
        let fixed = guard.ensure_english_on_with(
            100,
            || false,
            |_| EN_US_HKL,
            || Some(SavedProfile::from_hkl(THAI_HKL)),
            |hwnd, hkl| {
                posted.push((hwnd, hkl));
                ForceLangResult::Applied
            },
            |hkl| {
                tsf_activated.push(hkl);
                true
            },
        );
        assert_eq!(fixed, ForceLangResult::Applied);
        assert_eq!(posted, vec![(100, EN_US_HKL)]);
        assert_eq!(tsf_activated, vec![EN_US_HKL]);

        // Per-thread mode: HKL is English, skips TSF check and skips TSF activation
        let mut tsf_get_called = false;
        let mut post_called = false;
        let mut tsf_activate_called = false;
        let pt_ok = guard.ensure_english_on_with(
            100,
            || true,
            |_| EN_US_HKL,
            || {
                tsf_get_called = true;
                Some(SavedProfile::from_hkl(THAI_HKL))
            },
            |_, _| {
                post_called = true;
                ForceLangResult::Applied
            },
            |_| {
                tsf_activate_called = true;
                true
            },
        );
        assert_eq!(pt_ok, ForceLangResult::Applied);
        assert!(!tsf_get_called);
        assert!(!post_called);
        assert!(!tsf_activate_called);
    }
}
