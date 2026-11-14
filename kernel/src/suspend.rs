//! Suspend to RAM, ACPI's S3 (D597): `/sys/power/state` takes `mem`. The boot CPU runs it: the
//! firmware prepares (`\_PTS(3)`), every other CPU leaves service and is held, the HAL enters
//! S3 and returns once the machine woke, the held CPUs start again and come back into service,
//! the device lines are routed again, and the firmware is told (`\_WAK(3)`).

use crate::cpu::{self, Cpu};
use crate::devices::{acpi_namespace, interrupts};
use crate::thread;
use alloc::vec::Vec;
use core::time::Duration;
use eggos_hal::Platform;

/// The sleep state suspend to RAM is.
pub const SUSPEND_TO_RAM: u64 = 3;
const WAIT_STEP: Duration = Duration::from_millis(1);
const MOST_WAIT_STEPS: u32 = 2000;

fn wait_for<P: Platform>(mut done: impl FnMut() -> bool) -> bool {
    for _ in 0..MOST_WAIT_STEPS {
        if done() {
            return true;
        }
        thread::sleep::<P>(WAIT_STEP);
    }
    done()
}

/// Puts the machine in suspend to RAM and returns once it woke; `Err` with why when it could
/// not, every CPU then back in service.
pub fn suspend_to_ram<P: Platform>() -> Result<(), &'static str> {
    let cpus = cpu::CPUS.all();
    let Some(&boot) = cpus.first() else {
        return P::suspend_to_ram();
    };
    if !core::ptr::eq(cpu::current::<P>(), boot) {
        thread::set_running_affinity::<P>(1);
    }
    if !core::ptr::eq(cpu::current::<P>(), boot) {
        return Err("suspend runs on the boot CPU, which this thread could not reach");
    }
    let others: Vec<&'static Cpu> = cpus
        .iter()
        .copied()
        .skip(1)
        .filter(|cpu| cpu.is_online())
        .collect();
    acpi_namespace::prepare_to_sleep::<P>(SUSPEND_TO_RAM);
    let result = hold_others_and_sleep::<P>(&others);
    for &cpu in &others {
        if P::is_cpu_held(cpu.hardware_id()) {
            let _ = P::wake_held_cpu(cpu.hardware_id());
        }
        thread::bring_cpu_online::<P>(cpu);
    }
    if result.is_ok() {
        interrupts::route_again::<P>();
    }
    acpi_namespace::woke_from::<P>(SUSPEND_TO_RAM);
    result
}

fn hold_others_and_sleep<P: Platform>(others: &[&'static Cpu]) -> Result<(), &'static str> {
    for &cpu in others {
        thread::take_cpu_offline::<P>(cpu)?;
    }
    if !wait_for::<P>(|| others.iter().all(|cpu| thread::is_cpu_parked(cpu))) {
        return Err("a CPU did not leave service");
    }
    for &cpu in others {
        thread::hold_parked_cpu::<P>(cpu);
    }
    if !wait_for::<P>(|| others.iter().all(|cpu| P::is_cpu_held(cpu.hardware_id()))) {
        return Err("a CPU did not save its context");
    }
    P::suspend_to_ram()
}

/// Whether the machine offers suspend to RAM, for `/sys/power/state`'s text.
pub fn offered<P: Platform>() -> bool {
    P::acpi_dsdt().is_some_and(|dsdt| {
        eggos_hal::acpi::find_sleep_type(
            dsdt.get(eggos_hal::acpi::TABLE_HEADER_LENGTH..).unwrap_or_default(),
            b"_S3_",
        )
        .is_some()
    })
}
