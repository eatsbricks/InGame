use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::{
    load_settings_from, save_settings_to, settings_path, validate_escape_key, EscapeKey, Settings,
};
use crate::key_filter::FilterFlags;
use crate::state::{ProtectionAction, ProtectionEvent, ProtectionState, StateMachine};
use crate::win32::accessibility::{recovery_path, AccessibilityGuard};
use crate::win32::capslock::turn_off_caps_lock_if_on;
use crate::win32::detector::{
    inspect_current_foreground, is_window_alive, is_window_iconic, DetectorEvent,
    ForegroundDetector, ForegroundNotice, PidNameCache,
};
use crate::win32::hook::{HookSignal, KeyboardHookHandle};
use crate::win32::language::{is_grave_accent_os_toggle, ForceLangResult, LanguageGuard};
use crate::win32::minimizer::minimize_pubg_window;
use crate::win32::startup::{is_launch_at_startup_enabled, set_launch_at_startup};

const LANGUAGE_TICK_INTERVAL: Duration = Duration::from_millis(150);
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(50);
const WATCHDOG_EVERY_N_TICKS: u32 = 10;
const FORWARDER_STACK_SIZE: usize = 128 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSnapshot {
    pub state: ProtectionState,
    pub settings: Settings,
    pub has_english_layout: bool,
    pub elevation_warning: bool,
}

#[derive(Debug, Clone)]
pub struct CoordinatorConfig {
    pub settings_path: PathBuf,
    pub recovery_path: PathBuf,
    pub sync_startup_registry: bool,
    pub inspect_initial_foreground: bool,
    pub manage_caps_lock: bool,
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            settings_path: settings_path(),
            recovery_path: recovery_path(),
            sync_startup_registry: true,
            inspect_initial_foreground: true,
            manage_caps_lock: true,
        }
    }
}

enum CoordinatorCommand {
    UpdateSettings {
        patch: Settings,
        reply: SyncSender<Result<AppSnapshot, String>>,
    },
    TogglePause {
        reply: SyncSender<Result<AppSnapshot, String>>,
    },
    SetEscapeKey {
        key: EscapeKey,
        reply: SyncSender<Result<AppSnapshot, String>>,
    },
}

enum WorkerEvent {
    Foreground(ForegroundNotice),
    WindowMinimized(isize),
    Hook(HookSignal),
    Command(CoordinatorCommand),
    Shutdown,
}

trait PlatformBackend {
    fn is_grave_accent_os_toggle(&self) -> bool;
    fn has_english_layout(&self) -> bool;
    fn force_english_on(&mut self, hwnd: isize, prev_hwnd: Option<isize>) -> ForceLangResult;
    fn ensure_english_on(&mut self, hwnd: isize) -> ForceLangResult;
    fn restore_language_if_needed(&mut self, next_hwnd: isize);
    fn suppress_accessibility(&mut self);
    fn restore_accessibility(&mut self);
    fn turn_off_caps_lock_if_on(&mut self) -> bool;
    fn arm_hook(&mut self, flags: FilterFlags);
    fn update_hook_flags(&mut self, flags: FilterFlags);
    fn disarm_hook(&mut self);
    fn minimize_pubg_window(&mut self, pubg_hwnd: isize, fallback_hwnd: Option<isize>);
    fn current_foreground_hwnd(&self) -> isize;
    fn inspect_foreground(&mut self) -> ForegroundNotice;
    fn is_window_alive(&self, hwnd: isize) -> bool;
    fn is_window_iconic(&self, hwnd: isize) -> bool;
    fn set_launch_at_startup(&mut self, enabled: bool) -> Result<(), String>;
}

#[cfg(windows)]
fn query_foreground_hwnd() -> isize {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    // SAFETY: `GetForegroundWindow` has no preconditions and returns a handle or null.
    unsafe { GetForegroundWindow().0 as isize }
}

#[cfg(not(windows))]
fn query_foreground_hwnd() -> isize {
    0
}

struct Win32Backend {
    language: LanguageGuard,
    hook: Option<KeyboardHookHandle>,
    accessibility: AccessibilityGuard,
    pid_cache: PidNameCache,
    sync_startup_registry: bool,
    manage_caps_lock: bool,
}

impl Win32Backend {
    fn new(
        recovery_file: PathBuf,
        hook: KeyboardHookHandle,
        sync_startup_registry: bool,
        manage_caps_lock: bool,
    ) -> Self {
        Self {
            language: LanguageGuard::new(),
            hook: Some(hook),
            accessibility: AccessibilityGuard::with_recovery_path(recovery_file),
            pid_cache: PidNameCache::new(),
            sync_startup_registry,
            manage_caps_lock,
        }
    }

    fn shutdown_hook(&mut self) {
        if let Some(hook) = self.hook.take() {
            hook.shutdown();
        }
    }
}

impl PlatformBackend for Win32Backend {
    fn is_grave_accent_os_toggle(&self) -> bool {
        is_grave_accent_os_toggle()
    }

    fn has_english_layout(&self) -> bool {
        self.language.has_english_layout()
    }

    fn force_english_on(&mut self, hwnd: isize, prev_hwnd: Option<isize>) -> ForceLangResult {
        self.language.force_english_on(hwnd, prev_hwnd)
    }

    fn ensure_english_on(&mut self, hwnd: isize) -> ForceLangResult {
        self.language.ensure_english_on(hwnd)
    }

    fn restore_language_if_needed(&mut self, next_hwnd: isize) {
        self.language.restore_if_needed(next_hwnd);
    }

    fn suppress_accessibility(&mut self) {
        self.accessibility.suppress();
    }

    fn restore_accessibility(&mut self) {
        self.accessibility.restore();
    }

    fn turn_off_caps_lock_if_on(&mut self) -> bool {
        if self.manage_caps_lock {
            turn_off_caps_lock_if_on()
        } else {
            false
        }
    }

    fn arm_hook(&mut self, flags: FilterFlags) {
        if let Some(hook) = self.hook.as_ref() {
            hook.arm(flags);
        }
    }

    fn update_hook_flags(&mut self, flags: FilterFlags) {
        if let Some(hook) = self.hook.as_ref() {
            hook.update_flags(flags);
        }
    }

    fn disarm_hook(&mut self) {
        if let Some(hook) = self.hook.as_ref() {
            hook.disarm();
        }
    }

    fn minimize_pubg_window(&mut self, pubg_hwnd: isize, fallback_hwnd: Option<isize>) {
        minimize_pubg_window(pubg_hwnd, fallback_hwnd);
    }

    fn current_foreground_hwnd(&self) -> isize {
        query_foreground_hwnd()
    }

    fn inspect_foreground(&mut self) -> ForegroundNotice {
        inspect_current_foreground(&mut self.pid_cache)
    }

    fn is_window_alive(&self, hwnd: isize) -> bool {
        is_window_alive(hwnd)
    }

    fn is_window_iconic(&self, hwnd: isize) -> bool {
        is_window_iconic(hwnd)
    }

    fn set_launch_at_startup(&mut self, enabled: bool) -> Result<(), String> {
        if self.sync_startup_registry {
            set_launch_at_startup(enabled)
        } else {
            Ok(())
        }
    }
}

struct CoordinatorCore<B: PlatformBackend> {
    settings_path: PathBuf,
    settings: Settings,
    inspect_foreground_on_enable: bool,
    state_machine: StateMachine,
    last_non_pubg_hwnd: Option<isize>,
    elevation_warning: bool,
    backend: B,
}

impl<B: PlatformBackend> CoordinatorCore<B> {
    fn new(
        settings_path: PathBuf,
        settings: Settings,
        inspect_foreground_on_enable: bool,
        backend: B,
    ) -> Self {
        let state_machine = StateMachine::new(settings.enabled);
        Self {
            settings_path,
            settings,
            inspect_foreground_on_enable,
            state_machine,
            last_non_pubg_hwnd: None,
            elevation_warning: false,
            backend,
        }
    }

    fn snapshot(&self) -> AppSnapshot {
        AppSnapshot {
            state: self.state_machine.state(),
            settings: self.settings.clone(),
            has_english_layout: self.backend.has_english_layout(),
            elevation_warning: self.elevation_warning,
        }
    }

    fn state(&self) -> ProtectionState {
        self.state_machine.state()
    }

    fn has_pending_exit(&self) -> bool {
        self.state_machine.exiting_hwnd().is_some()
    }

    fn inspect_and_apply_current_foreground(&mut self) {
        let notice = self.backend.inspect_foreground();
        if !notice.is_pubg && notice.hwnd != 0 {
            self.last_non_pubg_hwnd = Some(notice.hwnd);
        }
        if self.settings.enabled && notice.is_pubg && notice.hwnd != 0 {
            self.handle_foreground(notice);
        }
    }

    fn build_filter_flags(&self) -> FilterFlags {
        let block_grave_accent = self.backend.is_grave_accent_os_toggle();
        FilterFlags::from_escape_key(
            self.settings.lock_language,
            self.settings.block_minimize,
            self.settings.block_caps_lock,
            block_grave_accent,
            &self.settings.escape_key,
        )
    }

    fn resolve_restore_target_hwnd(&self, pubg_hwnd: isize) -> isize {
        let fg = self.backend.current_foreground_hwnd();
        if fg != 0 && fg != pubg_hwnd {
            return fg;
        }
        if let Some(prev) = self.last_non_pubg_hwnd {
            if prev != 0 && prev != pubg_hwnd && self.backend.is_window_alive(prev) {
                return prev;
            }
        }
        0
    }

    fn execute_actions(&mut self, actions: Vec<ProtectionAction>, disarm_restore_hwnd: isize) {
        for action in actions {
            match action {
                ProtectionAction::Arm { pubg_hwnd } => {
                    let flags = self.build_filter_flags();
                    if self.settings.block_minimize {
                        self.backend.suppress_accessibility();
                    }
                    if self.settings.lock_language {
                        let res = self
                            .backend
                            .force_english_on(pubg_hwnd, self.last_non_pubg_hwnd);
                        self.elevation_warning =
                            matches!(res, ForceLangResult::AccessDeniedElevation);
                    } else {
                        self.elevation_warning = false;
                    }
                    if self.settings.block_caps_lock {
                        let _ = self.backend.turn_off_caps_lock_if_on();
                    }
                    self.backend.arm_hook(flags);
                }
                ProtectionAction::Disarm => {
                    self.backend.disarm_hook();
                    self.backend.restore_accessibility();
                    self.backend.restore_language_if_needed(disarm_restore_hwnd);
                }
                ProtectionAction::ExecuteEscapeExit { pubg_hwnd } => {
                    self.backend.disarm_hook();
                    self.backend.restore_accessibility();
                    self.backend
                        .minimize_pubg_window(pubg_hwnd, self.last_non_pubg_hwnd);
                    let target_hwnd = self.resolve_restore_target_hwnd(pubg_hwnd);
                    self.backend.restore_language_if_needed(target_hwnd);
                }
            }
        }
    }

    fn handle_foreground(&mut self, notice: ForegroundNotice) {
        if !notice.is_pubg && notice.hwnd != 0 {
            self.last_non_pubg_hwnd = Some(notice.hwnd);
        }

        let actions = self
            .state_machine
            .handle(ProtectionEvent::ForegroundChanged {
                hwnd: notice.hwnd,
                is_pubg: notice.is_pubg,
            });
        let restore_hwnd = if !notice.is_pubg { notice.hwnd } else { 0 };
        self.execute_actions(actions, restore_hwnd);
    }

    fn handle_window_minimized(&mut self, hwnd: isize) {
        let actions = self
            .state_machine
            .handle(ProtectionEvent::WindowMinimized { hwnd });
        if !actions.is_empty() {
            let restore_hwnd = self.resolve_restore_target_hwnd(hwnd);
            self.execute_actions(actions, restore_hwnd);
        }
    }

    fn handle_exit_poll_tick(&mut self) {
        let Some(exiting) = self.state_machine.exiting_hwnd() else {
            return;
        };

        if self.backend.is_window_iconic(exiting) {
            self.handle_window_minimized(exiting);
            return;
        }

        let current_fg = self.backend.current_foreground_hwnd();
        let pubg_window_alive = self.backend.is_window_alive(exiting);
        let actions = self.state_machine.handle(ProtectionEvent::WatchdogTick {
            current_fg,
            pubg_window_alive,
        });
        if !actions.is_empty() {
            let restore_hwnd = if current_fg != 0 && current_fg != exiting {
                current_fg
            } else {
                0
            };
            self.execute_actions(actions, restore_hwnd);
        }
    }

    fn check_protected_language(&mut self) {
        if self.state_machine.state() != ProtectionState::Protected || !self.settings.lock_language
        {
            return;
        }
        if let Some(pubg_hwnd) = self.state_machine.active_pubg_hwnd() {
            if self.backend.ensure_english_on(pubg_hwnd) == ForceLangResult::AccessDeniedElevation {
                self.elevation_warning = true;
            }
        }
    }

    fn handle_hook_signal(&mut self, signal: HookSignal) {
        match signal {
            HookSignal::EscapeTriggered => {
                let actions = self.state_machine.handle(ProtectionEvent::EscapeTriggered);
                self.execute_actions(actions, 0);
            }
            HookSignal::CheckLanguage => {
                self.check_protected_language();
            }
        }
    }

    fn handle_protected_tick(&mut self, run_watchdog: bool) {
        if self.state_machine.state() != ProtectionState::Protected {
            return;
        }

        if run_watchdog {
            if let Some(pubg_hwnd) = self.state_machine.active_pubg_hwnd() {
                let current_fg = self.backend.current_foreground_hwnd();
                let pubg_window_alive = self.backend.is_window_alive(pubg_hwnd);
                let actions = self.state_machine.handle(ProtectionEvent::WatchdogTick {
                    current_fg,
                    pubg_window_alive,
                });
                let next_hwnd = if current_fg != 0 && current_fg != pubg_hwnd {
                    current_fg
                } else {
                    0
                };
                if !actions.is_empty() && next_hwnd != 0 {
                    self.last_non_pubg_hwnd = Some(next_hwnd);
                }
                self.execute_actions(actions, next_hwnd);
            }
        }

        if self.state_machine.state() == ProtectionState::Protected {
            self.check_protected_language();
        }
    }

    fn apply_settings_update(&mut self, patch: Settings) -> Result<AppSnapshot, String> {
        validate_escape_key(&patch.escape_key)?;

        let prev = self.settings.clone();
        let startup_changed = patch.launch_at_startup != prev.launch_at_startup;
        if startup_changed {
            self.backend
                .set_launch_at_startup(patch.launch_at_startup)?;
        }

        if let Err(err) = save_settings_to(&self.settings_path, &patch) {
            if startup_changed {
                let _ = self.backend.set_launch_at_startup(prev.launch_at_startup);
            }
            return Err(err);
        }

        let was_protected = self.state_machine.state() == ProtectionState::Protected;
        self.settings = patch;

        if self.settings.enabled != prev.enabled {
            let actions = self
                .state_machine
                .handle(ProtectionEvent::SetEnabled(self.settings.enabled));
            let restore_hwnd = if actions.contains(&ProtectionAction::Disarm) {
                self.backend.current_foreground_hwnd()
            } else {
                0
            };
            self.execute_actions(actions, restore_hwnd);

            if self.settings.enabled && self.inspect_foreground_on_enable {
                self.inspect_and_apply_current_foreground();
            }
        }

        if was_protected && self.state_machine.state() == ProtectionState::Protected {
            let flags = self.build_filter_flags();
            self.backend.update_hook_flags(flags);

            if self.settings.block_minimize != prev.block_minimize {
                if self.settings.block_minimize {
                    self.backend.suppress_accessibility();
                } else {
                    self.backend.restore_accessibility();
                }
            }

            if self.settings.lock_language && !prev.lock_language {
                if let Some(pubg_hwnd) = self.state_machine.active_pubg_hwnd() {
                    let res = self
                        .backend
                        .force_english_on(pubg_hwnd, self.last_non_pubg_hwnd);
                    self.elevation_warning = matches!(res, ForceLangResult::AccessDeniedElevation);
                }
            } else if !self.settings.lock_language && prev.lock_language {
                self.elevation_warning = false;
            }

            if self.settings.block_caps_lock && !prev.block_caps_lock {
                let _ = self.backend.turn_off_caps_lock_if_on();
            }
        } else if !self.settings.lock_language {
            self.elevation_warning = false;
        }

        Ok(self.snapshot())
    }

    fn toggle_pause(&mut self) -> Result<AppSnapshot, String> {
        let mut next = self.settings.clone();
        next.enabled = !next.enabled;
        self.apply_settings_update(next)
    }

    fn set_escape_key(&mut self, key: EscapeKey) -> Result<AppSnapshot, String> {
        let mut next = self.settings.clone();
        next.escape_key = key;
        self.apply_settings_update(next)
    }

    fn shutdown_cleanup(&mut self) {
        let restore_hwnd = if self.state_machine.state() == ProtectionState::Protected {
            self.backend.current_foreground_hwnd()
        } else {
            0
        };
        self.backend.disarm_hook();
        self.backend.restore_accessibility();
        self.backend.restore_language_if_needed(restore_hwnd);
    }
}

struct CoordinatorRuntime {
    event_tx: Option<Sender<WorkerEvent>>,
    worker: Option<JoinHandle<()>>,
}

pub struct ProtectionCoordinator {
    runtime: Mutex<CoordinatorRuntime>,
    shared_snapshot: Arc<Mutex<AppSnapshot>>,
}

impl ProtectionCoordinator {
    pub fn start<F>(on_change: F) -> Result<Self, String>
    where
        F: FnMut(AppSnapshot) + Send + 'static,
    {
        Self::start_with_config(CoordinatorConfig::default(), on_change)
    }

    pub fn start_with_config<F>(config: CoordinatorConfig, mut on_change: F) -> Result<Self, String>
    where
        F: FnMut(AppSnapshot) + Send + 'static,
    {
        let (event_tx, event_rx) = mpsc::channel::<WorkerEvent>();
        let worker_event_tx = event_tx.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<Arc<Mutex<AppSnapshot>>, String>>(1);

        let worker = thread::Builder::new()
            .name("ingame-coordinator".to_string())
            .spawn(move || {
                // Recover any leftover crash state before touching hooks or accessibility settings.
                AccessibilityGuard::recover_from_crash_file(&config.recovery_path);

                let mut settings = load_settings_from(&config.settings_path);
                if config.sync_startup_registry {
                    settings.launch_at_startup = is_launch_at_startup_enabled();
                }

                let (fg_tx, fg_rx) = mpsc::channel::<DetectorEvent>();
                let detector = match ForegroundDetector::start(fg_tx) {
                    Ok(detector) => detector,
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                        return;
                    }
                };

                let (hook_tx, hook_rx) = mpsc::channel::<HookSignal>();
                let hook = match KeyboardHookHandle::spawn(hook_tx) {
                    Ok(hook) => hook,
                    Err(err) => {
                        detector.stop();
                        let _ = ready_tx.send(Err(err));
                        return;
                    }
                };

                let fg_forward_tx = worker_event_tx.clone();
                let fg_fwd = match thread::Builder::new()
                    .name("ingame-fg-fwd".to_string())
                    .stack_size(FORWARDER_STACK_SIZE)
                    .spawn(move || {
                        while let Ok(ev) = fg_rx.recv() {
                            let worker_ev = match ev {
                                DetectorEvent::Foreground(notice) => {
                                    WorkerEvent::Foreground(notice)
                                }
                                DetectorEvent::WindowMinimized(hwnd) => {
                                    WorkerEvent::WindowMinimized(hwnd)
                                }
                            };
                            if fg_forward_tx.send(worker_ev).is_err() {
                                break;
                            }
                        }
                    }) {
                    Ok(handle) => handle,
                    Err(err) => {
                        detector.stop();
                        hook.shutdown();
                        let _ = ready_tx.send(Err(format!(
                            "Failed to spawn foreground forwarder thread: {err}"
                        )));
                        return;
                    }
                };

                let hook_forward_tx = worker_event_tx;
                let hook_fwd = match thread::Builder::new()
                    .name("ingame-hook-fwd".to_string())
                    .stack_size(FORWARDER_STACK_SIZE)
                    .spawn(move || {
                        while let Ok(signal) = hook_rx.recv() {
                            if hook_forward_tx.send(WorkerEvent::Hook(signal)).is_err() {
                                break;
                            }
                        }
                    }) {
                    Ok(handle) => handle,
                    Err(err) => {
                        detector.stop();
                        hook.shutdown();
                        let _ = fg_fwd.join();
                        let _ = ready_tx
                            .send(Err(format!("Failed to spawn hook forwarder thread: {err}")));
                        return;
                    }
                };

                let backend = Win32Backend::new(
                    config.recovery_path.clone(),
                    hook,
                    config.sync_startup_registry,
                    config.manage_caps_lock,
                );
                let mut core = CoordinatorCore::new(
                    config.settings_path.clone(),
                    settings,
                    config.inspect_initial_foreground,
                    backend,
                );

                if config.inspect_initial_foreground {
                    core.inspect_and_apply_current_foreground();
                }

                let initial_snapshot = core.snapshot();
                let shared_snapshot = Arc::new(Mutex::new(initial_snapshot.clone()));
                let mut last_notified = initial_snapshot;

                if ready_tx.send(Ok(Arc::clone(&shared_snapshot))).is_err() {
                    core.shutdown_cleanup();
                    detector.stop();
                    core.backend.shutdown_hook();
                    let _ = fg_fwd.join();
                    let _ = hook_fwd.join();
                    return;
                }

                let mut next_tick_at: Option<Instant> = None;
                let mut tick_count: u32 = 0;

                loop {
                    let event = if core.state() == ProtectionState::Protected {
                        let deadline = *next_tick_at
                            .get_or_insert_with(|| Instant::now() + LANGUAGE_TICK_INTERVAL);
                        let wait = deadline.saturating_duration_since(Instant::now());

                        match event_rx.recv_timeout(wait) {
                            Ok(event) => Some(event),
                            Err(RecvTimeoutError::Timeout) => {
                                tick_count += 1;
                                let run_watchdog = tick_count >= WATCHDOG_EVERY_N_TICKS;
                                if run_watchdog {
                                    tick_count = 0;
                                }
                                core.handle_protected_tick(run_watchdog);
                                publish_if_changed(
                                    &core,
                                    &shared_snapshot,
                                    &mut last_notified,
                                    &mut on_change,
                                );
                                next_tick_at = if core.state() == ProtectionState::Protected {
                                    Some(Instant::now() + LANGUAGE_TICK_INTERVAL)
                                } else {
                                    tick_count = 0;
                                    None
                                };
                                None
                            }
                            Err(RecvTimeoutError::Disconnected) => break,
                        }
                    } else if core.has_pending_exit() {
                        next_tick_at = None;
                        tick_count = 0;
                        match event_rx.recv_timeout(EXIT_POLL_INTERVAL) {
                            Ok(event) => Some(event),
                            Err(RecvTimeoutError::Timeout) => {
                                core.handle_exit_poll_tick();
                                publish_if_changed(
                                    &core,
                                    &shared_snapshot,
                                    &mut last_notified,
                                    &mut on_change,
                                );
                                None
                            }
                            Err(RecvTimeoutError::Disconnected) => break,
                        }
                    } else {
                        next_tick_at = None;
                        tick_count = 0;
                        match event_rx.recv() {
                            Ok(event) => Some(event),
                            Err(_) => break,
                        }
                    };

                    let Some(event) = event else {
                        continue;
                    };

                    match event {
                        WorkerEvent::Foreground(notice) => {
                            core.handle_foreground(notice);
                            publish_if_changed(
                                &core,
                                &shared_snapshot,
                                &mut last_notified,
                                &mut on_change,
                            );
                        }
                        WorkerEvent::WindowMinimized(hwnd) => {
                            core.handle_window_minimized(hwnd);
                            publish_if_changed(
                                &core,
                                &shared_snapshot,
                                &mut last_notified,
                                &mut on_change,
                            );
                        }
                        WorkerEvent::Hook(signal) => {
                            core.handle_hook_signal(signal);
                            publish_if_changed(
                                &core,
                                &shared_snapshot,
                                &mut last_notified,
                                &mut on_change,
                            );
                        }
                        WorkerEvent::Command(cmd) => match cmd {
                            CoordinatorCommand::UpdateSettings { patch, reply } => {
                                let res = core.apply_settings_update(patch);
                                if res.is_ok() {
                                    publish_if_changed(
                                        &core,
                                        &shared_snapshot,
                                        &mut last_notified,
                                        &mut on_change,
                                    );
                                }
                                let _ = reply.send(res);
                            }
                            CoordinatorCommand::TogglePause { reply } => {
                                let res = core.toggle_pause();
                                if res.is_ok() {
                                    publish_if_changed(
                                        &core,
                                        &shared_snapshot,
                                        &mut last_notified,
                                        &mut on_change,
                                    );
                                }
                                let _ = reply.send(res);
                            }
                            CoordinatorCommand::SetEscapeKey { key, reply } => {
                                let res = core.set_escape_key(key);
                                if res.is_ok() {
                                    publish_if_changed(
                                        &core,
                                        &shared_snapshot,
                                        &mut last_notified,
                                        &mut on_change,
                                    );
                                }
                                let _ = reply.send(res);
                            }
                        },
                        WorkerEvent::Shutdown => break,
                    }

                    // Ensure continuous events (e.g. rapid modifier key-ups) never starve ticks/watchdog.
                    if core.state() == ProtectionState::Protected {
                        if let Some(deadline) = next_tick_at {
                            if Instant::now() >= deadline {
                                tick_count += 1;
                                let run_watchdog = tick_count >= WATCHDOG_EVERY_N_TICKS;
                                if run_watchdog {
                                    tick_count = 0;
                                }
                                core.handle_protected_tick(run_watchdog);
                                publish_if_changed(
                                    &core,
                                    &shared_snapshot,
                                    &mut last_notified,
                                    &mut on_change,
                                );
                                next_tick_at = if core.state() == ProtectionState::Protected {
                                    Some(Instant::now() + LANGUAGE_TICK_INTERVAL)
                                } else {
                                    tick_count = 0;
                                    None
                                };
                            }
                        } else {
                            next_tick_at = Some(Instant::now() + LANGUAGE_TICK_INTERVAL);
                            tick_count = 0;
                        }
                    } else {
                        next_tick_at = None;
                        tick_count = 0;
                    }
                }

                // Clean shutdown: disarm hook, restore accessibility/language, stop detector & hook,
                // then join forwarder threads (whose senders were just dropped).
                core.shutdown_cleanup();
                drop(event_rx);
                detector.stop();
                core.backend.shutdown_hook();
                let _ = fg_fwd.join();
                let _ = hook_fwd.join();
            })
            .map_err(|e| format!("Failed to spawn coordinator worker thread: {e}"))?;

        match ready_rx.recv() {
            Ok(Ok(shared_snapshot)) => Ok(Self {
                runtime: Mutex::new(CoordinatorRuntime {
                    event_tx: Some(event_tx),
                    worker: Some(worker),
                }),
                shared_snapshot,
            }),
            Ok(Err(err)) => {
                let _ = worker.join();
                Err(err)
            }
            Err(_) => {
                let _ = worker.join();
                Err("Coordinator worker thread terminated during startup".to_string())
            }
        }
    }

    pub fn snapshot(&self) -> AppSnapshot {
        self.shared_snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn update_settings(&self, patch: Settings) -> Result<AppSnapshot, String> {
        let (reply, rx) = mpsc::sync_channel(1);
        self.send_command(CoordinatorCommand::UpdateSettings { patch, reply })?;
        rx.recv()
            .map_err(|_| "Protection coordinator shut down before replying".to_string())?
    }

    pub fn toggle_pause(&self) -> Result<AppSnapshot, String> {
        let (reply, rx) = mpsc::sync_channel(1);
        self.send_command(CoordinatorCommand::TogglePause { reply })?;
        rx.recv()
            .map_err(|_| "Protection coordinator shut down before replying".to_string())?
    }

    pub fn set_escape_key(&self, key: EscapeKey) -> Result<AppSnapshot, String> {
        let (reply, rx) = mpsc::sync_channel(1);
        self.send_command(CoordinatorCommand::SetEscapeKey { key, reply })?;
        rx.recv()
            .map_err(|_| "Protection coordinator shut down before replying".to_string())?
    }

    pub fn notify_non_pubg_foreground(&self, hwnd: isize) {
        let tx = {
            let Ok(runtime) = self.runtime.lock() else {
                return;
            };
            runtime.event_tx.clone()
        };
        if let Some(tx) = tx {
            let _ = tx.send(WorkerEvent::Foreground(ForegroundNotice {
                hwnd,
                is_pubg: false,
            }));
        }
    }

    pub fn shutdown(&self) {
        let worker = {
            let Ok(mut runtime) = self.runtime.lock() else {
                return;
            };
            if let Some(tx) = runtime.event_tx.take() {
                let _ = tx.send(WorkerEvent::Shutdown);
            }
            runtime.worker.take()
        };

        if let Some(handle) = worker {
            if thread::current().id() != handle.thread().id() {
                let _ = handle.join();
            }
        }
    }

    fn send_command(&self, cmd: CoordinatorCommand) -> Result<(), String> {
        let tx = {
            let runtime = self
                .runtime
                .lock()
                .map_err(|_| "Coordinator runtime lock poisoned".to_string())?;
            runtime
                .event_tx
                .clone()
                .ok_or_else(|| "Protection coordinator is not running".to_string())?
        };
        tx.send(WorkerEvent::Command(cmd))
            .map_err(|_| "Protection coordinator is not running".to_string())
    }
}

impl Drop for ProtectionCoordinator {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn publish_if_changed<F>(
    core: &CoordinatorCore<Win32Backend>,
    shared_snapshot: &Arc<Mutex<AppSnapshot>>,
    last_notified: &mut AppSnapshot,
    on_change: &mut F,
) where
    F: FnMut(AppSnapshot),
{
    let current = core.snapshot();
    if current != *last_notified {
        *last_notified = current.clone();
        if let Ok(mut guard) = shared_snapshot.lock() {
            *guard = current.clone();
        }
        on_change(current);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_DIR_ID: AtomicU64 = AtomicU64::new(1);

    struct TempDirGuard {
        path: PathBuf,
    }

    impl TempDirGuard {
        fn new(name: &str) -> Self {
            let id = NEXT_TEST_DIR_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "pubg_helper_coordinator_test_{}_{}_{}",
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

    #[derive(Debug, PartialEq, Eq)]
    enum MockCall {
        SuppressAccessibility,
        RestoreAccessibility,
        ForceEnglish {
            hwnd: isize,
            prev_hwnd: Option<isize>,
        },
        EnsureEnglish(isize),
        RestoreLanguage(isize),
        TurnOffCapsLock,
        ArmHook(FilterFlags),
        UpdateHookFlags(FilterFlags),
        DisarmHook,
        MinimizePubg {
            pubg_hwnd: isize,
            fallback_hwnd: Option<isize>,
        },
        SetStartup(bool),
    }

    struct MockBackend {
        grave_toggle: bool,
        has_english: bool,
        force_result: ForceLangResult,
        ensure_result: ForceLangResult,
        caps_lock_turned_off: bool,
        current_fg: isize,
        fg_after_minimize: Option<isize>,
        alive_windows: Vec<isize>,
        iconic_windows: Vec<isize>,
        startup_fail: bool,
        calls: Vec<MockCall>,
    }

    impl Default for MockBackend {
        fn default() -> Self {
            Self {
                grave_toggle: true,
                has_english: true,
                force_result: ForceLangResult::Applied,
                ensure_result: ForceLangResult::Applied,
                caps_lock_turned_off: true,
                current_fg: 0,
                fg_after_minimize: None,
                alive_windows: vec![100, 200],
                iconic_windows: Vec::new(),
                startup_fail: false,
                calls: Vec::new(),
            }
        }
    }

    impl PlatformBackend for MockBackend {
        fn is_grave_accent_os_toggle(&self) -> bool {
            self.grave_toggle
        }

        fn has_english_layout(&self) -> bool {
            self.has_english
        }

        fn force_english_on(&mut self, hwnd: isize, prev_hwnd: Option<isize>) -> ForceLangResult {
            self.calls.push(MockCall::ForceEnglish { hwnd, prev_hwnd });
            self.force_result
        }

        fn ensure_english_on(&mut self, hwnd: isize) -> ForceLangResult {
            self.calls.push(MockCall::EnsureEnglish(hwnd));
            self.ensure_result
        }

        fn restore_language_if_needed(&mut self, next_hwnd: isize) {
            self.calls.push(MockCall::RestoreLanguage(next_hwnd));
        }

        fn suppress_accessibility(&mut self) {
            self.calls.push(MockCall::SuppressAccessibility);
        }

        fn restore_accessibility(&mut self) {
            self.calls.push(MockCall::RestoreAccessibility);
        }

        fn turn_off_caps_lock_if_on(&mut self) -> bool {
            self.calls.push(MockCall::TurnOffCapsLock);
            self.caps_lock_turned_off
        }

        fn arm_hook(&mut self, flags: FilterFlags) {
            self.calls.push(MockCall::ArmHook(flags));
        }

        fn update_hook_flags(&mut self, flags: FilterFlags) {
            self.calls.push(MockCall::UpdateHookFlags(flags));
        }

        fn disarm_hook(&mut self) {
            self.calls.push(MockCall::DisarmHook);
        }

        fn minimize_pubg_window(&mut self, pubg_hwnd: isize, fallback_hwnd: Option<isize>) {
            self.calls.push(MockCall::MinimizePubg {
                pubg_hwnd,
                fallback_hwnd,
            });
            if let Some(next_fg) = self.fg_after_minimize {
                self.current_fg = next_fg;
            }
        }

        fn current_foreground_hwnd(&self) -> isize {
            self.current_fg
        }

        fn inspect_foreground(&mut self) -> ForegroundNotice {
            ForegroundNotice {
                hwnd: self.current_fg,
                is_pubg: self.current_fg == 100,
            }
        }

        fn is_window_alive(&self, hwnd: isize) -> bool {
            self.alive_windows.contains(&hwnd)
        }

        fn is_window_iconic(&self, hwnd: isize) -> bool {
            self.iconic_windows.contains(&hwnd)
        }

        fn set_launch_at_startup(&mut self, enabled: bool) -> Result<(), String> {
            self.calls.push(MockCall::SetStartup(enabled));
            if self.startup_fail {
                Err("registry error".into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn coordinator_core_enforces_arm_escape_exit_order_live_updates_and_watchdog() {
        let dir = TempDirGuard::new("core_logic");
        let settings_path = dir.path.join("settings.json");

        let backend = MockBackend {
            grave_toggle: true,
            has_english: true,
            force_result: ForceLangResult::AccessDeniedElevation,
            ensure_result: ForceLangResult::Applied,
            caps_lock_turned_off: true,
            current_fg: 200,
            fg_after_minimize: Some(200),
            alive_windows: vec![100, 200],
            iconic_windows: Vec::new(),
            startup_fail: false,
            calls: Vec::new(),
        };

        let mut core = CoordinatorCore::new(settings_path, Settings::default(), true, backend);

        // Initial non-PUBG foreground is recorded as last_non_pubg_hwnd without arming
        core.inspect_and_apply_current_foreground();
        assert_eq!(core.state(), ProtectionState::Standby);
        assert_eq!(core.last_non_pubg_hwnd, Some(200));
        assert!(core.backend.calls.is_empty());

        // Switching to PUBG arms accessibility, language (with elevation warning), caps lock off, and hook
        core.backend.current_fg = 100;
        core.handle_foreground(ForegroundNotice {
            hwnd: 100,
            is_pubg: true,
        });
        assert_eq!(core.state(), ProtectionState::Protected);
        assert!(core.snapshot().elevation_warning);

        let expected_flags =
            FilterFlags::from_escape_key(true, true, true, true, &Settings::default().escape_key);
        assert_eq!(
            core.backend.calls,
            vec![
                MockCall::SuppressAccessibility,
                MockCall::ForceEnglish {
                    hwnd: 100,
                    prev_hwnd: Some(200),
                },
                MockCall::TurnOffCapsLock,
                MockCall::ArmHook(expected_flags),
            ]
        );
        core.backend.calls.clear();

        // CheckLanguage signal while Protected calls ensure_english_on
        core.handle_hook_signal(HookSignal::CheckLanguage);
        assert_eq!(core.backend.calls, vec![MockCall::EnsureEnglish(100)]);
        core.backend.calls.clear();

        // Live settings update while Protected updates hook flags, restores accessibility,
        // updates startup, and clears elevation_warning when lock_language turns OFF.
        let mut live_patch = core.snapshot().settings;
        live_patch.block_minimize = false;
        live_patch.lock_language = false;
        live_patch.launch_at_startup = true;
        live_patch.escape_key = EscapeKey {
            vk: 0x78,
            modifiers: 0,
            label: "F9".into(),
        };
        let live_snap = core
            .apply_settings_update(live_patch.clone())
            .expect("live settings update");
        assert_eq!(live_snap.state, ProtectionState::Protected);
        assert!(!live_snap.elevation_warning);
        let expected_live_flags =
            FilterFlags::from_escape_key(false, false, true, true, &live_patch.escape_key);
        assert_eq!(
            core.backend.calls,
            vec![
                MockCall::SetStartup(true),
                MockCall::UpdateHookFlags(expected_live_flags),
                MockCall::RestoreAccessibility,
            ]
        );
        core.backend.calls.clear();

        // Turning lock_language + block_minimize back ON while Protected suppresses & forces EN
        core.backend.force_result = ForceLangResult::Applied;
        let mut reenable_patch = live_patch;
        reenable_patch.block_minimize = true;
        reenable_patch.lock_language = true;
        let reenable_snap = core
            .apply_settings_update(reenable_patch.clone())
            .expect("reenable live protections");
        assert!(!reenable_snap.elevation_warning);
        let expected_reenable_flags =
            FilterFlags::from_escape_key(true, true, true, true, &reenable_patch.escape_key);
        assert_eq!(
            core.backend.calls,
            vec![
                MockCall::UpdateHookFlags(expected_reenable_flags),
                MockCall::SuppressAccessibility,
                MockCall::ForceEnglish {
                    hwnd: 100,
                    prev_hwnd: Some(200),
                },
            ]
        );
        core.backend.calls.clear();

        // EscapeTriggered executes strictly in order:
        // hook.disarm -> accessibility.restore -> minimize_pubg_window -> language.restore_if_needed
        core.handle_hook_signal(HookSignal::EscapeTriggered);
        assert_eq!(core.state(), ProtectionState::Standby);
        assert_eq!(
            core.backend.calls,
            vec![
                MockCall::DisarmHook,
                MockCall::RestoreAccessibility,
                MockCall::MinimizePubg {
                    pubg_hwnd: 100,
                    fallback_hwnd: Some(200),
                },
                MockCall::RestoreLanguage(200),
            ]
        );
        core.backend.calls.clear();

        // Focus leaves PUBG (200) -> clears exiting_hwnd -> returning to PUBG re-arms
        core.handle_foreground(ForegroundNotice {
            hwnd: 200,
            is_pubg: false,
        });
        core.backend.current_fg = 100;
        core.handle_foreground(ForegroundNotice {
            hwnd: 100,
            is_pubg: true,
        });
        assert_eq!(core.state(), ProtectionState::Protected);
        core.backend.calls.clear();

        // Healthy watchdog tick verifies liveness first and then checks English layout
        core.handle_protected_tick(true);
        assert_eq!(core.state(), ProtectionState::Protected);
        assert_eq!(core.backend.calls, vec![MockCall::EnsureEnglish(100)]);
        core.backend.calls.clear();

        // Watchdog tick on silent focus loss disarms BEFORE language check (never posts EN to background PUBG)
        core.backend.current_fg = 200;
        core.handle_protected_tick(true);
        assert_eq!(core.state(), ProtectionState::Standby);
        assert_eq!(
            core.backend.calls,
            vec![
                MockCall::DisarmHook,
                MockCall::RestoreAccessibility,
                MockCall::RestoreLanguage(200),
            ]
        );
        core.backend.calls.clear();

        // Re-arm and verify watchdog tick on dead PUBG window disarms without EnsureEnglish
        core.backend.current_fg = 100;
        core.handle_foreground(ForegroundNotice {
            hwnd: 100,
            is_pubg: true,
        });
        assert_eq!(core.state(), ProtectionState::Protected);
        core.backend.calls.clear();

        core.backend.alive_windows = vec![200];
        core.handle_protected_tick(true);
        assert_eq!(core.state(), ProtectionState::Standby);
        assert_eq!(
            core.backend.calls,
            vec![
                MockCall::DisarmHook,
                MockCall::RestoreAccessibility,
                MockCall::RestoreLanguage(0),
            ]
        );
    }

    #[test]
    fn coordinator_core_arms_immediately_when_pubg_already_foreground_at_startup() {
        let dir = TempDirGuard::new("initial_fg");
        let settings_path = dir.path.join("settings.json");

        let backend = MockBackend {
            current_fg: 100,
            ..MockBackend::default()
        };

        let mut core = CoordinatorCore::new(settings_path, Settings::default(), true, backend);
        core.inspect_and_apply_current_foreground();
        assert_eq!(core.state(), ProtectionState::Protected);
        assert_eq!(core.backend.calls.len(), 4);
    }

    #[test]
    fn coordinator_core_preserves_elevation_warning_across_disarm_so_settings_ui_can_display_it() {
        let dir = TempDirGuard::new("elevation_warning");
        let settings_path = dir.path.join("settings.json");

        let backend = MockBackend {
            force_result: ForceLangResult::AccessDeniedElevation,
            current_fg: 100,
            ..MockBackend::default()
        };

        let mut core = CoordinatorCore::new(settings_path, Settings::default(), false, backend);
        core.handle_foreground(ForegroundNotice {
            hwnd: 100,
            is_pubg: true,
        });
        assert!(core.snapshot().elevation_warning);

        // User switches to Settings window (Disarm -> Standby): warning remains visible
        core.handle_foreground(ForegroundNotice {
            hwnd: 200,
            is_pubg: false,
        });
        assert_eq!(core.state(), ProtectionState::Standby);
        assert!(core.snapshot().elevation_warning);

        // Next successful Arm clears the warning
        core.backend.force_result = ForceLangResult::Applied;
        core.handle_foreground(ForegroundNotice {
            hwnd: 100,
            is_pubg: true,
        });
        assert!(!core.snapshot().elevation_warning);

        // Non-access-denied PostMessageW failure does not spuriously raise elevation_warning
        core.handle_foreground(ForegroundNotice {
            hwnd: 200,
            is_pubg: false,
        });
        core.backend.force_result = ForceLangResult::Failed;
        core.handle_foreground(ForegroundNotice {
            hwnd: 100,
            is_pubg: true,
        });
        assert!(!core.snapshot().elevation_warning);
    }

    #[test]
    fn coordinator_core_handles_startup_error_pause_while_protected_and_pubg_hwnd_switch() {
        let dir = TempDirGuard::new("edge_cases");
        let settings_path = dir.path.join("settings.json");

        let backend = MockBackend {
            current_fg: 100,
            startup_fail: true,
            ..MockBackend::default()
        };

        let mut core =
            CoordinatorCore::new(settings_path.clone(), Settings::default(), false, backend);

        // Startup registry failure aborts settings update before writing disk or mutating state
        let bad_startup = Settings {
            launch_at_startup: true,
            ..Settings::default()
        };
        assert!(core.apply_settings_update(bad_startup).is_err());
        assert!(!core.snapshot().settings.launch_at_startup);
        assert!(!settings_path.exists());
        assert_eq!(core.backend.calls, vec![MockCall::SetStartup(true)]);
        core.backend.calls.clear();

        // Arm on PUBG hwnd 100, then switch directly to another PUBG hwnd 101:
        // Disarm passes 0 (no Thai restore onto PUBG) before re-arming 101.
        core.backend.startup_fail = false;
        core.handle_foreground(ForegroundNotice {
            hwnd: 100,
            is_pubg: true,
        });
        core.backend.calls.clear();

        core.handle_foreground(ForegroundNotice {
            hwnd: 101,
            is_pubg: true,
        });
        assert_eq!(core.state(), ProtectionState::Protected);
        let expected_flags =
            FilterFlags::from_escape_key(true, true, true, true, &Settings::default().escape_key);
        assert_eq!(
            core.backend.calls,
            vec![
                MockCall::DisarmHook,
                MockCall::RestoreAccessibility,
                MockCall::RestoreLanguage(0),
                MockCall::SuppressAccessibility,
                MockCall::ForceEnglish {
                    hwnd: 101,
                    prev_hwnd: None,
                },
                MockCall::TurnOffCapsLock,
                MockCall::ArmHook(expected_flags),
            ]
        );
        core.backend.calls.clear();

        // Pausing while Protected immediately disarms and transitions to Paused
        core.backend.current_fg = 101;
        let paused = core.toggle_pause().expect("pause while protected");
        assert_eq!(paused.state, ProtectionState::Paused);
        assert!(!paused.settings.enabled);
        assert_eq!(
            core.backend.calls,
            vec![
                MockCall::DisarmHook,
                MockCall::RestoreAccessibility,
                MockCall::RestoreLanguage(101),
            ]
        );
    }

    #[test]
    fn coordinator_starts_toggles_pause_updates_settings_and_shuts_down_cleanly() {
        let dir = TempDirGuard::new("lifecycle");
        let settings_path = dir.path.join("settings.json");
        let recovery_path = dir.path.join("recovery.json");
        let notifications = Arc::new(Mutex::new(Vec::<AppSnapshot>::new()));
        let notifications_cb = Arc::clone(&notifications);

        let config = CoordinatorConfig {
            settings_path: settings_path.clone(),
            recovery_path,
            sync_startup_registry: false,
            inspect_initial_foreground: false,
            manage_caps_lock: false,
        };

        let coordinator = ProtectionCoordinator::start_with_config(config, move |snap| {
            notifications_cb
                .lock()
                .expect("lock notifications")
                .push(snap);
        })
        .expect("start coordinator");

        let initial = coordinator.snapshot();
        assert_eq!(initial.state, ProtectionState::Standby);
        assert_eq!(initial.settings, Settings::default());
        assert!(!initial.elevation_warning);

        // Standby -> Paused
        let paused = coordinator.toggle_pause().expect("toggle to paused");
        assert_eq!(paused.state, ProtectionState::Paused);
        assert!(!paused.settings.enabled);
        assert!(!load_settings_from(&settings_path).enabled);

        // Paused -> Standby
        let resumed = coordinator.toggle_pause().expect("toggle back to standby");
        assert_eq!(resumed.state, ProtectionState::Standby);
        assert!(resumed.settings.enabled);
        assert!(load_settings_from(&settings_path).enabled);

        // Reject invalid bare W escape key
        let invalid_key = EscapeKey {
            vk: 0x57,
            modifiers: 0,
            label: "W".to_string(),
        };
        assert!(coordinator.set_escape_key(invalid_key).is_err());
        assert_eq!(coordinator.snapshot().settings.escape_key.vk, 0x79);

        // Accept valid F9 escape key
        let f9_key = EscapeKey {
            vk: 0x78,
            modifiers: 0,
            label: "F9".to_string(),
        };
        let with_f9 = coordinator
            .set_escape_key(f9_key.clone())
            .expect("accept F9 escape key");
        assert_eq!(with_f9.settings.escape_key, f9_key);
        assert_eq!(load_settings_from(&settings_path).escape_key, f9_key);

        // Update settings patch
        let patch = Settings {
            enabled: true,
            lock_language: false,
            block_minimize: false,
            block_caps_lock: false,
            escape_key: f9_key,
            launch_at_startup: true,
        };
        let updated = coordinator
            .update_settings(patch.clone())
            .expect("update settings");
        assert_eq!(updated.settings, patch);
        assert_eq!(load_settings_from(&settings_path), patch);

        // Callback observed transitions
        let seen = notifications.lock().expect("lock notifications");
        assert!(
            seen.len() >= 4,
            "expected at least 4 change notifications, got {}",
            seen.len()
        );
        drop(seen);

        // Drop shuts down cleanly without hanging
        let drop_start = Instant::now();
        drop(coordinator);
        assert!(
            drop_start.elapsed() < Duration::from_secs(1),
            "coordinator drop took too long: {:?}",
            drop_start.elapsed()
        );
    }

    #[test]
    fn coordinator_cleans_up_corrupt_recovery_file_on_startup() {
        let dir = TempDirGuard::new("crash_recovery");
        let settings_path = dir.path.join("settings.json");
        let recovery_path = dir.path.join("recovery.json");

        // Write invalid JSON so AccessibilityGuard::recover_from_crash_file deletes it
        // without mutating host Windows accessibility flags.
        fs::write(&recovery_path, "{ invalid recovery json").expect("write corrupt recovery file");
        assert!(recovery_path.exists());

        let coordinator = ProtectionCoordinator::start_with_config(
            CoordinatorConfig {
                settings_path,
                recovery_path: recovery_path.clone(),
                sync_startup_registry: false,
                inspect_initial_foreground: false,
                manage_caps_lock: false,
            },
            |_| {},
        )
        .expect("start coordinator");

        assert!(
            !recovery_path.exists(),
            "startup must run recover_from_crash_file and clean up recovery.json"
        );
        coordinator.shutdown();
        // Second shutdown is idempotent and commands after shutdown fail cleanly.
        coordinator.shutdown();
        assert!(coordinator.toggle_pause().is_err());
    }

    #[test]
    fn coordinator_own_window_focus_disarms_immediately_and_clears_escape_exit_suppression() {
        let dir = TempDirGuard::new("own_window_focus");
        let settings_path = dir.path.join("settings.json");
        let recovery_path = dir.path.join("recovery.json");

        let pubg_hwnd: isize = 100;
        let settings_hwnd: isize = 300;

        let backend = MockBackend {
            current_fg: pubg_hwnd,
            fg_after_minimize: Some(settings_hwnd),
            alive_windows: vec![pubg_hwnd, settings_hwnd],
            ..MockBackend::default()
        };

        let mut core =
            CoordinatorCore::new(settings_path.clone(), Settings::default(), false, backend);

        // 1. Arm on PUBG, then simulate own-process "settings" window focus event
        // (normally invisible to WINEVENT_SKIPOWNPROCESS; forwarded via WindowEvent::Focused(true)).
        core.handle_foreground(ForegroundNotice {
            hwnd: pubg_hwnd,
            is_pubg: true,
        });
        assert_eq!(core.state(), ProtectionState::Protected);
        core.backend.calls.clear();

        core.handle_foreground(ForegroundNotice {
            hwnd: settings_hwnd,
            is_pubg: false,
        });
        assert_eq!(core.state(), ProtectionState::Standby);
        assert_eq!(core.last_non_pubg_hwnd, Some(settings_hwnd));
        assert_eq!(
            core.backend.calls,
            vec![
                MockCall::DisarmHook,
                MockCall::RestoreAccessibility,
                MockCall::RestoreLanguage(settings_hwnd),
            ]
        );
        core.backend.calls.clear();

        // 2. Return to PUBG and trigger Escape (F10): minimizes PUBG to own "settings" window.
        core.backend.current_fg = pubg_hwnd;
        core.handle_foreground(ForegroundNotice {
            hwnd: pubg_hwnd,
            is_pubg: true,
        });
        assert_eq!(core.state(), ProtectionState::Protected);
        core.backend.calls.clear();

        core.handle_hook_signal(HookSignal::EscapeTriggered);
        assert_eq!(core.state(), ProtectionState::Standby);
        assert_eq!(
            core.backend.calls,
            vec![
                MockCall::DisarmHook,
                MockCall::RestoreAccessibility,
                MockCall::MinimizePubg {
                    pubg_hwnd,
                    fallback_hwnd: Some(settings_hwnd),
                },
                MockCall::RestoreLanguage(settings_hwnd),
            ]
        );
        core.backend.calls.clear();

        // Flickering PUBG foreground before "settings" focus arrives stays suppressed.
        core.handle_foreground(ForegroundNotice {
            hwnd: pubg_hwnd,
            is_pubg: true,
        });
        assert_eq!(core.state(), ProtectionState::Standby);

        // 3. Forwarded own-window focus clears `exiting_hwnd`, so re-entering PUBG re-arms!
        core.handle_foreground(ForegroundNotice {
            hwnd: settings_hwnd,
            is_pubg: false,
        });
        assert_eq!(core.state(), ProtectionState::Standby);

        core.backend.current_fg = pubg_hwnd;
        core.handle_foreground(ForegroundNotice {
            hwnd: pubg_hwnd,
            is_pubg: true,
        });
        assert_eq!(core.state(), ProtectionState::Protected);
        assert_eq!(core.backend.calls.len(), 4);

        // 4. Verify live ProtectionCoordinator::notify_non_pubg_foreground is non-blocking
        // and safe both while running and after shutdown.
        let coordinator = ProtectionCoordinator::start_with_config(
            CoordinatorConfig {
                settings_path,
                recovery_path,
                sync_startup_registry: false,
                inspect_initial_foreground: false,
                manage_caps_lock: false,
            },
            |_| {},
        )
        .expect("start coordinator");

        coordinator.notify_non_pubg_foreground(settings_hwnd);
        assert_eq!(coordinator.snapshot().state, ProtectionState::Standby);
        coordinator.shutdown();
        coordinator.notify_non_pubg_foreground(settings_hwnd);
    }

    #[test]
    fn escape_minimize_to_desktop_rearms_immediately_on_return_via_minimize_event_and_exit_poll() {
        let dir = TempDirGuard::new("escape_minimize_rearm");
        let settings_path = dir.path.join("settings.json");

        let pubg_hwnd: isize = 100;
        let backend = MockBackend {
            current_fg: pubg_hwnd,
            // Simulate minimizing to desktop where GetForegroundWindow() becomes 0 and
            // Windows emits EVENT_SYSTEM_MINIMIZESTART with no EVENT_SYSTEM_FOREGROUND.
            fg_after_minimize: Some(0),
            alive_windows: vec![pubg_hwnd],
            ..MockBackend::default()
        };

        let mut core = CoordinatorCore::new(settings_path, Settings::default(), false, backend);

        // Path 1: Cleared immediately by EVENT_SYSTEM_MINIMIZESTART (WindowMinimized).
        core.handle_foreground(ForegroundNotice {
            hwnd: pubg_hwnd,
            is_pubg: true,
        });
        assert_eq!(core.state(), ProtectionState::Protected);
        core.backend.calls.clear();

        core.handle_hook_signal(HookSignal::EscapeTriggered);
        assert_eq!(core.state(), ProtectionState::Standby);
        assert!(core.has_pending_exit());
        core.backend.calls.clear();

        core.handle_window_minimized(pubg_hwnd);
        assert!(!core.has_pending_exit());

        // Returning directly to PUBG immediately re-arms on the first foreground event.
        core.backend.current_fg = pubg_hwnd;
        core.handle_foreground(ForegroundNotice {
            hwnd: pubg_hwnd,
            is_pubg: true,
        });
        assert_eq!(core.state(), ProtectionState::Protected);
        assert!(core
            .backend
            .calls
            .iter()
            .any(|call| matches!(call, MockCall::ArmHook(_))));
        core.backend.calls.clear();

        // Path 2: Even if EVENT_SYSTEM_MINIMIZESTART never arrives, the 50 ms exit poll
        // clears exit suppression when GetForegroundWindow() != pubg_hwnd OR IsIconic(pubg_hwnd).
        core.handle_hook_signal(HookSignal::EscapeTriggered);
        assert!(core.has_pending_exit());
        core.backend.calls.clear();

        core.handle_exit_poll_tick();
        assert!(
            !core.has_pending_exit(),
            "exit poll must clear suppression when foreground moved to 0 (desktop)"
        );

        core.backend.current_fg = pubg_hwnd;
        core.handle_foreground(ForegroundNotice {
            hwnd: pubg_hwnd,
            is_pubg: true,
        });
        assert_eq!(core.state(), ProtectionState::Protected);
        core.backend.calls.clear();

        // Path 3: Exit poll also clears suppression when foreground still reports pubg_hwnd
        // transiently but IsIconic(pubg_hwnd) is true.
        core.backend.fg_after_minimize = Some(pubg_hwnd);
        core.handle_hook_signal(HookSignal::EscapeTriggered);
        assert!(core.has_pending_exit());
        core.backend.iconic_windows.push(pubg_hwnd);
        core.handle_exit_poll_tick();
        assert!(
            !core.has_pending_exit(),
            "exit poll must clear suppression once PUBG reports iconic"
        );
        core.backend.iconic_windows.clear();

        core.handle_foreground(ForegroundNotice {
            hwnd: pubg_hwnd,
            is_pubg: true,
        });
        assert_eq!(core.state(), ProtectionState::Protected);
    }

    #[test]
    fn coordinator_core_manages_caps_lock_on_arm_and_live_toggle_only() {
        let dir = TempDirGuard::new("caps_lock_guard");
        let settings_path = dir.path.join("settings.json");

        // 1. Start with block_caps_lock = false -> Arm must NOT call turn_off_caps_lock_if_on
        let initial_settings = Settings {
            block_caps_lock: false,
            ..Settings::default()
        };
        let backend = MockBackend {
            current_fg: 200,
            ..MockBackend::default()
        };
        let mut core = CoordinatorCore::new(settings_path, initial_settings, false, backend);

        core.handle_foreground(ForegroundNotice {
            hwnd: 100,
            is_pubg: true,
        });
        assert_eq!(core.state(), ProtectionState::Protected);
        assert!(!core
            .backend
            .calls
            .iter()
            .any(|c| matches!(c, MockCall::TurnOffCapsLock)));
        assert!(core.backend.calls.iter().any(|c| matches!(
            c,
            MockCall::ArmHook(flags) if !flags.block_caps_lock
        )));
        core.backend.calls.clear();

        // 2. Live toggle block_caps_lock false -> true while Protected updates flags AND turns off Caps Lock
        let mut patch_on = core.snapshot().settings;
        patch_on.block_caps_lock = true;
        core.apply_settings_update(patch_on)
            .expect("enable block_caps_lock live");
        assert_eq!(
            core.backend.calls,
            vec![
                MockCall::UpdateHookFlags(FilterFlags::from_escape_key(
                    true,
                    true,
                    true,
                    true,
                    &Settings::default().escape_key
                )),
                MockCall::TurnOffCapsLock,
            ]
        );
        core.backend.calls.clear();

        // 3. Updating another field while block_caps_lock stays true does NOT re-call TurnOffCapsLock
        let mut patch_other = core.snapshot().settings;
        patch_other.block_minimize = false;
        core.apply_settings_update(patch_other)
            .expect("update block_minimize while caps lock stays on");
        assert!(!core
            .backend
            .calls
            .iter()
            .any(|c| matches!(c, MockCall::TurnOffCapsLock)));
        core.backend.calls.clear();

        // 4. Disarm and shutdown_cleanup NEVER turn Caps Lock back ON
        core.handle_foreground(ForegroundNotice {
            hwnd: 200,
            is_pubg: false,
        });
        assert_eq!(core.state(), ProtectionState::Standby);
        assert!(!core
            .backend
            .calls
            .iter()
            .any(|c| matches!(c, MockCall::TurnOffCapsLock)));
        core.backend.calls.clear();

        // Re-arm with block_caps_lock = true: TurnOffCapsLock runs strictly BEFORE ArmHook
        core.handle_foreground(ForegroundNotice {
            hwnd: 100,
            is_pubg: true,
        });
        let turn_off_pos = core
            .backend
            .calls
            .iter()
            .position(|c| matches!(c, MockCall::TurnOffCapsLock))
            .expect("TurnOffCapsLock called on Arm");
        let arm_pos = core
            .backend
            .calls
            .iter()
            .position(|c| matches!(c, MockCall::ArmHook(flags) if flags.block_caps_lock))
            .expect("ArmHook called on Arm");
        assert!(turn_off_pos < arm_pos);
        core.backend.calls.clear();

        core.shutdown_cleanup();
        assert!(!core
            .backend
            .calls
            .iter()
            .any(|c| matches!(c, MockCall::TurnOffCapsLock)));
    }
}
