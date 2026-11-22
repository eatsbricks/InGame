import "./styles.css";

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import {
  formatHeldModifiers,
  isComboModifierCode,
  mapKeyboardEventToEscapeKey,
  type EscapeKey,
} from "./rebind.ts";

export type ProtectionState = "Standby" | "Protected" | "Paused";

export interface Settings {
  enabled: boolean;
  lock_language: boolean;
  block_minimize: boolean;
  block_caps_lock: boolean;
  escape_key: EscapeKey;
  launch_at_startup: boolean;
}

export interface AppSnapshot {
  state: ProtectionState;
  settings: Settings;
  has_english_layout: boolean;
  elevation_warning: boolean;
}

type BooleanSettingKey =
  | "lock_language"
  | "block_minimize"
  | "block_caps_lock"
  | "launch_at_startup";

function requireElement<T extends HTMLElement>(id: string): T {
  const el = document.getElementById(id);
  if (!el) {
    throw new Error(`Missing required DOM element: #${id}`);
  }
  return el as T;
}

const appShell = requireElement<HTMLDivElement>("app");
const statusBtn = requireElement<HTMLButtonElement>("status-btn");
const closeBtn = requireElement<HTMLButtonElement>("close-btn");
const toggleLockLanguageBtn = requireElement<HTMLButtonElement>(
  "toggle-lock-language",
);
const toggleBlockMinimizeBtn = requireElement<HTMLButtonElement>(
  "toggle-block-minimize",
);
const toggleBlockCapsLockBtn = requireElement<HTMLButtonElement>(
  "toggle-block-caps-lock",
);
const toggleLaunchStartupBtn = requireElement<HTMLButtonElement>(
  "toggle-launch-startup",
);
const escapeKeyBtn = requireElement<HTMLButtonElement>("escape-key-btn");
const rebindHintEl = requireElement<HTMLSpanElement>("rebind-hint");
const noticesEl = requireElement<HTMLElement>("notices");
const noticeNoEnEl = requireElement<HTMLParagraphElement>("notice-no-en");
const noticeElevationEl =
  requireElement<HTMLParagraphElement>("notice-elevation");
const noticeErrorEl = requireElement<HTMLParagraphElement>("notice-error");

const switchButtons: Record<BooleanSettingKey, HTMLButtonElement> = {
  lock_language: toggleLockLanguageBtn,
  block_minimize: toggleBlockMinimizeBtn,
  block_caps_lock: toggleBlockCapsLockBtn,
  launch_at_startup: toggleLaunchStartupBtn,
};

let currentSnapshot: AppSnapshot | null = null;
let isMutating = false;
let isCapturing = false;
let ipcErrorMessage: string | null = null;
let rebindFeedbackTimer: number | null = null;
let rebindInvalidFlashTimer: number | null = null;
let ipcErrorTimer: number | null = null;
let unlistenStateChanged: UnlistenFn | null = null;

function formatStatusLabel(state: ProtectionState): string {
  switch (state) {
    case "Protected":
      return "Active";
    case "Standby":
      return "Standby";
    case "Paused":
      return "Paused";
  }
}

function formatStatusTooltip(state: ProtectionState): string {
  return state === "Paused"
    ? "คลิกเพื่อเปิดการป้องกันต่อ"
    : "คลิกเพื่อพักการป้องกันชั่วคราว";
}

function updateNoticesVisibility(): void {
  const showNoEn = currentSnapshot ? !currentSnapshot.has_english_layout : false;
  const showElevation = currentSnapshot
    ? currentSnapshot.elevation_warning
    : false;
  const showError = ipcErrorMessage !== null;

  noticeNoEnEl.hidden = !showNoEn;
  noticeElevationEl.hidden = !showElevation;
  noticeErrorEl.hidden = !showError;
  if (showError && ipcErrorMessage !== null) {
    noticeErrorEl.textContent = ipcErrorMessage;
    noticeErrorEl.title = ipcErrorMessage;
  }

  noticesEl.hidden = !showNoEn && !showElevation && !showError;
}

function render(): void {
  if (!currentSnapshot) {
    updateNoticesVisibility();
    return;
  }

  const { state, settings } = currentSnapshot;
  const statusLabel = formatStatusLabel(state);
  const statusTooltip = formatStatusTooltip(state);

  appShell.dataset.state = state;
  statusBtn.textContent = statusLabel;
  statusBtn.title = statusTooltip;
  statusBtn.setAttribute("aria-label", `สถานะ: ${statusLabel} — ${statusTooltip}`);

  toggleLockLanguageBtn.setAttribute(
    "aria-checked",
    String(settings.lock_language),
  );
  toggleBlockMinimizeBtn.setAttribute(
    "aria-checked",
    String(settings.block_minimize),
  );
  toggleBlockCapsLockBtn.setAttribute(
    "aria-checked",
    String(settings.block_caps_lock),
  );
  toggleLaunchStartupBtn.setAttribute(
    "aria-checked",
    String(settings.launch_at_startup),
  );

  if (!isCapturing) {
    escapeKeyBtn.textContent = settings.escape_key.label;
    escapeKeyBtn.title = `${settings.escape_key.label} — คลิกเพื่อเปลี่ยนปุ่มพับจอออก`;
  }

  updateNoticesVisibility();
}

function applySnapshot(snapshot: AppSnapshot): void {
  currentSnapshot = snapshot;
  render();
}

function clearRebindFeedback(): void {
  if (rebindFeedbackTimer !== null) {
    window.clearTimeout(rebindFeedbackTimer);
    rebindFeedbackTimer = null;
  }
  if (rebindInvalidFlashTimer !== null) {
    window.clearTimeout(rebindInvalidFlashTimer);
    rebindInvalidFlashTimer = null;
  }
  delete escapeKeyBtn.dataset.invalid;
  rebindHintEl.textContent = "";
  rebindHintEl.hidden = true;
}

function showRebindFeedback(message: string, flashBorder: boolean): void {
  if (rebindFeedbackTimer !== null) {
    window.clearTimeout(rebindFeedbackTimer);
  }
  rebindHintEl.textContent = message;
  rebindHintEl.title = message;
  rebindHintEl.hidden = false;

  if (flashBorder) {
    escapeKeyBtn.dataset.invalid = "true";
    if (rebindInvalidFlashTimer !== null) {
      window.clearTimeout(rebindInvalidFlashTimer);
    }
    rebindInvalidFlashTimer = window.setTimeout(() => {
      delete escapeKeyBtn.dataset.invalid;
      rebindInvalidFlashTimer = null;
    }, 300);
  }

  rebindFeedbackTimer = window.setTimeout(() => {
    rebindHintEl.textContent = "";
    rebindHintEl.hidden = true;
    rebindFeedbackTimer = null;
  }, 3200);
}

function clearIpcError(): void {
  if (ipcErrorTimer !== null) {
    window.clearTimeout(ipcErrorTimer);
    ipcErrorTimer = null;
  }
  ipcErrorMessage = null;
  updateNoticesVisibility();
}

function showIpcError(message: string): void {
  if (ipcErrorTimer !== null) {
    window.clearTimeout(ipcErrorTimer);
  }
  ipcErrorMessage = message;
  updateNoticesVisibility();
  ipcErrorTimer = window.setTimeout(() => {
    ipcErrorMessage = null;
    ipcErrorTimer = null;
    updateNoticesVisibility();
  }, 3600);
}

function enterCapture(): void {
  if (!currentSnapshot || isMutating) {
    return;
  }
  clearRebindFeedback();
  isCapturing = true;
  escapeKeyBtn.dataset.capturing = "true";
  escapeKeyBtn.setAttribute("aria-pressed", "true");
  escapeKeyBtn.textContent = "Press key...";
  escapeKeyBtn.title = "กดปุ่มที่ต้องการ หรือ Esc เพื่อยกเลิก";
}

function exitCapture(clearFeedback: boolean): void {
  if (!isCapturing) {
    return;
  }
  isCapturing = false;
  delete escapeKeyBtn.dataset.capturing;
  escapeKeyBtn.setAttribute("aria-pressed", "false");
  if (clearFeedback) {
    clearRebindFeedback();
  }
  render();
}

async function refreshSnapshot(): Promise<void> {
  try {
    const latest = await invoke<AppSnapshot>("get_app_state");
    applySnapshot(latest);
  } catch {
    // Keep existing rendered snapshot if refresh fails transiently.
  }
}

async function commitEscapeKey(key: EscapeKey): Promise<void> {
  if (!currentSnapshot || isMutating) {
    return;
  }

  isMutating = true;
  escapeKeyBtn.disabled = true;
  escapeKeyBtn.textContent = key.label;

  try {
    const updated = await invoke<AppSnapshot>("set_escape_key", { key });
    clearIpcError();
    applySnapshot(updated);
  } catch {
    render();
    showRebindFeedback("บันทึกปุ่มไม่สำเร็จ", true);
    void refreshSnapshot();
  } finally {
    escapeKeyBtn.disabled = false;
    isMutating = false;
  }
}

async function toggleBooleanSetting(field: BooleanSettingKey): Promise<void> {
  if (!currentSnapshot || isMutating) {
    return;
  }
  if (isCapturing) {
    exitCapture(true);
  }

  isMutating = true;
  const switchEl = switchButtons[field];
  const nextValue = !currentSnapshot.settings[field];
  const patch: Settings = {
    ...currentSnapshot.settings,
    [field]: nextValue,
  };

  switchEl.setAttribute("aria-checked", String(nextValue));

  try {
    const updated = await invoke<AppSnapshot>("update_settings", { patch });
    clearIpcError();
    applySnapshot(updated);
  } catch {
    render();
    showIpcError("บันทึกการตั้งค่าไม่สำเร็จ");
    void refreshSnapshot();
  } finally {
    isMutating = false;
  }
}

async function handleTogglePause(): Promise<void> {
  if (!currentSnapshot || isMutating) {
    return;
  }
  if (isCapturing) {
    exitCapture(true);
  }

  isMutating = true;
  try {
    const updated = await invoke<AppSnapshot>("toggle_pause");
    clearIpcError();
    applySnapshot(updated);
  } catch {
    render();
    showIpcError("เปลี่ยนสถานะไม่สำเร็จ");
    void refreshSnapshot();
  } finally {
    isMutating = false;
  }
}

async function handleCloseWindow(): Promise<void> {
  if (isCapturing) {
    exitCapture(true);
  }
  try {
    await invoke<void>("close_settings_window");
  } catch {
    // Ignore close errors if window is already closing.
  }
}

function handleCaptureKeyDown(ev: KeyboardEvent): void {
  if (!isCapturing) {
    return;
  }

  ev.preventDefault();
  ev.stopPropagation();

  if (ev.repeat) {
    return;
  }

  const outcome = mapKeyboardEventToEscapeKey(ev);

  if (outcome.kind === "cancel") {
    exitCapture(true);
    return;
  }

  if (outcome.kind === "invalid") {
    if (!ev.metaKey && isComboModifierCode(ev.code)) {
      escapeKeyBtn.textContent = formatHeldModifiers(ev);
      showRebindFeedback(outcome.reason, false);
      return;
    }

    escapeKeyBtn.textContent = "Press key...";
    showRebindFeedback(outcome.reason, true);
    return;
  }

  exitCapture(true);
  void commitEscapeKey(outcome.key);
}

function handleCaptureKeyUp(ev: KeyboardEvent): void {
  if (!isCapturing || !isComboModifierCode(ev.code)) {
    return;
  }
  escapeKeyBtn.textContent = formatHeldModifiers(ev);
}

function stopDragPropagation(ev: MouseEvent): void {
  ev.stopPropagation();
}

function bindEvents(): void {
  statusBtn.addEventListener("mousedown", stopDragPropagation);
  closeBtn.addEventListener("mousedown", stopDragPropagation);

  statusBtn.addEventListener("click", () => {
    void handleTogglePause();
  });

  closeBtn.addEventListener("click", () => {
    void handleCloseWindow();
  });

  toggleLockLanguageBtn.addEventListener("click", () => {
    void toggleBooleanSetting("lock_language");
  });

  toggleBlockMinimizeBtn.addEventListener("click", () => {
    void toggleBooleanSetting("block_minimize");
  });

  toggleBlockCapsLockBtn.addEventListener("click", () => {
    void toggleBooleanSetting("block_caps_lock");
  });

  toggleLaunchStartupBtn.addEventListener("click", () => {
    void toggleBooleanSetting("launch_at_startup");
  });

  escapeKeyBtn.addEventListener("click", () => {
    if (isCapturing) {
      exitCapture(true);
    } else {
      enterCapture();
    }
  });

  window.addEventListener("keydown", handleCaptureKeyDown, true);
  window.addEventListener("keyup", handleCaptureKeyUp, true);

  window.addEventListener(
    "pointerdown",
    (ev) => {
      if (
        isCapturing &&
        ev.target instanceof Node &&
        !escapeKeyBtn.contains(ev.target)
      ) {
        exitCapture(true);
      }
    },
    true,
  );

  window.addEventListener("blur", () => {
    if (isCapturing) {
      exitCapture(true);
    }
  });

  document.addEventListener("visibilitychange", () => {
    if (document.hidden && isCapturing) {
      exitCapture(true);
    }
  });

  document.addEventListener("contextmenu", (ev) => {
    ev.preventDefault();
  });

  window.addEventListener("beforeunload", () => {
    clearRebindFeedback();
    clearIpcError();
    if (unlistenStateChanged) {
      unlistenStateChanged();
      unlistenStateChanged = null;
    }
  });
}

async function init(): Promise<void> {
  bindEvents();

  let eventReceived = false;
  try {
    unlistenStateChanged = await listen<AppSnapshot>(
      "app-state-changed",
      (event) => {
        eventReceived = true;
        applySnapshot(event.payload);
      },
    );
  } catch {
    // Allow initial state fetch even if listener registration fails outside Tauri.
  }

  try {
    const initial = await invoke<AppSnapshot>("get_app_state");
    if (!eventReceived) {
      applySnapshot(initial);
    }
  } catch {
    showIpcError("ไม่สามารถโหลดสถานะโปรแกรมได้");
  }
}

void init();
