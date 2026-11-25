//! The sound server's and `sound`'s system calls (D774): run as `soundd`, the program listens
//! on `/run/soundd`, waits in `ppoll` for its clients, mixes their streams and the test tone
//! into `/dev/sound` a few milliseconds ahead of the device, and saves every change to the
//! settings file; run as `sound`, it is the command, a client of the server.

use crate::command::{self, Command, FAILURE_STATUS, NO_DEVICE, Refusal, USAGE_STATUS};
use crate::mixer::{self, Stream};
use crate::protocol::{self, App, Change, Device, Kind, Request, State, TEST_TONE_HZ};
use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::CStr;
use eggos_runtime::errno::Errno;
use eggos_runtime::startup::Startup;
use eggos_runtime::syscall::{self, number};
use eggos_settings_file as settings_file;

type Fd = i32;

const STANDARD_OUTPUT: usize = 1;
const STANDARD_ERROR: usize = 2;
const AT_FDCWD: usize = -100isize as usize;
const AF_UNIX: usize = 1;
const SOCK_STREAM: usize = 1;
const SOCK_NONBLOCK: usize = 0o4000;
const SOCK_CLOEXEC: usize = 0o2_000_000;
const O_RDONLY: usize = 0;
const O_WRONLY: usize = 1;
const O_RDWR: usize = 2;
const DSP_RECORD_BYTES: usize = 16 + 8192;
const DSP_HEADER: usize = 16;
const DSP_ENDED: u32 = 1;
const APPS_PATH: &CStr = c"/var/lib/eggos/sound-apps";
const MOST_APPS: usize = 64;
const DSP_APP: &str = "oss";
const O_CREAT: usize = 0o100;
const O_TRUNC: usize = 0o1000;
const O_CLOEXEC: usize = 0o2_000_000;
const MSG_NOSIGNAL: usize = 0x4000;
const POLLIN: i16 = 1;
const POLLERR: i16 = 8;
const POLLHUP: i16 = 0x10;
const CLOCK_MONOTONIC: usize = 1;
const LISTEN_BACKLOG: usize = 16;
const MOST_CLIENTS: usize = 32;
const CHUNK_FRAMES: usize = 256;
const WAKE_MS: i64 = 5;
const LINE_BYTES: usize = 256;
const READ_BYTES: usize = 8192;
const SETTINGS_BYTES: usize = 16 * 1024;
const DEVICE_PATH: &CStr = c"/dev/sound";
const WINDOW_DEVICE: &CStr = c"/dev/window";
const SET_SOUND: usize = 0x4020_7746;
const SHELL_NAME_BYTES: usize = 28;
const DRIVER_PATH: &CStr = c"/proc/driver/sound";
const SOCKET: &CStr = c"/run/soundd";
const SIGPIPE: usize = eggos_runtime::signal::SIGPIPE as usize;
const SIGNAL_SET_BYTES: usize = eggos_runtime::signal::SET_BYTES;

#[repr(C)]
struct SocketAddress {
    family: u16,
    path: [u8; 108],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PollEntry {
    descriptor: i32,
    events: i16,
    returned: i16,
}

fn call(number: usize, arguments: [usize; 6]) -> Result<usize, Errno> {
    // SAFETY: the calls pass only memory this program owns, sized as the call uses it.
    Errno::result(unsafe { syscall::call(number, arguments) })
}

fn say(descriptor: usize, text: &str) {
    syscall::write_all(descriptor, text.as_bytes());
}

fn close(descriptor: Fd) {
    let _ = call(number::CLOSE, [descriptor as usize, 0, 0, 0, 0, 0]);
}

fn now_ms() -> u64 {
    let mut time = [0i64; 2];
    let _ = call(
        number::CLOCK_GETTIME,
        [CLOCK_MONOTONIC, time.as_mut_ptr() as usize, 0, 0, 0, 0],
    );
    time[0] as u64 * 1000 + time[1] as u64 / 1_000_000
}

fn address() -> (SocketAddress, usize) {
    let mut address = SocketAddress {
        family: AF_UNIX as u16,
        path: [0; 108],
    };
    let bytes = SOCKET.to_bytes();
    address.path[..bytes.len()].copy_from_slice(bytes);
    (address, 2 + bytes.len() + 1)
}

fn read_whole(path: &CStr, most: usize) -> Option<Vec<u8>> {
    let descriptor = call(
        number::OPENAT,
        [AT_FDCWD, path.as_ptr() as usize, O_RDONLY | O_CLOEXEC, 0, 0, 0],
    )
    .ok()? as Fd;
    let mut bytes = alloc::vec![0u8; most];
    let mut length = 0;
    while length < most {
        match call(
            number::READ,
            [
                descriptor as usize,
                bytes[length..].as_mut_ptr() as usize,
                most - length,
                0,
                0,
                0,
            ],
        ) {
            Ok(0) | Err(_) => break,
            Ok(read) => length += read,
        }
    }
    close(descriptor);
    bytes.truncate(length);
    Some(bytes)
}

fn devices() -> Vec<Device> {
    let text = read_whole(DRIVER_PATH, 4096).unwrap_or_default();
    Device::from_driver_text(core::str::from_utf8(&text).unwrap_or(""))
}

fn settings_path() -> alloc::ffi::CString {
    alloc::ffi::CString::new(settings_file::PATH).unwrap_or_default()
}

fn save_setting(key: &str, value: &str) -> Result<(), Errno> {
    let old = read_whole(&settings_path(), SETTINGS_BYTES).unwrap_or_default();
    let mut new = alloc::vec![0u8; SETTINGS_BYTES];
    let length = settings_file::with_value(&old, key, value.as_bytes(), &mut new)
        .map_err(|_| Errno::EINVAL)?;
    let mut name = [0u8; settings_file::TEMPORARY_PATH_CAPACITY];
    let pid = call(number::GETPID, [0; 6]).unwrap_or(0) as u32;
    let temporary = settings_file::temporary_path(pid, &mut name);
    let temporary = alloc::ffi::CString::new(temporary).map_err(|_| Errno::EINVAL)?;
    let descriptor = call(
        number::OPENAT,
        [
            AT_FDCWD,
            temporary.as_ptr() as usize,
            O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC,
            0o644,
            0,
            0,
        ],
    )? as Fd;
    let written = syscall::write_all(descriptor as usize, &new[..length]);
    close(descriptor);
    if !written {
        return Err(Errno::EIO);
    }
    call(
        number::RENAMEAT2,
        [
            AT_FDCWD,
            temporary.as_ptr() as usize,
            AT_FDCWD,
            settings_path().as_ptr() as usize,
            0,
            0,
        ],
    )
    .map(|_| ())
}

struct Client {
    socket: Fd,
    line: Vec<u8>,
    stream: Option<usize>,
    watching: bool,
}

struct Server {
    device: Fd,
    shell: Option<Fd>,
    devices: Vec<Device>,
    wanted_output: String,
    wanted_input: String,
    state: State,
    rate: u32,
    clients: Vec<Option<Client>>,
    streams: Vec<(usize, Stream)>,
    next_stream: usize,
    tone: Option<usize>,
    dsp: Option<(u32, usize)>,
    pending: Vec<u8>,
    apps: Vec<(String, u8, bool)>,
    stream_apps: Vec<(usize, String)>,
    alert: String,
    alert_volume: u8,
}

fn read_apps() -> Vec<(String, u8, bool)> {
    let text = read_whole(APPS_PATH, 8192).unwrap_or_default();
    core::str::from_utf8(&text)
        .unwrap_or("")
        .lines()
        .filter_map(|line| {
            let mut words = line.split(' ');
            let id = String::from(words.next()?);
            let volume = words.next()?.parse().ok().filter(|&volume: &u8| volume <= 100)?;
            let mute = words.next()? == "true";
            Some((id, volume, mute))
        })
        .take(MOST_APPS)
        .collect()
}

fn write_apps(apps: &[(String, u8, bool)]) {
    let mut text = String::new();
    for (id, volume, mute) in apps {
        text.push_str(&alloc::format!("{id} {volume} {mute}\n"));
    }
    let Ok(descriptor) = call(
        number::OPENAT,
        [
            AT_FDCWD,
            APPS_PATH.as_ptr() as usize,
            O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC,
            0o644,
            0,
            0,
        ],
    ) else {
        return;
    };
    syscall::write_all(descriptor, text.as_bytes());
    close(descriptor as Fd);
}

impl Server {
    fn open() -> Result<Self, Errno> {
        let device = call(
            number::OPENAT,
            [AT_FDCWD, DEVICE_PATH.as_ptr() as usize, O_RDWR | O_CLOEXEC, 0, 0, 0],
        )? as Fd;
        let devices = devices();
        let file = read_whole(&settings_path(), SETTINGS_BYTES).unwrap_or_default();
        let saved = settings_file::sound_settings(&file);
        let wanted_output = String::from(core::str::from_utf8(saved.output).unwrap_or("auto"));
        let wanted_input = String::from(core::str::from_utf8(saved.input).unwrap_or("auto"));
        let output = protocol::chosen(&devices, Kind::Output, &wanted_output)
            .or_else(|| protocol::chosen(&devices, Kind::Output, "auto"));
        let rate = output.map_or(48_000, |device| device.rate.max(1));
        let state = State {
            output: output.map(|device| device.id.clone()),
            volume: saved.volume,
            mute: saved.mute,
            input: protocol::chosen(&devices, Kind::Input, &wanted_input)
                .map(|device| device.id.clone()),
            input_volume: saved.input_volume,
            input_mute: saved.input_mute,
        };
        let alert = String::from(core::str::from_utf8(saved.alert).unwrap_or("chime"));
        let alert_volume = saved.alert_volume;
        let shell = call(
            number::OPENAT,
            [AT_FDCWD, WINDOW_DEVICE.as_ptr() as usize, 2 | O_CLOEXEC, 0, 0, 0],
        )
        .ok()
        .map(|descriptor| descriptor as Fd);
        Ok(Self {
            device,
            shell,
            devices,
            wanted_output,
            wanted_input,
            state,
            rate,
            clients: (0..MOST_CLIENTS).map(|_| None).collect(),
            streams: Vec::new(),
            next_stream: 0,
            tone: None,
            dsp: None,
            pending: Vec::new(),
            apps: read_apps(),
            stream_apps: Vec::new(),
            alert,
            alert_volume,
        })
    }

    fn reply(&self, socket: Fd, text: &str) {
        let mut line = String::from(text);
        line.push('\n');
        let _ = call(
            number::SENDTO,
            [
                socket as usize,
                line.as_ptr() as usize,
                line.len(),
                MSG_NOSIGNAL,
                0,
                0,
            ],
        );
    }

    fn tell_watchers(&self) {
        let line = self.state.line();
        for client in self.clients.iter().flatten().filter(|client| client.watching) {
            self.reply(client.socket, &line);
        }
        self.tell_shell();
    }

    fn tell_shell(&self) {
        let Some(shell) = self.shell else {
            return;
        };
        let mut record = [0u8; 4 + SHELL_NAME_BYTES];
        let output = self.state.output.as_deref().and_then(|id| {
            self.devices.iter().find(|device| device.id == id)
        });
        record[0] = u8::from(output.is_some());
        record[1] = self.state.volume;
        record[2] = u8::from(self.state.mute);
        let outputs = self.devices.iter().filter(|device| device.kind == Kind::Output);
        record[3] = outputs.count().min(255) as u8;
        if let Some(output) = output {
            let name = output.name.as_bytes();
            let length = name.len().min(SHELL_NAME_BYTES);
            record[4..4 + length].copy_from_slice(&name[..length]);
        }
        let _ = call(
            number::IOCTL,
            [shell as usize, SET_SOUND, record.as_ptr() as usize, 0, 0, 0],
        );
    }

    fn apply(&mut self, change: Change) -> Result<(), &'static str> {
        match &change {
            Change::Output(id) => {
                let device = protocol::chosen(&self.devices, Kind::Output, id)
                    .ok_or("no_such_device")?;
                self.state.output = Some(device.id.clone());
                self.wanted_output = id.clone();
            }
            Change::Input(id) => {
                let device = protocol::chosen(&self.devices, Kind::Input, id);
                if device.is_none() && id != "auto" {
                    return Err("no_such_device");
                }
                self.state.input = device.map(|device| device.id.clone());
                self.wanted_input = id.clone();
            }
            Change::Volume(volume) => self.state.volume = *volume,
            Change::Mute(mute) => self.state.mute = *mute,
            Change::InputVolume(volume) => self.state.input_volume = *volume,
            Change::InputMute(mute) => self.state.input_mute = *mute,
            Change::Alert(name) => self.alert = name.clone(),
            Change::AlertVolume(volume) => self.alert_volume = *volume,
            Change::AppVolume(app, _) | Change::AppMute(app, _) => {
                let open = self.stream_apps.iter().any(|(_, each)| each == app);
                let known = self.apps.iter().any(|(each, ..)| each == app);
                if !open && !known {
                    return Err("no_such_app");
                }
                let record = self.app_record(app);
                match &change {
                    Change::AppVolume(_, volume) => record.1 = *volume,
                    Change::AppMute(_, mute) => record.2 = *mute,
                    _ => {}
                }
                let gain = app_gain(record);
                write_apps(&self.apps);
                let ids: Vec<usize> = self
                    .stream_apps
                    .iter()
                    .filter(|(_, each)| each == app)
                    .map(|(id, _)| *id)
                    .collect();
                for id in ids {
                    if let Some(stream) = self.stream_mut(id) {
                        stream.set_volume(gain);
                    }
                }
            }
        }
        if change.is_saved_in_settings() {
            let (key, value) = change.setting();
            let _ = save_setting(&key, &value);
        }
        self.tell_watchers();
        Ok(())
    }

    fn app_record(&mut self, app: &str) -> &mut (String, u8, bool) {
        let at = match self.apps.iter().position(|(each, ..)| each == app) {
            Some(at) => at,
            None => {
                if self.apps.len() >= MOST_APPS {
                    self.apps.remove(0);
                }
                self.apps.push((String::from(app), 100, false));
                self.apps.len() - 1
            }
        };
        &mut self.apps[at]
    }

    fn start_stream(&mut self, stream: Stream, app: &str) -> usize {
        let gain = app_gain(self.app_record(app));
        let id = self.next_stream;
        self.next_stream += 1;
        let mut stream = stream;
        stream.set_volume(gain);
        self.streams.push((id, stream));
        self.stream_apps.push((id, String::from(app)));
        id
    }

    fn play_alert(&mut self) {
        if self.state.mute || self.state.output.is_none() {
            return;
        }
        let samples = mixer::alert(&self.alert, self.rate);
        if samples.is_empty() {
            return;
        }
        let mut stream = Stream::new(self.rate, 2);
        stream.push(&samples);
        stream.end();
        stream.set_volume(self.alert_volume);
        let id = self.next_stream;
        self.next_stream += 1;
        self.streams.push((id, stream));
    }

    fn open_apps(&self) -> Vec<App> {
        let mut listed: Vec<App> = Vec::new();
        for (_, app) in &self.stream_apps {
            if listed.iter().any(|each| &each.id == app) {
                continue;
            }
            let (volume, mute) = self
                .apps
                .iter()
                .find(|(each, ..)| each == app)
                .map_or((100, false), |(_, volume, mute)| (*volume, *mute));
            listed.push(App {
                id: app.clone(),
                volume,
                mute,
                playing: true,
                name: App::name_of(app),
            });
        }
        listed.sort_by(|a, b| a.name.cmp(&b.name));
        listed
    }

    fn start_tone(&mut self, milliseconds: u32) {
        let frames = (u64::from(self.rate) * u64::from(milliseconds) / 1000) as usize;
        let mut stream = Stream::new(self.rate, 2);
        stream.push(&mixer::tone(TEST_TONE_HZ, self.rate, frames));
        stream.end();
        let id = self.next_stream;
        self.next_stream += 1;
        self.streams.push((id, stream));
        self.tone = Some(id);
    }

    fn handle(&mut self, index: usize, request: Request) {
        let Some(socket) = self.clients[index].as_ref().map(|client| client.socket) else {
            return;
        };
        match request {
            Request::Get => self.reply(socket, &self.state.line()),
            Request::Devices => {
                for device in &self.devices {
                    self.reply(socket, &device.line());
                }
                self.reply(socket, "end");
            }
            Request::Level => self.reply(socket, "level 0"),
            Request::Watch => {
                if let Some(client) = self.clients[index].as_mut() {
                    client.watching = true;
                }
                self.reply(socket, &self.state.line());
            }
            Request::Set(change) => match self.apply(change) {
                Ok(()) => self.reply(socket, "ok"),
                Err(code) => self.reply(socket, &alloc::format!("error {code}")),
            },
            Request::Test { milliseconds } => {
                if self.state.output.is_none() {
                    self.reply(socket, "error no_sound_device");
                    return;
                }
                self.start_tone(milliseconds);
                self.reply(socket, "ok");
            }
            Request::Play {
                rate,
                channels,
                app,
            } => {
                let id = self.start_stream(Stream::new(rate, channels), &app);
                if let Some(client) = self.clients[index].as_mut() {
                    client.stream = Some(id);
                }
                self.reply(socket, "ok");
            }
            Request::Apps => {
                for app in self.open_apps() {
                    self.reply(socket, &app.line());
                }
                self.reply(socket, "end");
            }
            Request::Alert => {
                self.play_alert();
                self.reply(socket, "ok");
            }
            Request::AlertState => {
                let line = alloc::format!("alert {} {}", self.alert, self.alert_volume);
                self.reply(socket, &line);
            }
        }
    }

    fn stream_mut(&mut self, id: usize) -> Option<&mut Stream> {
        self.streams
            .iter_mut()
            .find(|(each, _)| *each == id)
            .map(|(_, stream)| stream)
    }

    fn readable(&mut self, index: usize) {
        let Some(client) = self.clients[index].as_ref() else {
            return;
        };
        let socket = client.socket;
        let stream = client.stream;
        let room = stream
            .and_then(|id| self.streams.iter().find(|(each, _)| *each == id))
            .map_or(READ_BYTES, |(_, stream)| (stream.room() * 2).min(READ_BYTES));
        if room == 0 {
            return;
        }
        let mut bytes = [0u8; READ_BYTES];
        let read = call(
            number::READ,
            [socket as usize, bytes.as_mut_ptr() as usize, room, 0, 0, 0],
        );
        let read = match read {
            Ok(0) | Err(_) => {
                self.drop_client(index);
                return;
            }
            Ok(read) => read,
        };
        if let Some(id) = stream {
            if let Some(stream) = self.stream_mut(id) {
                stream.push_bytes(&bytes[..read]);
            }
            return;
        }
        if let Some(client) = self.clients[index].as_mut() {
            client.line.extend_from_slice(&bytes[..read]);
        }
        loop {
            let Some(client) = self.clients[index].as_mut() else {
                return;
            };
            let Some(end) = client.line.iter().position(|&byte| byte == b'\n') else {
                if client.line.len() > LINE_BYTES {
                    client.line.clear();
                }
                return;
            };
            let line: Vec<u8> = client.line.drain(..=end).collect();
            let text = core::str::from_utf8(&line).unwrap_or("").trim();
            let Some(request) = Request::parse(text) else {
                continue;
            };
            let playing = matches!(request, Request::Play { .. });
            self.handle(index, request);
            if playing {
                let Some(client) = self.clients[index].as_mut() else {
                    return;
                };
                let rest = core::mem::take(&mut client.line);
                if let Some(id) = client.stream
                    && let Some(stream) = self.stream_mut(id)
                {
                    stream.push_bytes(&rest);
                }
                return;
            }
        }
    }

    fn drop_client(&mut self, index: usize) {
        if let Some(client) = self.clients[index].take() {
            close(client.socket);
            if let Some(id) = client.stream
                && let Some(stream) = self.stream_mut(id)
            {
                stream.end();
            }
        }
    }

    fn accept(&mut self, listener: Fd) {
        while let Ok(socket) = call(
            number::ACCEPT4,
            [listener as usize, 0, 0, SOCK_NONBLOCK | SOCK_CLOEXEC, 0, 0],
        ) {
            match self.clients.iter().position(Option::is_none) {
                Some(free) => {
                    self.clients[free] = Some(Client {
                        socket: socket as Fd,
                        line: Vec::new(),
                        stream: None,
                        watching: false,
                    })
                }
                None => close(socket as Fd),
            }
        }
    }

    fn feed_device(&mut self) {
        loop {
            if self.pending.is_empty() {
                let mut samples = [0i16; CHUNK_FRAMES * 2];
                let tone_playing = self
                    .tone
                    .is_some_and(|id| self.streams.iter().any(|(each, _)| *each == id));
                if !tone_playing {
                    self.tone = None;
                }
                let mute = self.state.mute && !tone_playing;
                let gain = mixer::gain(self.state.volume, mute);
                let streams = self.streams.iter_mut().map(|(_, stream)| stream);
                mixer::mix(streams, &mut samples, self.rate, gain);
                self.streams.retain(|(_, stream)| !stream.is_over());
                let streams = &self.streams;
                self.stream_apps
                    .retain(|(id, _)| streams.iter().any(|(each, _)| each == id));
                mixer::to_bytes(&samples, &mut self.pending);
            }
            let written = call(
                number::WRITE,
                [
                    self.device as usize,
                    self.pending.as_ptr() as usize,
                    self.pending.len(),
                    0,
                    0,
                    0,
                ],
            )
            .unwrap_or(0);
            self.pending.drain(..written.min(self.pending.len()));
            if !self.pending.is_empty() {
                return;
            }
        }
    }

    fn read_dsp(&mut self) {
        let mut record = [0u8; DSP_RECORD_BYTES];
        for _ in 0..4 {
            let read = call(
                number::READ,
                [self.device as usize, record.as_mut_ptr() as usize, record.len(), 0, 0, 0],
            )
            .unwrap_or(0);
            if read < DSP_HEADER {
                return;
            }
            let word = |at: usize| {
                u32::from_le_bytes([record[at], record[at + 1], record[at + 2], record[at + 3]])
            };
            let number = word(0);
            let rate = word(4);
            let channels = u16::from_le_bytes([record[8], record[9]]).clamp(1, 8) as u8;
            let bits = u16::from_le_bytes([record[10], record[11]]);
            let ended = word(12) & DSP_ENDED != 0;
            let id = match self.dsp {
                Some((known, id)) if known == number => id,
                _ => {
                    if let Some((_, old)) = self.dsp
                        && let Some(stream) = self.stream_mut(old)
                    {
                        stream.end();
                    }
                    let id = self.start_stream(Stream::new(rate, channels), DSP_APP);
                    self.dsp = Some((number, id));
                    id
                }
            };
            let samples = mixer::to_sixteen_bits(&record[DSP_HEADER..read], bits, false, 1.0);
            if let Some(stream) = self.stream_mut(id) {
                stream.push_bytes(&samples);
                if ended {
                    stream.end();
                }
            }
            if ended {
                self.dsp = None;
            }
        }
    }

    fn wants_input(&self, index: usize) -> bool {
        let Some(client) = self.clients[index].as_ref() else {
            return false;
        };
        client.stream.is_none_or(|id| {
            self.streams
                .iter()
                .find(|(each, _)| *each == id)
                .is_none_or(|(_, stream)| stream.room() > 0)
        })
    }
}

fn app_gain(record: &(String, u8, bool)) -> u8 {
    if record.2 { 0 } else { record.1 }
}

fn listen() -> Result<Fd, Errno> {
    let _ = call(
        number::UNLINKAT,
        [AT_FDCWD, SOCKET.as_ptr() as usize, 0, 0, 0, 0],
    );
    let socket = call(
        number::SOCKET,
        [AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0, 0, 0, 0],
    )? as Fd;
    let (address, length) = address();
    let bound = call(
        number::BIND,
        [socket as usize, &raw const address as usize, length, 0, 0, 0],
    )
    .and_then(|_| call(number::LISTEN, [socket as usize, LISTEN_BACKLOG, 0, 0, 0, 0]));
    if let Err(error) = bound {
        close(socket);
        return Err(error);
    }
    let _ = call(
        number::FCHMODAT,
        [AT_FDCWD, SOCKET.as_ptr() as usize, 0o666, 0, 0, 0],
    );
    Ok(socket)
}

fn ignore_broken_pipes() {
    let action = eggos_runtime::signal::action(eggos_runtime::signal::IGNORE);
    let _ = call(
        number::RT_SIGACTION,
        [SIGPIPE, action.as_ptr() as usize, 0, SIGNAL_SET_BYTES, 0, 0],
    );
}

fn serve() -> i32 {
    ignore_broken_pipes();
    let mut server = match Server::open() {
        Ok(server) => server,
        Err(error) => {
            syscall::write_all(STANDARD_ERROR, b"soundd: no sound output: ");
            syscall::write_all(STANDARD_ERROR, error.message());
            syscall::write_all(STANDARD_ERROR, b"\n");
            return 1;
        }
    };
    let listener = match listen() {
        Ok(listener) => listener,
        Err(error) => {
            syscall::write_all(STANDARD_ERROR, b"soundd: cannot listen on /run/soundd: ");
            syscall::write_all(STANDARD_ERROR, error.message());
            syscall::write_all(STANDARD_ERROR, b"\n");
            return 1;
        }
    };
    server.tell_shell();
    let mut entries = [PollEntry {
        descriptor: -1,
        events: 0,
        returned: 0,
    }; 1 + MOST_CLIENTS];
    let mut owners = [usize::MAX; 1 + MOST_CLIENTS];
    loop {
        let mut count = 0;
        entries[0] = PollEntry {
            descriptor: listener,
            events: POLLIN,
            returned: 0,
        };
        count += 1;
        for (index, client) in server.clients.iter().enumerate() {
            if let Some(client) = client {
                let events = if server.wants_input(index) { POLLIN } else { 0 };
                entries[count] = PollEntry {
                    descriptor: client.socket,
                    events,
                    returned: 0,
                };
                owners[count] = index;
                count += 1;
            }
        }
        let time = [0i64, WAKE_MS * 1_000_000];
        let _ = call(
            number::PPOLL,
            [entries.as_mut_ptr() as usize, count, time.as_ptr() as usize, 0, 8, 0],
        );
        if entries[0].returned & POLLIN != 0 {
            server.accept(listener);
        }
        for at in 1..count {
            if entries[at].returned & (POLLIN | POLLHUP | POLLERR) != 0 {
                server.readable(owners[at]);
            }
        }
        server.read_dsp();
        server.feed_device();
    }
}

struct Connection(Fd);

impl Drop for Connection {
    fn drop(&mut self) {
        close(self.0);
    }
}

impl Connection {
    fn open() -> Option<Self> {
        let socket = call(number::SOCKET, [AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, 0, 0, 0])
            .ok()? as Fd;
        let connection = Self(socket);
        let (address, length) = address();
        call(
            number::CONNECT,
            [socket as usize, &raw const address as usize, length, 0, 0, 0],
        )
        .ok()?;
        Some(connection)
    }

    fn send(&self, request: &Request) -> bool {
        let mut line = request.line();
        line.push('\n');
        syscall::write_all(self.0 as usize, line.as_bytes())
    }

    fn lines_until(&self, done: impl Fn(&str) -> bool) -> Vec<String> {
        let mut lines = Vec::new();
        let mut buffer: Vec<u8> = Vec::new();
        let mut bytes = [0u8; 512];
        loop {
            while let Some(end) = buffer.iter().position(|&byte| byte == b'\n') {
                let line: Vec<u8> = buffer.drain(..=end).collect();
                let text = String::from(core::str::from_utf8(&line).unwrap_or("").trim());
                let finished = done(&text);
                lines.push(text);
                if finished {
                    return lines;
                }
            }
            match call(
                number::READ,
                [self.0 as usize, bytes.as_mut_ptr() as usize, bytes.len(), 0, 0, 0],
            ) {
                Ok(0) | Err(_) => return lines,
                Ok(read) => buffer.extend_from_slice(&bytes[..read]),
            }
        }
    }

    fn ask(&self, request: &Request) -> Option<String> {
        self.send(request).then_some(())?;
        self.lines_until(|_| true).pop()
    }

    fn state(&self) -> Option<State> {
        State::parse(&self.ask(&Request::Get)?)
    }

    fn apps(&self) -> Vec<App> {
        if !self.send(&Request::Apps) {
            return Vec::new();
        }
        self.lines_until(|line| line == "end")
            .iter()
            .filter_map(|line| App::parse(line))
            .collect()
    }

    fn devices(&self) -> Vec<Device> {
        if !self.send(&Request::Devices) {
            return Vec::new();
        }
        self.lines_until(|line| line == "end")
            .iter()
            .filter_map(|line| Device::parse(line))
            .collect()
    }
}

fn refuse(code: &str, message: &str, status: i32) -> i32 {
    say(STANDARD_ERROR, &alloc::format!("sound: {code}: {message}\n"));
    status
}

fn sound(arguments: &[&str]) -> i32 {
    let command = match command::parse(arguments) {
        Ok(command) => command,
        Err(Refusal::Usage(text)) => return refuse("usage", &text, USAGE_STATUS),
        Err(Refusal::InvalidValue(text)) => return refuse("invalid_value", &text, USAGE_STATUS),
    };
    let Some(connection) = Connection::open() else {
        return match command {
            Command::Status { json: false } => {
                say(STANDARD_OUTPUT, NO_DEVICE);
                say(STANDARD_OUTPUT, "\n");
                0
            }
            Command::Status { json: true } => {
                say(STANDARD_OUTPUT, "{\"output\":null,\"input\":null}\n");
                0
            }
            _ => refuse("no_sound_device", NO_DEVICE, FAILURE_STATUS),
        };
    };
    let devices = connection.devices();
    let Some(state) = connection.state() else {
        return refuse("no_sound_device", NO_DEVICE, FAILURE_STATUS);
    };
    match &command {
        Command::Status { json: false } => {
            say(STANDARD_OUTPUT, &command::status_text(&state, &devices));
        }
        Command::Status { json: true } => {
            say(STANDARD_OUTPUT, &command::status_json(&state, &devices));
        }
        Command::Devices { json } => {
            if devices.is_empty() {
                return refuse("no_sound_device", NO_DEVICE, FAILURE_STATUS);
            }
            say(STANDARD_OUTPUT, &command::devices_text(&devices, *json));
        }
        Command::Apps { json } => {
            say(STANDARD_OUTPUT, &command::apps_text(&connection.apps(), *json));
        }
        Command::AppVolume(id, _) | Command::AppMute(id, _) => {
            return set_app(&connection, &command, id);
        }
        Command::Alert { name, volume } => {
            for change in [
                name.clone().map(Change::Alert),
                volume.map(Change::AlertVolume),
            ]
            .into_iter()
            .flatten()
            {
                let _ = connection.ask(&Request::Set(change));
            }
            let _ = connection.ask(&Request::Alert);
            let current = connection.ask(&Request::AlertState).unwrap_or_default();
            let alert = current.split(' ').nth(1).unwrap_or("chime");
            let title = command::alert_title(alert);
            let line = match name {
                Some(_) => alloc::format!("Alert sound: {title}.\n"),
                None => alloc::format!("Played the alert ({title}).\n"),
            };
            say(STANDARD_OUTPUT, &line);
        }
        Command::Play(options) => return play_file(options),
        _ => {
            let kind = match &command {
                Command::Input(_) => Some(Kind::Input),
                Command::Output(_) => Some(Kind::Output),
                _ => None,
            };
            if let (Some(kind), Command::Output(id) | Command::Input(id)) = (kind, &command)
                && !command::names_a_device(&devices, kind, id)
            {
                let message = alloc::format!("no sound device '{id}' (sound devices lists them)");
                return refuse("no_such_device", &message, FAILURE_STATUS);
            }
            let Some(request) = command::request_of(&command, &state) else {
                return 0;
            };
            match connection.ask(&request).as_deref() {
                Some("ok") => {}
                Some(answer) => {
                    let code = answer.strip_prefix("error ").unwrap_or("failed");
                    return refuse(code, NO_DEVICE, FAILURE_STATUS);
                }
                None => return refuse("no_sound_device", NO_DEVICE, FAILURE_STATUS),
            }
            if let Command::Test { seconds } = command {
                let until = now_ms() + u64::from(seconds) * 1000;
                while now_ms() < until {
                    let pause = [0i64, 50_000_000];
                    let _ = call(
                        number::CLOCK_NANOSLEEP,
                        [CLOCK_MONOTONIC, 0, pause.as_ptr() as usize, 0, 0, 0],
                    );
                }
            }
            let after = connection.state().unwrap_or(state);
            say(STANDARD_OUTPUT, &command::done_text(&command, &after, &devices));
        }
    }
    0
}

fn set_app(connection: &Connection, command: &Command, id: &str) -> i32 {
    let Some(app) = connection.apps().into_iter().find(|app| app.id == id) else {
        let message = alloc::format!("no app '{id}' is playing (sound apps lists them)");
        return refuse("no_such_app", &message, FAILURE_STATUS);
    };
    let (change, done) = match command {
        Command::AppVolume(_, step) => {
            let volume = command::stepped(app.volume, *step);
            let done = alloc::format!("{}: volume {volume}%.\n", app.name);
            (Change::AppVolume(app.id.clone(), volume), done)
        }
        Command::AppMute(_, toggle) => {
            let mute = match toggle {
                command::Toggle::On => true,
                command::Toggle::Off => false,
                command::Toggle::Flip => !app.mute,
            };
            let word = if mute { "muted" } else { "unmuted" };
            (Change::AppMute(app.id.clone(), mute), alloc::format!("{}: {word}.\n", app.name))
        }
        _ => return 0,
    };
    match connection.ask(&Request::Set(change)).as_deref() {
        Some("ok") => {
            say(STANDARD_OUTPUT, &done);
            0
        }
        _ => refuse("no_such_app", "the app stopped playing", FAILURE_STATUS),
    }
}

fn play_file(options: &command::PlayOptions) -> i32 {
    let Ok(path) = alloc::ffi::CString::new(options.path.as_str()) else {
        return refuse("no_such_file", &options.path, FAILURE_STATUS);
    };
    let Some(bytes) = read_whole(&path, 64 << 20) else {
        let message = alloc::format!("{}: no such file", options.path);
        return refuse("no_such_file", &message, FAILURE_STATUS);
    };
    let Some(wave) = crate::wave::read_wave(&bytes) else {
        let message = alloc::format!("{}: not a PCM WAV file", options.path);
        return refuse("not_a_wav", &message, FAILURE_STATUS);
    };
    let data = bytes
        .get(wave.data_offset..wave.data_offset + wave.data_length)
        .unwrap_or(&[]);
    let frame_bytes = usize::from(wave.frame_bytes).max(1);
    let most = options.seconds.map_or(data.len(), |seconds| {
        (wave.sample_rate as usize * seconds as usize * frame_bytes).min(data.len())
    });
    let data = &data[..most - most % frame_bytes];
    let volume = f32::from(options.volume.unwrap_or(100)) / 100.0;
    let float = wave.format == crate::wave::SampleFormat::Float;
    let samples = mixer::to_sixteen_bits(data, wave.bits, float, volume);
    let channels = u8::try_from(wave.channels).unwrap_or(2);
    let Some(mut player) = crate::client::Player::open(wave.sample_rate, channels, "sound") else {
        return refuse("no_sound_device", NO_DEVICE, FAILURE_STATUS);
    };
    let name = options.path.rsplit('/').next().unwrap_or(&options.path);
    let length_ms = (data.len() / frame_bytes) as u64 * 1000 / u64::from(wave.sample_rate.max(1));
    let tenths = length_ms / 100;
    let seconds = alloc::format!("{}.{} s", tenths / 10, tenths % 10);
    player.push(&samples);
    if !options.wait {
        while !player.pump() {
            sleep_ms(10);
        }
        say(STANDARD_OUTPUT, &alloc::format!("Playing {name} ({seconds}).\n"));
        return 0;
    }
    say(STANDARD_OUTPUT, &alloc::format!("Playing {name} ({seconds})...\n"));
    let ends = now_ms() + length_ms;
    while !player.pump() || now_ms() < ends {
        sleep_ms(20);
    }
    say(STANDARD_OUTPUT, &alloc::format!("Played {name}.\n"));
    0
}

fn sleep_ms(milliseconds: i64) {
    let pause = [0i64, milliseconds * 1_000_000];
    let _ = call(
        number::CLOCK_NANOSLEEP,
        [CLOCK_MONOTONIC, 0, pause.as_ptr() as usize, 0, 0, 0],
    );
}

/// The program's main function: `soundd` serves, `sound` is the command.
pub fn run(startup: &Startup) -> i32 {
    let mut arguments = startup.arguments();
    let program = arguments.next().map_or(&b""[..], CStr::to_bytes);
    let name = program.rsplit(|&byte| byte == b'/').next().unwrap_or(program);
    if name == b"soundd" {
        return serve();
    }
    let words: Vec<&str> = arguments
        .map(|argument| core::str::from_utf8(argument.to_bytes()).unwrap_or(""))
        .collect();
    sound(&words)
}
