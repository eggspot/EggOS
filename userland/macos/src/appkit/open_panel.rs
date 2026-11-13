//! NSSavePanel and NSOpenPanel (D392, docs/ux/file-picker-v0.3.md section 14): the panels
//! keep what the app sets and, run, start the system file picker `/bin/pick` with it, the
//! app's windows still drawing and its input set aside while the picker is open, and answer
//! the URLs the person chose.

use super::{
    NSOPENPANEL, NSOPENPANEL_META, NSSAVEPANEL, NSSAVEPANEL_META, application, store, store_copy,
    with_text,
};
use crate::foundation::{NSARRAY, NSURL, autoreleased, class_pointer, new_instance};
use crate::libsystem::children::{self, Actions};
use crate::linux;
use crate::objc::abi::{Id, Sel};
use crate::objc::buffer::Buffer;
use crate::objc::reference_count::dispose;
use crate::objc::root::NSOBJECT;
use crate::objc::{Runtime, methods, sel, send, send1, send2};
use core::ffi::{c_char, c_int};

const MODAL_OK: isize = 1;
const MODAL_CANCEL: isize = 0;
const O_NONBLOCK_CLOEXEC: usize = 0o4000 | 0o2_000_000;
const EAGAIN: i32 = 11;
const IDLE: [i64; 2] = [0, 20_000_000];
const PICK: &[u8] = b"/bin/pick\0";
const MOST_ARGUMENTS: usize = 64;

#[repr(C)]
#[derive(Debug)]
struct PanelFields {
    isa: usize,
    title: Id,
    message: Id,
    prompt: Id,
    name: Id,
    directory: Id,
    allowed: Id,
    urls: Id,
    choose_files: bool,
    choose_directories: bool,
    multiple: bool,
    is_open: bool,
}

fn panel(object: Id) -> &'static mut PanelFields {
    // SAFETY: the callers pass NSSavePanel or NSOpenPanel instances, whose memory starts with
    // these fields.
    unsafe { &mut *object.cast::<PanelFields>() }
}

fn new_panel(class: &'static crate::objc::abi::ClassObject, is_open: bool) -> Id {
    let object = new_instance(class_pointer(class));
    if object.is_null() {
        return object;
    }
    let fields = panel(object);
    fields.choose_files = true;
    fields.is_open = is_open;
    autoreleased(object)
}

extern "C" fn save_panel(_: Id, _: Sel) -> Id {
    new_panel(&NSSAVEPANEL, false)
}

extern "C" fn open_panel(_: Id, _: Sel) -> Id {
    new_panel(&NSOPENPANEL, true)
}

extern "C" fn dealloc(this: Id, _: Sel) {
    let fields = panel(this);
    for slot in [
        &mut fields.title,
        &mut fields.message,
        &mut fields.prompt,
        &mut fields.name,
        &mut fields.directory,
        &mut fields.allowed,
        &mut fields.urls,
    ] {
        store(slot, core::ptr::null_mut());
    }
    dispose(this);
}

macro_rules! object_accessors {
    ($get:ident, $set:ident, $field:ident, $copy:expr) => {
        extern "C" fn $get(this: Id, _: Sel) -> Id {
            panel(this).$field
        }

        extern "C" fn $set(this: Id, _: Sel, value: Id) {
            match $copy {
                true => store_copy(&mut panel(this).$field, value),
                false => store(&mut panel(this).$field, value),
            }
        }
    };
}

object_accessors!(title, set_title, title, true);
object_accessors!(message, set_message, message, true);
object_accessors!(prompt, set_prompt, prompt, true);
object_accessors!(name_field, set_name_field, name, true);
object_accessors!(directory_url, set_directory_url, directory, false);
object_accessors!(allowed_types, set_allowed_types, allowed, false);

macro_rules! flag_accessors {
    ($get:ident, $set:ident, $field:ident) => {
        extern "C" fn $get(this: Id, _: Sel) -> u8 {
            u8::from(panel(this).$field)
        }

        extern "C" fn $set(this: Id, _: Sel, value: u8) {
            panel(this).$field = value != 0;
        }
    };
}

flag_accessors!(can_choose_files, set_can_choose_files, choose_files);
flag_accessors!(
    can_choose_directories,
    set_can_choose_directories,
    choose_directories
);
flag_accessors!(allows_multiple, set_allows_multiple, multiple);

extern "C" fn ignore_word(_: Id, _: Sel, _: usize) {}

struct Command {
    arena: Buffer<u8>,
    starts: Buffer<usize>,
}

impl Command {
    fn push(&mut self, bytes: &[u8]) {
        self.starts.push(self.arena.len());
        for &byte in bytes {
            self.arena.push(byte);
        }
        self.arena.push(0);
    }

    fn option(&mut self, name: &[u8], value: Id) {
        if value.is_null() {
            return;
        }
        with_text(value, |text| {
            if !text.is_empty() && !text.as_bytes().contains(&0) {
                self.push(name);
                self.push(text.as_bytes());
            }
        });
    }

    fn free(&mut self) {
        self.arena.free();
        self.starts.free();
    }
}

fn command_of(this: Id) -> Command {
    let fields = panel(this);
    let mut command = Command {
        arena: Buffer::new(),
        starts: Buffer::new(),
    };
    command.push(b"pick");
    let mode: &[u8] = match (
        fields.is_open,
        fields.choose_files,
        fields.choose_directories,
    ) {
        (false, _, _) => b"save",
        (true, false, true) => b"folder",
        (true, _, _) => b"open",
    };
    command.push(mode);
    let title = match fields.title.is_null() {
        true => fields.message,
        false => fields.title,
    };
    command.option(b"--title", title);
    command.option(b"--button", fields.prompt);
    if !fields.directory.is_null() {
        command.option(b"--folder", send(fields.directory, sel!("path")));
    }
    if mode == b"save" {
        command.option(b"--name", fields.name);
    }
    if mode == b"open" {
        if fields.choose_directories {
            command.push(b"--folders");
        }
        if fields.multiple {
            command.push(b"--multiple");
        }
    }
    if mode != b"folder" && !fields.allowed.is_null() {
        let count = send(fields.allowed, sel!("count")) as usize;
        for index in 0..count {
            let kind = send1(fields.allowed, sel!("objectAtIndex:"), index);
            let mut filter = Buffer::new();
            with_text(kind, |text| {
                if text.is_empty() || text.contains(['|', ';', '\0']) {
                    return;
                }
                for &byte in text.as_bytes() {
                    filter.push(byte);
                }
                filter.push(b'|');
                for &byte in b"*." {
                    filter.push(byte);
                }
                for &byte in text.as_bytes() {
                    filter.push(byte);
                }
            });
            if filter.len() > 0 {
                command.push(b"--filter");
                command.push(filter.as_slice());
            }
            filter.free();
        }
    }
    command
}

fn run_pick(command: &Command, output: &mut Buffer<u8>) -> Option<c_int> {
    let mut ends = [0 as c_int; 2];
    crate::libsystem::file::open_pipe(&mut ends, O_NONBLOCK_CLOEXEC).ok()?;
    let mut pointers: [*const c_char; MOST_ARGUMENTS] = [core::ptr::null(); MOST_ARGUMENTS];
    for (pointer, &start) in pointers
        .iter_mut()
        .zip(command.starts.as_slice())
        .take(MOST_ARGUMENTS - 1)
    {
        *pointer = command.arena.as_slice()[start..].as_ptr().cast();
    }
    let mut actions: *mut Actions = core::ptr::null_mut();
    let mut child: c_int = 0;
    // SAFETY: the action list is made here, used for one spawn and destroyed; the path and the
    // arguments are NUL-terminated and the argument list null-ended; a null environment is the
    // program's own.
    let spawned = unsafe {
        children::file_actions_init(&mut actions);
        children::file_actions_add_dup2(&mut actions, ends[1], 1);
        let result = children::posix_spawn(
            &mut child,
            PICK.as_ptr().cast(),
            &actions,
            core::ptr::null(),
            pointers.as_ptr(),
            core::ptr::null(),
        );
        children::file_actions_destroy(&mut actions);
        result
    };
    let _ = linux::close(ends[1] as usize);
    if spawned != 0 {
        let _ = linux::close(ends[0] as usize);
        return None;
    }
    let mut chunk = [0u8; 512];
    loop {
        match linux::read(ends[0] as usize, &mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                for &byte in &chunk[..read] {
                    output.push(byte);
                }
            }
            Err(errno) if errno.0 == EAGAIN => {
                application::display_all();
                application::read_records(&mut |_| {});
                let _ = linux::sleep(IDLE);
            }
            Err(_) => break,
        }
    }
    let _ = linux::close(ends[0] as usize);
    let mut status: c_int = 0;
    // SAFETY: `waitpid` writes one status into the borrowed local.
    unsafe { children::waitpid(child, &mut status, 0) };
    Some((status >> 8) & 0xff)
}

fn urls_of(lines: &[u8]) -> Id {
    let mut urls: Buffer<Id> = Buffer::new();
    for line in lines
        .split(|&byte| byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let path = crate::foundation::string_from_utf8(line);
        let url = send1(
            class_pointer(&NSURL).cast(),
            sel!("fileURLWithPath:"),
            path as usize,
        );
        if !url.is_null() {
            urls.push(url);
        }
    }
    let array = send2(
        class_pointer(&NSARRAY).cast(),
        sel!("arrayWithObjects:count:"),
        urls.as_slice().as_ptr() as usize,
        urls.len(),
    );
    urls.free();
    array
}

extern "C" fn run_modal(this: Id, _: Sel) -> isize {
    let mut command = command_of(this);
    let mut output = Buffer::new();
    let status = run_pick(&command, &mut output);
    command.free();
    let answer = match status {
        Some(0) => {
            store(&mut panel(this).urls, urls_of(output.as_slice()));
            MODAL_OK
        }
        _ => {
            store(&mut panel(this).urls, core::ptr::null_mut());
            MODAL_CANCEL
        }
    };
    output.free();
    answer
}

extern "C" fn begin_with_handler(this: Id, _: Sel, handler: Id) {
    let answer = run_modal(this, sel!("runModal"));
    if handler.is_null() {
        return;
    }
    let invoke = crate::objc::block::invoke_address(handler);
    // SAFETY: the completion handler is a block taking the block and the modal response.
    let invoke: extern "C" fn(Id, isize) = unsafe { core::mem::transmute(invoke) };
    invoke(handler, answer);
}

extern "C" fn begin_sheet(this: Id, sel: Sel, _: Id, handler: Id) {
    begin_with_handler(this, sel, handler);
}

extern "C" fn urls(this: Id, _: Sel) -> Id {
    match panel(this).urls.is_null() {
        true => send(class_pointer(&NSARRAY).cast(), sel!("array")),
        false => panel(this).urls,
    }
}

extern "C" fn url(this: Id, _: Sel) -> Id {
    let chosen = panel(this).urls;
    match chosen.is_null() || send(chosen, sel!("count")).is_null() {
        true => core::ptr::null_mut(),
        false => send1(chosen, sel!("objectAtIndex:"), 0),
    }
}

pub fn define(runtime: &mut Runtime) -> bool {
    let save_methods = methods![
        "dealloc" => dealloc,
        "title" => title,
        "setTitle:" => set_title,
        "message" => message,
        "setMessage:" => set_message,
        "prompt" => prompt,
        "setPrompt:" => set_prompt,
        "nameFieldStringValue" => name_field,
        "setNameFieldStringValue:" => set_name_field,
        "directoryURL" => directory_url,
        "setDirectoryURL:" => set_directory_url,
        "allowedFileTypes" => allowed_types,
        "setAllowedFileTypes:" => set_allowed_types,
        "setCanCreateDirectories:" => ignore_word,
        "setShowsHiddenFiles:" => ignore_word,
        "setExtensionHidden:" => ignore_word,
        "setCanSelectHiddenExtension:" => ignore_word,
        "setTreatsFilePackagesAsDirectories:" => ignore_word,
        "runModal" => run_modal,
        "beginWithCompletionHandler:" => begin_with_handler,
        "beginSheetModalForWindow:completionHandler:" => begin_sheet,
        "URL" => url,
    ];
    let save_class = methods![
        "savePanel" => save_panel,
    ];
    let open_methods = methods![
        "canChooseFiles" => can_choose_files,
        "setCanChooseFiles:" => set_can_choose_files,
        "canChooseDirectories" => can_choose_directories,
        "setCanChooseDirectories:" => set_can_choose_directories,
        "allowsMultipleSelection" => allows_multiple,
        "setAllowsMultipleSelection:" => set_allows_multiple,
        "setResolvesAliases:" => ignore_word,
        "URLs" => urls,
    ];
    let open_class = methods![
        "openPanel" => open_panel,
    ];
    super::define(
        runtime,
        b"NSSavePanel\0",
        (&NSSAVEPANEL, &NSSAVEPANEL_META),
        &NSOBJECT,
        core::mem::size_of::<PanelFields>(),
        &save_methods,
        &save_class,
    ) && super::define(
        runtime,
        b"NSOpenPanel\0",
        (&NSOPENPANEL, &NSOPENPANEL_META),
        &NSSAVEPANEL,
        core::mem::size_of::<PanelFields>(),
        &open_methods,
        &open_class,
    )
}
