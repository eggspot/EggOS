use crate::options::parse_defined;
use crate::output::{FAILURE, Output, decimal};
use crate::system::{SymbolicLinks, System};
use crate::window_capture::{Attempt, Destination, Encoded, Failure, Source, encode_to, positive};
use crate::wm::{USAGE_STATUS, number, open_device};
use crate::zone::{local_seconds, local_zone};
use core::ffi::CStr;
use eggos_calendar::DateTime;
use eggos_commands::{commands, errors};
use eggos_png::scale::scaled_size;
use eggos_runtime::errno::Errno;

pub const CAPTURE_SCREEN: u32 = 0xc030_773d;
pub const CAPTURE_SCREEN_SIZE: usize = 48;
pub const SHOW_NOTICE: u32 = 0x4048_773e;
pub const SHOW_NOTICE_SIZE: usize = 72;
pub const NOTICE_TEXT: usize = 8;
pub const NOTICE_CAPACITY: usize = 64;
pub const PICK_REGION: u32 = 0x4004_773f;
pub const PICK_REGION_SIZE: usize = 4;

const NOTICE_MS: u32 = 3000;
const PATH_CAPACITY: usize = 256;
const DIRECTORY_MODE: u32 = 0o755;
const CLOCK_SET_SINCE: i64 = 1_577_836_800;
const MOST_IN_ONE_SECOND: u32 = 99;
const DEFAULT_HOME: &[u8] = b"/home/root";
const FOLDERS: [&[u8]; 2] = [b"Pictures", b"Screenshots"];
const SAVED_NOTICE: &[u8] = b"Saved to Pictures/Screenshots";
const SAVED_ELSEWHERE_NOTICE: &[u8] = b"Screenshot saved";
const SAVE_FAILED_NOTICE: &[u8] = b"Could not save the screenshot (";
const CAPTURE_FAILED_NOTICE: &[u8] = b"Could not capture the screen (";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Screen,
    Window(u32),
    Region([u32; 4]),
    Pick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Options<'a> {
    target: Target,
    targets: u32,
    file: Option<&'a CStr>,
    max_width: Option<&'a [u8]>,
    max_bytes: Option<&'a [u8]>,
    notify: bool,
}

#[derive(Clone, Copy)]
struct PathText {
    bytes: [u8; PATH_CAPACITY],
    length: usize,
}

impl PathText {
    fn new() -> Self {
        Self {
            bytes: [0; PATH_CAPACITY],
            length: 0,
        }
    }

    fn push(&mut self, part: &[u8]) -> Result<(), Errno> {
        let end = self.length + part.len();
        if end >= PATH_CAPACITY {
            return Err(Errno::ENAMETOOLONG);
        }
        self.bytes[self.length..end].copy_from_slice(part);
        self.length = end;
        Ok(())
    }

    fn push_padded(&mut self, value: i64, width: usize) -> Result<(), Errno> {
        let mut digits = [0u8; 20];
        let text = decimal(value.unsigned_abs(), &mut digits);
        for _ in text.len()..width {
            self.push(b"0")?;
        }
        self.push(text)
    }

    fn c_str(&mut self) -> &CStr {
        self.bytes[self.length] = 0;
        CStr::from_bytes_with_nul(&self.bytes[..=self.length]).unwrap_or(c"")
    }
}

pub fn region_of(text: &[u8]) -> Option<[u32; 4]> {
    let mut values = [0u32; 4];
    let mut parts = text.split(|&byte| byte == b',');
    for value in &mut values {
        *value = number(parts.next()?).and_then(|value| u32::try_from(value).ok())?;
    }
    (parts.next().is_none() && values[2] > 0 && values[3] > 0).then_some(values)
}

fn parse<'a, S: System>(output: &mut Output<S>, arguments: &[&'a CStr]) -> Option<Options<'a>> {
    let mut options = Options {
        target: Target::Screen,
        targets: 0,
        file: None,
        max_width: None,
        max_bytes: None,
        notify: false,
    };
    let operands = parse_defined(output, arguments, &commands::SCREENSHOT, |output, given| {
        let value = given.value.unwrap_or_default();
        match (given.option.short, given.option.long) {
            (Some(b'o'), _) => options.file = given.value_as_c_string(),
            (_, Some("max-width")) => options.max_width = given.value,
            (_, Some("max-bytes")) => options.max_bytes = given.value,
            (_, Some("notify")) => options.notify = true,
            (_, Some("pick")) => {
                options.target = Target::Pick;
                options.targets += 1;
            }
            (_, Some("window")) => {
                let Some(window) = number(value)
                    .and_then(|window| u32::try_from(window).ok())
                    .filter(|&window| window > 0)
                else {
                    output.coded(&errors::INVALID_VALUE, &[b"invalid window id '", value, b"'"]);
                    return Err(crate::options::Refused::Reported);
                };
                options.target = Target::Window(window);
                options.targets += 1;
            }
            (_, Some("region")) => {
                let Some(region) = region_of(value) else {
                    output.coded(
                        &errors::INVALID_VALUE,
                        &[b"invalid region '", value, b"': use x,y,w,h with w and h at least 1"],
                    );
                    return Err(crate::options::Refused::Reported);
                };
                options.target = Target::Region(region);
                options.targets += 1;
            }
            _ => {}
        }
        Ok(())
    })
    .ok()?;
    if !operands.all().is_empty() || options.targets > 1 {
        output.coded(
            &errors::USAGE,
            &[b"usage: screenshot [--window <id> | --region <x>,<y>,<w>,<h> | --pick] [-o <file>] \
               [--max-width <n>] [--max-bytes <n>] [--json] [--notify]"],
        );
        return None;
    }
    Some(options)
}

pub fn screenshot<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let Some(options) = parse(output, arguments) else {
        return USAGE_STATUS;
    };
    let (Ok(max_width), Ok(max_bytes)) = (positive(options.max_width), positive(options.max_bytes))
    else {
        output.coded(
            &errors::USAGE,
            &[b"--max-width and --max-bytes take a positive number"],
        );
        return USAGE_STATUS;
    };
    let Some(device) = open_device(output) else {
        return FAILURE;
    };
    let status = match options.target {
        Target::Pick => pick(output, device),
        target => shoot(output, device, target, &options, max_width, max_bytes),
    };
    output.system.close(device);
    status
}

fn pick<S: System>(output: &mut Output<S>, device: usize) -> i32 {
    let mut argument = [0u8; PICK_REGION_SIZE];
    match output
        .system
        .device_control(device, PICK_REGION, &mut argument)
    {
        Ok(()) => 0,
        Err(errno) => {
            output.coded(
                &errors::NO_WINDOW_SERVER,
                &[b"the region picker did not open: ", errno.message()],
            );
            FAILURE
        }
    }
}

fn shoot<S: System>(
    output: &mut Output<S>,
    device: usize,
    target: Target,
    options: &Options<'_>,
    max_width: Option<u32>,
    max_bytes: Option<u32>,
) -> i32 {
    let source = match target {
        Target::Window(window) => Source::Screen {
            region: [0; 4],
            window,
        },
        Target::Region(region) => Source::Screen { region, window: 0 },
        _ => Source::Screen {
            region: [0; 4],
            window: 0,
        },
    };
    let mut chosen = PathText::new();
    let destination = match options.file {
        Some(file) if file.to_bytes() == b"-" => Destination::StandardOutput,
        Some(file) => Destination::File(file),
        None => match default_path(&mut output.system) {
            Ok(path) => {
                chosen = path;
                Destination::File(chosen.c_str())
            }
            Err(errno) => {
                output.coded(
                    &errors::WRITE_FAILED,
                    &[b"~/Pictures/Screenshots: ", errno.message()],
                );
                notify(output, device, options.notify, Err((true, errno)));
                return FAILURE;
            }
        },
    };
    let encoded = encode_to(
        &mut output.system,
        device,
        source,
        destination,
        max_width,
        max_bytes,
    );
    match encoded {
        Ok(encoded) => {
            let saved = match (options.file, &destination) {
                (None, _) => SAVED_NOTICE,
                _ => SAVED_ELSEWHERE_NOTICE,
            };
            if let Destination::File(file) = destination
                && !options.notify
            {
                report(output, file, target, &encoded);
            }
            notify(output, device, options.notify, Ok(saved));
            0
        }
        Err(failure) => {
            let (saving, errno) = explain(output, failure, target, destination);
            notify(output, device, options.notify, Err((saving, errno)));
            FAILURE
        }
    }
}

fn explain<S: System>(
    output: &mut Output<S>,
    failure: Failure,
    target: Target,
    destination: Destination<'_>,
) -> (bool, Errno) {
    let path = match destination {
        Destination::File(file) => file.to_bytes(),
        Destination::StandardOutput => b"standard output",
    };
    let refused = match failure {
        Failure::Create(errno) | Failure::Failed(Attempt::WriteFailed(errno)) => {
            output.coded(&errors::WRITE_FAILED, &[path, b": ", errno.message()]);
            return (true, errno);
        }
        Failure::Refused(errno) | Failure::Failed(Attempt::Refused(errno)) => errno,
        Failure::Failed(Attempt::TooBig) => {
            output.error(&[b"the PNG does not fit in --max-bytes"]);
            return (false, Errno::ENOSPC);
        }
        Failure::Unsized => {
            output.error(&[b"the screen is too wide to capture"]);
            return (false, Errno::EINVAL);
        }
        Failure::Failed(_) => {
            output.error(&[b"the screen kept changing size; try again"]);
            return (false, Errno::EAGAIN);
        }
    };
    let mut digits = [0u8; 20];
    match (refused, target) {
        (Errno::ENOENT, Target::Window(window)) => output.coded(
            &errors::NO_SUCH_WINDOW,
            &[
                b"no window ",
                decimal(u64::from(window), &mut digits),
            ],
        ),
        (Errno::EINVAL, Target::Region(_)) => output.coded(
            &errors::INVALID_VALUE,
            &[b"the region is not inside the screen"],
        ),
        (errno, _) => output.coded(
            &errors::NO_WINDOW_SERVER,
            &[b"the screen could not be captured: ", errno.message()],
        ),
    }
    (false, refused)
}

fn default_path<S: System>(system: &mut S) -> Result<PathText, Errno> {
    let mut folder = PathText::new();
    match system.variable(b"HOME") {
        Some(home) if home.starts_with(b"/") => folder.push(home)?,
        _ => folder.push(DEFAULT_HOME)?,
    }
    for name in FOLDERS {
        folder.push(b"/")?;
        folder.push(name)?;
        match system.make_directory(folder.c_str(), DIRECTORY_MODE) {
            Ok(()) | Err(Errno::EEXIST) => {}
            Err(errno) => return Err(errno),
        }
    }
    let utc = system.now() as i64;
    let moment = (utc >= CLOCK_SET_SINCE).then(|| {
        let zone = local_zone(system);
        DateTime::from_unix(local_seconds(&zone, utc))
    });
    for attempt in 1..=MOST_IN_ONE_SECOND {
        let mut path = folder;
        path.push(b"/Screenshot")?;
        if let Some(moment) = moment {
            path.push(b" ")?;
            path.push_padded(moment.year, 4)?;
            for (separator, value) in [
                (b"-", moment.month),
                (b"-", moment.day),
                (b" ", moment.hour),
                (b".", moment.minute),
                (b".", moment.second),
            ] {
                path.push(separator)?;
                path.push_padded(i64::from(value), 2)?;
            }
        }
        if attempt > 1 {
            path.push(b" ")?;
            path.push_padded(i64::from(attempt), 1)?;
        }
        path.push(b".png")?;
        match system.status(None, path.c_str(), SymbolicLinks::DoNotFollow) {
            Err(Errno::ENOENT) => return Ok(path),
            Ok(_) => {}
            Err(errno) => return Err(errno),
        }
    }
    Err(Errno::EEXIST)
}

fn report<S: System>(output: &mut Output<S>, file: &CStr, target: Target, encoded: &Encoded) {
    let width = encoded.size.width;
    let height = encoded.size.height;
    let (image_width, image_height) = scaled_size(width, height, encoded.factor);
    if output.is_json() {
        output.write(b"{");
        for (index, (key, value)) in [
            (&b"width"[..], width),
            (&b"height"[..], height),
            (&b"image_width"[..], image_width),
            (&b"image_height"[..], image_height),
            (&b"scale"[..], encoded.factor),
        ]
        .into_iter()
        .enumerate()
        {
            if index > 0 {
                output.write(b",");
            }
            write_key(output, key);
            output.write_number(u64::from(value), 0);
        }
        output.write(b",");
        write_key(output, b"bytes");
        output.write_number(encoded.bytes, 0);
        match target {
            Target::Region([x, y, ..]) => {
                output.write(b",");
                write_key(output, b"x");
                output.write_number(u64::from(x), 0);
                output.write(b",");
                write_key(output, b"y");
                output.write_number(u64::from(y), 0);
            }
            Target::Window(window) => {
                output.write(b",");
                write_key(output, b"window");
                output.write_number(u64::from(window), 0);
            }
            _ => {}
        }
        output.write(b",");
        write_key(output, b"path");
        write_json_text(output, file.to_bytes());
        output.write(b"}\n");
        return;
    }
    output.write(b"Saved ");
    output.write(file.to_bytes());
    output.write(b" (");
    output.write_number(u64::from(width), 0);
    output.write(b"x");
    output.write_number(u64::from(height), 0);
    if encoded.factor > 1 {
        output.write(b" as ");
        output.write_number(u64::from(image_width), 0);
        output.write(b"x");
        output.write_number(u64::from(image_height), 0);
    }
    output.write(b")\n");
}

fn write_key<S: System>(output: &mut Output<S>, key: &[u8]) {
    output.write(b"\"");
    output.write(key);
    output.write(b"\":");
}

fn write_json_text<S: System>(output: &mut Output<S>, text: &[u8]) {
    output.write(b"\"");
    for &byte in text {
        match byte {
            b'"' => output.write(b"\\\""),
            b'\\' => output.write(b"\\\\"),
            byte if byte < b' ' => {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                output.write(&[
                    b'\\',
                    b'u',
                    b'0',
                    b'0',
                    HEX[usize::from(byte >> 4)],
                    HEX[usize::from(byte & 15)],
                ]);
            }
            byte => output.write(&[byte]),
        }
    }
    output.write(b"\"");
}

fn errno_name(errno: Errno) -> &'static [u8] {
    match errno {
        Errno::EROFS => b"EROFS",
        Errno::ENOSPC => b"ENOSPC",
        Errno::EACCES => b"EACCES",
        Errno::EPERM => b"EPERM",
        Errno::EINVAL => b"EINVAL",
        Errno::ENOENT => b"ENOENT",
        Errno::EIO => b"EIO",
        Errno::ENOMEM => b"ENOMEM",
        Errno::ENXIO => b"ENXIO",
        Errno::EEXIST => b"EEXIST",
        Errno::ENAMETOOLONG => b"ENAMETOOLONG",
        Errno::EAGAIN => b"EAGAIN",
        _ => b"error",
    }
}

fn notify<S: System>(
    output: &mut Output<S>,
    device: usize,
    wanted: bool,
    outcome: Result<&[u8], (bool, Errno)>,
) {
    if !wanted {
        return;
    }
    let mut argument = [0u8; SHOW_NOTICE_SIZE];
    let mut length = 0;
    let mut put = |part: &[u8]| {
        let start = NOTICE_TEXT + length;
        let taken = part.len().min(NOTICE_CAPACITY - length);
        argument[start..start + taken].copy_from_slice(&part[..taken]);
        length += taken;
    };
    match outcome {
        Ok(text) => put(text),
        Err((saving, errno)) => {
            put(match saving {
                true => SAVE_FAILED_NOTICE,
                false => CAPTURE_FAILED_NOTICE,
            });
            put(errno_name(errno));
            put(b")");
        }
    }
    argument[..4].copy_from_slice(&(length as u32).to_ne_bytes());
    argument[4..8].copy_from_slice(&NOTICE_MS.to_ne_bytes());
    let _ = output
        .system
        .device_control(device, SHOW_NOTICE, &mut argument);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{Fake, FakeWindow, Node};
    use eggos_png::{decode, decoded_size};

    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 40;

    fn screen() -> Vec<u32> {
        (0..WIDTH * HEIGHT)
            .map(|index| (index % WIDTH) * 0x0001_0203 + (index / WIDTH) * 0x0003_0100)
            .map(|pixel| pixel & 0x00ff_ffff)
            .collect()
    }

    fn machine() -> Fake {
        let mut fake = Fake::default();
        fake.screen_pixels = Some((WIDTH, HEIGHT, screen()));
        fake.windows.push(FakeWindow {
            id: 129,
            state: 0,
            rect: [8, 4, 20, 10],
            client: 67,
            title: b"Notes".to_vec(),
        });
        fake.now = 1_793_455_512;
        fake.with_directory("/home", 0)
            .with_directory("/home/root", 0)
    }

    fn run(arguments: &[&CStr], fake: Fake) -> (i32, Fake) {
        let mut output = Output::new(fake, b"screenshot");
        let status = screenshot(&mut output, arguments);
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

    fn saved(fake: &Fake) -> Vec<(Vec<u8>, Vec<u8>)> {
        fake.nodes
            .iter()
            .filter(|(path, _)| path.starts_with(b"/home/root/Pictures/Screenshots/"))
            .filter_map(|(path, (node, _))| match node {
                Node::File(content) => Some((path.clone(), content.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_whole_screen_goes_to_pictures_screenshots_named_by_the_local_time() {
        let (status, fake) = run(&[], machine());
        assert_eq!(status, 0);
        let files = saved(&fake);
        assert_eq!(files.len(), 1);
        let (name, png) = &files[0];
        assert!(name.starts_with(b"/home/root/Pictures/Screenshots/Screenshot 20"));
        assert!(name.ends_with(b".png"));
        assert_eq!(image(png), (WIDTH, HEIGHT, screen()));
        let text = String::from_utf8(fake.standard_output.clone()).unwrap();
        assert!(text.starts_with("Saved /home/root/Pictures/Screenshots/Screenshot "));
        assert!(text.ends_with(" (64x40)\n"));
    }

    #[test]
    fn a_second_shot_in_the_same_second_gets_a_number() {
        let (_, fake) = run(&[], machine());
        let (status, fake) = run(&[], fake);
        assert_eq!(status, 0);
        let files = saved(&fake);
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|(name, _)| name.ends_with(b" 2.png")));
    }

    #[test]
    fn an_unset_clock_names_it_screenshot_png() {
        let mut fake = machine();
        fake.now = 5;
        let (status, fake) = run(&[], fake);
        assert_eq!(status, 0);
        assert_eq!(
            saved(&fake)[0].0,
            b"/home/root/Pictures/Screenshots/Screenshot.png"
        );
    }

    #[test]
    fn a_region_and_a_window_are_cut_from_the_screen() {
        let (status, fake) = run(
            &[c"--region", c"2,3,10,5", c"-o", c"/home/root/r.png"],
            machine(),
        );
        assert_eq!(status, 0);
        let (width, height, pixels) = image(&fake.contents("/home/root/r.png"));
        assert_eq!((width, height), (10, 5));
        assert_eq!(pixels[0], screen()[(3 * WIDTH + 2) as usize]);
        let (status, fake) = run(
            &[c"--window", c"129", c"-o", c"/home/root/w.png", c"--json"],
            fake,
        );
        assert_eq!(status, 0);
        let (width, height, _) = image(&fake.contents("/home/root/w.png"));
        assert_eq!((width, height), (20, 10));
        let line = String::from_utf8(fake.standard_output.clone()).unwrap();
        assert!(line.contains(r#""width":20,"height":10,"#), "{line}");
        assert!(line.contains(r#""window":129"#), "{line}");
        assert!(line.contains(r#""path":"/home/root/w.png""#), "{line}");
    }

    #[test]
    fn notify_shows_the_outcome_on_screen_and_prints_nothing() {
        let (status, fake) = run(&[c"--notify"], machine());
        assert_eq!(status, 0);
        assert!(fake.standard_output.is_empty());
        assert_eq!(fake.notices, [SAVED_NOTICE.to_vec()]);
        let (status, fake) = run(&[c"--notify", c"--region", c"60,30,10,10"], machine());
        assert_eq!(status, 1);
        assert_eq!(fake.notices, [b"Could not capture the screen (EINVAL)".to_vec()]);
    }

    #[test]
    fn bad_targets_are_refused_with_their_reason() {
        let (status, fake) = run(&[c"--window", c"7"], machine());
        assert_eq!(status, 1);
        assert!(String::from_utf8_lossy(&fake.standard_error).contains("no window 7"));
        let (status, _) = run(&[c"--region", c"1,2,0,4"], machine());
        assert_eq!(status, 2);
        let (status, _) = run(&[c"--pick", c"--window", c"129"], machine());
        assert_eq!(status, 2);
    }

    #[test]
    fn pick_opens_the_picker_and_returns() {
        let (status, fake) = run(&[c"--pick"], machine());
        assert_eq!(status, 0);
        assert_eq!(fake.picks, 1);
        assert!(saved(&fake).is_empty());
    }

    #[test]
    fn the_protocol_numbers_match_the_window_server() {
        use eggos_drivers::desktop::protocol::{request, size};
        assert_eq!(CAPTURE_SCREEN, request::CAPTURE_SCREEN);
        assert_eq!(CAPTURE_SCREEN_SIZE, size::CAPTURE_SCREEN);
        assert_eq!(SHOW_NOTICE, request::SHOW_NOTICE);
        assert_eq!(SHOW_NOTICE_SIZE, size::SHOW_NOTICE);
        assert_eq!(PICK_REGION, request::PICK_REGION);
        assert_eq!(PICK_REGION_SIZE, size::PICK_REGION);
        assert_eq!(
            NOTICE_CAPACITY,
            eggos_drivers::desktop::protocol::NOTICE_TEXT_CAPACITY
        );
        assert_eq!(NOTICE_TEXT, eggos_drivers::desktop::protocol::NOTICE_TEXT);
    }
}
