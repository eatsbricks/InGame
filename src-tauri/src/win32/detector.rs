use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::cell::RefCell;

const PID_CACHE_TTL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForegroundNotice {
    pub hwnd: isize,
    pub is_pubg: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectorEvent {
    Foreground(ForegroundNotice),
    WindowMinimized(isize),
}

pub fn is_pubg_exe_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("tslgame.exe")
}

#[cfg(windows)]
fn decode_wide_cstr(wide: &[u16]) -> String {
    let end = wide
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..end])
}

#[cfg(windows)]
struct OwnedHandle(windows::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;

        if !self.0.is_invalid() {
            // SAFETY: `self.0` is a valid kernel handle owned exclusively by this guard.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(windows)]
fn snapshot_running_processes() -> Option<HashMap<u32, bool>> {
    use std::mem::size_of;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    // SAFETY: `TH32CS_SNAPPROCESS` enumerates process metadata via Toolhelp snapshot
    // without opening any handle (`OpenProcess`) to `TslGame.exe` or any other process.
    let Ok(raw_handle) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return None;
    };
    if raw_handle.is_invalid() {
        return None;
    }
    let snapshot = OwnedHandle(raw_handle);

    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };

    let mut refreshed = HashMap::new();
    // SAFETY: `snapshot.0` is a valid process snapshot handle and `entry.dwSize` is initialized.
    if unsafe { Process32FirstW(snapshot.0, &mut entry) }.is_ok() {
        loop {
            let exe_name = decode_wide_cstr(&entry.szExeFile);
            refreshed.insert(entry.th32ProcessID, is_pubg_exe_name(&exe_name));

            // SAFETY: `snapshot.0` and `entry` remain valid for subsequent enumeration steps.
            if unsafe { Process32NextW(snapshot.0, &mut entry) }.is_err() {
                break;
            }
        }
    }

    if refreshed.is_empty() {
        None
    } else {
        Some(refreshed)
    }
}

#[cfg(not(windows))]
fn snapshot_running_processes() -> Option<HashMap<u32, bool>> {
    None
}

#[derive(Debug)]
pub struct PidNameCache {
    entries: HashMap<u32, bool>,
    last_refresh: Option<Instant>,
    ttl: Duration,
}

impl Default for PidNameCache {
    fn default() -> Self {
        Self::new()
    }
}

impl PidNameCache {
    pub fn new() -> Self {
        Self::with_ttl(PID_CACHE_TTL)
    }

    fn with_ttl(ttl: Duration) -> Self {
        Self {
            entries: HashMap::new(),
            last_refresh: None,
            ttl,
        }
    }

    pub fn is_pubg_pid(&mut self, pid: u32) -> bool {
        self.is_pubg_pid_at(pid, Instant::now(), snapshot_running_processes)
    }

    fn is_pubg_pid_at<F>(&mut self, pid: u32, now: Instant, load_snapshot: F) -> bool
    where
        F: FnOnce() -> Option<HashMap<u32, bool>>,
    {
        if pid == 0 {
            return false;
        }

        let is_fresh = self
            .last_refresh
            .is_some_and(|refreshed_at| now.saturating_duration_since(refreshed_at) < self.ttl);

        if is_fresh {
            if let Some(&is_pubg) = self.entries.get(&pid) {
                return is_pubg;
            }
        }

        if let Some(refreshed) = load_snapshot() {
            if !refreshed.is_empty() {
                self.entries = refreshed;
                self.last_refresh = Some(now);
                return self.entries.get(&pid).copied().unwrap_or(false);
            }
        }

        // Never return stale entries if snapshot refresh failed (prevents false arming on reused PIDs).
        false
    }
}

#[cfg(windows)]
fn classify_hwnd(hwnd: isize, cache: &mut PidNameCache) -> ForegroundNotice {
    use std::ffi::c_void;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetAncestor, GetWindowThreadProcessId, IsIconic, IsWindowVisible, GA_ROOT,
    };

    if hwnd == 0 {
        return ForegroundNotice {
            hwnd: 0,
            is_pubg: false,
        };
    }

    let raw = HWND(hwnd as *mut c_void);
    // SAFETY: Win32 window inspection calls safely validate `raw` without opening any process handle.
    let is_visible_top_level = unsafe {
        IsWindowVisible(raw).as_bool()
            && !IsIconic(raw).as_bool()
            && GetAncestor(raw, GA_ROOT) == raw
    };

    let is_pubg = if is_visible_top_level {
        let mut pid: u32 = 0;
        // SAFETY: `raw` is a window handle and `pid` is a valid out pointer; no process handle is opened.
        unsafe {
            GetWindowThreadProcessId(raw, Some(&mut pid));
        }
        pid != 0 && cache.is_pubg_pid(pid)
    } else {
        false
    };

    ForegroundNotice { hwnd, is_pubg }
}

#[cfg(windows)]
pub fn inspect_current_foreground(cache: &mut PidNameCache) -> ForegroundNotice {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    // SAFETY: `GetForegroundWindow` has no preconditions and returns a handle or null.
    let hwnd = unsafe { GetForegroundWindow() };
    classify_hwnd(hwnd.0 as isize, cache)
}

#[cfg(not(windows))]
pub fn inspect_current_foreground(_cache: &mut PidNameCache) -> ForegroundNotice {
    ForegroundNotice {
        hwnd: 0,
        is_pubg: false,
    }
}

#[cfg(windows)]
pub fn is_window_alive(hwnd: isize) -> bool {
    use std::ffi::c_void;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::IsWindow;

    if hwnd == 0 {
        return false;
    }

    // SAFETY: `IsWindow` safely validates any HWND value without side effects.
    unsafe { IsWindow(Some(HWND(hwnd as *mut c_void))).as_bool() }
}

#[cfg(not(windows))]
pub fn is_window_alive(_hwnd: isize) -> bool {
    false
}

#[cfg(windows)]
pub fn is_window_iconic(hwnd: isize) -> bool {
    use std::ffi::c_void;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::IsIconic;

    if hwnd == 0 {
        return false;
    }

    // SAFETY: `IsIconic` safely validates any HWND value without opening any process handle.
    unsafe { IsIconic(HWND(hwnd as *mut c_void)).as_bool() }
}

#[cfg(not(windows))]
pub fn is_window_iconic(_hwnd: isize) -> bool {
    false
}

#[cfg(windows)]
struct DetectorContext {
    tx: Sender<DetectorEvent>,
    cache: PidNameCache,
}

#[cfg(windows)]
thread_local! {
    static DETECTOR_CTX: RefCell<Option<DetectorContext>> = const { RefCell::new(None) };
}

#[cfg(windows)]
unsafe extern "system" fn win_event_proc(
    _hook: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
    event: u32,
    hwnd: windows::Win32::Foundation::HWND,
    id_object: i32,
    id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, CHILDID_SELF, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND,
        EVENT_SYSTEM_MINIMIZESTART, OBJID_WINDOW,
    };

    if id_object != OBJID_WINDOW.0 || id_child != CHILDID_SELF as i32 {
        return;
    }

    let raw_hwnd = hwnd.0 as isize;

    let _ = DETECTOR_CTX.try_with(|cell| {
        let Ok(mut slot) = cell.try_borrow_mut() else {
            return;
        };
        let Some(ctx) = slot.as_mut() else {
            return;
        };

        match event {
            EVENT_SYSTEM_FOREGROUND => {
                let notice = classify_hwnd(raw_hwnd, &mut ctx.cache);
                let _ = ctx.tx.send(DetectorEvent::Foreground(notice));
            }
            EVENT_SYSTEM_MINIMIZESTART => {
                if raw_hwnd != 0 {
                    let _ = ctx.tx.send(DetectorEvent::WindowMinimized(raw_hwnd));
                }
            }
            EVENT_SYSTEM_MINIMIZEEND => {
                // Re-classify on restore end when this window is foreground so a race where
                // EVENT_SYSTEM_FOREGROUND fired before IsIconic cleared never misses re-arming.
                // SAFETY: `GetForegroundWindow` has no preconditions.
                if raw_hwnd != 0 && unsafe { GetForegroundWindow() } == hwnd {
                    let notice = classify_hwnd(raw_hwnd, &mut ctx.cache);
                    let _ = ctx.tx.send(DetectorEvent::Foreground(notice));
                }
            }
            _ => {}
        }
    });
}

pub struct ForegroundDetector {
    #[cfg(windows)]
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl ForegroundDetector {
    #[cfg(windows)]
    pub fn start(tx: Sender<DetectorEvent>) -> Result<Self, String> {
        use std::sync::mpsc;
        use windows::Win32::System::Threading::GetCurrentThreadId;
        use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent};
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, GetMessageW, PeekMessageW, TranslateMessage, EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART, MSG, PM_NOREMOVE,
            WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
        };

        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<u32, String>>(1);

        let handle = std::thread::Builder::new()
            .name("ingame-fg-detector".to_string())
            .spawn(move || {
                let mut msg = MSG::default();
                // SAFETY: Calling `PeekMessageW` with `PM_NOREMOVE` forces Windows to create this
                // thread's message queue before signaling readiness so `WM_QUIT` can never be dropped.
                unsafe {
                    let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
                }

                // SAFETY: `GetCurrentThreadId` is always safe to call.
                let tid = unsafe { GetCurrentThreadId() };

                DETECTOR_CTX.with(|cell| {
                    *cell.borrow_mut() = Some(DetectorContext {
                        tx,
                        cache: PidNameCache::new(),
                    });
                });

                // SAFETY: `win_event_proc` is a valid `WINEVENTPROC` invoked on this thread's
                // message loop because `WINEVENT_OUTOFCONTEXT` is set.
                let fg_hook = unsafe {
                    SetWinEventHook(
                        EVENT_SYSTEM_FOREGROUND,
                        EVENT_SYSTEM_FOREGROUND,
                        None,
                        Some(win_event_proc),
                        0,
                        0,
                        WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                    )
                };

                if fg_hook.is_invalid() {
                    DETECTOR_CTX.with(|cell| {
                        *cell.borrow_mut() = None;
                    });
                    let _ = ready_tx.send(Err("Failed to install foreground WinEvent hook".into()));
                    return;
                }

                // SAFETY: Same out-of-context hook setup for minimize start/end events so
                // minimizing straight to Desktop (which emits no EVENT_SYSTEM_FOREGROUND) and
                // restoring from taskbar are always observed cleanly.
                let min_hook = unsafe {
                    SetWinEventHook(
                        EVENT_SYSTEM_MINIMIZESTART,
                        EVENT_SYSTEM_MINIMIZEEND,
                        None,
                        Some(win_event_proc),
                        0,
                        0,
                        WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                    )
                };

                if min_hook.is_invalid() {
                    // SAFETY: `fg_hook` was created on this thread and must be unhooked on error.
                    let _ = unsafe { UnhookWinEvent(fg_hook) };
                    DETECTOR_CTX.with(|cell| {
                        *cell.borrow_mut() = None;
                    });
                    let _ = ready_tx.send(Err("Failed to install minimize WinEvent hook".into()));
                    return;
                }

                let _ = ready_tx.send(Ok(tid));

                loop {
                    // SAFETY: `msg` is a valid writable `MSG` struct on this thread.
                    let status = unsafe { GetMessageW(&mut msg, None, 0, 0) }.0;
                    if status <= 0 {
                        break;
                    }
                    // SAFETY: `msg` was populated by `GetMessageW`.
                    unsafe {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }

                // SAFETY: Both hooks were created on this thread and are unhooked once.
                let _ = unsafe { UnhookWinEvent(min_hook) };
                let _ = unsafe { UnhookWinEvent(fg_hook) };
                DETECTOR_CTX.with(|cell| {
                    *cell.borrow_mut() = None;
                });
            })
            .map_err(|e| format!("Failed to spawn foreground detector thread: {e}"))?;

        match ready_rx.recv() {
            Ok(Ok(thread_id)) => Ok(Self {
                thread_id,
                thread: Some(handle),
            }),
            Ok(Err(err)) => {
                let _ = handle.join();
                Err(err)
            }
            Err(_) => {
                let _ = handle.join();
                Err("Foreground detector thread terminated during startup".to_string())
            }
        }
    }

    #[cfg(not(windows))]
    pub fn start(_tx: Sender<DetectorEvent>) -> Result<Self, String> {
        Ok(Self { thread: None })
    }

    pub fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        if let Some(handle) = self.thread.take() {
            #[cfg(windows)]
            {
                use windows::Win32::Foundation::{LPARAM, WPARAM};
                use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};

                // SAFETY: `self.thread_id` initialized its Win32 message queue before `start` returned.
                let _ =
                    unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
            }

            if std::thread::current().id() != handle.thread().id() {
                let _ = handle.join();
            }
        }
    }
}

impl Drop for ForegroundDetector {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    fn matches_tslgame_exe_case_insensitively() {
        assert!(is_pubg_exe_name("TslGame.exe"));
        assert!(is_pubg_exe_name("tslgame.exe"));
        assert!(is_pubg_exe_name("TSLGAME.EXE"));
        assert!(!is_pubg_exe_name("TslGame_BE.exe"));
        assert!(!is_pubg_exe_name("notepad.exe"));
        assert!(!is_pubg_exe_name(""));
    }

    #[test]
    fn pid_cache_resolves_current_process_without_openprocess() {
        let mut cache = PidNameCache::new();
        let current_pid = std::process::id();
        assert!(!cache.is_pubg_pid(current_pid));
        assert_eq!(cache.entries.get(&current_pid), Some(&false));
        // Second lookup hits populated cache cleanly.
        assert!(!cache.is_pubg_pid(current_pid));
        // PID 0 is always rejected without poisoning the cache.
        assert!(!cache.is_pubg_pid(0));
    }

    #[test]
    fn foreground_detector_starts_and_stops_cleanly() {
        let start = Instant::now();
        let (tx, _rx) = mpsc::channel();
        let detector = ForegroundDetector::start(tx).expect("start foreground detector");
        detector.stop();
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "foreground detector lifecycle exceeded 500 ms: {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn is_window_alive_handles_null_hwnd() {
        assert!(!is_window_alive(0));
        assert!(!is_window_iconic(0));
    }

    #[test]
    fn pid_cache_expires_stale_entries_after_ttl_and_handles_pid_reuse() {
        let mut cache = PidNameCache::with_ttl(Duration::from_secs(30));
        let t0 = Instant::now();
        let mut snapshot_calls = 0usize;

        // Initial miss populates cache: PID 4242 is non-PUBG, PID 7777 is PUBG.
        let is_4242 = cache.is_pubg_pid_at(4242, t0, || {
            snapshot_calls += 1;
            Some(HashMap::from([(4242, false), (7777, true)]))
        });
        assert!(!is_4242);
        assert_eq!(snapshot_calls, 1);

        // Rapid lookups within TTL hit cache without taking another snapshot.
        let is_4242_cached = cache.is_pubg_pid_at(4242, t0 + Duration::from_secs(10), || {
            snapshot_calls += 1;
            panic!("snapshot must not run on fresh cache hit");
        });
        assert!(!is_4242_cached);
        let is_7777_cached = cache.is_pubg_pid_at(7777, t0 + Duration::from_secs(29), || {
            snapshot_calls += 1;
            panic!("snapshot must not run on fresh cache hit");
        });
        assert!(is_7777_cached);
        assert_eq!(snapshot_calls, 1);

        // After TTL expires, PID reuse (4242 -> TslGame.exe, 7777 -> notepad.exe)
        // triggers a fresh snapshot instead of returning inverted stale values.
        let is_4242_reused = cache.is_pubg_pid_at(4242, t0 + Duration::from_secs(30), || {
            snapshot_calls += 1;
            Some(HashMap::from([(4242, true), (7777, false)]))
        });
        assert!(is_4242_reused);
        assert_eq!(snapshot_calls, 2);

        // Lookup of 7777 within the new TTL hits refreshed cache and sees `false`.
        let is_7777_reused = cache.is_pubg_pid_at(7777, t0 + Duration::from_secs(35), || {
            snapshot_calls += 1;
            panic!("snapshot must not run on fresh cache hit after refresh");
        });
        assert!(!is_7777_reused);
        assert_eq!(snapshot_calls, 2);

        // If snapshot fails after TTL expires, stale `true` must never be returned.
        let is_4242_after_failed_refresh =
            cache.is_pubg_pid_at(4242, t0 + Duration::from_secs(61), || {
                snapshot_calls += 1;
                None
            });
        assert!(!is_4242_after_failed_refresh);
        assert_eq!(snapshot_calls, 3);
    }
}
