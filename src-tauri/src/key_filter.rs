use crate::config::{EscapeKey, MOD_ALT, MOD_CTRL, MOD_SHIFT};

const VK_TAB: u32 = 0x09;
const VK_CAPITAL: u32 = 0x14;
const VK_ESCAPE: u32 = 0x1B;
const VK_SPACE: u32 = 0x20;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;
const VK_OEM_3: u32 = 0xC0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterFlags {
    pub lock_language: bool,
    pub block_minimize: bool,
    pub block_caps_lock: bool,
    pub block_grave_accent: bool,
    pub escape_vk: u32,
    pub escape_modifiers: u32,
}

impl FilterFlags {
    pub const fn from_escape_key(
        lock_language: bool,
        block_minimize: bool,
        block_caps_lock: bool,
        block_grave_accent: bool,
        escape_key: &EscapeKey,
    ) -> Self {
        Self {
            lock_language,
            block_minimize,
            block_caps_lock,
            block_grave_accent,
            escape_vk: escape_key.vk,
            escape_modifiers: escape_key.modifiers,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyInput {
    pub vk: u32,
    pub is_key_down: bool,
    pub alt_down: bool,
    pub ctrl_down: bool,
    pub shift_down: bool,
    pub win_down: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterDecision {
    Pass,
    Swallow,
    TriggerEscape,
    PassAndCheckLanguage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyFilterState {
    swallowed_mask: [u64; 4],
    exiting: bool,
}

impl Default for KeyFilterState {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyFilterState {
    pub const fn new() -> Self {
        Self {
            swallowed_mask: [0; 4],
            exiting: false,
        }
    }

    pub fn reset(&mut self) {
        self.swallowed_mask = [0; 4];
        self.exiting = false;
    }

    pub fn evaluate(&mut self, flags: FilterFlags, input: KeyInput) -> FilterDecision {
        if !input.is_key_down {
            if self.take_swallowed(input.vk) {
                return FilterDecision::Swallow;
            }
            if flags.lock_language && is_lang_trigger_modifier(input.vk) {
                return FilterDecision::PassAndCheckLanguage;
            }
            return FilterDecision::Pass;
        }

        if matches_escape_key(flags, input) {
            self.mark_swallowed(input.vk);
            if !self.exiting {
                self.exiting = true;
                return FilterDecision::TriggerEscape;
            }
            return FilterDecision::Swallow;
        }

        if flags.block_caps_lock && input.vk == VK_CAPITAL {
            self.mark_swallowed(input.vk);
            return FilterDecision::Swallow;
        }

        if flags.block_minimize && is_minimize_shortcut(input) {
            self.mark_swallowed(input.vk);
            return FilterDecision::Swallow;
        }

        if flags.lock_language && is_language_shortcut(flags.block_grave_accent, input) {
            self.mark_swallowed(input.vk);
            return FilterDecision::Swallow;
        }

        self.clear_swallowed(input.vk);
        FilterDecision::Pass
    }

    #[inline]
    fn mark_swallowed(&mut self, vk: u32) {
        if vk < 256 {
            let idx = (vk >> 6) as usize;
            let bit = 1u64 << (vk & 63);
            self.swallowed_mask[idx] |= bit;
        }
    }

    #[inline]
    fn clear_swallowed(&mut self, vk: u32) {
        if vk < 256 {
            let idx = (vk >> 6) as usize;
            let bit = 1u64 << (vk & 63);
            self.swallowed_mask[idx] &= !bit;
        }
    }

    #[inline]
    fn take_swallowed(&mut self, vk: u32) -> bool {
        if vk < 256 {
            let idx = (vk >> 6) as usize;
            let bit = 1u64 << (vk & 63);
            let was_set = (self.swallowed_mask[idx] & bit) != 0;
            self.swallowed_mask[idx] &= !bit;
            was_set
        } else {
            false
        }
    }
}

#[inline]
const fn is_lang_trigger_modifier(vk: u32) -> bool {
    matches!(vk, 0x10..=0x12 | 0xA0..=0xA5)
}

#[inline]
const fn matches_escape_key(flags: FilterFlags, input: KeyInput) -> bool {
    if flags.escape_vk == 0 || input.vk != flags.escape_vk {
        return false;
    }

    if flags.escape_modifiers == 0 {
        !input.alt_down && !input.ctrl_down && !input.win_down
    } else {
        let active = (if input.ctrl_down { MOD_CTRL } else { 0 })
            | (if input.alt_down { MOD_ALT } else { 0 })
            | (if input.shift_down { MOD_SHIFT } else { 0 });
        active == flags.escape_modifiers && !input.win_down
    }
}

#[inline]
const fn is_minimize_shortcut(input: KeyInput) -> bool {
    match input.vk {
        VK_LWIN | VK_RWIN => true,
        VK_TAB => input.alt_down,
        VK_ESCAPE => input.alt_down || input.ctrl_down,
        _ => false,
    }
}

#[inline]
const fn is_language_shortcut(block_grave_accent: bool, input: KeyInput) -> bool {
    match input.vk {
        VK_SPACE => input.win_down,
        VK_OEM_3 => block_grave_accent,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VK_SHIFT: u32 = 0x10;
    const VK_CONTROL: u32 = 0x11;
    const VK_MENU: u32 = 0x12;
    const VK_F10: u32 = 0x79;
    const VK_LSHIFT: u32 = 0xA0;
    const VK_RSHIFT: u32 = 0xA1;
    const VK_LCONTROL: u32 = 0xA2;
    const VK_RCONTROL: u32 = 0xA3;
    const VK_LMENU: u32 = 0xA4;
    const VK_RMENU: u32 = 0xA5;

    fn default_flags() -> FilterFlags {
        FilterFlags {
            lock_language: true,
            block_minimize: true,
            block_caps_lock: true,
            block_grave_accent: true,
            escape_vk: VK_F10,
            escape_modifiers: 0,
        }
    }

    fn key(vk: u32, is_key_down: bool) -> KeyInput {
        KeyInput {
            vk,
            is_key_down,
            alt_down: false,
            ctrl_down: false,
            shift_down: false,
            win_down: false,
        }
    }

    #[test]
    fn blocks_win_keys_and_matches_keyup_symmetry() {
        let mut state = KeyFilterState::new();
        let flags = default_flags();

        assert_eq!(
            state.evaluate(flags, key(VK_LWIN, true)),
            FilterDecision::Swallow
        );
        assert_eq!(
            state.evaluate(flags, key(VK_LWIN, false)),
            FilterDecision::Swallow
        );
        assert_eq!(
            state.evaluate(flags, key(VK_LWIN, false)),
            FilterDecision::Pass
        );

        assert_eq!(
            state.evaluate(flags, key(VK_RWIN, true)),
            FilterDecision::Swallow
        );
        assert_eq!(
            state.evaluate(flags, key(VK_RWIN, false)),
            FilterDecision::Swallow
        );

        let no_min_flags = FilterFlags {
            block_minimize: false,
            ..flags
        };
        assert_eq!(
            state.evaluate(no_min_flags, key(VK_LWIN, true)),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(no_min_flags, key(VK_LWIN, false)),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(no_min_flags, key(VK_RWIN, true)),
            FilterDecision::Pass
        );
    }

    #[test]
    fn blocks_alt_tab_alt_esc_ctrl_esc_without_swallowing_alt_or_ctrl() {
        let mut state = KeyFilterState::new();
        let flags = default_flags();

        // Alt down passes; Tab down while Alt held is swallowed
        assert_eq!(
            state.evaluate(flags, key(VK_LMENU, true)),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(
                flags,
                KeyInput {
                    alt_down: true,
                    ..key(VK_TAB, true)
                }
            ),
            FilterDecision::Swallow
        );
        // Alt released first — must never be swallowed
        assert_eq!(
            state.evaluate(flags, key(VK_LMENU, false)),
            FilterDecision::PassAndCheckLanguage
        );
        // Tab up with alt_down: false must STILL be swallowed for key-up symmetry
        assert_eq!(
            state.evaluate(flags, key(VK_TAB, false)),
            FilterDecision::Swallow
        );
        // Next normal Tab down & up pass cleanly
        assert_eq!(
            state.evaluate(flags, key(VK_TAB, true)),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(flags, key(VK_TAB, false)),
            FilterDecision::Pass
        );

        // Alt+Esc swallowed with key-up symmetry
        assert_eq!(
            state.evaluate(
                flags,
                KeyInput {
                    alt_down: true,
                    ..key(VK_ESCAPE, true)
                }
            ),
            FilterDecision::Swallow
        );
        assert_eq!(
            state.evaluate(flags, key(VK_ESCAPE, false)),
            FilterDecision::Swallow
        );

        // Ctrl down passes; Ctrl+Esc swallowed with key-up symmetry
        assert_eq!(
            state.evaluate(flags, key(VK_LCONTROL, true)),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(
                flags,
                KeyInput {
                    ctrl_down: true,
                    ..key(VK_ESCAPE, true)
                }
            ),
            FilterDecision::Swallow
        );
        assert_eq!(
            state.evaluate(flags, key(VK_ESCAPE, false)),
            FilterDecision::Swallow
        );
        // Bare Esc passes
        assert_eq!(
            state.evaluate(flags, key(VK_ESCAPE, true)),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(flags, key(VK_ESCAPE, false)),
            FilterDecision::Pass
        );

        // After reset(), a pending Tab up returns Pass instead of Swallow
        assert_eq!(
            state.evaluate(
                flags,
                KeyInput {
                    alt_down: true,
                    ..key(VK_TAB, true)
                }
            ),
            FilterDecision::Swallow
        );
        state.reset();
        assert_eq!(
            state.evaluate(flags, key(VK_TAB, false)),
            FilterDecision::Pass
        );
    }

    #[test]
    fn preserves_alt_shift_and_ctrl_shift_while_signaling_language_check() {
        let mut state = KeyFilterState::new();
        let flags = default_flags();

        // Alt+Shift sequence: never swallows, signals PassAndCheckLanguage on modifier up
        assert_eq!(
            state.evaluate(flags, key(VK_LMENU, true)),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(
                flags,
                KeyInput {
                    alt_down: true,
                    ..key(VK_LSHIFT, true)
                }
            ),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(
                flags,
                KeyInput {
                    alt_down: true,
                    ..key(VK_LSHIFT, false)
                }
            ),
            FilterDecision::PassAndCheckLanguage
        );
        assert_eq!(
            state.evaluate(
                flags,
                KeyInput {
                    shift_down: true,
                    ..key(VK_LMENU, false)
                }
            ),
            FilterDecision::PassAndCheckLanguage
        );

        // Ctrl+Shift sequence: never swallows, signals PassAndCheckLanguage on modifier up
        assert_eq!(
            state.evaluate(flags, key(VK_LCONTROL, true)),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(
                flags,
                KeyInput {
                    ctrl_down: true,
                    ..key(VK_RSHIFT, true)
                }
            ),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(
                flags,
                KeyInput {
                    ctrl_down: true,
                    ..key(VK_RSHIFT, false)
                }
            ),
            FilterDecision::PassAndCheckLanguage
        );
        assert_eq!(
            state.evaluate(
                flags,
                KeyInput {
                    shift_down: true,
                    ..key(VK_LCONTROL, false)
                }
            ),
            FilterDecision::PassAndCheckLanguage
        );

        // All Alt/Shift/Ctrl VK codes (0x10..=0x12, 0xA0..=0xA5)
        for vk in [
            VK_SHIFT,
            VK_CONTROL,
            VK_MENU,
            VK_LSHIFT,
            VK_RSHIFT,
            VK_LCONTROL,
            VK_RCONTROL,
            VK_LMENU,
            VK_RMENU,
        ] {
            assert_eq!(state.evaluate(flags, key(vk, true)), FilterDecision::Pass);
            assert_eq!(
                state.evaluate(flags, key(vk, false)),
                FilterDecision::PassAndCheckLanguage
            );
        }

        // When lock_language is false, modifier key-up returns Pass (not PassAndCheckLanguage)
        let no_lang_flags = FilterFlags {
            lock_language: false,
            ..flags
        };
        assert_eq!(
            state.evaluate(no_lang_flags, key(VK_LSHIFT, false)),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(no_lang_flags, key(VK_LMENU, false)),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(no_lang_flags, key(VK_LCONTROL, false)),
            FilterDecision::Pass
        );
    }

    #[test]
    fn blocks_win_space_even_when_block_minimize_is_false() {
        let mut state = KeyFilterState::new();
        let flags = FilterFlags {
            lock_language: true,
            block_minimize: false,
            block_caps_lock: true,
            block_grave_accent: false,
            escape_vk: VK_F10,
            escape_modifiers: 0,
        };

        // Bare LWin passes because block_minimize is false
        assert_eq!(
            state.evaluate(flags, key(VK_LWIN, true)),
            FilterDecision::Pass
        );
        // Space while Win held is swallowed by lock_language
        assert_eq!(
            state.evaluate(
                flags,
                KeyInput {
                    win_down: true,
                    ..key(VK_SPACE, true)
                }
            ),
            FilterDecision::Swallow
        );
        // Matching Space up is swallowed even if Win was released first
        assert_eq!(
            state.evaluate(flags, key(VK_LWIN, false)),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(flags, key(VK_SPACE, false)),
            FilterDecision::Swallow
        );
        // Normal Space (Jump) still passes
        assert_eq!(
            state.evaluate(flags, key(VK_SPACE, true)),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(flags, key(VK_SPACE, false)),
            FilterDecision::Pass
        );

        // Alt+Tab passes because block_minimize is false
        assert_eq!(
            state.evaluate(
                flags,
                KeyInput {
                    alt_down: true,
                    ..key(VK_TAB, true)
                }
            ),
            FilterDecision::Pass
        );

        // When lock_language is also false, Win+Space passes
        let all_off = FilterFlags {
            lock_language: false,
            ..flags
        };
        assert_eq!(
            state.evaluate(
                all_off,
                KeyInput {
                    win_down: true,
                    ..key(VK_SPACE, true)
                }
            ),
            FilterDecision::Pass
        );
    }

    #[test]
    fn grave_accent_blocked_only_when_both_lock_language_and_block_grave_accent() {
        let mut state = KeyFilterState::new();
        let both_on = FilterFlags {
            lock_language: true,
            block_minimize: true,
            block_caps_lock: true,
            block_grave_accent: true,
            escape_vk: VK_F10,
            escape_modifiers: 0,
        };

        assert_eq!(
            state.evaluate(both_on, key(VK_OEM_3, true)),
            FilterDecision::Swallow
        );
        assert_eq!(
            state.evaluate(both_on, key(VK_OEM_3, false)),
            FilterDecision::Swallow
        );

        let no_grave = FilterFlags {
            block_grave_accent: false,
            ..both_on
        };
        assert_eq!(
            state.evaluate(no_grave, key(VK_OEM_3, true)),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(no_grave, key(VK_OEM_3, false)),
            FilterDecision::Pass
        );

        let no_lang = FilterFlags {
            lock_language: false,
            block_grave_accent: true,
            ..both_on
        };
        assert_eq!(
            state.evaluate(no_lang, key(VK_OEM_3, true)),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(no_lang, key(VK_OEM_3, false)),
            FilterDecision::Pass
        );
    }

    #[test]
    fn escape_key_single_and_combo_triggers_once_and_swallows() {
        let mut state = KeyFilterState::new();
        let default_escape = EscapeKey {
            vk: VK_F10,
            modifiers: 0,
            label: "F10".into(),
        };
        let f10_flags = FilterFlags::from_escape_key(true, true, true, true, &default_escape);

        // Alt/Ctrl/Win + F10 do not trigger bare F10 escape
        assert_eq!(
            state.evaluate(
                f10_flags,
                KeyInput {
                    alt_down: true,
                    ..key(VK_F10, true)
                }
            ),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(
                f10_flags,
                KeyInput {
                    ctrl_down: true,
                    ..key(VK_F10, true)
                }
            ),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(
                f10_flags,
                KeyInput {
                    win_down: true,
                    ..key(VK_F10, true)
                }
            ),
            FilterDecision::Pass
        );

        // Bare F10 down triggers once; repeats and matching up are swallowed
        assert_eq!(
            state.evaluate(f10_flags, key(VK_F10, true)),
            FilterDecision::TriggerEscape
        );
        assert_eq!(
            state.evaluate(f10_flags, key(VK_F10, true)),
            FilterDecision::Swallow
        );
        assert_eq!(
            state.evaluate(f10_flags, key(VK_F10, false)),
            FilterDecision::Swallow
        );
        // Pressing F10 again while still exiting stays swallowed
        assert_eq!(
            state.evaluate(f10_flags, key(VK_F10, true)),
            FilterDecision::Swallow
        );

        // Reset clears exiting flag and swallowed bitmask; Shift+F10 also triggers bare F10
        state.reset();
        assert_eq!(
            state.evaluate(
                f10_flags,
                KeyInput {
                    shift_down: true,
                    ..key(VK_F10, true)
                }
            ),
            FilterDecision::TriggerEscape
        );
        assert_eq!(
            state.evaluate(f10_flags, key(VK_F10, false)),
            FilterDecision::Swallow
        );

        // Combo Ctrl+Alt+Tab takes precedence over Alt+Tab swallow
        state.reset();
        let combo_escape = EscapeKey {
            vk: VK_TAB,
            modifiers: MOD_CTRL | MOD_ALT,
            label: "Ctrl+Alt+Tab".into(),
        };
        let combo_flags = FilterFlags::from_escape_key(true, true, true, true, &combo_escape);

        // Alt+Tab without Ctrl is still swallowed by block_minimize
        assert_eq!(
            state.evaluate(
                combo_flags,
                KeyInput {
                    alt_down: true,
                    ..key(VK_TAB, true)
                }
            ),
            FilterDecision::Swallow
        );
        assert_eq!(
            state.evaluate(combo_flags, key(VK_TAB, false)),
            FilterDecision::Swallow
        );

        // Ctrl+Alt+Tab triggers escape instead of swallowing as Alt+Tab
        assert_eq!(
            state.evaluate(
                combo_flags,
                KeyInput {
                    ctrl_down: true,
                    alt_down: true,
                    ..key(VK_TAB, true)
                }
            ),
            FilterDecision::TriggerEscape
        );
        // Repeat while exiting is swallowed
        assert_eq!(
            state.evaluate(
                combo_flags,
                KeyInput {
                    ctrl_down: true,
                    alt_down: true,
                    ..key(VK_TAB, true)
                }
            ),
            FilterDecision::Swallow
        );
        // Tab up after modifiers released is still swallowed
        assert_eq!(
            state.evaluate(combo_flags, key(VK_TAB, false)),
            FilterDecision::Swallow
        );

        // Extra unexpected modifiers (Shift or Win) do not match Ctrl+Alt+Tab escape
        state.reset();
        assert_eq!(
            state.evaluate(
                combo_flags,
                KeyInput {
                    ctrl_down: true,
                    alt_down: true,
                    shift_down: true,
                    ..key(VK_TAB, true)
                }
            ),
            FilterDecision::Swallow
        );
        assert_eq!(
            state.evaluate(combo_flags, key(VK_TAB, false)),
            FilterDecision::Swallow
        );
        assert_eq!(
            state.evaluate(
                combo_flags,
                KeyInput {
                    ctrl_down: true,
                    alt_down: true,
                    win_down: true,
                    ..key(VK_TAB, true)
                }
            ),
            FilterDecision::Swallow
        );
    }

    #[test]
    fn handles_boundary_vk_codes_and_passed_keydown_symmetry() {
        let mut state = KeyFilterState::default();
        assert_eq!(state, KeyFilterState::new());
        let flags = default_flags();

        // Out-of-range VK codes never panic
        assert_eq!(
            state.evaluate(flags, key(0x100, true)),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(flags, key(0x100, false)),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(flags, key(u32::MAX, true)),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(flags, key(u32::MAX, false)),
            FilterDecision::Pass
        );

        // If Alt is released mid-hold and Tab auto-repeats as a normal Pass key-down,
        // the subsequent Tab key-up must also Pass (never leave Tab stuck down).
        assert_eq!(
            state.evaluate(
                flags,
                KeyInput {
                    alt_down: true,
                    ..key(VK_TAB, true)
                }
            ),
            FilterDecision::Swallow
        );
        assert_eq!(
            state.evaluate(flags, key(VK_TAB, true)),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(flags, key(VK_TAB, false)),
            FilterDecision::Pass
        );
    }

    #[test]
    fn blocks_caps_lock_down_and_up_when_enabled_and_passes_when_disabled() {
        let mut state = KeyFilterState::new();
        let flags = default_flags();

        // When block_caps_lock is true, Caps Lock down (bare or with modifiers) and up are swallowed
        assert_eq!(
            state.evaluate(flags, key(VK_CAPITAL, true)),
            FilterDecision::Swallow
        );
        assert_eq!(
            state.evaluate(flags, key(VK_CAPITAL, false)),
            FilterDecision::Swallow
        );
        // Subsequent unmatched key-up passes cleanly
        assert_eq!(
            state.evaluate(flags, key(VK_CAPITAL, false)),
            FilterDecision::Pass
        );

        // Shift+CapsLock or Ctrl+CapsLock is also swallowed
        assert_eq!(
            state.evaluate(
                flags,
                KeyInput {
                    shift_down: true,
                    ..key(VK_CAPITAL, true)
                }
            ),
            FilterDecision::Swallow
        );
        assert_eq!(
            state.evaluate(flags, key(VK_CAPITAL, false)),
            FilterDecision::Swallow
        );

        // When disabled, Caps Lock passes even if other guards are on, unless a pending up remains
        let no_caps_flags = FilterFlags {
            block_caps_lock: false,
            ..flags
        };
        assert_eq!(
            state.evaluate(no_caps_flags, key(VK_CAPITAL, true)),
            FilterDecision::Pass
        );
        assert_eq!(
            state.evaluate(no_caps_flags, key(VK_CAPITAL, false)),
            FilterDecision::Pass
        );

        // Key-up symmetry across live toggle off or reset()
        assert_eq!(
            state.evaluate(flags, key(VK_CAPITAL, true)),
            FilterDecision::Swallow
        );
        // Live toggle off before key-up: matching key-up is still swallowed for symmetry
        assert_eq!(
            state.evaluate(no_caps_flags, key(VK_CAPITAL, false)),
            FilterDecision::Swallow
        );

        // reset() clears pending swallowed Caps Lock key-up
        assert_eq!(
            state.evaluate(flags, key(VK_CAPITAL, true)),
            FilterDecision::Swallow
        );
        state.reset();
        assert_eq!(
            state.evaluate(flags, key(VK_CAPITAL, false)),
            FilterDecision::Pass
        );

        // EscapeKey precedence remains first (e.g., Ctrl+CapsLock as EscapeKey)
        let caps_escape = EscapeKey {
            vk: VK_CAPITAL,
            modifiers: MOD_CTRL,
            label: "Ctrl+CapsLock".into(),
        };
        let escape_flags = FilterFlags::from_escape_key(true, true, true, true, &caps_escape);
        assert_eq!(
            state.evaluate(
                escape_flags,
                KeyInput {
                    ctrl_down: true,
                    ..key(VK_CAPITAL, true)
                }
            ),
            FilterDecision::TriggerEscape
        );
        assert_eq!(
            state.evaluate(escape_flags, key(VK_CAPITAL, false)),
            FilterDecision::Swallow
        );
    }
}
