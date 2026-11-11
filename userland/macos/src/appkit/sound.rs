//! NSSound (D383): a sound made from WAV data in memory, a WAV file or a name in the bundle's
//! resources, which keeps its bytes and knows its length. EggOS has no audio device yet, so
//! `play` answers NO and nothing is heard; everything else behaves as on macOS.

use super::{NSSOUND, NSSOUND_META, store, store_copy, with_text};
use crate::foundation::{NSBUNDLE, NSDATA, class_pointer, new_instance};
use crate::objc::abi::{Id, Sel};
use crate::objc::reference_count::{dispose, objc_release};
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, methods, send, send1, sel};
use eggos_macos_gui::sound::{Wave, read_wave};

const EXTENSIONS: [&str; 3] = ["", ".wav", ".wave"];
const LONGEST_PATH: usize = 1024;

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
        false => send1(class_pointer(&NSDATA).cast(), sel!("dataWithContentsOfFile:"), path as usize),
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
                let made = init_with_contents_of_file(object, sel!("initWithContentsOfFile:byReference:"), super::string(text), 1);
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
    let fields = sound(this);
    store(&mut fields.data, core::ptr::null_mut());
    store(&mut fields.name, core::ptr::null_mut());
    dispose(this);
}

extern "C" fn play(_: Id, _: Sel) -> u8 {
    0
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
        "stop" => no,
        "pause" => no,
        "resume" => no,
        "isPlaying" => no,
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
