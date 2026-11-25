//! winmm (D178, D776): EggOS has no joystick and no Media Control Interface device yet, so
//! `joyGetPosEx` finds every joystick unplugged, `joyGetDevCapsW` finds no joystick driver
//! and `mciSendStringW` finds no device installed. Sound goes to the sound server:
//! `PlaySound` and `sndPlaySound` play a WAV file, resource or memory image (synchronously, or
//! on a thread of their own with `SND_ASYNC`, looping with `SND_LOOP`, the next call or a null
//! name stopping it), and `waveOut` is one device (`EggOS sound`) whose queued headers are
//! handed over in order by the device's own thread, each marked done and reported
//! (`WOM_DONE` by function, window message or event) once its time has played.

use super::call_program_nine;
use super::{Bool, FALSE, TRUE};
use crate::library::Export;
use crate::text;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use eggos_soundd::client::{self, Player};
use eggos_soundd::mixer::to_sixteen_bits;
use eggos_soundd::wave::{SampleFormat, read_wave};

const MMSYSERR_NODRIVER: u32 = 6;
const MMSYSERR_INVALPARAM: u32 = 11;
const JOYERR_PARMS: u32 = 165;
const JOYERR_UNPLUGGED: u32 = 167;
const MCIERR_DEVICE_NOT_INSTALLED: u32 = 256 + 50;
const JOYSTICKS: u32 = 16;
const SND_ASYNC: u32 = 0x1;
const SND_MEMORY: u32 = 0x4;
const SND_LOOP: u32 = 0x8;
const SND_NOSTOP: u32 = 0x10;
const SND_ALIAS: u32 = 0x0001_0000;
const SND_FILENAME: u32 = 0x0002_0000;
const SND_RESOURCE_FLAG: u32 = 0x0004_0000;
const MMSYSERR_NOERROR: u32 = 0;
const MMSYSERR_BADDEVICEID: u32 = 2;
const MMSYSERR_INVALHANDLE: u32 = 5;
const MMSYSERR_NOMEM: u32 = 7;
const WAVERR_BADFORMAT: u32 = 32;
const WAVERR_STILLPLAYING: u32 = 33;
const WAVERR_UNPREPARED: u32 = 34;
const WAVE_MAPPER: u32 = 0xffff_ffff;
const WAVE_FORMAT_QUERY: u32 = 0x1;
const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xfffe;
const CALLBACK_TYPE: u32 = 0x0007_0000;
const CALLBACK_WINDOW: u32 = 0x0001_0000;
const CALLBACK_FUNCTION: u32 = 0x0003_0000;
const CALLBACK_EVENT: u32 = 0x0005_0000;
const WOM_OPEN: u32 = 0x3bb;
const WOM_CLOSE: u32 = 0x3bc;
const WOM_DONE: u32 = 0x3bd;
const WHDR_DONE: u32 = 0x1;
const WHDR_PREPARED: u32 = 0x2;
const WHDR_INQUEUE: u32 = 0x10;
const HEADER_DATA: usize = 0;
const HEADER_LENGTH: usize = 8;
const HEADER_FLAGS: usize = 24;
const CAPS_BYTES: usize = 84;
const TIME_BYTES: u32 = 4;
const PUMP_NS: u64 = 10_000_000;
const DEVICE_NAME: &str = "EggOS sound";
const APP: &str = "windows-apps";
const WAVE_TYPE: [u16; 4] = [b'W' as u16, b'A' as u16, b'V' as u16, b'E' as u16];

static PLAYING: AtomicU32 = AtomicU32::new(0);
static GENERATION: AtomicU32 = AtomicU32::new(0);

fn now_ms() -> u64 {
    crate::linux::clock_nanoseconds(1) / 1_000_000
}

fn riff_length(address: usize) -> usize {
    // SAFETY: `SND_MEMORY`'s contract: a whole WAV file, which starts with its RIFF header.
    let header = unsafe { core::slice::from_raw_parts(address as *const u8, 8) };
    u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize + 8
}

fn file_bytes(path: &[u8]) -> Option<Vec<u8>> {
    let fd = crate::linux::open(path, crate::linux::O_RDONLY).ok()?;
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match crate::linux::read(fd, &mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => bytes.extend_from_slice(&chunk[..read]),
        }
    }
    let _ = crate::linux::close(fd);
    Some(bytes)
}

fn sound_bytes(name: usize, module: usize, flags: u32, wide: bool) -> Option<Vec<u8>> {
    if flags & SND_MEMORY != 0 {
        let length = riff_length(name);
        // SAFETY: `SND_MEMORY`'s contract: `length` readable bytes of the program's WAV file.
        let bytes = unsafe { core::slice::from_raw_parts(name as *const u8, length) };
        return Some(bytes.to_vec());
    }
    if flags & SND_RESOURCE_FLAG == SND_RESOURCE_FLAG && wide {
        // SAFETY: `PlaySoundW`'s contract: an id or a NUL-terminated name.
        let wanted = unsafe { super::resource::name_from(name) };
        let (address, size) =
            super::resource::find(module, super::resource::Name::Text(&WAVE_TYPE), wanted)?;
        // SAFETY: `find` answers a range inside the loaded image.
        let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, size as usize) };
        return Some(bytes.to_vec());
    }
    if flags & SND_ALIAS != 0 && flags & SND_FILENAME == 0 {
        return None;
    }
    let mut path = [0u8; text::PATH_CAPACITY];
    let length = match wide {
        true => {
            // SAFETY: `PlaySoundW`'s contract: a NUL-terminated UTF-16 file name.
            let units = unsafe { super::file::wide_string(name as *const u16) };
            text::eggos_path(text::utf16_chars(units), &mut path)?
        }
        false => {
            // SAFETY: `PlaySoundA`'s contract: a NUL-terminated file name.
            let bytes = unsafe { core::ffi::CStr::from_ptr(name as *const core::ffi::c_char) };
            let chars = bytes.to_bytes().iter().map(|&byte| char::from(byte));
            text::eggos_path(chars, &mut path)?
        }
    };
    file_bytes(&path[..length])
}

struct Sound {
    player: Player,
    samples: Vec<u8>,
    duration_ms: u64,
    looping: bool,
    generation: u32,
}

fn prepared(bytes: &[u8]) -> Option<(Player, Vec<u8>, u64)> {
    let wave = read_wave(bytes)?;
    let data = bytes.get(wave.data_offset..wave.data_offset + wave.data_length)?;
    let float = wave.format == SampleFormat::Float;
    let samples = to_sixteen_bits(data, wave.bits, float, 1.0);
    let channels = u8::try_from(wave.channels).ok()?;
    let player = Player::open(wave.sample_rate, channels, APP)?;
    Some((player, samples, (wave.duration() * 1000.0) as u64))
}

fn play_through(sound: &mut Sound) {
    loop {
        let ends = now_ms() + sound.duration_ms;
        sound.player.push(&sound.samples);
        while GENERATION.load(Ordering::Acquire) == sound.generation {
            let handed = sound.player.pump();
            if handed && now_ms() >= ends {
                break;
            }
            crate::linux::sleep_nanoseconds(PUMP_NS);
        }
        if !sound.looping || GENERATION.load(Ordering::Acquire) != sound.generation {
            break;
        }
    }
    let _ = PLAYING.compare_exchange(sound.generation, 0, Ordering::AcqRel, Ordering::Acquire);
}

extern "efiapi" fn sound_thread(parameter: usize) -> u32 {
    // SAFETY: `play_sound` leaked this box for the thread, which alone owns it now.
    let mut sound = unsafe { Box::from_raw(parameter as *mut Sound) };
    play_through(&mut sound);
    0
}

fn play_sound(name: usize, module: usize, flags: u32, wide: bool) -> Bool {
    let generation = GENERATION.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
    if name == 0 {
        return TRUE;
    }
    if flags & SND_NOSTOP != 0 && PLAYING.load(Ordering::Acquire) != 0 {
        return FALSE;
    }
    let Some(bytes) = sound_bytes(name, module, flags, wide) else {
        return match flags & SND_ALIAS != 0 {
            true => TRUE,
            false => FALSE,
        };
    };
    let Some((player, samples, duration_ms)) = prepared(&bytes) else {
        return FALSE;
    };
    let mut sound = Sound {
        player,
        samples,
        duration_ms,
        looping: flags & SND_LOOP != 0 && flags & SND_ASYNC != 0,
        generation,
    };
    PLAYING.store(generation, Ordering::Release);
    if flags & SND_ASYNC == 0 {
        play_through(&mut sound);
        return TRUE;
    }
    let parameter = Box::into_raw(Box::new(sound)) as usize;
    // SAFETY: `sound_thread` takes one argument as a Windows thread procedure does.
    let thread = unsafe {
        super::thread::create_thread(0, 0, sound_thread as usize, parameter, 0, ptr::null_mut())
    };
    if thread == 0 {
        // SAFETY: the thread never started, so the box is still this function's.
        drop(unsafe { Box::from_raw(parameter as *mut Sound) });
        return FALSE;
    }
    TRUE
}

extern "efiapi" fn play_sound_w(name: usize, module: usize, flags: u32) -> Bool {
    play_sound(name, module, flags, true)
}

extern "efiapi" fn play_sound_a(name: usize, module: usize, flags: u32) -> Bool {
    play_sound(name, module, flags, false)
}

extern "efiapi" fn snd_play_sound_w(name: usize, flags: u32) -> Bool {
    play_sound(name, 0, flags, true)
}

extern "efiapi" fn snd_play_sound_a(name: usize, flags: u32) -> Bool {
    play_sound(name, 0, flags, false)
}

#[derive(Clone, Copy)]
struct Format {
    rate: u32,
    channels: u8,
    bits: u16,
    float: bool,
    frame_bytes: u32,
}

fn format_of(address: usize) -> Option<Format> {
    if address == 0 {
        return None;
    }
    // SAFETY: `waveOutOpen`'s contract: a readable `WAVEFORMATEX`, at least 16 bytes.
    let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, 16) };
    let word = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let tag = word(0);
    let channels = word(2);
    let rate = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let bits = word(14);
    let float = match tag {
        WAVE_FORMAT_PCM => false,
        WAVE_FORMAT_IEEE_FLOAT => true,
        WAVE_FORMAT_EXTENSIBLE => {
            // SAFETY: an extensible format carries its 22 extra bytes, the sub-format at 24.
            let extended = unsafe { core::slice::from_raw_parts(address as *const u8, 26) };
            match u16::from_le_bytes([extended[24], extended[25]]) {
                WAVE_FORMAT_PCM => false,
                WAVE_FORMAT_IEEE_FLOAT => true,
                _ => return None,
            }
        }
        _ => return None,
    };
    let known = matches!((bits, float), (8 | 16 | 24 | 32, false) | (32 | 64, true));
    if !known || channels == 0 || channels > 8 || rate == 0 {
        return None;
    }
    Some(Format {
        rate,
        channels: channels as u8,
        bits,
        float,
        frame_bytes: u32::from(channels) * u32::from(bits / 8),
    })
}

struct Queued {
    header: usize,
    ends_ms: u64,
}

struct Device {
    lock: AtomicBool,
    closing: AtomicBool,
    finished: AtomicBool,
    inner: UnsafeCell<Inner>,
}

struct Inner {
    player: Player,
    format: Format,
    callback: (u32, usize, usize),
    queue: Vec<Queued>,
    last_end_ms: u64,
    bytes_played: u64,
    volume: u32,
    paused: bool,
}

// SAFETY: `Inner` is reached only while `lock` is held.
unsafe impl Sync for Device {}

impl Device {
    fn with<R>(&self, work: impl FnOnce(&mut Inner) -> R) -> R {
        while self
            .lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        // SAFETY: the lock is held, so this is the only reference to `inner`.
        let result = work(unsafe { &mut *self.inner.get() });
        self.lock.store(false, Ordering::Release);
        result
    }
}

fn header_word(header: usize, at: usize) -> u32 {
    // SAFETY: `WAVEHDR`'s contract: the program keeps the header alive while it is queued.
    unsafe { ((header + at) as *const u32).read_unaligned() }
}

fn set_header_flags(header: usize, flags: u32) {
    // SAFETY: as in `header_word`; the program reads `dwFlags` only to poll it.
    unsafe { ((header + HEADER_FLAGS) as *mut u32).write_volatile(flags) };
}

fn notify(handle: usize, callback: (u32, usize, usize), message: u32, header: usize) {
    let (kind, target, instance) = callback;
    match kind {
        CALLBACK_FUNCTION if target != 0 => {
            // SAFETY: `waveOutOpen`'s contract: a `waveOutProc` taking these five arguments.
            let _ = unsafe {
                let message = message as usize;
                call_program_nine(target, [handle, message, instance, header, 0, 0, 0, 0, 0])
            };
        }
        CALLBACK_WINDOW if target != 0 => {
            let _ = super::gui::post(target, message, handle, header);
        }
        CALLBACK_EVENT if target != 0 => super::sync::set_event_quietly(target, true),
        _ => {}
    }
}

extern "efiapi" fn device_thread(handle: usize) -> u32 {
    // SAFETY: `waveOutOpen` leaked the device; `waveOutClose` frees it only after `finished`.
    let device = unsafe { &*(handle as *const Device) };
    while !device.closing.load(Ordering::Acquire) {
        let now = now_ms();
        let (done, callback) = device.with(|inner| {
            if !inner.paused {
                inner.player.pump();
            }
            let handed = inner.player.waiting() == 0;
            let mut done = Vec::new();
            while let Some(first) = inner.queue.first() {
                if inner.paused || !handed || now < first.ends_ms {
                    break;
                }
                let header = inner.queue.remove(0).header;
                inner.bytes_played += u64::from(header_word(header, HEADER_LENGTH));
                done.push(header);
            }
            (done, inner.callback)
        });
        for header in done {
            let flags = header_word(header, HEADER_FLAGS);
            set_header_flags(header, (flags & !WHDR_INQUEUE) | WHDR_DONE);
            notify(handle, callback, WOM_DONE, header);
        }
        crate::linux::sleep_nanoseconds(PUMP_NS);
    }
    device.finished.store(true, Ordering::Release);
    0
}

fn device_of(handle: usize) -> Option<&'static Device> {
    // SAFETY: the program passes a handle `waveOutOpen` gave it, alive until `waveOutClose`.
    (handle != 0).then(|| unsafe { &*(handle as *const Device) })
}

unsafe extern "efiapi" fn wave_out_open(
    out: *mut usize,
    device: u32,
    format: usize,
    callback: usize,
    instance: usize,
    flags: u32,
) -> u32 {
    if device != 0 && device != WAVE_MAPPER {
        return MMSYSERR_BADDEVICEID;
    }
    let Some(format) = format_of(format) else {
        return WAVERR_BADFORMAT;
    };
    if flags & WAVE_FORMAT_QUERY != 0 {
        return match client::reachable() {
            true => MMSYSERR_NOERROR,
            false => MMSYSERR_BADDEVICEID,
        };
    }
    let Some(player) = Player::open(format.rate, format.channels, APP) else {
        return MMSYSERR_BADDEVICEID;
    };
    let callback = (flags & CALLBACK_TYPE, callback, instance);
    let device = Box::new(Device {
        lock: AtomicBool::new(false),
        closing: AtomicBool::new(false),
        finished: AtomicBool::new(false),
        inner: UnsafeCell::new(Inner {
            player,
            format,
            callback,
            queue: Vec::new(),
            last_end_ms: 0,
            bytes_played: 0,
            volume: 0xffff_ffff,
            paused: false,
        }),
    });
    let handle = Box::into_raw(device) as usize;
    // SAFETY: `device_thread` takes one argument as a Windows thread procedure does.
    let thread = unsafe {
        super::thread::create_thread(0, 0, device_thread as usize, handle, 0, ptr::null_mut())
    };
    if thread == 0 {
        // SAFETY: no thread holds the device.
        drop(unsafe { Box::from_raw(handle as *mut Device) });
        return MMSYSERR_NOMEM;
    }
    if !out.is_null() {
        // SAFETY: `waveOutOpen`'s contract: a writable handle.
        unsafe { out.write(handle) };
    }
    notify(handle, callback, WOM_OPEN, 0);
    MMSYSERR_NOERROR
}

extern "efiapi" fn wave_out_prepare_header(_handle: usize, header: usize, _size: u32) -> u32 {
    if header == 0 {
        return MMSYSERR_INVALHANDLE;
    }
    let flags = header_word(header, HEADER_FLAGS);
    set_header_flags(header, (flags | WHDR_PREPARED) & !WHDR_DONE);
    MMSYSERR_NOERROR
}

extern "efiapi" fn wave_out_unprepare_header(_handle: usize, header: usize, _size: u32) -> u32 {
    if header == 0 {
        return MMSYSERR_INVALHANDLE;
    }
    let flags = header_word(header, HEADER_FLAGS);
    if flags & WHDR_INQUEUE != 0 {
        return WAVERR_STILLPLAYING;
    }
    set_header_flags(header, flags & !WHDR_PREPARED);
    MMSYSERR_NOERROR
}

extern "efiapi" fn wave_out_write(handle: usize, header: usize, _size: u32) -> u32 {
    let Some(device) = device_of(handle) else {
        return MMSYSERR_INVALHANDLE;
    };
    let flags = header_word(header, HEADER_FLAGS);
    if flags & WHDR_PREPARED == 0 {
        return WAVERR_UNPREPARED;
    }
    let address = {
        // SAFETY: as in `header_word`: `lpData` is the header's first field.
        unsafe { ((header + HEADER_DATA) as *const usize).read_unaligned() }
    };
    let length = header_word(header, HEADER_LENGTH) as usize;
    set_header_flags(header, (flags | WHDR_INQUEUE) & !WHDR_DONE);
    device.with(|inner| {
        // SAFETY: `WAVEHDR`'s contract: `dwBufferLength` readable bytes at `lpData`.
        let data = unsafe { core::slice::from_raw_parts(address as *const u8, length) };
        let left = inner.volume & 0xffff;
        let volume = left as f32 / 65535.0;
        let samples = to_sixteen_bits(data, inner.format.bits, inner.format.float, volume);
        inner.player.push(&samples);
        let frames = length as u64 / u64::from(inner.format.frame_bytes.max(1));
        let duration = frames * 1000 / u64::from(inner.format.rate.max(1));
        let start = inner.last_end_ms.max(now_ms());
        inner.last_end_ms = start + duration;
        inner.queue.push(Queued {
            header,
            ends_ms: inner.last_end_ms,
        });
    });
    MMSYSERR_NOERROR
}

fn reset_device(handle: usize, device: &Device) {
    let (done, callback) = device.with(|inner| {
        let done: Vec<usize> = inner.queue.drain(..).map(|queued| queued.header).collect();
        let format = inner.format;
        if let Some(player) = Player::open(format.rate, format.channels, APP) {
            inner.player = player;
        }
        inner.last_end_ms = 0;
        (done, inner.callback)
    });
    for header in done {
        let flags = header_word(header, HEADER_FLAGS);
        set_header_flags(header, (flags & !WHDR_INQUEUE) | WHDR_DONE);
        notify(handle, callback, WOM_DONE, header);
    }
}

extern "efiapi" fn wave_out_reset(handle: usize) -> u32 {
    let Some(device) = device_of(handle) else {
        return MMSYSERR_INVALHANDLE;
    };
    reset_device(handle, device);
    MMSYSERR_NOERROR
}

extern "efiapi" fn wave_out_close(handle: usize) -> u32 {
    let Some(device) = device_of(handle) else {
        return MMSYSERR_INVALHANDLE;
    };
    if device.with(|inner| !inner.queue.is_empty()) {
        return WAVERR_STILLPLAYING;
    }
    let callback = device.with(|inner| inner.callback);
    device.closing.store(true, Ordering::Release);
    while !device.finished.load(Ordering::Acquire) {
        crate::linux::sleep_nanoseconds(PUMP_NS);
    }
    notify(handle, callback, WOM_CLOSE, 0);
    // SAFETY: the device thread has finished, and the program gives the handle up here.
    drop(unsafe { Box::from_raw(handle as *mut Device) });
    MMSYSERR_NOERROR
}

extern "efiapi" fn wave_out_pause(handle: usize) -> u32 {
    let Some(device) = device_of(handle) else {
        return MMSYSERR_INVALHANDLE;
    };
    device.with(|inner| inner.paused = true);
    MMSYSERR_NOERROR
}

extern "efiapi" fn wave_out_restart(handle: usize) -> u32 {
    let Some(device) = device_of(handle) else {
        return MMSYSERR_INVALHANDLE;
    };
    device.with(|inner| inner.paused = false);
    MMSYSERR_NOERROR
}

extern "efiapi" fn wave_out_get_position(handle: usize, time: *mut u32, size: u32) -> u32 {
    let Some(device) = device_of(handle) else {
        return MMSYSERR_INVALHANDLE;
    };
    if time.is_null() || size < 8 {
        return MMSYSERR_INVALHANDLE;
    }
    let played = device.with(|inner| inner.bytes_played);
    // SAFETY: `waveOutGetPosition`'s contract: a writable `MMTIME` of `size` bytes.
    unsafe {
        time.write(TIME_BYTES);
        time.add(1).write(played as u32);
    }
    MMSYSERR_NOERROR
}

extern "efiapi" fn wave_out_set_volume(handle: usize, volume: u32) -> u32 {
    let Some(device) = device_of(handle) else {
        return MMSYSERR_INVALHANDLE;
    };
    device.with(|inner| inner.volume = volume);
    MMSYSERR_NOERROR
}

extern "efiapi" fn wave_out_get_volume(handle: usize, volume: *mut u32) -> u32 {
    let Some(device) = device_of(handle) else {
        return MMSYSERR_INVALHANDLE;
    };
    if !volume.is_null() {
        let value = device.with(|inner| inner.volume);
        // SAFETY: `waveOutGetVolume`'s contract: a writable `DWORD`.
        unsafe { volume.write(value) };
    }
    MMSYSERR_NOERROR
}

extern "efiapi" fn wave_out_get_num_devs() -> u32 {
    u32::from(client::reachable())
}

unsafe extern "efiapi" fn wave_out_get_dev_caps_w(device: usize, caps: *mut u8, size: u32) -> u32 {
    if device != 0 && device as u32 != WAVE_MAPPER {
        return MMSYSERR_BADDEVICEID;
    }
    if caps.is_null() || (size as usize) < CAPS_BYTES {
        return MMSYSERR_INVALHANDLE;
    }
    let mut record = [0u8; CAPS_BYTES];
    for (index, unit) in DEVICE_NAME.encode_utf16().enumerate().take(31) {
        record[8 + index * 2..10 + index * 2].copy_from_slice(&unit.to_le_bytes());
    }
    record[72..76].copy_from_slice(&0x000f_ffffu32.to_le_bytes());
    record[76..78].copy_from_slice(&2u16.to_le_bytes());
    record[80..84].copy_from_slice(&0x000cu32.to_le_bytes());
    // SAFETY: `waveOutGetDevCapsW`'s contract: `size` writable bytes, at least `CAPS_BYTES`.
    unsafe { core::ptr::copy_nonoverlapping(record.as_ptr(), caps, CAPS_BYTES) };
    MMSYSERR_NOERROR
}

extern "efiapi" fn joy_get_pos_ex(joystick: u32, information: *mut u8) -> u32 {
    match (joystick < JOYSTICKS, information.is_null()) {
        (false, _) => JOYERR_PARMS,
        (true, true) => MMSYSERR_INVALPARAM,
        (true, false) => JOYERR_UNPLUGGED,
    }
}

extern "efiapi" fn joy_get_dev_caps_w(_joystick: usize, _capabilities: *mut u8, _size: u32) -> u32 {
    MMSYSERR_NODRIVER
}

unsafe extern "efiapi" fn mci_send_string_w(
    _command: *const u16,
    answer: *mut u16,
    answer_length: u32,
    _callback: usize,
) -> u32 {
    if !answer.is_null() && answer_length > 0 {
        // SAFETY: `mciSendStringW`'s contract: `answer_length` writable units.
        unsafe { answer.write(0) };
    }
    MCIERR_DEVICE_NOT_INSTALLED
}

pub(super) const EXPORTS: &[Export] = exports![
    b"joyGetPosEx" => joy_get_pos_ex,
    b"joyGetDevCapsW" => joy_get_dev_caps_w,
    b"mciSendStringW" => mci_send_string_w,
    b"PlaySoundW" => play_sound_w,
    b"PlaySoundA" => play_sound_a,
    b"PlaySound" => play_sound_a,
    b"sndPlaySoundW" => snd_play_sound_w,
    b"sndPlaySoundA" => snd_play_sound_a,
    b"waveOutOpen" => wave_out_open,
    b"waveOutClose" => wave_out_close,
    b"waveOutPrepareHeader" => wave_out_prepare_header,
    b"waveOutUnprepareHeader" => wave_out_unprepare_header,
    b"waveOutWrite" => wave_out_write,
    b"waveOutReset" => wave_out_reset,
    b"waveOutPause" => wave_out_pause,
    b"waveOutRestart" => wave_out_restart,
    b"waveOutGetPosition" => wave_out_get_position,
    b"waveOutSetVolume" => wave_out_set_volume,
    b"waveOutGetVolume" => wave_out_get_volume,
    b"waveOutGetNumDevs" => wave_out_get_num_devs,
    b"waveOutGetDevCapsW" => wave_out_get_dev_caps_w,
];
