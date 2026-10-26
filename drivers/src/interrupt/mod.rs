//! Interrupt controllers: route device interrupts to a CPU, report which one is pending and
//! take the end-of-interrupt.

pub mod gicv2;
pub mod i8259;
pub mod io_apic;
pub mod local_apic;
pub mod plic;

pub use gicv2::{Gicv2CpuInterface, Gicv2Distributor};
pub use i8259::LegacyPic;
pub use io_apic::IoApic;
pub use local_apic::LocalApic;
pub use plic::Plic;
