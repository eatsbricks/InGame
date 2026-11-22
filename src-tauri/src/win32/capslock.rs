/// Magic `dwExtraInfo` tag attached to our single scoped `SendInput` when turning off
/// an already-active Caps Lock on entering PUBG ("PUBGCAPS" in ASCII).
pub const CAPS_LOCK_AUTO_OFF_EXTRA_INFO: usize = 0x5055_4247_4341_5053_u64 as usize;

#[cfg(windows)]
pub fn is_caps_lock_on() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CAPITAL};

    // SAFETY: `GetKeyState` queries the thread/system toggle bit for `VK_CAPITAL` with no preconditions.
    let state = unsafe { GetKeyState(i32::from(VK_CAPITAL.0)) };
    (state & 0x0001) != 0
}

#[cfg(not(windows))]
pub fn is_caps_lock_on() -> bool {
    false
}

#[cfg(windows)]
fn send_caps_lock_toggle() -> bool {
    use std::mem::size_of;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VK_CAPITAL,
    };

    let inputs = [
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_CAPITAL,
                    wScan: 0,
                    dwFlags: KEYBD_EVENT_FLAGS(0),
                    time: 0,
                    dwExtraInfo: CAPS_LOCK_AUTO_OFF_EXTRA_INFO,
                },
            },
        },
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_CAPITAL,
                    wScan: 0,
                    dwFlags: KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: CAPS_LOCK_AUTO_OFF_EXTRA_INFO,
                },
            },
        },
    ];

    // SAFETY: `inputs` is a valid 2-element array of `INPUT` structs with matching `cbSize`.
    let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    sent == inputs.len() as u32
}

#[cfg(not(windows))]
fn send_caps_lock_toggle() -> bool {
    false
}

fn turn_off_caps_lock_with<FIsOn, FSend>(is_on: FIsOn, send_toggle: FSend) -> bool
where
    FIsOn: FnOnce() -> bool,
    FSend: FnOnce() -> bool,
{
    if !is_on() {
        return false;
    }

    send_toggle()
}

pub fn turn_off_caps_lock_if_on() -> bool {
    turn_off_caps_lock_with(is_caps_lock_on, send_caps_lock_toggle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_off_caps_lock_with_is_noop_when_off_and_sends_once_when_on() {
        let mut send_count = 0usize;
        let turned_off_when_already_off = turn_off_caps_lock_with(
            || false,
            || {
                send_count += 1;
                true
            },
        );
        assert!(!turned_off_when_already_off);
        assert_eq!(send_count, 0);

        let turned_off_when_on = turn_off_caps_lock_with(
            || true,
            || {
                send_count += 1;
                true
            },
        );
        assert!(turned_off_when_on);
        assert_eq!(send_count, 1);

        // If SendInput fails, turn_off_caps_lock_with propagates false.
        let failed_send = turn_off_caps_lock_with(|| true, || false);
        assert!(!failed_send);
    }
}
