use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::JoinHandle;

#[cfg(windows)]
use std::cell::RefCell;

use crate::key_filter::FilterFlags;
#[cfg(windows)]
use crate::key_filter::{FilterDecision, KeyFilterState, KeyInput};
#[cfg(any(windows, test))]
use crate::win32::capslock::CAPS_LOCK_AUTO_OFF_EXTRA_INFO;

const BLOCK_CAPS_LOCK_BIT: u64 = 1 << 60;
const LOCK_LANGUAGE_BIT: u64 = 1 << 61;
const BLOCK_MINIMIZE_BIT: u64 = 1 << 62;
const BLOCK_GRAVE_ACCENT_BIT: u64 = 1 << 63;
const MODIFIERS_MASK: u64 = 0x0FFF_FFFF;

#[cfg(any(windows, test))]
const VK_CAPITAL_CODE: u32 = 0x14;

#[cfg(windows)]
const WM_HOOK_ARM: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 1;
#[cfg(windows)]
const WM_HOOK_DISARM: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookSignal {
    EscapeTriggered,
    CheckLanguage,
}

#[inline]
fn pack_flags(flags: FilterFlags) -> u64 {
    let mut packed = u64::from(flags.escape_vk);
    packed |= (u64::from(flags.escape_modifiers) & MODIFIERS_MASK) << 32;
    if flags.block_caps_lock {
        packed |= BLOCK_CAPS_LOCK_BIT;
    }
    if flags.lock_language {
        packed |= LOCK_LANGUAGE_BIT;
    }
    if flags.block_minimize {
        packed |= BLOCK_MINIMIZE_BIT;
    }
    if flags.block_grave_accent {
        packed |= BLOCK_GRAVE_ACCENT_BIT;
    }
    packed
}

#[inline]
fn unpack_flags(packed: u64) -> FilterFlags {
    FilterFlags {
        lock_language: (packed & LOCK_LANGUAGE_BIT) != 0,
        block_minimize: (packed & BLOCK_MINIMIZE_BIT) != 0,
        block_caps_lock: (packed & BLOCK_CAPS_LOCK_BIT) != 0,
        block_grave_accent: (packed & BLOCK_GRAVE_ACCENT_BIT) != 0,
        escape_vk: packed as u32,
        escape_modifiers: ((packed >> 32) & MODIFIERS_MASK) as u32,
    }
}

struct AtomicFilterFlags {
    packed: AtomicU64,
}

impl AtomicFilterFlags {
    const fn new() -> Self {
        Self {
            packed: AtomicU64::new(0),
        }
    }

    #[inline]
    fn store(&self, flags: FilterFlags) {
        self.packed.store(pack_flags(flags), Ordering::Release);
    }

    #[inline]
    #[cfg(any(windows, test))]
    fn load(&self) -> FilterFlags {
        unpack_flags(self.packed.load(Ordering::Acquire))
    }
}

#[cfg(windows)]
struct HookContext {
    flags: Arc<AtomicFilterFlags>,
    state: KeyFilterState,
    signal_tx: Sender<HookSignal>,
}

#[cfg(windows)]
thread_local! {
    static HOOK_CTX: RefCell<Option<HookContext>> = const { RefCell::new(None) };
}

#[cfg(windows)]
#[inline]
fn is_async_key_down(vk: i32) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

    // SAFETY: `GetAsyncKeyState` is a lock-free query of current physical key state.
    let state = unsafe { GetAsyncKeyState(vk) };
    (state as u16 & 0x8000) != 0
}

#[cfg(any(windows, test))]
#[inline]
fn is_self_caps_lock_auto_off(vk: u32, flags: u32, extra_info: usize) -> bool {
    #[cfg(windows)]
    let injected_flag = windows::Win32::UI::WindowsAndMessaging::LLKHF_INJECTED.0;
    #[cfg(not(windows))]
    let injected_flag = 0x10_u32;

    vk == VK_CAPITAL_CODE
        && (flags & injected_flag) != 0
        && extra_info == CAPS_LOCK_AUTO_OFF_EXTRA_INFO
}

#[cfg(windows)]
unsafe extern "system" fn low_level_keyboard_proc(
    n_code: i32,
    w_param: windows::Win32::Foundation::WPARAM,
    l_param: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VK_LWIN, VK_RWIN, VK_SHIFT};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_ALTDOWN, WM_KEYDOWN, WM_KEYUP,
        WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    if n_code != HC_ACTION as i32 || l_param.0 == 0 {
        // SAFETY: Forwarding non-action or null-payload hook events per Win32 contract.
        return unsafe { CallNextHookEx(None, n_code, w_param, l_param) };
    }

    let is_key_down = match w_param.0 as u32 {
        WM_KEYDOWN | WM_SYSKEYDOWN => true,
        WM_KEYUP | WM_SYSKEYUP => false,
        _ => {
            // SAFETY: Forwarding unknown keyboard message types untouched.
            return unsafe { CallNextHookEx(None, n_code, w_param, l_param) };
        }
    };

    // SAFETY: When `n_code == HC_ACTION` and `l_param != 0`, Windows guarantees `l_param`
    // points to a valid `KBDLLHOOKSTRUCT` for the duration of this callback.
    let kb = unsafe { &*(l_param.0 as *const KBDLLHOOKSTRUCT) };

    if is_self_caps_lock_auto_off(kb.vkCode, kb.flags.0, kb.dwExtraInfo) {
        // SAFETY: Passing our own scoped Caps Lock auto-off toggle directly to the next hook
        // so live-enabling `block_caps_lock` while Protected still clears an active Caps Lock.
        return unsafe { CallNextHookEx(None, n_code, w_param, l_param) };
    }

    let (alt_down, ctrl_down, shift_down, win_down) = if is_key_down {
        (
            (kb.flags.0 & LLKHF_ALTDOWN.0) != 0,
            is_async_key_down(i32::from(VK_CONTROL.0)),
            is_async_key_down(i32::from(VK_SHIFT.0)),
            is_async_key_down(i32::from(VK_LWIN.0)) || is_async_key_down(i32::from(VK_RWIN.0)),
        )
    } else {
        (false, false, false, false)
    };

    let input = KeyInput {
        vk: kb.vkCode,
        is_key_down,
        alt_down,
        ctrl_down,
        shift_down,
        win_down,
    };

    let decision = HOOK_CTX
        .try_with(|cell| {
            let Ok(mut slot) = cell.try_borrow_mut() else {
                return FilterDecision::Pass;
            };
            let Some(ctx) = slot.as_mut() else {
                return FilterDecision::Pass;
            };

            let flags = ctx.flags.load();
            let decision = ctx.state.evaluate(flags, input);
            match decision {
                FilterDecision::TriggerEscape => {
                    // Non-blocking send on unbounded `mpsc::Sender`; never blocks the hook thread.
                    let _ = ctx.signal_tx.send(HookSignal::EscapeTriggered);
                }
                FilterDecision::PassAndCheckLanguage => {
                    // Non-blocking send on unbounded `mpsc::Sender`; only sent on modifier key-up.
                    let _ = ctx.signal_tx.send(HookSignal::CheckLanguage);
                }
                FilterDecision::Pass | FilterDecision::Swallow => {}
            }
            decision
        })
        .unwrap_or(FilterDecision::Pass);

    match decision {
        FilterDecision::Swallow | FilterDecision::TriggerEscape => LRESULT(1),
        FilterDecision::Pass | FilterDecision::PassAndCheckLanguage => {
            // SAFETY: Passing unblocked keystroke to the next hook in the chain.
            unsafe { CallNextHookEx(None, n_code, w_param, l_param) }
        }
    }
}

#[cfg(windows)]
fn reset_hook_filter_state() {
    HOOK_CTX.with(|cell| {
        if let Ok(mut slot) = cell.try_borrow_mut() {
            if let Some(ctx) = slot.as_mut() {
                ctx.state.reset();
            }
        }
    });
}

#[cfg(windows)]
fn install_ll_keyboard_hook() -> Result<windows::Win32::UI::WindowsAndMessaging::HHOOK, String> {
    use windows::Win32::Foundation::HINSTANCE;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowsHookExW, WH_KEYBOARD_LL};

    // SAFETY: `GetModuleHandleW(None)` returns the current executable's module handle.
    let hinstance = unsafe { GetModuleHandleW(None) }.ok().map(HINSTANCE::from);

    // SAFETY: `low_level_keyboard_proc` is a valid `HOOKPROC` installed on this dedicated
    // message-loop thread with `dwThreadId = 0` as required for `WH_KEYBOARD_LL`.
    unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(low_level_keyboard_proc), hinstance, 0) }
        .map_err(|e| format!("SetWindowsHookExW(WH_KEYBOARD_LL) failed: {e}"))
}

#[cfg(windows)]
fn uninstall_ll_keyboard_hook(
    installed: &mut Option<windows::Win32::UI::WindowsAndMessaging::HHOOK>,
    armed: &AtomicBool,
) {
    use windows::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx;

    if let Some(hhook) = installed.take() {
        // SAFETY: `hhook` was returned by `SetWindowsHookExW` on this thread and is unhooked once.
        let _ = unsafe { UnhookWindowsHookEx(hhook) };
    }
    armed.store(false, Ordering::Release);
}

#[cfg(any(windows, test))]
fn apply_arm_message<H, FReset, FInstall>(
    installed_hook: &mut Option<H>,
    armed: &AtomicBool,
    reset_state: FReset,
    install_hook: FInstall,
) where
    FReset: FnOnce(),
    FInstall: FnOnce() -> Result<H, String>,
{
    if installed_hook.is_none() {
        reset_state();
        if let Ok(hhook) = install_hook() {
            *installed_hook = Some(hhook);
            armed.store(true, Ordering::Release);
        }
    }
}

#[cfg(windows)]
fn post_thread_msg(thread_id: u32, msg: u32) {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW;

    // SAFETY: Target thread created its Win32 message queue before `spawn` returned.
    let _ = unsafe { PostThreadMessageW(thread_id, msg, WPARAM(0), LPARAM(0)) };
}

pub struct KeyboardHookHandle {
    flags: Arc<AtomicFilterFlags>,
    armed: Arc<AtomicBool>,
    #[cfg(windows)]
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl KeyboardHookHandle {
    #[cfg(windows)]
    pub fn spawn(signal_tx: Sender<HookSignal>) -> Result<Self, String> {
        use std::sync::mpsc;
        use windows::Win32::System::Threading::GetCurrentThreadId;
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, GetMessageW, PeekMessageW, TranslateMessage, HHOOK, MSG, PM_NOREMOVE,
        };

        let flags = Arc::new(AtomicFilterFlags::new());
        let armed = Arc::new(AtomicBool::new(false));
        let thread_flags = Arc::clone(&flags);
        let thread_armed = Arc::clone(&armed);
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<u32, String>>(1);

        let handle = std::thread::Builder::new()
            .name("ingame-kbd-hook".to_string())
            .spawn(move || {
                let mut msg = MSG::default();
                // SAFETY: Calling `PeekMessageW` with `PM_NOREMOVE` forces creation of the
                // Win32 thread message queue before signaling ready so no posted message is dropped.
                unsafe {
                    let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
                }

                // SAFETY: `GetCurrentThreadId` has no preconditions.
                let tid = unsafe { GetCurrentThreadId() };

                HOOK_CTX.with(|cell| {
                    *cell.borrow_mut() = Some(HookContext {
                        flags: thread_flags,
                        state: KeyFilterState::new(),
                        signal_tx,
                    });
                });

                let _ = ready_tx.send(Ok(tid));

                let mut installed_hook: Option<HHOOK> = None;

                loop {
                    // SAFETY: `msg` is a valid writable `MSG` struct on this thread.
                    let status = unsafe { GetMessageW(&mut msg, None, 0, 0) }.0;
                    if status <= 0 {
                        break;
                    }

                    if msg.hwnd.0.is_null() {
                        match msg.message {
                            WM_HOOK_ARM => {
                                apply_arm_message(
                                    &mut installed_hook,
                                    &thread_armed,
                                    reset_hook_filter_state,
                                    install_ll_keyboard_hook,
                                );
                                continue;
                            }
                            WM_HOOK_DISARM => {
                                uninstall_ll_keyboard_hook(&mut installed_hook, &thread_armed);
                                reset_hook_filter_state();
                                continue;
                            }
                            _ => {}
                        }
                    }

                    // SAFETY: `msg` was populated by `GetMessageW`.
                    unsafe {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }

                uninstall_ll_keyboard_hook(&mut installed_hook, &thread_armed);
                HOOK_CTX.with(|cell| {
                    *cell.borrow_mut() = None;
                });
            })
            .map_err(|e| format!("Failed to spawn keyboard hook thread: {e}"))?;

        match ready_rx.recv() {
            Ok(Ok(thread_id)) => Ok(Self {
                flags,
                armed,
                thread_id,
                thread: Some(handle),
            }),
            Ok(Err(err)) => {
                let _ = handle.join();
                Err(err)
            }
            Err(_) => {
                let _ = handle.join();
                Err("Keyboard hook thread terminated during startup".to_string())
            }
        }
    }

    #[cfg(not(windows))]
    pub fn spawn(_signal_tx: Sender<HookSignal>) -> Result<Self, String> {
        Ok(Self {
            flags: Arc::new(AtomicFilterFlags::new()),
            armed: Arc::new(AtomicBool::new(false)),
            thread: None,
        })
    }

    pub fn arm(&self, flags: FilterFlags) {
        self.flags.store(flags);
        #[cfg(windows)]
        post_thread_msg(self.thread_id, WM_HOOK_ARM);
        #[cfg(not(windows))]
        self.armed.store(true, Ordering::Release);
    }

    pub fn update_flags(&self, flags: FilterFlags) {
        self.flags.store(flags);
    }

    pub fn disarm(&self) {
        #[cfg(windows)]
        post_thread_msg(self.thread_id, WM_HOOK_DISARM);
        #[cfg(not(windows))]
        self.armed.store(false, Ordering::Release);
    }

    pub fn shutdown(mut self) {
        self.shutdown_inner();
    }

    #[cfg(test)]
    fn is_armed(&self) -> bool {
        self.armed.load(Ordering::Acquire)
    }

    fn shutdown_inner(&mut self) {
        if let Some(handle) = self.thread.take() {
            #[cfg(windows)]
            {
                use windows::Win32::UI::WindowsAndMessaging::WM_QUIT;

                post_thread_msg(self.thread_id, WM_HOOK_DISARM);
                post_thread_msg(self.thread_id, WM_QUIT);
            }

            if std::thread::current().id() != handle.thread().id() {
                let _ = handle.join();
            }
        }
        self.armed.store(false, Ordering::Release);
    }
}

impl Drop for KeyboardHookHandle {
    fn drop(&mut self) {
        self.shutdown_inner();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{MOD_ALT, MOD_CTRL, MOD_SHIFT};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    fn pack_and_unpack_filter_flags_roundtrip() {
        let cases = [
            FilterFlags {
                lock_language: false,
                block_minimize: false,
                block_caps_lock: false,
                block_grave_accent: false,
                escape_vk: 0,
                escape_modifiers: 0,
            },
            FilterFlags {
                lock_language: true,
                block_minimize: true,
                block_caps_lock: true,
                block_grave_accent: true,
                escape_vk: 0x79,
                escape_modifiers: 0,
            },
            FilterFlags {
                lock_language: true,
                block_minimize: false,
                block_caps_lock: true,
                block_grave_accent: true,
                escape_vk: 0x09,
                escape_modifiers: MOD_CTRL | MOD_ALT | MOD_SHIFT,
            },
            FilterFlags {
                lock_language: false,
                block_minimize: true,
                block_caps_lock: false,
                block_grave_accent: false,
                escape_vk: 0xFF,
                escape_modifiers: MOD_ALT,
            },
            FilterFlags {
                lock_language: false,
                block_minimize: false,
                block_caps_lock: true,
                block_grave_accent: false,
                escape_vk: 0x14,
                escape_modifiers: MOD_CTRL | MOD_ALT | MOD_SHIFT,
            },
        ];

        let atomic = AtomicFilterFlags::new();
        for flags in cases {
            atomic.store(flags);
            assert_eq!(atomic.load(), flags);
        }
    }

    #[test]
    fn self_injected_caps_lock_bypass_matches_only_exact_magic_and_vk_capital() {
        let injected = 0x10_u32;
        assert!(is_self_caps_lock_auto_off(
            VK_CAPITAL_CODE,
            injected,
            CAPS_LOCK_AUTO_OFF_EXTRA_INFO
        ));
        assert!(is_self_caps_lock_auto_off(
            VK_CAPITAL_CODE,
            injected | 0x80, // e.g., LLKHF_UP
            CAPS_LOCK_AUTO_OFF_EXTRA_INFO
        ));
        // Wrong VK
        assert!(!is_self_caps_lock_auto_off(
            0x41,
            injected,
            CAPS_LOCK_AUTO_OFF_EXTRA_INFO
        ));
        // Physical keypress (LLKHF_INJECTED not set) even if extraInfo somehow matches
        assert!(!is_self_caps_lock_auto_off(
            VK_CAPITAL_CODE,
            0,
            CAPS_LOCK_AUTO_OFF_EXTRA_INFO
        ));
        // Different dwExtraInfo
        assert!(!is_self_caps_lock_auto_off(VK_CAPITAL_CODE, injected, 0));
        assert!(!is_self_caps_lock_auto_off(
            VK_CAPITAL_CODE,
            injected,
            CAPS_LOCK_AUTO_OFF_EXTRA_INFO ^ 1
        ));
    }

    #[test]
    fn hook_thread_spawns_arms_disarms_and_shuts_down_cleanly() {
        let start = Instant::now();
        let (tx, _rx) = mpsc::channel();
        let handle = KeyboardHookHandle::spawn(tx).expect("spawn keyboard hook thread");
        assert!(!handle.is_armed());

        let default_flags = FilterFlags {
            lock_language: true,
            block_minimize: true,
            block_caps_lock: true,
            block_grave_accent: true,
            escape_vk: 0x79, // VK_F10
            escape_modifiers: 0,
        };
        handle.arm(default_flags);

        // Wait briefly for the hook thread to process WM_HOOK_ARM so we also confirm
        // SetWindowsHookExW actually succeeded on the hook thread.
        let arm_deadline = Instant::now() + Duration::from_millis(200);
        while !handle.is_armed() && Instant::now() < arm_deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(handle.is_armed(), "WH_KEYBOARD_LL should be armed");

        let updated_flags = FilterFlags {
            lock_language: false,
            block_minimize: true,
            block_caps_lock: false,
            block_grave_accent: false,
            escape_vk: 0x78, // VK_F9
            escape_modifiers: 0,
        };
        handle.update_flags(updated_flags);
        assert_eq!(handle.flags.load(), updated_flags);

        handle.disarm();
        let disarm_deadline = Instant::now() + Duration::from_millis(200);
        while handle.is_armed() && Instant::now() < disarm_deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!handle.is_armed(), "WH_KEYBOARD_LL should be disarmed");

        handle.shutdown();

        assert!(
            start.elapsed() < Duration::from_millis(500),
            "hook lifecycle exceeded 500 ms: {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn hook_handle_drop_disarms_and_joins_cleanly_even_after_repeated_arm_disarm() {
        let start = Instant::now();
        let (tx, _rx) = mpsc::channel();
        let handle = KeyboardHookHandle::spawn(tx).expect("spawn keyboard hook thread");

        let flags = FilterFlags {
            lock_language: true,
            block_minimize: true,
            block_caps_lock: true,
            block_grave_accent: false,
            escape_vk: 0x79,
            escape_modifiers: 0,
        };
        // Repeated arm & disarm must remain idempotent without leaking hooks or hanging.
        handle.arm(flags);
        handle.arm(flags);
        handle.disarm();
        handle.disarm();
        handle.arm(flags);
        drop(handle);

        assert!(
            start.elapsed() < Duration::from_millis(500),
            "hook drop lifecycle exceeded 500 ms: {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn redundant_arm_preserves_live_key_filter_swallowed_mask() {
        use crate::key_filter::{FilterDecision, KeyFilterState, KeyInput};

        const VK_TAB: u32 = 0x09;
        const VK_LWIN: u32 = 0x5B;

        let flags = FilterFlags {
            lock_language: true,
            block_minimize: true,
            block_caps_lock: true,
            block_grave_accent: true,
            escape_vk: 0x79,
            escape_modifiers: 0,
        };

        let mut installed_hook: Option<usize> = None;
        let armed = AtomicBool::new(false);
        let mut state = KeyFilterState::new();
        let mut reset_calls = 0usize;
        let mut install_calls = 0usize;

        // First arm resets state and installs the hook.
        apply_arm_message(
            &mut installed_hook,
            &armed,
            || {
                reset_calls += 1;
                state.reset();
            },
            || {
                install_calls += 1;
                Ok(1)
            },
        );
        assert_eq!(installed_hook, Some(1));
        assert!(armed.load(Ordering::Acquire));
        assert_eq!(reset_calls, 1);
        assert_eq!(install_calls, 1);

        // Simulate swallowing LWin down and Alt+Tab down while armed.
        assert_eq!(
            state.evaluate(
                flags,
                KeyInput {
                    vk: VK_LWIN,
                    is_key_down: true,
                    alt_down: false,
                    ctrl_down: false,
                    shift_down: false,
                    win_down: false,
                }
            ),
            FilterDecision::Swallow
        );
        assert_eq!(
            state.evaluate(
                flags,
                KeyInput {
                    vk: VK_TAB,
                    is_key_down: true,
                    alt_down: true,
                    ctrl_down: false,
                    shift_down: false,
                    win_down: false,
                }
            ),
            FilterDecision::Swallow
        );

        // Redundant WM_HOOK_ARM while already armed must NOT reset KeyFilterState or reinstall.
        apply_arm_message(
            &mut installed_hook,
            &armed,
            || {
                reset_calls += 1;
                state.reset();
            },
            || {
                install_calls += 1;
                Ok(2)
            },
        );
        assert_eq!(installed_hook, Some(1));
        assert!(armed.load(Ordering::Acquire));
        assert_eq!(reset_calls, 1);
        assert_eq!(install_calls, 1);

        // Matching key-up events must still be swallowed for key-up symmetry.
        assert_eq!(
            state.evaluate(
                flags,
                KeyInput {
                    vk: VK_LWIN,
                    is_key_down: false,
                    alt_down: false,
                    ctrl_down: false,
                    shift_down: false,
                    win_down: false,
                }
            ),
            FilterDecision::Swallow
        );
        assert_eq!(
            state.evaluate(
                flags,
                KeyInput {
                    vk: VK_TAB,
                    is_key_down: false,
                    alt_down: false,
                    ctrl_down: false,
                    shift_down: false,
                    win_down: false,
                }
            ),
            FilterDecision::Swallow
        );
    }
}
