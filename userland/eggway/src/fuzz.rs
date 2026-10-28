//! The compositor's fuzz target (D140): sessions built from valid requests (globals, surfaces,
//! toplevels, pools, buffers, commits, moves) cut, flipped, repeated and mixed with random
//! words, sent in random chunks with random descriptors, while the window server's input comes
//! at random. After each session every client hangs up; then every descriptor must be closed,
//! no pool may stay counted and no client may stay connected. A panic fails the run. Host tests
//! run a bounded number; `cargo xtask fuzz wayland` runs it for longer.

use crate::compositor::Compositor;
use crate::fake::{Fake, message};
use crate::system::Fd;
use eggos_wayland::wire::Value;

/// What a session left behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// Descriptors still open after every client went.
    Leaked(usize),
    /// Pool memory still counted.
    PoolBytes(usize),
    /// Clients still connected after they hung up.
    Clients(usize),
}

/// A small, fast random number generator (xorshift64).
#[derive(Debug, Clone, Copy)]
pub struct Generator(u64);

impl Generator {
    /// A generator from `seed` (0 is made 1).
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    /// The next number.
    pub fn next_number(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A number below `bound` (0 when `bound` is 0).
    pub fn below(&mut self, bound: u64) -> u64 {
        match bound {
            0 => 0,
            bound => self.next_number() % bound,
        }
    }
}

fn corpus(size: i32) -> Vec<Vec<u8>> {
    let bind = |name: u32, interface: &'static [u8], version: u32, id: u32| {
        message(
            2,
            0,
            &[
                Value::Uint(name),
                Value::UntypedNewId {
                    interface,
                    version,
                    id,
                },
            ],
        )
    };
    let rect = [
        Value::Int(0),
        Value::Int(0),
        Value::Int(size),
        Value::Int(size),
    ];
    vec![
        message(1, 1, &[Value::NewId(2)]),
        bind(1, b"wl_compositor", 4, 3),
        bind(2, b"wl_shm", 1, 4),
        bind(3, b"wl_seat", 5, 5),
        bind(4, b"wl_output", 3, 6),
        bind(5, b"xdg_wm_base", 3, 7),
        message(5, 0, &[Value::NewId(8)]),
        message(5, 1, &[Value::NewId(9)]),
        message(3, 0, &[Value::NewId(10)]),
        message(7, 2, &[Value::NewId(11), Value::Object(10)]),
        message(11, 1, &[Value::NewId(12)]),
        message(12, 2, &[Value::String(b"fuzz")]),
        message(10, 6, &[]),
        message(11, 4, &[Value::Uint(1)]),
        message(
            4,
            0,
            &[Value::NewId(13), Value::Fd, Value::Int(size * size * 4)],
        ),
        message(
            13,
            0,
            &[
                Value::NewId(14),
                Value::Int(0),
                Value::Int(size),
                Value::Int(size),
                Value::Int(size * 4),
                Value::Uint(1),
            ],
        ),
        message(10, 1, &[Value::Object(14), Value::Int(0), Value::Int(0)]),
        message(10, 9, &rect),
        message(10, 3, &[Value::NewId(15)]),
        message(10, 6, &[]),
        message(12, 5, &[Value::Object(5), Value::Uint(3)]),
        message(12, 6, &[Value::Object(5), Value::Uint(3), Value::Uint(10)]),
        message(12, 9, &[]),
        message(12, 13, &[]),
        message(12, 8, &[Value::Int(size), Value::Int(size)]),
        message(13, 2, &[Value::Int(size * size * 8)]),
        message(7, 1, &[Value::NewId(16)]),
        message(
            11,
            2,
            &[Value::NewId(17), Value::Object(0), Value::Object(16)],
        ),
        message(14, 0, &[]),
        message(12, 0, &[]),
        message(11, 0, &[]),
        message(10, 0, &[]),
        message(1, 0, &[Value::NewId(18)]),
    ]
}

fn mutate(generator: &mut Generator, mut bytes: Vec<u8>) -> Vec<u8> {
    match generator.below(8) {
        0 => {
            let at = generator.below(bytes.len() as u64) as usize;
            if let Some(byte) = bytes.get_mut(at) {
                *byte ^= 1 << generator.below(8);
            }
        }
        1 => bytes.truncate(generator.below(bytes.len() as u64 + 1) as usize),
        2 => {
            let copy = bytes.clone();
            bytes.extend(copy);
        }
        3 => {
            for _ in 0..generator.below(4) {
                bytes.extend((generator.next_number() as u32).to_ne_bytes());
            }
        }
        4 => {
            let at = (4 + generator.below(4)) as usize;
            if let Some(byte) = bytes.get_mut(at) {
                *byte = generator.next_number() as u8;
            }
        }
        _ => {}
    }
    bytes
}

fn descriptors(generator: &mut Generator, fake: &mut Fake, size: i32) -> Vec<Fd> {
    (0..generator.below(3))
        .map(|_| match generator.below(3) {
            0 => fake.other_file(),
            _ => fake.memory_file(vec![generator.next_number() as u32; (size * size) as usize]),
        })
        .collect()
}

fn session(generator: &mut Generator) -> Result<(), Fault> {
    let mut fake = Fake::default();
    let mut compositor = Compositor::new();
    let clients = 1 + generator.below(2) as usize;
    let sockets: Vec<Fd> = (0..clients)
        .map(|_| {
            let socket = fake.connect();
            compositor.add_client(&mut fake, socket);
            socket
        })
        .collect();
    let size = 1 + generator.below(64) as i32;
    let messages = corpus(size);
    for _ in 0..1 + generator.below(40) {
        let client = generator.below(clients as u64) as usize;
        let mut bytes = Vec::new();
        for _ in 0..1 + generator.below(6) {
            let index = match generator.below(4) {
                0 => generator.below(messages.len() as u64) as usize,
                _ => (generator.below(4) as usize + bytes.len() / 16).min(messages.len() - 1),
            };
            bytes.extend(mutate(generator, messages[index].clone()));
        }
        if generator.below(10) == 0 {
            bytes = (0..generator.below(64))
                .map(|_| generator.next_number() as u8)
                .collect();
        }
        let fds = descriptors(generator, &mut fake, size);
        let cut = generator.below(bytes.len() as u64 + 1) as usize;
        let rest = bytes.split_off(cut);
        fake.push(sockets[client], bytes, fds);
        fake.push(sockets[client], rest, Vec::new());
        if let Some((&window, _)) = fake.windows.iter().find(|(_, window)| !window.destroyed) {
            let kind = 1 + generator.below(13) as u16;
            let values = [
                generator.below(400) as u32,
                generator.below(400) as u32,
                [0x110, 0x111, 1, 30][generator.below(4) as usize],
                generator.below(3) as u32,
            ];
            fake.record(window, kind, values);
        }
        fake.now += generator.below(40);
        for index in 0..clients {
            compositor.readable(&mut fake, index);
            compositor.device_readable(&mut fake, index);
            if generator.below(3) == 0 {
                compositor.writable(&mut fake, index);
            }
        }
        compositor.tick(&mut fake);
    }
    for &socket in &sockets {
        fake.hang_up(socket);
    }
    for _ in 0..200 {
        if compositor.client_count() == 0 {
            break;
        }
        for index in 0..clients {
            compositor.readable(&mut fake, index);
        }
        compositor.tick(&mut fake);
    }
    if compositor.client_count() != 0 {
        return Err(Fault::Clients(compositor.client_count()));
    }
    if compositor.pool_bytes() != 0 {
        return Err(Fault::PoolBytes(compositor.pool_bytes()));
    }
    if !fake.open.is_empty() {
        return Err(Fault::Leaked(fake.open.len()));
    }
    Ok(())
}

/// Runs `sessions` hostile sessions from `seed`; the first fault and its session's index.
pub fn run(seed: u64, sessions: usize) -> Result<(), (Fault, usize)> {
    let mut generator = Generator::new(seed);
    for index in 0..sessions {
        session(&mut generator).map_err(|fault| (fault, index))?;
    }
    Ok(())
}
