use crate::system::System;
use core::ffi::CStr;
use eggos_calendar::zone::{self, Rule, UTC};
use eggos_settings_file::TIME_ZONE;

pub const SETTINGS: &CStr = c"/home/root/.config/eggos/settings";
const SETTINGS_BYTES: usize = 4096;

pub fn local_zone<S: System>(system: &mut S) -> Rule {
    if let Some(variable) = system.time_zone_variable() {
        return core::str::from_utf8(variable)
            .ok()
            .filter(|text| !text.is_empty())
            .and_then(zone::zone)
            .unwrap_or(UTC);
    }
    let Ok(descriptor) = system.open(SETTINGS) else {
        return UTC;
    };
    let mut file = [0u8; SETTINGS_BYTES];
    let mut length = 0;
    while let Some(free) = file.get_mut(length..).filter(|free| !free.is_empty()) {
        match system.read(descriptor, free) {
            Ok(0) | Err(_) => break,
            Ok(count) => length += count,
        }
    }
    system.close(descriptor);
    eggos_settings_file::value(&file[..length], TIME_ZONE)
        .and_then(|value| core::str::from_utf8(value).ok())
        .and_then(zone::zone)
        .unwrap_or(UTC)
}

pub fn local_seconds(zone: &Rule, utc: i64) -> i64 {
    utc + zone.at(utc).offset
}
