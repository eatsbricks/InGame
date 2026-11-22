export const MOD_CTRL = 0x01;
export const MOD_ALT = 0x02;
export const MOD_SHIFT = 0x04;

export interface EscapeKey {
  vk: number;
  modifiers: number;
  label: string;
}

export type RebindOutcome =
  | { kind: "cancel" }
  | { kind: "invalid"; reason: string }
  | { kind: "ok"; key: EscapeKey };

export interface RebindKeyInput {
  code: string;
  key: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}

interface KeyEntry {
  vk: number;
  label: string;
  safeBare: boolean;
}

const META_CODES = new Set(["MetaLeft", "MetaRight", "OSLeft", "OSRight"]);

const COMBO_MODIFIER_CODES = new Set([
  "ControlLeft",
  "ControlRight",
  "AltLeft",
  "AltRight",
  "ShiftLeft",
  "ShiftRight",
]);

function buildKeyMap(): ReadonlyMap<string, KeyEntry> {
  const map = new Map<string, KeyEntry>([
    ["Pause", { vk: 0x13, label: "Pause", safeBare: true }],
    ["ScrollLock", { vk: 0x91, label: "ScrollLock", safeBare: true }],
    ["Insert", { vk: 0x2d, label: "Insert", safeBare: true }],
    ["Home", { vk: 0x24, label: "Home", safeBare: true }],
    ["End", { vk: 0x23, label: "End", safeBare: true }],
    ["PageUp", { vk: 0x21, label: "PageUp", safeBare: true }],
    ["PageDown", { vk: 0x22, label: "PageDown", safeBare: true }],
    ["Tab", { vk: 0x09, label: "Tab", safeBare: false }],
    ["Backquote", { vk: 0xc0, label: "~", safeBare: false }],
    ["Space", { vk: 0x20, label: "Space", safeBare: false }],
  ]);

  for (let i = 1; i <= 12; i += 1) {
    map.set(`F${i}`, { vk: 0x6f + i, label: `F${i}`, safeBare: true });
  }

  for (let i = 0; i < 26; i += 1) {
    const letter = String.fromCharCode(0x41 + i);
    map.set(`Key${letter}`, { vk: 0x41 + i, label: letter, safeBare: false });
  }

  for (let i = 0; i <= 9; i += 1) {
    map.set(`Digit${i}`, { vk: 0x30 + i, label: String(i), safeBare: false });
  }

  return map;
}

const KEY_MAP = buildKeyMap();

export function isComboModifierCode(code: string): boolean {
  return COMBO_MODIFIER_CODES.has(code);
}

export function formatHeldModifiers(ev: {
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}): string {
  const parts: string[] = [];
  if (ev.ctrlKey) parts.push("Ctrl");
  if (ev.altKey) parts.push("Alt");
  if (ev.shiftKey) parts.push("Shift");
  return parts.length > 0 ? `${parts.join("+")}+...` : "Press key...";
}

export function mapKeyboardEventToEscapeKey(ev: RebindKeyInput): RebindOutcome {
  if (ev.code === "Escape" || ev.key === "Escape") {
    if (!ev.ctrlKey && !ev.altKey && !ev.shiftKey && !ev.metaKey) {
      return { kind: "cancel" };
    }
    return { kind: "invalid", reason: "ใช้ปุ่ม Esc ไม่ได้" };
  }

  if (
    ev.metaKey ||
    META_CODES.has(ev.code) ||
    ev.key === "Meta" ||
    ev.key === "OS"
  ) {
    return { kind: "invalid", reason: "ใช้ปุ่ม Win ไม่ได้" };
  }

  if (
    COMBO_MODIFIER_CODES.has(ev.code) ||
    ev.key === "Control" ||
    ev.key === "Alt" ||
    ev.key === "Shift"
  ) {
    return { kind: "invalid", reason: "กดปุ่มอื่นร่วมด้วย" };
  }

  // On Windows, Ctrl+Pause translates to VK_CANCEL (0x03) at the driver level,
  // which will never match VK_PAUSE (0x13) inside WH_KEYBOARD_LL.
  if ((ev.code === "Pause" && ev.ctrlKey) || ev.key === "Cancel") {
    return { kind: "invalid", reason: "ใช้ Ctrl+Pause ไม่ได้" };
  }

  const entry = KEY_MAP.get(ev.code);
  if (!entry) {
    return { kind: "invalid", reason: "ใช้ F1–F12 หรือ +Ctrl/Alt/Shift" };
  }

  const modifiers =
    (ev.ctrlKey ? MOD_CTRL : 0) |
    (ev.altKey ? MOD_ALT : 0) |
    (ev.shiftKey ? MOD_SHIFT : 0);

  if (modifiers === 0 && !entry.safeBare) {
    return { kind: "invalid", reason: "กดร่วมกับ Ctrl/Alt/Shift" };
  }

  const parts: string[] = [];
  if (ev.ctrlKey) parts.push("Ctrl");
  if (ev.altKey) parts.push("Alt");
  if (ev.shiftKey) parts.push("Shift");
  parts.push(entry.label);

  return {
    kind: "ok",
    key: {
      vk: entry.vk,
      modifiers,
      label: parts.join("+"),
    },
  };
}
