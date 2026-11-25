//! NSSound (D383, D776): a sound made from WAV data in memory, a WAV file or a name in the
//! bundle's resources, which keeps its bytes and knows its length. `play` streams its samples,
//! as 16-bit samples at its volume, to the sound server from the main queue's timer, never
//! blocking the app; `stop` ends it; the delegate hears `sound:didFinishPlaying:`. Without a
//! sound server `play` answers NO and nothing is heard.

use super::{NSSOUND, NSSOUND_META, store, store_copy, with_text};
use crate::foundation::{NSBUNDLE, NSDATA, class_pointer, new_instance};
use crate::objc::abi::{Id, Sel};
use crate::objc::reference_count::{dispose, objc_release, objc_retain};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, methods, sel, send, send_bool, send1, send2};
use alloc::boxed::Box;
use alloc::vec::Vec;
use eggos_soundd::client::Player;
use eggos_macos_gui::sound::{Wave, read_wave};

const EXTENSIONS: [&str; 3] = ["", ".wav", ".wave"];
const LONGEST_PATH: usize = 1024;
const PUMP_MS: u64 = 20;
const APP: &str = "mac-apps";

#[repr(C)]
#[derive(Debug)]
struct SoundFields {
    isa: usize,
    data: Id,
    wave: Wave,
    volume: f32,
    loops: bool,
    name: Id,
    delegate: Id,
    playing: usize,
}

struct Playing {
    player: Player,
    samples: Vec<u8>,
    ends_ms: u64,
}

fn now_ms() -> u64 {
    crate::linux::clock_time(1).map_or(0, |time| time[0] as u64 * 1000 + time[1] as u64 / 1_000_000)
}

fn sound(object: Id) -> &'static mut SoundFields {
    // SAFETY: the callers pass NSSound instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<SoundFields>() }
}

fn adopt(this: Id, data: Id) -> Id {
    if data.is_null() {
        objc_release(this);
        return core::ptr::null_mut();
    }
    let bytes = crate::foundation::data_bytes(data);
    let Some(wave) = read_wave(bytes) else {
        objc_release(this);
        return core::ptr::null_mut();
    };
    let fields = sound(this);
    store(&mut fields.data, data);
    fields.wave = wave;
    fields.volume = 1.0;
    this
}

extern "C" fn init_with_data(this: Id, _: Sel, data: Id) -> Id {
    if this.is_null() {
        return this;
    }
    adopt(this, data)
}

extern "C" fn init_with_contents_of_file(this: Id, _: Sel, path: Id, _: u8) -> Id {
    if this.is_null() {
        return this;
    }
    let data = match path.is_null() {
        true => path,
        false => send1(
            class_pointer(&NSDATA).cast(),
            sel!("dataWithContentsOfFile:"),
            path as usize,
        ),
    };
    adopt(this, data)
}

extern "C" fn init_with_contents_of_url(this: Id, sel: Sel, url: Id, by_reference: u8) -> Id {
    let path = match url.is_null() {
        true => url,
        false => send(url, sel!("path")),
    };
    init_with_contents_of_file(this, sel, path, by_reference)
}

extern "C" fn sound_named(_: Id, _: Sel, name: Id) -> Id {
    let bundle = send(class_pointer(&NSBUNDLE).cast(), sel!("mainBundle"));
    if bundle.is_null() || name.is_null() {
        return core::ptr::null_mut();
    }
    let folder = send(bundle, sel!("resourcePath"));
    let mut path = [0u8; LONGEST_PATH];
    with_text(folder, |folder| {
        with_text(name, |base| {
            for extension in EXTENSIONS {
                let parts = [folder, "/", base, extension];
                let length: usize = parts.iter().map(|part| part.len()).sum();
                if length > LONGEST_PATH || base.is_empty() {
                    continue;
                }
                let mut at = 0;
                for part in parts {
                    path[at..at + part.len()].copy_from_slice(part.as_bytes());
                    at += part.len();
                }
                let Ok(text) = core::str::from_utf8(&path[..length]) else {
                    continue;
                };
                let object = new_instance(class_pointer(&NSSOUND));
                if object.is_null() {
                    return core::ptr::null_mut();
                }
                let made = init_with_contents_of_file(
                    object,
                    sel!("initWithContentsOfFile:byReference:"),
                    super::string(text),
                    1,
                );
                if !made.is_null() {
                    store_copy(&mut sound(made).name, name);
                    return crate::foundation::autoreleased(made);
                }
            }
            core::ptr::null_mut()
        })
    })
}

extern "C" fn dealloc(this: Id, _: Sel) {
    end(this, false);
    let fields = sound(this);
    store(&mut fields.data, core::ptr::null_mut());
    store(&mut fields.name, core::ptr::null_mut());
    dispose(this);
}

fn playing(this: Id) -> Option<&'static mut Playing> {
    let address = sound(this).playing;
    // SAFETY: a non-zero `playing` is the `Box<Playing>` `play` leaked, freed only by `end`.
    (address != 0).then(|| unsafe { &mut *(address as *mut Playing) })
}

fn end(this: Id, finished: bool) {
    let address = core::mem::replace(&mut sound(this).playing, 0);
    if address == 0 {
        return;
    }
    // SAFETY: `play` made `address` with `Box::into_raw`, and it is cleared before this drop.
    drop(unsafe { Box::from_raw(address as *mut Playing) });
    let delegate = sound(this).delegate;
    let selector = sel!("sound:didFinishPlaying:");
    if !delegate.is_null() && send_bool(delegate, sel!("respondsToSelector:"), selector as usize) {
        send2(delegate, selector, this as usize, usize::from(finished));
    }
}

extern "C" fn pump(context: usize) {
    let this = context as Id;
    let Some(playing) = playing(this) else {
        objc_release(this);
        return;
    };
    let handed = playing.player.pump();
    let now = now_ms();
    if handed && now >= playing.ends_ms {
        match sound(this).loops {
            true => {
                let length = sound(this).wave.duration();
                playing.ends_ms = now + (length * 1000.0) as u64;
                let samples = core::mem::take(&mut playing.samples);
                playing.player.push(&samples);
                playing.samples = samples;
            }
            false => {
                end(this, true);
                objc_release(this);
                return;
            }
        }
    }
    crate::libsystem::dispatch::after_ms(PUMP_MS, context, pump);
}

extern "C" fn play(this: Id, _: Sel) -> u8 {
    if sound(this).playing != 0 {
        return 0;
    }
    let fields = sound(this);
    let wave = fields.wave;
    let bytes = crate::foundation::data_bytes(fields.data);
    let Some(data) = bytes.get(wave.data_offset..wave.data_offset + wave.data_length) else {
        return 0;
    };
    let float = wave.format == eggos_macos_gui::sound::SampleFormat::Float;
    let samples = eggos_soundd::mixer::to_sixteen_bits(data, wave.bits, float, fields.volume);
    let channels = u8::try_from(wave.channels).unwrap_or(2);
    let Some(mut player) = Player::open(wave.sample_rate, channels, APP) else {
        return 0;
    };
    player.push(&samples);
    let ends_ms = now_ms() + (wave.duration() * 1000.0) as u64;
    let playing = Box::new(Playing {
        player,
        samples,
        ends_ms,
    });
    sound(this).playing = Box::into_raw(playing) as usize;
    objc_retain(this);
    crate::libsystem::dispatch::after_ms(PUMP_MS, this as usize, pump);
    1
}

extern "C" fn stop(this: Id, _: Sel) -> u8 {
    let was_playing = sound(this).playing != 0;
    end(this, false);
    u8::from(was_playing)
}

extern "C" fn is_playing(this: Id, _: Sel) -> u8 {
    u8::from(sound(this).playing != 0)
}

extern "C" fn no(_: Id, _: Sel) -> u8 {
    0
}

extern "C" fn duration(this: Id, _: Sel) -> f64 {
    sound(this).wave.duration()
}

extern "C" fn current_time(_: Id, _: Sel) -> f64 {
    0.0
}

extern "C" fn ignore_time(_: Id, _: Sel, _: f64) {}

extern "C" fn volume(this: Id, _: Sel) -> f32 {
    sound(this).volume
}

extern "C" fn set_volume(this: Id, _: Sel, volume: f32) {
    sound(this).volume = volume.clamp(0.0, 1.0);
}

extern "C" fn loops(this: Id, _: Sel) -> u8 {
    u8::from(sound(this).loops)
}

extern "C" fn set_loops(this: Id, _: Sel, loops: u8) {
    sound(this).loops = loops != 0;
}

extern "C" fn name(this: Id, _: Sel) -> Id {
    sound(this).name
}

extern "C" fn set_name(this: Id, _: Sel, name: Id) -> u8 {
    store_copy(&mut sound(this).name, name);
    1
}

extern "C" fn delegate(this: Id, _: Sel) -> Id {
    sound(this).delegate
}

extern "C" fn set_delegate(this: Id, _: Sel, delegate: Id) {
    sound(this).delegate = delegate;
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "initWithData:" => init_with_data,
        "initWithContentsOfFile:byReference:" => init_with_contents_of_file,
        "initWithContentsOfURL:byReference:" => init_with_contents_of_url,
        "dealloc" => dealloc,
        "play" => play,
        "stop" => stop,
        "pause" => no,
        "resume" => no,
        "isPlaying" => is_playing,
        "duration" => duration,
        "currentTime" => current_time,
        "setCurrentTime:" => ignore_time,
        "volume" => volume,
        "setVolume:" => set_volume,
        "loops" => loops,
        "setLoops:" => set_loops,
        "name" => name,
        "setName:" => set_name,
        "delegate" => delegate,
        "setDelegate:" => set_delegate,
    ];
    let class = methods![
        "soundNamed:" => sound_named,
    ];
    super::define(
        runtime,
        b"NSSound\0",
        (&NSSOUND, &NSSOUND_META),
        &NSOBJECT,
        core::mem::size_of::<SoundFields>(),
        &instance,
        &class,
    )
}
