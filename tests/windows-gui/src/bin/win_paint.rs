//! `win-paint.exe`: an ordinary Windows GUI program (subsystem `WINDOWS`), built from our own
//! source for `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc` and linked by the standard
//! toolchain's `rust-lld` with its imports from kernel32, user32, gdi32 and ntdll (no C
//! runtime), that the Windows GUI boot test runs from `eggsh` (D129). It registers a window
//! class, creates a window and paints the scene of `eggos_windows_gui_tests` on `WM_PAINT`; a
//! left click toggles the ellipse's colour (redrawn through `GetDC`), a letter or digit is added
//! to the typed line, Space opens a message box whose answer the status line shows, and closing
//! the window ends the program with 0. It writes one `WIN-PAINT` line for each step to its
//! standard output, which the test redirects to a file: a GUI program has no console.

#![cfg_attr(all(windows, not(test)), no_std, no_main)]

#[cfg(all(windows, not(test)))]
mod program {
    use core::ffi::c_void;
    use core::fmt::Write;
    use eggos_windows_gui_tests::{self as scene, Painter, Pen, Rect, Scene};

    type Window = usize;
    type Context = usize;
    type Object = usize;

    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const CS_VREDRAW: u32 = 0x1;
    const CS_HREDRAW: u32 = 0x2;
    const WS_OVERLAPPED_WINDOW: u32 = 0x00cf_0000;
    const CW_USE_DEFAULT: i32 = i32::MIN;
    const SW_SHOW_NORMAL: i32 = 1;
    const WM_CREATE: u32 = 0x0001;
    const WM_DESTROY: u32 = 0x0002;
    const WM_PAINT: u32 = 0x000f;
    const WM_KEY_DOWN: u32 = 0x0100;
    const WM_CHAR: u32 = 0x0102;
    const WM_LEFT_BUTTON_DOWN: u32 = 0x0201;
    const WM_APP: u32 = 0x8000;
    const PM_NO_REMOVE: u32 = 0;
    const VK_SPACE: usize = 0x20;
    const GWLP_USER_DATA: i32 = -21;
    const SM_CY_CAPTION: i32 = 4;
    const IDC_ARROW: usize = 32512;
    const IDI_APPLICATION: usize = 32512;
    const PS_SOLID: i32 = 0;
    const NULL_BRUSH: i32 = 5;
    const TRANSPARENT: i32 = 1;
    const OPAQUE: i32 = 2;
    const EXTRA_BYTES: i32 = 16;
    const EXTRA_VALUE: usize = 0x1234;
    const SENT: (usize, isize) = (20, 22);
    const POSTED: usize = 7;
    const UNITS: usize = 128;

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
    struct CreateStruct {
        create_parameters: *mut c_void,
        rest: [usize; 9],
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
    struct PaintStruct {
        context: Context,
        erase: i32,
        paint: Rect,
        restore: i32,
        include_update: i32,
        reserved: [u8; 32],
    }

    #[repr(C)]
    struct Size {
        width: i32,
        height: i32,
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
        fn GetMessageW(message: *mut Message, window: Window, first: u32, last: u32) -> i32;
        fn PeekMessageW(
            message: *mut Message,
            window: Window,
            first: u32,
            last: u32,
            remove: u32,
        ) -> i32;
        fn TranslateMessage(message: *const Message) -> i32;
        fn DispatchMessageW(message: *const Message) -> isize;
        fn DefWindowProcW(window: Window, message: u32, wparam: usize, lparam: isize) -> isize;
        safe fn PostQuitMessage(code: i32);
        safe fn PostMessageW(window: Window, message: u32, wparam: usize, lparam: isize) -> i32;
        fn SendMessageW(window: Window, message: u32, wparam: usize, lparam: isize) -> isize;
        fn GetClientRect(window: Window, rect: *mut Rect) -> i32;
        safe fn GetWindowLongPtrW(window: Window, index: i32) -> isize;
        safe fn SetWindowLongPtrW(window: Window, index: i32, value: isize) -> isize;
        safe fn GetSystemMetrics(index: i32) -> i32;
        safe fn LoadCursorW(instance: usize, name: usize) -> usize;
        safe fn LoadIconW(instance: usize, name: usize) -> usize;
        fn BeginPaint(window: Window, paint: *mut PaintStruct) -> Context;
        fn EndPaint(window: Window, paint: *const PaintStruct) -> i32;
        safe fn GetDC(window: Window) -> Context;
        safe fn ReleaseDC(window: Window, context: Context) -> i32;
        fn InvalidateRect(window: Window, rect: *const Rect, erase: i32) -> i32;
        fn FillRect(context: Context, rect: *const Rect, brush: Object) -> i32;
        fn DrawTextW(
            context: Context,
            text: *const u16,
            count: i32,
            rect: *mut Rect,
            format: u32,
        ) -> i32;
        fn MessageBoxW(window: Window, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }

    #[link(name = "gdi32", kind = "raw-dylib")]
    unsafe extern "system" {
        safe fn CreateSolidBrush(colour: u32) -> Object;
        safe fn CreatePen(style: i32, width: i32, colour: u32) -> Object;
        safe fn GetStockObject(index: i32) -> Object;
        safe fn SelectObject(context: Context, object: Object) -> Object;
        safe fn DeleteObject(object: Object) -> i32;
        safe fn Rectangle(context: Context, left: i32, top: i32, right: i32, bottom: i32) -> i32;
        safe fn Ellipse(context: Context, left: i32, top: i32, right: i32, bottom: i32) -> i32;
        fn MoveToEx(context: Context, x: i32, y: i32, previous: *mut c_void) -> i32;
        safe fn LineTo(context: Context, x: i32, y: i32) -> i32;
        safe fn SetPixel(context: Context, x: i32, y: i32, colour: u32) -> u32;
        safe fn SetTextColor(context: Context, colour: u32) -> u32;
        safe fn SetBkColor(context: Context, colour: u32) -> u32;
        safe fn SetBkMode(context: Context, mode: i32) -> i32;
        fn TextOutW(context: Context, x: i32, y: i32, text: *const u16, count: i32) -> i32;
        fn GetTextExtentPoint32W(
            context: Context,
            text: *const u16,
            count: i32,
            size: *mut Size,
        ) -> i32;
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
        bytes: [u8; 128],
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
            let mut line = Line { bytes: [0; 128], length: 0 };
            let _ = write!(line, "WIN-PAINT ");
            let _ = write!(line, $($argument)*);
            let _ = line.write_str("\n");
            let mut written = 0;
            let output = GetStdHandle(STD_OUTPUT_HANDLE);
            // SAFETY: `WriteFile` reads the line's own bytes and writes the count into a local
            // (a null handle, the terminal's, fails harmlessly).
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

        fn pointer(&self) -> *const u16 {
            self.units.as_ptr()
        }
    }

    struct GdiPainter {
        context: Context,
    }

    impl GdiPainter {
        fn shape(&mut self, pen: Pen, brush: Option<u32>, draw: impl FnOnce(Context)) {
            let pen = CreatePen(PS_SOLID, pen.width, pen.colour);
            let brush = match brush {
                Some(colour) => CreateSolidBrush(colour),
                None => GetStockObject(NULL_BRUSH),
            };
            let old_pen = SelectObject(self.context, pen);
            let old_brush = SelectObject(self.context, brush);
            draw(self.context);
            SelectObject(self.context, old_pen);
            SelectObject(self.context, old_brush);
            DeleteObject(pen);
            DeleteObject(brush);
        }

        fn text_mode(&mut self, colour: u32, background: Option<u32>) {
            SetTextColor(self.context, colour);
            match background {
                Some(background) => {
                    SetBkColor(self.context, background);
                    SetBkMode(self.context, OPAQUE);
                }
                None => {
                    SetBkMode(self.context, TRANSPARENT);
                }
            }
        }
    }

    impl Painter for GdiPainter {
        fn fill(&mut self, rect: Rect, colour: u32) {
            let brush = CreateSolidBrush(colour);
            // SAFETY: the rectangle is a local.
            unsafe { FillRect(self.context, &rect, brush) };
            DeleteObject(brush);
        }

        fn rectangle(&mut self, rect: Rect, pen: Pen, brush: Option<u32>) {
            self.shape(pen, brush, |context| {
                Rectangle(context, rect.left, rect.top, rect.right, rect.bottom);
            });
        }

        fn ellipse(&mut self, rect: Rect, pen: Pen, brush: Option<u32>) {
            self.shape(pen, brush, |context| {
                Ellipse(context, rect.left, rect.top, rect.right, rect.bottom);
            });
        }

        fn line(&mut self, from: (i32, i32), to: (i32, i32), pen: Pen) {
            self.shape(pen, None, |context| {
                // SAFETY: no previous point is asked for.
                unsafe { MoveToEx(context, from.0, from.1, core::ptr::null_mut()) };
                LineTo(context, to.0, to.1);
            });
        }

        fn pixel(&mut self, x: i32, y: i32, colour: u32) {
            SetPixel(self.context, x, y, colour);
        }

        fn text(&mut self, x: i32, y: i32, text: &str, colour: u32, background: Option<u32>) {
            self.text_mode(colour, background);
            let wide = Wide::new(text);
            // SAFETY: the text is `wide.length` units of the local buffer.
            unsafe { TextOutW(self.context, x, y, wide.pointer(), wide.length as i32) };
        }

        fn text_in(
            &mut self,
            rect: Rect,
            text: &str,
            format: u32,
            colour: u32,
            background: Option<u32>,
        ) {
            self.text_mode(colour, background);
            let wide = Wide::new(text);
            let mut rect = rect;
            // SAFETY: the text is NUL-terminated in the local buffer, the rectangle a local.
            unsafe { DrawTextW(self.context, wide.pointer(), -1, &mut rect, format) };
        }

        fn text_width(&mut self, text: &str) -> i32 {
            let wide = Wide::new(text);
            let mut size = Size {
                width: 0,
                height: 0,
            };
            // SAFETY: the text is `wide.length` units of the local buffer, the size a local.
            unsafe {
                GetTextExtentPoint32W(self.context, wide.pointer(), wide.length as i32, &mut size)
            };
            size.width
        }
    }

    fn scene_of(window: Window) -> Option<&'static mut Scene> {
        let scene = GetWindowLongPtrW(window, GWLP_USER_DATA) as *mut Scene;
        // SAFETY: the window's user data is the scene `WinMainCRTStartup` owns for as long as
        // its message loop runs, or null before `WM_CREATE` stored it.
        unsafe { scene.as_mut() }
    }

    extern "system" fn window_procedure(
        window: Window,
        message: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize {
        match message {
            WM_CREATE => {
                // SAFETY: `WM_CREATE`'s `lParam` is the `CREATESTRUCTW` whose first field is
                // the parameter `CreateWindowExW` was given: the scene.
                let scene = unsafe { (*(lparam as *const CreateStruct)).create_parameters };
                SetWindowLongPtrW(window, GWLP_USER_DATA, scene as isize);
                say!("created");
                0
            }
            WM_APP => wparam as isize + lparam,
            message if message == WM_APP + 1 => {
                say!("posted {wparam} dispatched");
                0
            }
            WM_PAINT => {
                let mut paint = PaintStruct {
                    context: 0,
                    erase: 0,
                    paint: Rect {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    },
                    restore: 0,
                    include_update: 0,
                    reserved: [0; 32],
                };
                // SAFETY: the paint structure is a local.
                let context = unsafe { BeginPaint(window, &mut paint) };
                if let Some(scene) = scene_of(window) {
                    scene.paint(&mut GdiPainter { context });
                }
                // SAFETY: as above.
                unsafe { EndPaint(window, &paint) };
                0
            }
            WM_LEFT_BUTTON_DOWN => {
                if let Some(scene) = scene_of(window) {
                    scene.clicked = !scene.clicked;
                    let context = GetDC(window);
                    scene.paint_ellipse(&mut GdiPainter { context });
                    ReleaseDC(window, context);
                }
                let (x, y) = (lparam as i16, (lparam >> 16) as i16);
                say!("clicked {x} {y}");
                0
            }
            WM_CHAR => {
                if let Some(scene) = scene_of(window)
                    && scene.type_character(wparam as u32)
                {
                    let rect = scene.typed_rect();
                    // SAFETY: the rectangle is a local.
                    unsafe { InvalidateRect(window, &rect, 1) };
                    say!("typed {}", wparam as u8 as char);
                }
                0
            }
            WM_KEY_DOWN if wparam == VK_SPACE => {
                let text = Wide::new(scene::MESSAGE);
                let caption = Wide::new(scene::MESSAGE_CAPTION);
                // SAFETY: both strings are NUL-terminated locals.
                let result = unsafe {
                    MessageBoxW(
                        window,
                        text.pointer(),
                        caption.pointer(),
                        scene::MESSAGE_TYPE,
                    )
                };
                if let Some(scene) = scene_of(window) {
                    scene.message_box_result = Some(result);
                    let rect = scene.status_rect();
                    // SAFETY: the rectangle is a local.
                    unsafe { InvalidateRect(window, &rect, 1) };
                }
                say!("message box {result}");
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
        let mut scene = Scene::new();
        let class_name = Wide::new("EggOSWinPaint");
        let title = Wide::new(scene::TITLE);
        // SAFETY: a null name asks for the program's own module.
        let instance = unsafe { GetModuleHandleW(core::ptr::null()) };
        let class = WindowClassEx {
            size: core::mem::size_of::<WindowClassEx>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            procedure: window_procedure,
            class_extra: 0,
            window_extra: EXTRA_BYTES,
            instance,
            icon: LoadIconW(0, IDI_APPLICATION),
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
        say!("class registered");
        let (width, height) = scene::OUTSIDE;
        // SAFETY: the names are NUL-terminated locals; the scene outlives the window, which is
        // destroyed before the message loop ends.
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
                (&raw mut scene).cast(),
            )
        };
        if window == 0 {
            say!("CreateWindowExW failed");
            return 1;
        }
        let mut client = Rect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: the rectangle is a local.
        unsafe { GetClientRect(window, &mut client) };
        let caption = GetSystemMetrics(SM_CY_CAPTION);
        say!(
            "window {}x{} caption {caption}",
            client.right,
            client.bottom
        );
        SetWindowLongPtrW(window, 8, EXTRA_VALUE as isize);
        let extra = GetWindowLongPtrW(window, 8);
        say!("extra {extra}");
        // SAFETY: the window's procedure answers `WM_APP` with the sum of its parameters.
        let sent = unsafe { SendMessageW(window, WM_APP, SENT.0, SENT.1) };
        say!("sent {sent}");
        let mut message = Message::default();
        PostMessageW(window, WM_APP + 1, POSTED, 0);
        // SAFETY: the message is a local.
        let waiting =
            unsafe { PeekMessageW(&mut message, window, WM_APP + 1, WM_APP + 1, PM_NO_REMOVE) };
        if waiting != 0 {
            say!("posted {} waiting", message.wparam);
        }
        ShowWindow(window, SW_SHOW_NORMAL);
        UpdateWindow(window);
        // SAFETY: the message is a local.
        while unsafe { GetMessageW(&mut message, 0, 0, 0) } > 0 {
            // SAFETY: the message is the one the loop got.
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
