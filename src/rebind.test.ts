import test from "node:test";
import assert from "node:assert/strict";

import {
  MOD_ALT,
  MOD_CTRL,
  MOD_SHIFT,
  mapKeyboardEventToEscapeKey,
} from "./rebind.ts";

interface KeyInput {
  code: string;
  key: string;
  ctrlKey?: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
  metaKey?: boolean;
}

function makeEvent(input: KeyInput) {
  return {
    code: input.code,
    key: input.key,
    ctrlKey: input.ctrlKey ?? false,
    altKey: input.altKey ?? false,
    shiftKey: input.shiftKey ?? false,
    metaKey: input.metaKey ?? false,
  };
}

test("esc_cancels_rebind_capture", () => {
  const result = mapKeyboardEventToEscapeKey(
    makeEvent({
      code: "Escape",
      key: "Escape",
      ctrlKey: false,
      altKey: false,
      shiftKey: false,
      metaKey: false,
    }),
  );

  assert.deepEqual(result, { kind: "cancel" });
});

test("accepts_f10_pause_scrolllock_and_ctrl_alt_tab", () => {
  assert.deepEqual(
    mapKeyboardEventToEscapeKey(makeEvent({ code: "F10", key: "F10" })),
    {
      kind: "ok",
      key: { vk: 0x79, modifiers: 0, label: "F10" },
    },
  );

  assert.deepEqual(
    mapKeyboardEventToEscapeKey(makeEvent({ code: "Pause", key: "Pause" })),
    {
      kind: "ok",
      key: { vk: 0x13, modifiers: 0, label: "Pause" },
    },
  );

  assert.deepEqual(
    mapKeyboardEventToEscapeKey(
      makeEvent({ code: "ScrollLock", key: "ScrollLock" }),
    ),
    {
      kind: "ok",
      key: { vk: 0x91, modifiers: 0, label: "ScrollLock" },
    },
  );

  assert.deepEqual(
    mapKeyboardEventToEscapeKey(
      makeEvent({
        code: "Tab",
        key: "Tab",
        ctrlKey: true,
        altKey: true,
      }),
    ),
    {
      kind: "ok",
      key: {
        vk: 0x09,
        modifiers: MOD_CTRL | MOD_ALT,
        label: "Ctrl+Alt+Tab",
      },
    },
  );
});

test("rejects_bare_wasd_space_tab_and_bare_modifiers", () => {
  const rejectedCases: KeyInput[] = [
    { code: "KeyW", key: "w" },
    { code: "KeyA", key: "a" },
    { code: "KeyS", key: "s" },
    { code: "KeyD", key: "d" },
    { code: "Space", key: " " },
    { code: "Tab", key: "Tab" },
    { code: "ControlLeft", key: "Control", ctrlKey: true },
    { code: "AltLeft", key: "Alt", altKey: true },
    { code: "ShiftLeft", key: "Shift", shiftKey: true },
    { code: "MetaLeft", key: "Meta", metaKey: true },
    { code: "ControlRight", key: "Control", ctrlKey: true },
    { code: "AltRight", key: "Alt", altKey: true },
    { code: "ShiftRight", key: "Shift", shiftKey: true },
    { code: "MetaRight", key: "Meta", metaKey: true },
    { code: "ControlLeft", key: "Control" },
    { code: "AltLeft", key: "Alt" },
    { code: "ShiftLeft", key: "Shift" },
    { code: "MetaLeft", key: "Meta" },
    { code: "AltLeft", key: "Alt", ctrlKey: true, altKey: true },
  ];

  for (const input of rejectedCases) {
    const result = mapKeyboardEventToEscapeKey(makeEvent(input));
    assert.equal(
      result.kind,
      "invalid",
      `expected ${input.code} to be rejected as invalid`,
    );
    assert.ok(result.reason.length > 0, "expected non-empty rejection reason");
  }
});

test("maps_all_safe_bare_keys_to_exact_win32_vk_codes", () => {
  const expected: Array<[string, number, string]> = [
    ["F1", 0x70, "F1"],
    ["F2", 0x71, "F2"],
    ["F3", 0x72, "F3"],
    ["F4", 0x73, "F4"],
    ["F5", 0x74, "F5"],
    ["F6", 0x75, "F6"],
    ["F7", 0x76, "F7"],
    ["F8", 0x77, "F8"],
    ["F9", 0x78, "F9"],
    ["F10", 0x79, "F10"],
    ["F11", 0x7a, "F11"],
    ["F12", 0x7b, "F12"],
    ["Insert", 0x2d, "Insert"],
    ["Home", 0x24, "Home"],
    ["End", 0x23, "End"],
    ["PageUp", 0x21, "PageUp"],
    ["PageDown", 0x22, "PageDown"],
  ];

  for (const [code, vk, label] of expected) {
    assert.deepEqual(
      mapKeyboardEventToEscapeKey(makeEvent({ code, key: code })),
      {
        kind: "ok",
        key: { vk, modifiers: 0, label },
      },
    );
  }
});

test("maps_modifier_combinations_and_rejects_win_modifier_or_unsupported_keys", () => {
  assert.equal(MOD_CTRL, 0x01);
  assert.equal(MOD_ALT, 0x02);
  assert.equal(MOD_SHIFT, 0x04);

  assert.deepEqual(
    mapKeyboardEventToEscapeKey(
      makeEvent({ code: "KeyW", key: "W", ctrlKey: true, shiftKey: true }),
    ),
    {
      kind: "ok",
      key: { vk: 0x57, modifiers: 0x05, label: "Ctrl+Shift+W" },
    },
  );

  assert.deepEqual(
    mapKeyboardEventToEscapeKey(
      makeEvent({ code: "Digit1", key: "1", altKey: true }),
    ),
    {
      kind: "ok",
      key: { vk: 0x31, modifiers: 0x02, label: "Alt+1" },
    },
  );

  assert.deepEqual(
    mapKeyboardEventToEscapeKey(
      makeEvent({ code: "Backquote", key: "`", ctrlKey: true }),
    ),
    {
      kind: "ok",
      key: { vk: 0xc0, modifiers: 0x01, label: "Ctrl+~" },
    },
  );

  assert.deepEqual(
    mapKeyboardEventToEscapeKey(
      makeEvent({ code: "Space", key: " ", ctrlKey: true, altKey: true }),
    ),
    {
      kind: "ok",
      key: { vk: 0x20, modifiers: 0x03, label: "Ctrl+Alt+Space" },
    },
  );

  // Win/Meta combined with any key must be rejected (unsupported by config.rs)
  const winF10 = mapKeyboardEventToEscapeKey(
    makeEvent({ code: "F10", key: "F10", metaKey: true }),
  );
  assert.equal(winF10.kind, "invalid");

  // Bare digit, bare Backquote, modified Escape, and unmapped keys must be rejected
  for (const input of [
    { code: "Digit0", key: "0" },
    { code: "Backquote", key: "`" },
    { code: "Escape", key: "Escape", ctrlKey: true },
    { code: "Enter", key: "Enter" },
    { code: "ArrowUp", key: "ArrowUp", ctrlKey: true },
  ]) {
    assert.equal(
      mapKeyboardEventToEscapeKey(makeEvent(input)).kind,
      "invalid",
      `expected ${input.code} to be invalid`,
    );
  }
});

test("rejects_ctrl_pause_which_translates_to_vk_cancel_on_windows", () => {
  const rejectedPauseCombos: KeyInput[] = [
    { code: "Pause", key: "Cancel", ctrlKey: true },
    { code: "Pause", key: "Pause", ctrlKey: true },
    { code: "Pause", key: "Cancel", ctrlKey: true, altKey: true },
    { code: "Pause", key: "Cancel", ctrlKey: true, shiftKey: true },
    { code: "Pause", key: "Cancel" },
  ];

  for (const input of rejectedPauseCombos) {
    const result = mapKeyboardEventToEscapeKey(makeEvent(input));
    assert.equal(
      result.kind,
      "invalid",
      `expected Pause with ctrlKey=${String(input.ctrlKey)} key=${input.key} to be rejected`,
    );
  }

  // Non-Ctrl Pause combos still deliver VK_PAUSE (0x13) cleanly
  assert.deepEqual(
    mapKeyboardEventToEscapeKey(
      makeEvent({ code: "Pause", key: "Pause", altKey: true }),
    ),
    {
      kind: "ok",
      key: { vk: 0x13, modifiers: MOD_ALT, label: "Alt+Pause" },
    },
  );
});

test("invalid_feedback_messages_stay_compact_for_300px_row", () => {
  const invalidSamples: KeyInput[] = [
    { code: "Escape", key: "Escape", ctrlKey: true },
    { code: "MetaLeft", key: "Meta", metaKey: true },
    { code: "ControlLeft", key: "Control", ctrlKey: true },
    { code: "Pause", key: "Cancel", ctrlKey: true },
    { code: "Enter", key: "Enter" },
    { code: "KeyW", key: "w" },
  ];

  for (const input of invalidSamples) {
    const result = mapKeyboardEventToEscapeKey(makeEvent(input));
    assert.equal(result.kind, "invalid");
    // Keep feedback concise so #rebind-hint never truncates inside Row 3 at 300px width
    assert.ok(
      result.reason.length <= 32,
      `expected compact feedback (<= 32 chars) for ${input.code}, got "${result.reason}" (${result.reason.length} chars)`,
    );
  }
});
