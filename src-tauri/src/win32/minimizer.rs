#[inline]
fn is_eligible_foreground_candidate(is_window: bool, is_visible: bool, is_iconic: bool) -> bool {
    is_window && is_visible && !is_iconic
}

fn select_foreground_target<FValid, FShell>(
    pubg_hwnd: isize,
    fallback_hwnd: Option<isize>,
    is_valid_visible_window: FValid,
    get_shell_window: FShell,
) -> Option<isize>
where
    FValid: FnOnce(isize) -> bool,
    FShell: FnOnce() -> isize,
{
    if let Some(candidate) = fallback_hwnd {
        if candidate != 0 && candidate != pubg_hwnd && is_valid_visible_window(candidate) {
            return Some(candidate);
        }
    }

    let shell = get_shell_window();
    if shell != 0 && shell != pubg_hwnd {
        Some(shell)
    } else {
        None
    }
}

#[cfg(windows)]
pub fn minimize_pubg_window(pubg_hwnd: isize, fallback_hwnd: Option<isize>) {
    use std::ffi::c_void;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetShellWindow, IsIconic, IsWindow, IsWindowVisible, SetForegroundWindow, ShowWindowAsync,
        SW_MINIMIZE,
    };

    if pubg_hwnd == 0 {
        return;
    }

    let pubg = HWND(pubg_hwnd as *mut c_void);
    // SAFETY: `ShowWindowAsync` safely validates `pubg` and posts a non-blocking minimization
    // request so a frozen game thread can never hang the coordinator, without opening any
    // process handle or injecting synthetic input.
    unsafe {
        let _ = ShowWindowAsync(pubg, SW_MINIMIZE);
    }

    let target = select_foreground_target(
        pubg_hwnd,
        fallback_hwnd,
        |candidate| {
            let hwnd = HWND(candidate as *mut c_void);
            // SAFETY: `IsWindow`, `IsWindowVisible`, and `IsIconic` safely validate window handles.
            unsafe {
                is_eligible_foreground_candidate(
                    IsWindow(Some(hwnd)).as_bool(),
                    IsWindowVisible(hwnd).as_bool(),
                    IsIconic(hwnd).as_bool(),
                )
            }
        },
        || {
            // SAFETY: `GetShellWindow` takes no arguments and returns a handle or null.
            unsafe { GetShellWindow().0 as isize }
        },
    );

    if let Some(target_hwnd) = target {
        // SAFETY: `target_hwnd` is a non-null validated window handle or desktop shell window.
        unsafe {
            let _ = SetForegroundWindow(HWND(target_hwnd as *mut c_void));
        }
    }
}

#[cfg(not(windows))]
pub fn minimize_pubg_window(_pubg_hwnd: isize, _fallback_hwnd: Option<isize>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_foreground_target_prefers_valid_non_pubg_fallback_else_shell() {
        assert_eq!(
            select_foreground_target(100, Some(200), |_| true, || 999),
            Some(200)
        );
        assert_eq!(
            select_foreground_target(100, Some(100), |_| true, || 999),
            Some(999)
        );
        assert_eq!(
            select_foreground_target(100, Some(0), |_| true, || 999),
            Some(999)
        );
        assert_eq!(
            select_foreground_target(100, Some(200), |_| false, || 999),
            Some(999)
        );
        assert_eq!(
            select_foreground_target(100, None, |_| true, || 999),
            Some(999)
        );
        assert_eq!(select_foreground_target(100, None, |_| true, || 0), None);
    }

    #[test]
    fn minimized_fallback_window_is_rejected_and_falls_back_to_shell() {
        assert!(is_eligible_foreground_candidate(true, true, false));
        assert!(!is_eligible_foreground_candidate(true, true, true));
        assert!(!is_eligible_foreground_candidate(true, false, false));
        assert!(!is_eligible_foreground_candidate(false, true, false));

        assert_eq!(
            select_foreground_target(
                100,
                Some(200),
                |_| is_eligible_foreground_candidate(true, true, true),
                || 999
            ),
            Some(999)
        );
        assert_eq!(
            select_foreground_target(
                100,
                Some(200),
                |_| is_eligible_foreground_candidate(true, true, false),
                || 999
            ),
            Some(200)
        );
    }

    #[test]
    fn minimize_pubg_window_ignores_null_hwnd() {
        minimize_pubg_window(0, None);
    }
}
