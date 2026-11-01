//! `win-controls2.exe`: an ordinary Windows GUI program (subsystem `WINDOWS`), built from our
//! own source for `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc` like `win-controls.exe`,
//! that the Windows GUI boot test runs to drive the controls D227 added: an edit's selection, a
//! password edit, a digits-only edit, a group box with two automatic radio buttons, a
//! horizontal scroll bar control, and item values on a list's and a combo box's items. Its
//! message loop goes through `IsDialogMessageW`, so Tab, the arrows and Esc work as in a
//! dialog. It writes one `WIN-CONTROLS2` line for each step to its standard output.

#![cfg_attr(all(windows, not(test)), no_std, no_main)]

#[cfg(all(windows, not(test)))]
mod program {
    use core::ffi::c_void;
    use core::fmt::Write;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use eggos_windows_gui_tests::controls::Control;
    use eggos_windows_gui_tests::controls2 as scene;

    type Window = usize;

    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const WS_OVERLAPPED_WINDOW: u32 = 0x00cf_0000;
    const CW_USE_DEFAULT: i32 = i32::MIN;
    const SW_SHOW_NORMAL: i32 = 1;
    const WM_CREATE: u32 = 0x0001;
    const WM_DESTROY: u32 = 0x0002;
    const WM_COMMAND: u32 = 0x0111;
    const WM_HSCROLL: u32 = 0x0114;
    const EM_GETSEL: u32 = 0x00b0;
    const BM_GETCHECK: u32 = 0x00f0;
    const BM_SETCHECK: u32 = 0x00f1;
    const LB_ADDSTRING: u32 = 0x0180;
    const LB_GETITEMDATA: u32 = 0x0199;
    const LB_SETITEMDATA: u32 = 0x019a;
    const CB_ADDSTRING: u32 = 0x0143;
    const CB_GETITEMDATA: u32 = 0x0150;
    const CB_SETITEMDATA: u32 = 0x0151;
    const BN_CLICKED: usize = 0;
    const ID_CANCEL: usize = 2;
    const SB_CTL: i32 = 2;
    const SB_LINE_DOWN: usize = 1;
    const SIF_RANGE: u32 = 0x1;
    const SIF_PAGE: u32 = 0x2;
    const SIF_POS: u32 = 0x4;
    const IDC_ARROW: usize = 32512;
    const UNITS: usize = 64;

    static WINDOW: AtomicUsize = AtomicUsize::new(0);

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
    struct ScrollInfo {
        size: u32,
        mask: u32,
        minimum: i32,
        maximum: i32,
        page: u32,
        position: i32,
        track_position: i32,
    }

    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> usize;
        safe fn GetStdHandle(which: u32) -> usize;
        fn WriteFile(h: usize, b: *const u8, n: u32, w: *mut u32, o: *mut c_void) -> i32;
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
        fn GetWindowTextA(window: Window, text: *mut u8, capacity: i32) -> i32;
        fn SetScrollInfo(window: Window, bar: i32, info: *const ScrollInfo, redraw: i32) -> i32;
        safe fn GetScrollPos(window: Window, bar: i32) -> i32;
        safe fn SetScrollPos(window: Window, bar: i32, position: i32, redraw: i32) -> i32;
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
            let _ = write!(line, "WIN-CONTROLS2 ");
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
    }

    impl Wide {
        fn new(text: &str) -> Self {
            let mut wide = Wide { units: [0; UNITS] };
            for (unit, value) in wide.units[..UNITS - 1].iter_mut().zip(text.encode_utf16()) {
                *unit = value;
            }
            wide
        }

        fn pointer(&self) -> *const u16 {
            self.units.as_ptr()
        }
    }

    struct Narrow {
        bytes: [u8; UNITS],
        length: usize,
    }

    impl Narrow {
        fn as_str(&self) -> &str {
            core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("?")
        }
    }

    fn item(control: &Control) -> Window {
        GetDlgItem(WINDOW.load(Ordering::Relaxed), control.id as i32)
    }

    fn send(control: &Control, message: u32, wparam: usize, lparam: isize) -> isize {
        // SAFETY: the messages this program sends carry no pointers, or pointers to its locals.
        unsafe { SendMessageW(item(control), message, wparam, lparam) }
    }

    fn text_of(control: &Control) -> Narrow {
        let mut wide = [0u16; UNITS];
        // SAFETY: the buffer is a local of `UNITS` units.
        let length = unsafe { GetWindowTextW(item(control), wide.as_mut_ptr(), UNITS as i32) };
        let mut text = Narrow {
            bytes: [0; UNITS],
            length: 0,
        };
        let units = &wide[..(length.max(0) as usize).min(UNITS)];
        for character in char::decode_utf16(units.iter().copied()) {
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

    fn narrow_text_of(control: &Control) -> Narrow {
        let mut text = Narrow {
            bytes: [0; UNITS],
            length: 0,
        };
        // SAFETY: the buffer is a local of `UNITS` bytes.
        let length =
            unsafe { GetWindowTextA(item(control), text.bytes.as_mut_ptr(), UNITS as i32) };
        text.length = (length.max(0) as usize).min(UNITS - 1);
        text
    }

    fn add_string(control: &Control, message: u32, text: &str) {
        let wide = Wide::new(text);
        send(control, message, 0, wide.pointer() as isize);
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
        let word = Wide::new(scene::WORD_TEXT);
        // SAFETY: the text is a NUL-terminated local.
        unsafe { SetWindowTextW(item(&scene::WORD), word.pointer()) };
        let (minimum, maximum, page) = scene::BAR_RANGE;
        let info = ScrollInfo {
            size: core::mem::size_of::<ScrollInfo>() as u32,
            mask: SIF_RANGE | SIF_PAGE | SIF_POS,
            minimum,
            maximum,
            page,
            position: 0,
            track_position: 0,
        };
        // SAFETY: the structure is a local.
        let position = unsafe { SetScrollInfo(item(&scene::BAR), SB_CTL, &info, 1) };
        send(&scene::SMALL, BM_SETCHECK, 1, 0);
        add_string(&scene::VALUES, LB_ADDSTRING, "beta");
        add_string(&scene::VALUES, LB_ADDSTRING, "alpha");
        send(&scene::VALUES, LB_SETITEMDATA, 0, 7);
        send(&scene::VALUES, LB_SETITEMDATA, 1, 9);
        let values = [
            send(&scene::VALUES, LB_GETITEMDATA, 0, 0),
            send(&scene::VALUES, LB_GETITEMDATA, 1, 0),
            send(&scene::VALUES, LB_GETITEMDATA, 5, 0),
        ];
        add_string(&scene::CHOICES, CB_ADDSTRING, "gamma");
        send(&scene::CHOICES, CB_SETITEMDATA, 0, 11);
        let choice = send(&scene::CHOICES, CB_GETITEMDATA, 0, 0);
        say!(
            "controls created, bar at {position}, list data {} {} {}, combo data {choice}",
            values[0],
            values[1],
            values[2]
        );
    }

    fn report_and_close(window: Window) {
        let word = narrow_text_of(&scene::WORD);
        let (mut start, mut end) = (0u32, 0u32);
        send(
            &scene::WORD,
            EM_GETSEL,
            &mut start as *mut u32 as usize,
            &mut end as *mut u32 as isize,
        );
        say!("word {} selection {start} {end}", word.as_str());
        say!("password {}", text_of(&scene::PASSWORD).as_str());
        say!("number {}", text_of(&scene::NUMBER).as_str());
        say!(
            "radios {} {}",
            send(&scene::SMALL, BM_GETCHECK, 0, 0),
            send(&scene::LARGE, BM_GETCHECK, 0, 0)
        );
        say!("scroll {}", GetScrollPos(item(&scene::BAR), SB_CTL));
        DestroyWindow(window);
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
            WM_COMMAND => {
                match (wparam & 0xffff, (wparam >> 16) & 0xffff) {
                    (ID_CANCEL, _) => report_and_close(window),
                    (id, BN_CLICKED) if id == scene::LARGE.id || id == scene::SMALL.id => {
                        let checked = send(&scene::LARGE, BM_GETCHECK, 0, 0);
                        say!("button {id} clicked, large {checked}");
                    }
                    _ => {}
                }
                0
            }
            WM_HSCROLL if lparam as usize == item(&scene::BAR) => {
                if wparam & 0xffff == SB_LINE_DOWN {
                    let bar = item(&scene::BAR);
                    let position = GetScrollPos(bar, SB_CTL) + 1;
                    SetScrollPos(bar, SB_CTL, position, 1);
                    say!("scrolled a line to {}", GetScrollPos(bar, SB_CTL));
                }
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
        let class_name = Wide::new("EggOSWinControls2");
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
                0,
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
