//! winmm (D178): EggOS has no joystick and no Media Control Interface device yet, so
//! `joyGetPosEx` finds every joystick unplugged, `joyGetDevCapsW` finds no joystick driver
//! and `mciSendStringW` finds no device installed.

use crate::library::Export;

const MMSYSERR_NODRIVER: u32 = 6;
const MMSYSERR_INVALPARAM: u32 = 11;
const JOYERR_PARMS: u32 = 165;
const JOYERR_UNPLUGGED: u32 = 167;
const MCIERR_DEVICE_NOT_INSTALLED: u32 = 256 + 50;
const JOYSTICKS: u32 = 16;

extern "efiapi" fn joy_get_pos_ex(joystick: u32, information: *mut u8) -> u32 {
    match (joystick < JOYSTICKS, information.is_null()) {
        (false, _) => JOYERR_PARMS,
        (true, true) => MMSYSERR_INVALPARAM,
        (true, false) => JOYERR_UNPLUGGED,
    }
}

extern "efiapi" fn joy_get_dev_caps_w(_joystick: usize, _capabilities: *mut u8, _size: u32) -> u32 {
    MMSYSERR_NODRIVER
}

unsafe extern "efiapi" fn mci_send_string_w(
    _command: *const u16,
    answer: *mut u16,
    answer_length: u32,
    _callback: usize,
) -> u32 {
    if !answer.is_null() && answer_length > 0 {
        // SAFETY: `mciSendStringW`'s contract: `answer_length` writable units.
        unsafe { answer.write(0) };
    }
    MCIERR_DEVICE_NOT_INSTALLED
}

pub(super) const EXPORTS: &[Export] = exports![
    b"joyGetPosEx" => joy_get_pos_ex,
    b"joyGetDevCapsW" => joy_get_dev_caps_w,
    b"mciSendStringW" => mci_send_string_w,
];
