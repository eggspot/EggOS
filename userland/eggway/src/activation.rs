//! Raising windows on request (`xdg_activation_v1`, D176): a client asks for a token, which
//! it may pass on (to a program it starts, in `XDG_ACTIVATION_TOKEN`), and a token given back
//! with a surface raises and focuses that surface's window, as if the user had picked it. A
//! token works once, for [`TOKEN_LIFETIME_MS`], and only if the client that asked for it had
//! the keyboard focus when it did: a program in the background cannot take the focus from what
//! the user is doing.

use crate::client::ClientSlot;
use crate::compositor::{Message, Shared, new_object, root_of};
use crate::system::System;
use eggos_wayland::protocol::{Interface, activation_token};
use eggos_wayland::wire::Value;

/// Tokens waiting to be used, from every client together; a new one replaces the oldest.
pub const MAX_TOKENS: usize = 8;
/// How long a token works after it is given.
pub const TOKEN_LIFETIME_MS: u64 = 10_000;
/// What every token starts with; 16 hexadecimal digits follow.
pub const TOKEN_PREFIX: &[u8] = b"eggway-";
/// A token's length.
pub const TOKEN_BYTES: usize = TOKEN_PREFIX.len() + 16;

const COMMITTED: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Issued {
    value: u64,
    issued_ms: u64,
    from_focused: bool,
}

/// The tokens given and not used yet.
#[derive(Debug, Default)]
pub struct Activation {
    tokens: [Option<Issued>; MAX_TOKENS],
    count: u64,
}

fn mixed(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// `value` as a token's text.
pub fn token_text(value: u64) -> [u8; TOKEN_BYTES] {
    let mut text = [0u8; TOKEN_BYTES];
    text[..TOKEN_PREFIX.len()].copy_from_slice(TOKEN_PREFIX);
    for (at, digit) in text[TOKEN_PREFIX.len()..].iter_mut().enumerate() {
        let nibble = (value >> (60 - 4 * at)) & 0xf;
        *digit = b"0123456789abcdef"[nibble as usize];
    }
    text
}

/// The value a token's `text` stands for; `None` for text no token of ours has.
pub fn token_value(text: &[u8]) -> Option<u64> {
    let digits = text.strip_prefix(TOKEN_PREFIX)?;
    if digits.len() != 16 {
        return None;
    }
    digits.iter().try_fold(0u64, |value, &digit| {
        let nibble = match digit {
            b'0'..=b'9' => digit - b'0',
            b'a'..=b'f' => digit - b'a' + 10,
            _ => return None,
        };
        Some(value << 4 | u64::from(nibble))
    })
}

impl Activation {
    /// Gives a new token at `now_ms`, remembering whether its client had the keyboard focus.
    pub fn issue(&mut self, now_ms: u64, from_focused: bool) -> u64 {
        self.count = self.count.wrapping_add(1);
        let value = mixed(self.count ^ now_ms.rotate_left(32));
        let issued = Issued {
            value,
            issued_ms: now_ms,
            from_focused,
        };
        let slot = match self.tokens.iter().position(Option::is_none) {
            Some(free) => free,
            None => self
                .tokens
                .iter()
                .enumerate()
                .min_by_key(|(_, token)| token.map_or(0, |token| token.issued_ms))
                .map_or(0, |(oldest, _)| oldest),
        };
        self.tokens[slot] = Some(issued);
        value
    }

    /// Uses token `value` at `now_ms`: whether it may raise a window. A token is used once,
    /// whatever the answer.
    pub fn redeem(&mut self, value: u64, now_ms: u64) -> bool {
        let Some(slot) = self
            .tokens
            .iter_mut()
            .find(|token| token.is_some_and(|token| token.value == value))
        else {
            return false;
        };
        let Some(token) = slot.take() else {
            return false;
        };
        token.from_focused && now_ms.saturating_sub(token.issued_ms) <= TOKEN_LIFETIME_MS
    }
}

/// `xdg_activation_v1`'s requests: a new token object, or `activate`, which raises and
/// focuses the surface's window when the token allows it (and does nothing otherwise, as the
/// protocol lets the compositor choose).
pub fn activation_request<S: System>(
    client: &mut ClientSlot,
    shared: &mut Shared,
    system: &mut S,
    message: &Message,
) {
    match message.opcode {
        1 => {
            new_object(
                client,
                message.uint(0),
                Interface::ActivationToken,
                message.version,
                0,
            );
        }
        2 => {
            let allowed = token_value(message.values[0].bytes())
                .is_some_and(|value| shared.activation.redeem(value, system.now_ms()));
            let Some(surface) = client
                .object(message.uint(1))
                .filter(|object| object.interface == Some(Interface::Surface))
                .map(|object| object.index)
            else {
                return;
            };
            let window = root_of(client, surface)
                .and_then(|root| client.surface(root).and_then(|root| root.window));
            if let (true, Some(window), Some(device)) = (allowed, window, client.device) {
                let _ = system.focus_window(device, window.id);
            }
        }
        _ => {}
    }
}

/// A token object's requests: its serial, app id and surface say where the request came from,
/// which EggOS judges by the keyboard focus instead; `commit` gives the token (once).
pub fn token_request<S: System>(
    client: &mut ClientSlot,
    shared: &mut Shared,
    system: &mut S,
    message: &Message,
) {
    if message.opcode != 3 {
        return;
    }
    if message.index == COMMITTED {
        client.protocol_error(
            message.object,
            activation_token::ALREADY_USED,
            b"token already committed",
        );
        return;
    }
    client.create(
        message.object,
        Interface::ActivationToken,
        message.version,
        COMMITTED,
    );
    let focused = shared.keyboard_client == Some(client.index);
    let value = shared.activation.issue(system.now_ms(), focused);
    client.post(
        message.object,
        message.version,
        activation_token::DONE,
        &[Value::String(&token_text(value))],
    );
}
