//! `soundd`, EggOS's sound server, and `sound`, its command (D774, docs/ux/sound-v0.1.md
//! sections 1, 5 and 6): the server owns the output's volume and mute (the six `sound.*`
//! settings), mixes every stream programs play into the one output (`/dev/sound`, D773) and
//! answers on `/run/soundd` in lines of text ([`protocol`]); `sound` is its CLI. Everything but
//! the system calls is host-tested.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod command;
#[cfg(eggos_user)]
pub mod linux;
pub mod mixer;
pub mod protocol;
