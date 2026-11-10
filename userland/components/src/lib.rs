//! The component manager, part 1 (D18, D238, docs/components.md): the component store's format
//! ([`index`]), finding the store on an attached disk or in the host share, the installed
//! components' records, installing a component atomically with every file checked against the
//! store's SHA-256, and removing one that nothing needs and nothing runs ([`manager`]); every
//! sentence the three front-ends say ([`messages`]); and the `component` command
//! ([`command`]), which `/bin/settings` runs when started as `/bin/component`. Everything but the
//! system calls is host-tested against a fake file system.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod command;
pub mod index;
pub mod manager;
pub mod messages;
#[cfg(eggos_user)]
pub mod program;

#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;
