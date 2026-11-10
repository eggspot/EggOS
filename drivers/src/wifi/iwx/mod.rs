//! Intel's Wi-Fi 6 and 6E adapters (AX200, AX201, AX210, AX211; D272), written with OpenBSD's
//! `iwx(4)` driver as the reference, under the BSD terms its notices carry (`NOTICE` beside this
//! file): which adapter a PCI function is and the firmware it runs ([`devices`]), and the
//! firmware file's sections and capabilities ([`firmware`]).

pub mod devices;
pub mod firmware;
