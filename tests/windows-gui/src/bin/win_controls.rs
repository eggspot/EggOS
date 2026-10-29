//! `win-controls.exe`: an ordinary Windows GUI program (subsystem `WINDOWS`), built from our own
//! source for `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc` like `win-paint.exe`, that
//! the Windows GUI boot test runs to drive the system's controls (D145): a window with a menu
//! bar, a label, an edit, a default push button, an automatic check box, a sorted list and a
//! status line, two timers (one a window's, one a thread's with a timer procedure), and the
//! File menu's Save As and Open pickers, whose file it writes and reads back. Its message loop
//! goes through `IsDialogMessageW`. It writes one `WIN-CONTROLS` line for each step to its
//! standard output, which the test redirects to a file.

#![cfg_attr(all(windows, not(test)), no_std, no_main)]

#[cfg(all(windows, not(test)))]
mod program {
    use core::ffi::c_void;
    use core::fmt::Write;
    use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
    use eggos_windows_gui_tests::controls::{self as scene, Control};

    type Window = usize;

    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const WS_OVERLAPPED_WINDOW: u32 = 0x00cf_0000;
    const CW_USE_DEFAULT: i32 = i32::MIN;
    const SW_SHOW_NORMAL: i32 = 1;
    const WM_CREATE: u32 = 0x0001;
    const WM_DESTROY: u32 = 0x0002;
    const WM_COMMAND: u32 = 0x0111;
    const WM_TIMER: u32 = 0x0113;
    const BM_GETCHECK: u32 = 0x00f0;
    const LB_ADDSTRING: u32 = 0x0180;
    const LB_GETCURSEL: u32 = 0x0188;
    const LB_GETTEXT: u32 = 0x0189;
    const BN_CLICKED: usize = 0;
    const EN_CHANGE: usize = 0x0300;
    const LBN_SELCHANGE: usize = 1;
    const MF_STRING: u32 = 0;
    const MF_POPUP: u32 = 0x10;
    const MF_SEPARATOR: u32 = 0x800;
    const IDC_ARROW: usize = 32512;
    const OFN_PATH_MUST_EXIST: u32 = 0x800;
    const OFN_FILE_MUST_EXIST: u32 = 0x1000;
    const OFN_OVERWRITE_PROMPT: u32 = 0x2;
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const CREATE_ALWAYS: u32 = 2;
    const OPEN_EXISTING: u32 = 3;
    const INVALID_HANDLE: usize = usize::MAX;
    const UNITS: usize = 260;

    static WINDOW: AtomicUsize = AtomicUsize::new(0);
    static TIMERS_FIRED: AtomicU32 = AtomicU32::new(0);
    static EDIT_CHANGES: AtomicU32 = AtomicU32::new(0);

    #[repr(C)]
    struct WindowClassEx {
        size: u32,
        style: u32,
        procedure: extern "system" fn(Window, u32, usize, isize) -> isize,
        class_extra: i32,
        window_extra: i32,
        instance: usize,
        icon: usize,
        cursor: usize,
        background: usize,
        menu_name: *const u16,
        class_name: *const u16,
        small_icon: usize,
    }

    #[repr(C)]
    #[derive(Default)]
    struct Message {
        window: Window,
        message: u32,
        wparam: usize,
        lparam: isize,
        time: u32,
        x: i32,
        y: i32,
        private: u32,
    }

    #[repr(C)]
    struct OpenFileName {
        size: u32,
        owner: Window,
        instance: usize,
        filter: *const u16,
        custom_filter: *mut u16,
        max_custom_filter: u32,
        filter_index: u32,
        file: *mut u16,
        max_file: u32,
        file_title: *mut u16,
        max_file_title: u32,
        initial_dir: *const u16,
        title: *const u16,
        flags: u32,
        file_offset: u16,
        file_extension: u16,
        default_extension: *const u16,
        custom_data: usize,
        hook: usize,
        template_name: *const u16,
        reserved: usize,
        reserved_flags: u32,
        flags_ex: u32,
    }

    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> usize;
        safe fn GetStdHandle(which: u32) -> usize;
        fn WriteFile(h: usize, b: *const u8, n: u32, w: *mut u32, o: *mut c_void) -> i32;
        fn ReadFile(h: usize, b: *mut u8, n: u32, r: *mut u32, o: *mut c_void) -> i32;
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *mut c_void,
            disposition: u32,
            flags: u32,
            template: usize,
        ) -> usize;
        fn CreateDirectoryW(name: *const u16, security: *mut c_void) -> i32;
        safe fn CloseHandle(handle: usize) -> i32;
        safe fn ExitProcess(code: u32) -> !;
    }

    #[link(name = "user32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn RegisterClassExW(class: *const WindowClassEx) -> u16;
        fn CreateWindowExW(
            ex_style: u32,
            class: *const u16,
            name: *const u16,
            style: u32,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            parent: Window,
            menu: usize,
            instance: usize,
            parameter: *mut c_void,
        ) -> Window;
        safe fn ShowWindow(window: Window, command: i32) -> i32;
        safe fn UpdateWindow(window: Window) -> i32;
        safe fn DestroyWindow(window: Window) -> i32;
        fn GetMessageW(message: *mut Message, window: Window, first: u32, last: u32) -> i32;
        fn IsDialogMessageW(dialog: Window, message: *const Message) -> i32;
        fn TranslateMessage(message: *const Message) -> i32;
        fn DispatchMessageW(message: *const Message) -> isize;
        fn DefWindowProcW(window: Window, message: u32, wparam: usize, lparam: isize) -> isize;
        safe fn PostQuitMessage(code: i32);
        fn SendMessageW(window: Window, message: u32, wparam: usize, lparam: isize) -> isize;
        safe fn LoadCursorW(instance: usize, name: usize) -> usize;
        safe fn GetDlgItem(dialog: Window, id: i32) -> Window;
        fn SetWindowTextW(window: Window, text: *const u16) -> i32;
        fn GetWindowTextW(window: Window, text: *mut u16, capacity: i32) -> i32;
        safe fn CreateMenu() -> usize;
        safe fn CreatePopupMenu() -> usize;
        fn AppendMenuW(menu: usize, flags: u32, id: usize, label: *const u16) -> i32;
        safe fn SetTimer(
            window: Window,
            id: usize,
            interval: u32,
            procedure: Option<extern "system" fn(Window, u32, usize, u32)>,
        ) -> usize;
        safe fn KillTimer(window: Window, id: usize) -> i32;
    }

    #[link(name = "comdlg32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn GetOpenFileNameW(request: *mut OpenFileName) -> i32;
        fn GetSaveFileNameW(request: *mut OpenFileName) -> i32;
    }

    #[link(name = "ntdll", kind = "raw-dylib")]
    unsafe extern "C" {
        fn memcpy(destination: *mut c_void, source: *const c_void, length: usize) -> *mut c_void;
        fn memmove(destination: *mut c_void, source: *const c_void, length: usize) -> *mut c_void;
        fn memset(destination: *mut c_void, value: i32, length: usize) -> *mut c_void;
        fn memcmp(a: *const c_void, b: *const c_void, length: usize) -> i32;
        fn __chkstk();
    }

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
        ExitProcess(101)
    }

    struct Line {
        bytes: [u8; 160],
        length: usize,
    }

    impl Write for Line {
        fn write_str(&mut self, text: &str) -> core::fmt::Result {
            let taken = text.len().min(self.bytes.len() - self.length);
            self.bytes[self.length..self.length + taken].copy_from_slice(&text.as_bytes()[..taken]);
            self.length += taken;
            Ok(())
        }
    }

    macro_rules! say {
        ($($argument:tt)*) => {{
            let mut line = Line { bytes: [0; 160], length: 0 };
            let _ = write!(line, "WIN-CONTROLS ");
            let _ = write!(line, $($argument)*);
            let _ = line.write_str("\n");
            let mut written = 0;
            let output = GetStdHandle(STD_OUTPUT_HANDLE);
            // SAFETY: `WriteFile` reads the line's own bytes and writes the count into a local.
            unsafe {
                WriteFile(
                    output,
                    line.bytes.as_ptr(),
                    line.length as u32,
                    &mut written,
                    core::ptr::null_mut(),
                )
            };
        }};
    }

    struct Wide {
        units: [u16; UNITS],
        length: usize,
    }

    impl Wide {
        fn new(text: &str) -> Self {
            let mut wide = Wide {
                units: [0; UNITS],
                length: 0,
            };
            for (unit, value) in wide.units[..UNITS - 1].iter_mut().zip(text.encode_utf16()) {
                *unit = value;
                wide.length += 1;
            }
            wide
        }

        fn empty() -> Self {
            Wide::new("")
        }

        fn pointer(&self) -> *const u16 {
            self.units.as_ptr()
        }

        fn text(&self) -> Text {
            let mut text = Text {
                bytes: [0; UNITS],
                length: 0,
            };
            for character in char::decode_utf16(self.units[..self.length].iter().copied()) {
                let mut buffer = [0u8; 4];
                let encoded = character.unwrap_or('?').encode_utf8(&mut buffer);
                let end = text.length + encoded.len();
                if end > UNITS {
                    break;
                }
                text.bytes[text.length..end].copy_from_slice(encoded.as_bytes());
                text.length = end;
            }
            text
        }
    }

    struct Text {
        bytes: [u8; UNITS],
        length: usize,
    }

    impl Text {
        fn as_str(&self) -> &str {
            core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("?")
        }
    }

    fn item(window: Window, control: &Control) -> Window {
        GetDlgItem(window, control.id as i32)
    }

    fn set_status(text: &str) {
        let window = WINDOW.load(Ordering::Relaxed);
        let wide = Wide::new(text);
        // SAFETY: the text is a NUL-terminated local.
        unsafe { SetWindowTextW(item(window, &scene::STATUS), wide.pointer()) };
    }

    fn timer_fired(which: u32) {
        let fired = TIMERS_FIRED.fetch_or(which, Ordering::Relaxed) | which;
        say!("timer {which} fired");
        if fired == 3 {
            set_status(scene::STATUS_TIMERS);
        }
    }

    extern "system" fn thread_timer(_: Window, _: u32, id: usize, _: u32) {
        KillTimer(0, id);
        timer_fired(2);
    }

    fn create_controls(window: Window, instance: usize) {
        for control in scene::CONTROLS {
            let class = Wide::new(control.class);
            let text = Wide::new(control.text);
            let (x, y, width, height) = control.rect;
            // SAFETY: the names are NUL-terminated locals.
            let created = unsafe {
                CreateWindowExW(
                    0,
                    class.pointer(),
                    text.pointer(),
                    control.style,
                    x,
                    y,
                    width,
                    height,
                    window,
                    control.id,
                    instance,
                    core::ptr::null_mut(),
                )
            };
            if created == 0 {
                say!("control {} failed", control.id);
            }
        }
        let first = Wide::new(scene::FIRST_ITEM);
        // SAFETY: the item is a NUL-terminated local the list copies.
        unsafe {
            SendMessageW(
                item(window, &scene::LIST),
                LB_ADDSTRING,
                0,
                first.pointer() as isize,
            )
        };
        SetTimer(window, scene::WINDOW_TIMER.0, scene::WINDOW_TIMER.1, None);
        let thread = SetTimer(0, 0, scene::THREAD_TIMER_INTERVAL, Some(thread_timer));
        say!("controls created, thread timer {}", u32::from(thread != 0));
    }

    fn menu() -> usize {
        let bar = CreateMenu();
        for (entry, items) in scene::MENU {
            let popup = CreatePopupMenu();
            for (label, id) in items {
                let (flags, text) = match label {
                    Some(label) => (MF_STRING, Wide::new(label)),
                    None => (MF_SEPARATOR, Wide::empty()),
                };
                // SAFETY: the label is a NUL-terminated local the menu copies.
                unsafe { AppendMenuW(popup, flags, *id, text.pointer()) };
            }
            let label = Wide::new(entry);
            // SAFETY: as above.
            unsafe { AppendMenuW(bar, MF_POPUP, popup, label.pointer()) };
        }
        bar
    }

    fn pick(window: Window, save: bool) -> Option<(Wide, u16, u16)> {
        let mut file = Wide::empty();
        let filter = Wide::new(scene::FILTER);
        let folder = Wide::new(scene::FOLDER);
        let extension = Wide::new(scene::DEFAULT_EXTENSION);
        let mut request = OpenFileName {
            size: core::mem::size_of::<OpenFileName>() as u32,
            owner: window,
            instance: 0,
            filter: filter.pointer(),
            custom_filter: core::ptr::null_mut(),
            max_custom_filter: 0,
            filter_index: 1,
            file: file.units.as_mut_ptr(),
            max_file: UNITS as u32,
            file_title: core::ptr::null_mut(),
            max_file_title: 0,
            initial_dir: folder.pointer(),
            title: core::ptr::null(),
            flags: match save {
                true => OFN_PATH_MUST_EXIST | OFN_OVERWRITE_PROMPT,
                false => OFN_PATH_MUST_EXIST | OFN_FILE_MUST_EXIST,
            },
            file_offset: 0,
            file_extension: 0,
            default_extension: extension.pointer(),
            custom_data: 0,
            hook: 0,
            template_name: core::ptr::null(),
            reserved: 0,
            reserved_flags: 0,
            flags_ex: 0,
        };
        let chosen = match save {
            // SAFETY: the request and its strings and buffer are locals that outlive the call.
            true => unsafe { GetSaveFileNameW(&mut request) },
            // SAFETY: as above.
            false => unsafe { GetOpenFileNameW(&mut request) },
        };
        if chosen == 0 {
            say!("picker cancelled");
            return None;
        }
        file.length = file.units.iter().position(|unit| *unit == 0).unwrap_or(0);
        Some((file, request.file_offset, request.file_extension))
    }

    fn open_file(path: &Wide, access: u32, disposition: u32) -> usize {
        // SAFETY: the path is a NUL-terminated local.
        unsafe {
            CreateFileW(
                path.pointer(),
                access,
                0,
                core::ptr::null_mut(),
                disposition,
                0,
                0,
            )
        }
    }

    fn save(window: Window) {
        let Some((path, offset, extension)) = pick(window, true) else {
            return;
        };
        let file = open_file(&path, GENERIC_WRITE, CREATE_ALWAYS);
        if file == INVALID_HANDLE {
            say!("could not create {}", path.text().as_str());
            return;
        }
        let mut written = 0;
        // SAFETY: the bytes are the scene's text, the count a local.
        unsafe {
            WriteFile(
                file,
                scene::FILE_TEXT.as_ptr(),
                scene::FILE_TEXT.len() as u32,
                &mut written,
                core::ptr::null_mut(),
            )
        };
        CloseHandle(file);
        say!(
            "saved {} offset {offset} extension {extension} wrote {written}",
            path.text().as_str()
        );
        set_status(scene::STATUS_SAVED);
    }

    fn open(window: Window) {
        let Some((path, offset, _)) = pick(window, false) else {
            return;
        };
        let file = open_file(&path, GENERIC_READ, OPEN_EXISTING);
        if file == INVALID_HANDLE {
            say!("could not open {}", path.text().as_str());
            return;
        }
        let mut bytes = [0u8; 32];
        let mut read = 0;
        // SAFETY: the buffer and the count are locals.
        unsafe {
            ReadFile(
                file,
                bytes.as_mut_ptr(),
                bytes.len() as u32,
                &mut read,
                core::ptr::null_mut(),
            )
        };
        CloseHandle(file);
        let text = core::str::from_utf8(&bytes[..read as usize]).unwrap_or("?");
        say!(
            "opened {} offset {offset} read {}",
            path.text().as_str(),
            text.trim_end()
        );
        if text.trim_end() == scene::TYPED {
            set_status(scene::STATUS_READ);
        }
    }

    fn command(window: Window, id: usize, notification: usize) {
        match (id, notification) {
            (id, BN_CLICKED) if id == scene::ADD.id => {
                let mut text = Wide::empty();
                // SAFETY: the buffer is a local of `UNITS` units.
                let length = unsafe {
                    GetWindowTextW(
                        item(window, &scene::NAME),
                        text.units.as_mut_ptr(),
                        UNITS as i32,
                    )
                };
                text.length = length.max(0) as usize;
                // SAFETY: the item is NUL-terminated in the local, which the list copies.
                let index = unsafe {
                    SendMessageW(
                        item(window, &scene::LIST),
                        LB_ADDSTRING,
                        0,
                        text.pointer() as isize,
                    )
                };
                let empty = Wide::empty();
                // SAFETY: the text is a NUL-terminated local.
                unsafe { SetWindowTextW(item(window, &scene::NAME), empty.pointer()) };
                say!(
                    "added {} at {index} after {} edit changes",
                    text.text().as_str(),
                    EDIT_CHANGES.load(Ordering::Relaxed)
                );
                set_status(scene::STATUS_ADDED);
            }
            (id, BN_CLICKED) if id == scene::UPPER_CASE.id => {
                // SAFETY: `BM_GETCHECK` takes no pointers.
                let checked =
                    unsafe { SendMessageW(item(window, &scene::UPPER_CASE), BM_GETCHECK, 0, 0) };
                say!("upper case {checked}");
                if checked == 1 {
                    set_status(scene::STATUS_UPPER_CASE);
                }
            }
            (id, EN_CHANGE) if id == scene::NAME.id => {
                EDIT_CHANGES.fetch_add(1, Ordering::Relaxed);
            }
            (id, LBN_SELCHANGE) if id == scene::LIST.id => {
                let list = item(window, &scene::LIST);
                // SAFETY: `LB_GETCURSEL` takes no pointers.
                let index = unsafe { SendMessageW(list, LB_GETCURSEL, 0, 0) };
                let mut text = Wide::empty();
                // SAFETY: the buffer is a local of `UNITS` units, more than any item.
                let length = unsafe {
                    SendMessageW(
                        list,
                        LB_GETTEXT,
                        index as usize,
                        text.units.as_mut_ptr() as isize,
                    )
                };
                text.length = length.max(0) as usize;
                say!("selected {index} {}", text.text().as_str());
                if text.text().as_str() == scene::TYPED {
                    set_status(scene::STATUS_SELECTED);
                }
            }
            (scene::SAVE_ID, 0) => save(window),
            (scene::OPEN_ID, 0) => open(window),
            (scene::EXIT_ID, 0) => {
                say!("exit chosen");
                DestroyWindow(window);
            }
            _ => {}
        }
    }

    extern "system" fn window_procedure(
        window: Window,
        message: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize {
        match message {
            WM_CREATE => {
                WINDOW.store(window, Ordering::Relaxed);
                // SAFETY: a null name asks for the program's own module.
                let instance = unsafe { GetModuleHandleW(core::ptr::null()) };
                create_controls(window, instance);
                0
            }
            WM_TIMER if wparam == scene::WINDOW_TIMER.0 => {
                KillTimer(window, wparam);
                timer_fired(1);
                0
            }
            WM_COMMAND => {
                command(window, wparam & 0xffff, (wparam >> 16) & 0xffff);
                0
            }
            WM_DESTROY => {
                say!("destroyed");
                PostQuitMessage(0);
                0
            }
            // SAFETY: the default procedure takes the message as it came.
            _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
        }
    }

    #[unsafe(no_mangle)]
    extern "system" fn WinMainCRTStartup() -> u32 {
        core::hint::black_box([
            memcpy as *const () as usize,
            memmove as *const () as usize,
            memset as *const () as usize,
            memcmp as *const () as usize,
            __chkstk as *const () as usize,
        ]);
        let folder = Wide::new(scene::FOLDER);
        // SAFETY: the name is a NUL-terminated local.
        let made = unsafe { CreateDirectoryW(folder.pointer(), core::ptr::null_mut()) };
        say!("folder made {made}");
        let class_name = Wide::new("EggOSWinControls");
        let title = Wide::new(scene::TITLE);
        // SAFETY: a null name asks for the program's own module.
        let instance = unsafe { GetModuleHandleW(core::ptr::null()) };
        let class = WindowClassEx {
            size: core::mem::size_of::<WindowClassEx>() as u32,
            style: 0,
            procedure: window_procedure,
            class_extra: 0,
            window_extra: 0,
            instance,
            icon: 0,
            cursor: LoadCursorW(0, IDC_ARROW),
            background: scene::BACKGROUND,
            menu_name: core::ptr::null(),
            class_name: class_name.pointer(),
            small_icon: 0,
        };
        // SAFETY: the class and its name are locals that outlive the call.
        if unsafe { RegisterClassExW(&class) } == 0 {
            say!("RegisterClassExW failed");
            return 1;
        }
        let (width, height) = scene::OUTSIDE;
        // SAFETY: the names are NUL-terminated locals.
        let window = unsafe {
            CreateWindowExW(
                0,
                class_name.pointer(),
                title.pointer(),
                WS_OVERLAPPED_WINDOW,
                CW_USE_DEFAULT,
                CW_USE_DEFAULT,
                width,
                height,
                0,
                menu(),
                instance,
                core::ptr::null_mut(),
            )
        };
        if window == 0 {
            say!("CreateWindowExW failed");
            return 1;
        }
        say!("window created");
        ShowWindow(window, SW_SHOW_NORMAL);
        UpdateWindow(window);
        let mut message = Message::default();
        // SAFETY: the message is a local.
        while unsafe { GetMessageW(&mut message, 0, 0, 0) } > 0 {
            // SAFETY: the message is the one the loop got.
            if unsafe { IsDialogMessageW(window, &message) } != 0 {
                continue;
            }
            // SAFETY: as above.
            unsafe { TranslateMessage(&message) };
            // SAFETY: as above.
            unsafe { DispatchMessageW(&message) };
        }
        say!("quit {}", message.wparam);
        message.wparam as u32
    }
}

#[cfg(any(not(windows), test))]
fn main() {}
