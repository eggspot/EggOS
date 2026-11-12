//! NSStoryboard (D406): a compiled storyboard, `<name>.storyboardc` in the bundle's resources,
//! is a folder of nibs and an `Info.plist` naming them:
//! `NSViewControllerIdentifiersToNibNames` maps each controller's identifier to its scene's nib,
//! `NSStoryboardDesignatedEntryPointIdentifier` names the initial controller and
//! `NSStoryboardMainMenu` the main menu's nib. Instantiating a
//! controller loads its scene's nib with the storyboard as File's Owner and gives back the
//! scene's controller (its first top-level window or view controller); a placeholder in a
//! scene that stands for another scene's controller (a window's content relationship) is
//! made from this storyboard while it loads (`current`). `NSApplicationMain` runs an app
//! whose `Info.plist` names `NSMainStoryboardFile`: the main menu, then the initial
//! controller, its window shown.

use super::{NSSTORYBOARD, NSSTORYBOARD_META, NSVIEWCONTROLLER, NSWINDOWCONTROLLER, nib};
use super::{store, with_text};
use crate::foundation::{NSBUNDLE, class_pointer, new_instance, property_list_of_file};
use crate::foundation::{autoreleased, string_from_utf8};
use crate::objc::abi::{Id, Sel};
use crate::objc::reference_count::{dispose, objc_release, objc_retain};
use crate::objc::root::{self, NSOBJECT};
use crate::objc::{Runtime, methods, send, send1, sel};
use core::sync::atomic::{AtomicUsize, Ordering};

const SUFFIX: &str = ".storyboardc";
const INFO: &str = "/Info.plist";
const NIB_SUFFIX: &str = ".nib";
const LONGEST_PATH: usize = 1024;
const ENTRY_KEY: &[u8] = b"NSStoryboardDesignatedEntryPointIdentifier";
const MENU_KEY: &[u8] = b"NSStoryboardMainMenu";
const NIBS_KEY: &[u8] = b"NSViewControllerIdentifiersToNibNames";

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static MAIN: AtomicUsize = AtomicUsize::new(0);

#[repr(C)]
#[derive(Debug)]
struct StoryboardFields {
    isa: usize,
    folder: Id,
    bundle: Id,
    info: Id,
}

fn fields(object: Id) -> &'static mut StoryboardFields {
    // SAFETY: the callers pass NSStoryboard instances, whose memory starts with these fields.
    unsafe { &mut *object.cast::<StoryboardFields>() }
}

fn path_string(parts: &[&str]) -> Id {
    let mut path = [0u8; LONGEST_PATH];
    let mut at = 0;
    for part in parts {
        let end = at + part.len();
        if end > LONGEST_PATH {
            return core::ptr::null_mut();
        }
        path[at..end].copy_from_slice(part.as_bytes());
        at = end;
    }
    string_from_utf8(&path[..at])
}

fn info_value(this: Id, key: &[u8]) -> Id {
    let info = fields(this).info;
    if info.is_null() {
        return info;
    }
    send1(info, sel!("objectForKey:"), string_from_utf8(key) as usize)
}

extern "C" fn storyboard_with_name(_: Id, _: Sel, name: Id, bundle: Id) -> Id {
    if name.is_null() {
        return core::ptr::null_mut();
    }
    let bundle = match bundle.is_null() {
        true => send(class_pointer(&NSBUNDLE).cast(), sel!("mainBundle")),
        false => bundle,
    };
    let resources = send(bundle, sel!("resourcePath"));
    let folder = with_text(resources, |resources| {
        with_text(name, |name| path_string(&[resources, "/", name, SUFFIX]))
    });
    let info_path = with_text(folder, |folder| path_string(&[folder, INFO]));
    let info = property_list_of_file(info_path);
    if info.is_null() {
        return core::ptr::null_mut();
    }
    let this = new_instance(class_pointer(&NSSTORYBOARD));
    if this.is_null() {
        objc_release(info);
        return this;
    }
    let held = fields(this);
    store(&mut held.folder, folder);
    store(&mut held.bundle, bundle);
    held.info = info;
    autoreleased(this)
}

extern "C" fn main_storyboard(_: Id, _: Sel) -> Id {
    MAIN.load(Ordering::Acquire) as Id
}

fn nib_name_for(this: Id, identifier: Id) -> Id {
    let names = info_value(this, NIBS_KEY);
    let named = match names.is_null() {
        true => names,
        false => send1(names, sel!("objectForKey:"), identifier as usize),
    };
    match named.is_null() {
        true => identifier,
        false => named,
    }
}

fn is_controller(object: Id) -> bool {
    root::is_kind_of(object, &NSWINDOWCONTROLLER) || root::is_kind_of(object, &NSVIEWCONTROLLER)
}

fn load_scene(this: Id, nib_name: Id) -> Option<nib::Loaded> {
    let folder = fields(this).folder;
    let path = with_text(folder, |folder| {
        with_text(nib_name, |name| path_string(&[folder, "/", name, NIB_SUFFIX]))
    });
    let previous = CURRENT.swap(this as usize, Ordering::AcqRel);
    let loaded = with_text(path, |path| nib::load_file(path.as_bytes(), this));
    CURRENT.store(previous, Ordering::Release);
    loaded.ok()
}

extern "C" fn instantiate_with_identifier(this: Id, _: Sel, identifier: Id) -> Id {
    if identifier.is_null() {
        return identifier;
    }
    let Some(mut loaded) = load_scene(this, nib_name_for(this, identifier)) else {
        return core::ptr::null_mut();
    };
    let mut found = core::ptr::null_mut();
    for &object in loaded.top_level.as_slice() {
        if found.is_null() && is_controller(object) {
            found = objc_retain(object);
        }
        objc_release(object);
    }
    loaded.top_level.free();
    if found.is_null() {
        return found;
    }
    let setter = sel!("setStoryboard:");
    if super::responds(found, setter) {
        send1(found, setter, this as usize);
    }
    if root::is_kind_of(found, &NSVIEWCONTROLLER) {
        send1(found, sel!("setIdentifier:"), identifier as usize);
    }
    autoreleased(found)
}

extern "C" fn instantiate_initial(this: Id, _: Sel) -> Id {
    let entry = info_value(this, ENTRY_KEY);
    if entry.is_null() {
        return entry;
    }
    instantiate_with_identifier(this, sel!("instantiateControllerWithIdentifier:"), entry)
}

extern "C" fn dealloc(this: Id, _: Sel) {
    let held = fields(this);
    store(&mut held.folder, core::ptr::null_mut());
    store(&mut held.bundle, core::ptr::null_mut());
    store(&mut held.info, core::ptr::null_mut());
    dispose(this);
}

/// The controller with `identifier` from the storyboard loading now, for a scene's
/// placeholder; nil outside a storyboard's load.
pub fn controller_for_placeholder(identifier: &[u8]) -> Id {
    let storyboard = CURRENT.load(Ordering::Acquire) as Id;
    if storyboard.is_null() || identifier.is_empty() {
        return core::ptr::null_mut();
    }
    let made = instantiate_with_identifier(
        storyboard,
        sel!("instantiateControllerWithIdentifier:"),
        string_from_utf8(identifier),
    );
    match made.is_null() {
        true => made,
        false => objc_retain(made),
    }
}

/// Runs the app's main storyboard `name`: its main menu, then its initial controller, shown;
/// false when it cannot be read.
pub fn run_main(name: &[u8], application: Id) -> bool {
    let storyboard = storyboard_with_name(
        class_pointer(&NSSTORYBOARD).cast(),
        sel!("storyboardWithName:bundle:"),
        string_from_utf8(name),
        core::ptr::null_mut(),
    );
    if storyboard.is_null() {
        return false;
    }
    MAIN.store(objc_retain(storyboard) as usize, Ordering::Release);
    let menu = info_value(storyboard, MENU_KEY);
    if !menu.is_null()
        && let Some(mut loaded) = load_scene(storyboard, menu)
    {
        if !loaded.main_menu.is_null() {
            send1(application, sel!("setMainMenu:"), loaded.main_menu as usize);
        }
        loaded.top_level.free();
    }
    let initial = instantiate_initial(storyboard, sel!("instantiateInitialController"));
    if initial.is_null() {
        return true;
    }
    let held = objc_retain(initial);
    match root::is_kind_of(held, &NSWINDOWCONTROLLER) {
        true => {
            send1(held, sel!("showWindow:"), 0);
        }
        false => {
            send1(held, sel!("presentViewControllerAsModalWindow:"), held as usize);
        }
    }
    true
}

pub fn define(runtime: &mut Runtime) -> bool {
    let instance = methods![
        "instantiateInitialController" => instantiate_initial,
        "instantiateControllerWithIdentifier:" => instantiate_with_identifier,
        "dealloc" => dealloc,
    ];
    let class = methods![
        "storyboardWithName:bundle:" => storyboard_with_name,
        "mainStoryboard" => main_storyboard,
    ];
    super::define(
        runtime,
        b"NSStoryboard\0",
        (&NSSTORYBOARD, &NSSTORYBOARD_META),
        &NSOBJECT,
        core::mem::size_of::<StoryboardFields>(),
        &instance,
        &class,
    )
}
