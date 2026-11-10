//! Intel's Wi-Fi 6 and 6E adapters (AX200, AX201, AX210, AX211; D272), written with OpenBSD's
//! `iwx(4)` driver as the reference, under the BSD terms its notices carry (`NOTICE.md` beside
//! this file): which adapter a PCI function is and the firmware it runs ([`devices`]), the
//! firmware file's sections and capabilities ([`firmware`]), the host interface that powers the
//! adapter up and hands it the firmware and its commands ([`transport`]), and what its NVM says
//! ([`nvm`]), readying the firmware and scanning ([`scan`]), and joining a network and sending
//! frames to it ([`station`]).

pub mod devices;
pub mod firmware;
pub mod nvm;
pub mod scan;
pub mod station;
pub mod transport;
