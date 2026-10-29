use crate::output::{FAILURE, Output};
use crate::system::{CapturedRows, System};
use crate::window_input::parse;
use crate::wm::{USAGE_STATUS, number, open_device, refused, usage};
use core::ffi::CStr;
use eggos_png::scale::{Downscale, MAX_FACTOR, factor_for, scaled_size};
use eggos_png::{EncodeError, Encoder, EncoderWorkspace, PngSink};
use eggos_runtime::errno::Errno;

pub const MAX_CAPTURE_WIDTH: u32 = 4096;
pub const MAX_SCALE_STEPS: u32 = 8;

const BAND_BYTES: usize = 64 * 1024;
const FILE_MODE: u32 = 0o600;
const NAME_CAPACITY: usize = 32;

#[derive(Debug)]
enum SinkError {
    Write(Errno),
    TooBig,
}

struct FileSink<'s, S: System> {
    system: &'s mut S,
    descriptor: usize,
    written: u64,
    limit: u64,
}

impl<S: System> PngSink for FileSink<'_, S> {
    type Error = SinkError;

    fn write(&mut self, bytes: &[u8]) -> Result<(), SinkError> {
        self.written += bytes.len() as u64;
        if self.written > self.limit {
            return Err(SinkError::TooBig);
        }
        self.system
            .write(self.descriptor, bytes)
            .map_err(SinkError::Write)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Attempt {
    Done(u64),
    TooBig,
    Resized(CapturedRows),
    Refused(Errno),
    WriteFailed(Errno),
    Broken,
}

struct Buffers {
    work: EncoderWorkspace,
    band: [u8; BAND_BYTES],
    row: [u32; MAX_CAPTURE_WIDTH as usize],
    out: [u32; MAX_CAPTURE_WIDTH as usize],
    sums: [u64; 3 * MAX_CAPTURE_WIDTH as usize],
}

struct Target {
    device: usize,
    window: u32,
    size: CapturedRows,
    factor: u32,
    descriptor: usize,
    limit: u64,
}

fn attempt_of(error: EncodeError<SinkError>) -> Attempt {
    match error {
        EncodeError::Sink(SinkError::TooBig) => Attempt::TooBig,
        EncodeError::Sink(SinkError::Write(errno)) => Attempt::WriteFailed(errno),
        _ => Attempt::Broken,
    }
}

fn encode_once<S: System>(system: &mut S, target: &Target, buffers: &mut Buffers) -> Attempt {
    let CapturedRows { width, height, .. } = target.size;
    let (out_width, out_height) = scaled_size(width, height, target.factor);
    let sink = FileSink {
        system,
        descriptor: target.descriptor,
        written: 0,
        limit: target.limit,
    };
    let Buffers {
        work,
        band,
        row,
        out,
        sums,
    } = buffers;
    let mut encoder = match Encoder::new(out_width, out_height, work, sink) {
        Ok(encoder) => encoder,
        Err(error) => return attempt_of(error),
    };
    let Some(mut downscale) = Downscale::new(width, height, target.factor, sums) else {
        return Attempt::Broken;
    };
    let row_bytes = width as usize * 4;
    let rows_per_band = (BAND_BYTES / row_bytes).max(1) as u32;
    let mut first_row = 0;
    while first_row < height {
        let wanted = rows_per_band.min(height - first_row);
        let got = match encoder.sink_mut().system.capture_window_rows(
            target.device,
            target.window,
            first_row,
            wanted,
            &mut band[..wanted as usize * row_bytes],
        ) {
            Ok(got) => got,
            Err(errno) => return Attempt::Refused(errno),
        };
        if got.width != width || got.height != height {
            return Attempt::Resized(got);
        }
        if got.rows == 0 || got.rows > wanted {
            return Attempt::Broken;
        }
        for line in band[..got.rows as usize * row_bytes].chunks_exact(row_bytes) {
            for (pixel, bytes) in row.iter_mut().zip(line.as_chunks::<4>().0) {
                *pixel = u32::from_ne_bytes(*bytes);
            }
            if downscale.push_row(&row[..width as usize], out)
                && let Err(error) = encoder.write_row(&out[..out_width as usize])
            {
                return attempt_of(error);
            }
        }
        first_row += got.rows;
    }
    match encoder.finish() {
        Ok(sink) => Attempt::Done(sink.written),
        Err(error) => attempt_of(error),
    }
}

fn default_name(window: u32, out: &mut [u8; NAME_CAPACITY]) -> &CStr {
    let mut digits = [0u8; 20];
    let digits = crate::output::decimal(u64::from(window), &mut digits);
    let mut length = 0;
    for part in [&b"window-"[..], digits, b".png\0"] {
        out[length..length + part.len()].copy_from_slice(part);
        length += part.len();
    }
    CStr::from_bytes_with_nul(&out[..length]).unwrap_or(c"window.png")
}

fn positive(text: Option<&[u8]>) -> Result<Option<u32>, ()> {
    match text {
        None => Ok(None),
        Some(text) => number(text)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|&value| value > 0)
            .map(Some)
            .ok_or(()),
    }
}

pub fn capture<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let Some(parsed) = parse(arguments, &[b"-o", b"--max-width", b"--max-bytes"]) else {
        return usage(output);
    };
    if parsed.count != 1 {
        return usage(output);
    }
    let window_text = parsed.positionals[0];
    let Some(window) = number(window_text).and_then(|id| u32::try_from(id).ok()) else {
        output.error(&[b"invalid window id '", window_text, b"'"]);
        return USAGE_STATUS;
    };
    let (Ok(max_width), Ok(max_bytes)) = (positive(parsed.max_width), positive(parsed.max_bytes))
    else {
        output.error(&[b"--max-width and --max-bytes take a positive number"]);
        return USAGE_STATUS;
    };
    let mut name = [0u8; NAME_CAPACITY];
    let file = match parsed.output_file {
        Some(file) => file,
        None => default_name(window, &mut name),
    };
    let Some(device) = open_device(output) else {
        return FAILURE;
    };
    let status = capture_to(
        output,
        device,
        window,
        window_text,
        file,
        max_width,
        max_bytes,
    );
    output.system.close(device);
    status
}

fn capture_to<S: System>(
    output: &mut Output<S>,
    device: usize,
    window: u32,
    window_text: &[u8],
    file: &CStr,
    max_width: Option<u32>,
    max_bytes: Option<u32>,
) -> i32 {
    let mut size = match output
        .system
        .capture_window_rows(device, window, 0, 0, &mut [])
    {
        Ok(size) => size,
        Err(errno) => return capture_refused(output, window_text, errno),
    };
    let mut buffers = Buffers {
        work: EncoderWorkspace::new(),
        band: [0; BAND_BYTES],
        row: [0; MAX_CAPTURE_WIDTH as usize],
        out: [0; MAX_CAPTURE_WIDTH as usize],
        sums: [0; 3 * MAX_CAPTURE_WIDTH as usize],
    };
    let mut factor = factor_for(size.width, max_width.unwrap_or(0));
    let mut steps = 0;
    let mut restarted = false;
    loop {
        if size.width == 0 || size.height == 0 || size.width > MAX_CAPTURE_WIDTH {
            output.error(&[window_text, b": the window's size can't be captured"]);
            return FAILURE;
        }
        let descriptor = match output.system.create(file, FILE_MODE, true) {
            Ok(descriptor) => descriptor,
            Err(errno) => {
                output.file_error(file, errno);
                return FAILURE;
            }
        };
        let target = Target {
            device,
            window,
            size,
            factor,
            descriptor,
            limit: max_bytes.map_or(u64::MAX, u64::from),
        };
        let attempt = encode_once(&mut output.system, &target, &mut buffers);
        output.system.close(descriptor);
        match attempt {
            Attempt::Done(bytes) => {
                report(output, window_text, &target, bytes, file);
                return 0;
            }
            Attempt::TooBig if steps < MAX_SCALE_STEPS && factor < MAX_FACTOR => {
                steps += 1;
                factor += 1;
                continue;
            }
            Attempt::Resized(new) if !restarted => {
                restarted = true;
                size = new;
                factor = factor_for(size.width, max_width.unwrap_or(0));
                continue;
            }
            _ => {}
        }
        let _ = output.system.remove(file, false);
        return match attempt {
            Attempt::TooBig => {
                output.error(&[window_text, b": the PNG does not fit in --max-bytes"]);
                FAILURE
            }
            Attempt::Resized(_) => {
                output.error(&[window_text, b": the window kept changing size; try again"]);
                FAILURE
            }
            Attempt::Refused(Errno::ENOENT) => {
                output.error(&[b"window ", window_text, b" closed"]);
                FAILURE
            }
            Attempt::Refused(errno) => capture_refused(output, window_text, errno),
            Attempt::WriteFailed(errno) => {
                output.file_error(file, errno);
                FAILURE
            }
            _ => {
                output.error(&[window_text, b": the capture failed"]);
                FAILURE
            }
        };
    }
}

fn capture_refused<S: System>(output: &mut Output<S>, window_text: &[u8], errno: Errno) -> i32 {
    match errno {
        Errno::EOPNOTSUPP => {
            output.error(&[
                window_text,
                b": drawn by the kernel (the Terminal), which can't be captured yet",
            ]);
            FAILURE
        }
        errno => refused(output, window_text, errno),
    }
}

fn report<S: System>(
    output: &mut Output<S>,
    window_text: &[u8],
    target: &Target,
    bytes: u64,
    file: &CStr,
) {
    let CapturedRows { width, height, .. } = target.size;
    let (out_width, out_height) = scaled_size(width, height, target.factor);
    output.write(b"window ");
    output.write(window_text);
    output.write(b": ");
    for (index, value) in [width, height, out_width, out_height]
        .into_iter()
        .enumerate()
    {
        output.write_number(u64::from(value), 0);
        output.write(match index {
            0 | 2 => b"x",
            1 => b" as ",
            _ => b" (scale 1/",
        });
    }
    output.write_number(u64::from(target.factor), 0);
    output.write(b"), ");
    output.write_number(bytes, 0);
    output.write(b" bytes, ");
    output.write(file.to_bytes());
    output.write(b"; input coordinates are window pixels: multiply image coordinates by ");
    output.write_number(u64::from(target.factor), 0);
    output.write(b"\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{Fake, FakeWindow};
    use crate::wm::{KERNEL_CLIENT, wm};
    use eggos_png::{decode, decoded_size};

    fn pattern(width: u32, height: u32) -> Vec<u32> {
        let mut state = 0x1234_5678u32;
        (0..width * height)
            .map(|index| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                match index % 7 {
                    0 => state & 0x00ff_ffff,
                    _ => ((index / width) * 0x0003_0201) & 0x00ff_ffff,
                }
            })
            .collect()
    }

    fn machine(width: u32, height: u32) -> Fake {
        let mut fake = Fake::default();
        for (id, client) in [(64, KERNEL_CLIENT), (129, 67)] {
            fake.windows.push(FakeWindow {
                id,
                state: 0,
                rect: [0, 0, width, height],
                client,
                title: b"window".to_vec(),
            });
        }
        fake.window_pixels
            .insert(129, (width, height, pattern(width, height)));
        fake.with_directory("/home", 0)
    }

    fn run(arguments: &[&CStr], fake: Fake) -> (i32, Fake) {
        let mut output = Output::new(fake, b"wm");
        let status = wm(&mut output, arguments);
        let status = output.finish(status);
        (status, output.system)
    }

    fn image(png: &[u8]) -> (u32, u32, Vec<u32>) {
        let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
        let mut inflated = vec![0u8; decoded_size(width, height).unwrap()];
        let mut pixels = vec![0u32; (width * height) as usize];
        decode(png, &mut inflated, &mut pixels).unwrap();
        (width, height, pixels)
    }

    fn shrunk(width: u32, height: u32, factor: u32, pixels: &[u32]) -> Vec<u32> {
        let mut sums = vec![0u64; 3 * width as usize];
        let mut downscale = Downscale::new(width, height, factor, &mut sums).unwrap();
        let mut row = vec![0u32; downscale.output_width() as usize];
        let mut out = Vec::new();
        for source in pixels.chunks(width as usize) {
            if downscale.push_row(source, &mut row) {
                out.extend_from_slice(&row);
            }
        }
        out
    }

    #[test]
    fn a_capture_writes_the_window_s_content_as_a_png() {
        let (status, fake) = run(&[c"capture", c"129"], machine(300, 200));
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        let png = fake.contents("window-129.png");
        assert_eq!(image(&png), (300, 200, pattern(300, 200)));
        assert_eq!(
            fake.output(),
            format!(
                "window 129: 300x200 as 300x200 (scale 1/1), {} bytes, window-129.png; input \
                 coordinates are window pixels: multiply image coordinates by 1\n",
                png.len()
            )
        );
        assert!(fake.capture_calls > 2, "several bands");
        let (_, (_, status)) = fake
            .nodes
            .iter()
            .find(|(path, _)| path.ends_with(b"window-129.png"))
            .unwrap();
        assert_eq!(status.mode & 0o777, 0o600, "window contents can be private");
    }

    #[test]
    fn a_wide_window_is_shrunk_to_the_maximum_width() {
        let (status, fake) = run(
            &[
                c"capture",
                c"-o",
                c"/home/shot.png",
                c"--max-width",
                c"150",
                c"129",
            ],
            machine(301, 100),
        );
        assert_eq!(status, 0);
        let (width, height, pixels) = image(&fake.contents("/home/shot.png"));
        assert_eq!((width, height), (101, 34));
        assert_eq!(pixels, shrunk(301, 100, 3, &pattern(301, 100)));
        assert!(fake.output().contains("301x100 as 101x34 (scale 1/3)"));
        assert!(fake.output().ends_with("multiply image coordinates by 3\n"));
    }

    #[test]
    fn a_png_over_the_byte_limit_is_redone_one_step_smaller() {
        let (_, whole) = run(&[c"capture", c"129"], machine(256, 256));
        let full = whole.contents("window-129.png").len();
        let limit = std::ffi::CString::new((full - 1).to_string()).unwrap();
        let (status, fake) = run(
            &[c"capture", c"--max-bytes", &limit, c"129"],
            machine(256, 256),
        );
        assert_eq!(status, 0);
        assert!(fake.output().contains("as 128x128 (scale 1/2)"));
        assert!(fake.contents("window-129.png").len() < full);
        let (status, fake) = run(
            &[c"capture", c"--max-bytes", c"40", c"129"],
            machine(256, 256),
        );
        assert_eq!(status, FAILURE);
        assert!(fake.errors().contains("does not fit"));
        assert!(!fake.nodes.contains_key(&b"window-129.png"[..]));
    }

    #[test]
    fn the_terminal_and_unknown_windows_are_refused_by_name() {
        let (status, fake) = run(&[c"capture", c"64"], machine(10, 10));
        assert_eq!(status, FAILURE);
        assert_eq!(
            fake.errors(),
            "wm: 64: drawn by the kernel (the Terminal), which can't be captured yet\n"
        );
        let (status, fake) = run(&[c"capture", c"7"], machine(10, 10));
        assert_eq!(status, FAILURE);
        assert_eq!(fake.errors(), "wm: 7: no such window\n");
    }

    #[test]
    fn a_window_closed_between_bands_is_reported() {
        let mut closing = machine(1024, 64);
        closing.capture_fails_at = Some((2, Errno::ENOENT));
        let (status, fake) = run(&[c"capture", c"129"], closing);
        assert_eq!(status, FAILURE);
        assert_eq!(fake.errors(), "wm: window 129 closed\n");
        assert!(!fake.nodes.contains_key(&b"window-129.png"[..]));
    }

    #[test]
    fn a_window_resized_during_the_capture_is_captured_again_once() {
        let mut resizing = machine(1024, 64);
        resizing.resized_at = Some((2, 30, 20));
        let (status, fake) = run(&[c"capture", c"129"], resizing);
        assert_eq!(status, 0);
        let (width, height, _) = image(&fake.contents("window-129.png"));
        assert_eq!((width, height), (30, 20));
    }

    #[test]
    fn bad_options_are_usage_errors() {
        for arguments in [
            &[c"capture"][..],
            &[c"capture", c"129", c"130"][..],
            &[c"capture", c"--max-width", c"0", c"129"][..],
            &[c"capture", c"--max-bytes", c"x", c"129"][..],
            &[c"capture", c"--scale", c"2", c"129"][..],
            &[c"capture", c"-o"][..],
        ] {
            let (status, fake) = run(arguments, machine(10, 10));
            assert_eq!(status, 2, "{arguments:?}");
            assert_eq!(fake.capture_calls, 0);
        }
    }
}
