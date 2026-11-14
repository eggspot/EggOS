use crate::system::System;
use core::ffi::CStr;
use eggos_calendar::zone::{self, UTC, Zone};
use eggos_settings_file::TIME_ZONE;

pub const SETTINGS: &CStr = c"/home/root/.config/eggos/settings";
const SETTINGS_BYTES: usize = 4096;
const TZIF_BYTES: usize = 16 * 1024;

fn read_file<S: System>(system: &mut S, path: &CStr, out: &mut [u8]) -> Option<usize> {
    let descriptor = system.open(path).ok()?;
    let mut length = 0;
    while let Some(free) = out.get_mut(length..).filter(|free| !free.is_empty()) {
        match system.read(descriptor, free) {
            Ok(0) | Err(_) => break,
            Ok(count) => length += count,
        }
    }
    system.close(descriptor);
    Some(length)
}

fn zone_named<S: System>(system: &mut S, text: &str) -> Option<Zone> {
    let rule = zone::zone(text)?;
    let mut path = [0u8; 64];
    let Some(path) = zone::tzif_path(text, &mut path).and_then(|path| CStr::from_bytes_with_nul(path).ok()) else {
        return Some(Zone::of_rule(rule));
    };
    let mut file = [0u8; TZIF_BYTES];
    match read_file(system, path, &mut file) {
        Some(length) if length > 0 => Some(Zone::with_history(rule, &file[..length])),
        _ => Some(Zone::of_rule(rule)),
    }
}

pub fn local_zone<S: System>(system: &mut S) -> Zone {
    if let Some(variable) = system.time_zone_variable() {
        let mut text = [0u8; 64];
        let length = variable.len().min(text.len());
        text[..length].copy_from_slice(&variable[..length]);
        return core::str::from_utf8(&text[..length])
            .ok()
            .filter(|text| !text.is_empty())
            .and_then(|text| zone_named(system, text))
            .unwrap_or(Zone::of_rule(UTC));
    }
    let mut file = [0u8; SETTINGS_BYTES];
    let Some(length) = read_file(system, SETTINGS, &mut file) else {
        return Zone::of_rule(UTC);
    };
    let mut name = [0u8; 64];
    let Some(value) = eggos_settings_file::value(&file[..length], TIME_ZONE) else {
        return Zone::of_rule(UTC);
    };
    let name_length = value.len().min(name.len());
    name[..name_length].copy_from_slice(&value[..name_length]);
    core::str::from_utf8(&name[..name_length])
        .ok()
        .and_then(|text| zone_named(system, text))
        .unwrap_or(Zone::of_rule(UTC))
}

pub fn local_seconds(zone: &Zone, utc: i64) -> i64 {
    utc.saturating_add(zone.at(utc).offset)
}
