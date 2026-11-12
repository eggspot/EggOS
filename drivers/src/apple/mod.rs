//! Apple silicon's coprocessors (D543): the mailbox each one is reached through and RTKit, the
//! protocol their firmware speaks. The display coprocessor (DCP), the system management
//! controller and the NVMe controller's ANS all build on these.

pub mod mailbox;
pub mod rtkit;
