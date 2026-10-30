//! What a key pressed in a terminal window does (docs/ux/terminal-windows-v0.3.md sections 2
//! and 4, docs/ux/terminal-v0.3.md "Keyboard"): the bytes it sends the shell, through the
//! console terminal's keymap ([`eggos_terminal::keymap`]) for keys that type no text and as
//! the character the keyboard layout typed (D185) for those that do; a move of the scrollback
//! view; or one of the window's own shortcuts.

use eggos_terminal::ViewMove;
use eggos_terminal::key;
use eggos_terminal::keymap::{self, CursorKeys, KeyBytes, Modifiers as KeyModifiers};
use eggos_toolkit::input::Modifiers;

/// The window server's bit for Caps Lock in a key event's modifiers.
pub const CAPS_LOCK: u32 = 16;

const ESC: u8 = 0x1b;

/// What a key press in a terminal window does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// Sends these bytes to the shell; the view goes back to the live screen.
    Send(KeyBytes),
    /// Moves the scrollback view (Shift with Page Up, Page Down, Home or End).
    View(ViewMove),
    /// Opens another terminal window (Ctrl+Shift+N, Cmd+N).
    NewWindow,
    /// Asks to close the window (Ctrl+Shift+W, Super+W).
    Close,
    /// Nothing.
    Nothing,
}

/// The keymap's modifiers for a key event's: the window server's bits (Shift, Ctrl, Alt, Super,
/// Caps Lock, AltGr) in the keymap's order.
pub fn key_modifiers(modifiers: Modifiers) -> KeyModifiers {
    let pairs = [
        (Modifiers::SHIFT, KeyModifiers::SHIFT),
        (Modifiers::CTRL, KeyModifiers::CTRL),
        (Modifiers::ALT, KeyModifiers::ALT),
        (Modifiers::SUPER, KeyModifiers::SUPER),
        (CAPS_LOCK, KeyModifiers::CAPS_LOCK),
        (Modifiers::ALT_GRAPH, KeyModifiers::ALT_GRAPH),
    ];
    pairs
        .into_iter()
        .filter(|&(bit, _)| modifiers.0 & bit != 0)
        .fold(KeyModifiers::NONE, |mapped, (_, modifier)| {
            mapped.with(modifier)
        })
}

/// What pressing key `code` with `modifiers` does, `character` being what the key typed in the
/// keyboard's layout (0 for none, as for a dead key), with the arrows sent in `cursor_keys`'
/// form. A key that types text (code 0 is a character on its own, D185) sends that character
/// as UTF-8, after `ESC` with Alt (not AltGr); with Ctrl, and for the other keys, the keymap
/// encodes it.
pub fn key_action(
    code: u16,
    modifiers: Modifiers,
    character: u32,
    cursor_keys: CursorKeys,
) -> KeyAction {
    let held = |bit: u32| modifiers.0 & bit != 0;
    let (shift, ctrl, super_key) = (
        held(Modifiers::SHIFT),
        held(Modifiers::CTRL),
        held(Modifiers::SUPER),
    );
    let alt = held(Modifiers::ALT) && !held(Modifiers::ALT_GRAPH);
    let window_shortcut = (ctrl && shift && !alt) || (super_key && !ctrl);
    match code {
        key::N if window_shortcut => return KeyAction::NewWindow,
        key::W if window_shortcut => return KeyAction::Close,
        _ => {}
    }
    if shift && !ctrl && !alt && !super_key {
        let view = match code {
            key::PAGE_UP => Some(ViewMove::PageBack),
            key::PAGE_DOWN => Some(ViewMove::PageForward),
            key::HOME => Some(ViewMove::Oldest),
            key::END => Some(ViewMove::Live),
            _ => None,
        };
        if let Some(view) = view {
            return KeyAction::View(view);
        }
    }
    let types_text = code == 0 || keymap::printable(code).is_some();
    if types_text && !ctrl && !super_key {
        let Some(typed) = char::from_u32(character).filter(|typed| !typed.is_control()) else {
            return KeyAction::Nothing;
        };
        let mut bytes = KeyBytes::EMPTY;
        if alt {
            bytes.push_byte(ESC);
        }
        let mut encoded = [0u8; 4];
        for &byte in typed.encode_utf8(&mut encoded).as_bytes() {
            bytes.push_byte(byte);
        }
        return KeyAction::Send(bytes);
    }
    match keymap::encode(code, key_modifiers(modifiers), cursor_keys) {
        bytes if bytes.is_empty() => KeyAction::Nothing,
        bytes => KeyAction::Send(bytes),
    }
}
