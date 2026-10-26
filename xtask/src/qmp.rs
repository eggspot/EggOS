//! A minimal QEMU Machine Protocol client: what the boot tests need to look at the display
//! (`screendump`), over a TCP socket on localhost. QMP speaks one JSON object per line; the
//! client only sends commands it writes itself and looks for the `return` or `error` answer,
//! skipping the asynchronous events in between, so it needs no JSON parser.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// A free TCP port on localhost for QEMU's `-qmp tcp:...` to listen on.
pub fn free_port() -> Result<u16, String> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|e| format!("no free local port for QMP: {e}"))
}

/// QEMU's arguments to listen for QMP on `port`.
pub fn qemu_args(port: u16) -> Vec<String> {
    vec![
        "-qmp".to_string(),
        format!("tcp:127.0.0.1:{port},server=on,wait=off"),
    ]
}

/// A QMP session in command mode.
#[derive(Debug)]
pub struct Qmp {
    writer: TcpStream,
    reader: BufReader<TcpStream>,
}

impl Qmp {
    /// Connects to QEMU's QMP socket on `port`, retrying while QEMU starts, and leaves the
    /// capabilities negotiation.
    pub fn connect(port: u16) -> Result<Self, String> {
        let started = Instant::now();
        let stream = loop {
            match TcpStream::connect((Ipv4Addr::LOCALHOST, port)) {
                Ok(stream) => break stream,
                Err(e) if started.elapsed() > CONNECT_TIMEOUT => {
                    return Err(format!("could not connect to QEMU's QMP socket: {e}"));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        };
        stream
            .set_read_timeout(Some(ANSWER_TIMEOUT))
            .map_err(|e| e.to_string())?;
        let reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
        let mut qmp = Self {
            writer: stream,
            reader,
        };
        let greeting = qmp.line()?;
        if !greeting.contains("\"QMP\"") {
            return Err(format!("QEMU's QMP socket greeted with {greeting:?}"));
        }
        qmp.execute("{\"execute\": \"qmp_capabilities\"}")?;
        Ok(qmp)
    }

    /// Saves the display's current picture to `path` as a binary PPM.
    pub fn screendump(&mut self, path: &Path) -> Result<(), String> {
        let command = format!(
            "{{\"execute\": \"screendump\", \"arguments\": {{\"filename\": {}}}}}",
            json_string(&path.to_string_lossy())
        );
        self.execute(&command)
    }

    fn execute(&mut self, command: &str) -> Result<(), String> {
        writeln!(self.writer, "{command}").map_err(|e| format!("QMP: {e}"))?;
        loop {
            let line = self.line()?;
            if line.contains("\"return\"") {
                return Ok(());
            }
            if line.contains("\"error\"") {
                return Err(format!("QMP refused {command}: {line}"));
            }
        }
    }

    fn line(&mut self) -> Result<String, String> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => Err("QEMU closed its QMP socket".into()),
            Ok(_) => Ok(line),
            Err(e) => Err(format!("QMP: {e}")),
        }
    }
}

/// `text` as a JSON string literal.
pub fn json_string(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            control if u32::from(control) < 0x20 => {
                quoted.push_str(&format!("\\u{:04x}", u32::from(control)));
            }
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

/// A picture of the display: its size and its pixels, row after row, each `0x00RRGGBB`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

impl Picture {
    /// Reads a binary PPM (`P6`, 8 bits per channel), as QEMU's `screendump` writes it.
    pub fn from_ppm(bytes: &[u8]) -> Result<Self, String> {
        let mut fields = Vec::new();
        let mut at = 0;
        while fields.len() < 4 {
            while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
                at += 1;
            }
            let start = at;
            while bytes
                .get(at)
                .is_some_and(|byte| !byte.is_ascii_whitespace())
            {
                at += 1;
            }
            if start == at {
                return Err("the PPM header is cut short".into());
            }
            fields.push(String::from_utf8_lossy(&bytes[start..at]).into_owned());
        }
        let number = |field: &str| {
            field
                .parse::<u32>()
                .map_err(|_| format!("bad PPM header field {field:?}"))
        };
        if fields[0] != "P6" || number(&fields[3])? != 255 {
            return Err(format!(
                "not an 8-bit binary PPM: {} {}",
                fields[0], fields[3]
            ));
        }
        let (width, height) = (number(&fields[1])?, number(&fields[2])?);
        let data = bytes.get(at + 1..).unwrap_or_default();
        let expected = width as usize * height as usize * 3;
        if data.len() != expected {
            return Err(format!(
                "the PPM holds {} bytes of pixels, not {expected}",
                data.len()
            ));
        }
        let pixels = data
            .as_chunks::<3>()
            .0
            .iter()
            .map(|&[red, green, blue]| {
                u32::from(red) << 16 | u32::from(green) << 8 | u32::from(blue)
            })
            .collect();
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    /// FNV-1a, 64 bits, over the pixels' little-endian bytes: the kernel's `pixel_hash`.
    pub fn hash(&self) -> u64 {
        const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        self.pixels
            .iter()
            .flat_map(|pixel| pixel.to_le_bytes())
            .fold(OFFSET_BASIS, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(PRIME)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_strings_escape_quotes_backslashes_and_controls() {
        assert_eq!(
            json_string("C:\\target\\a \"b\".ppm\n"),
            "\"C:\\\\target\\\\a \\\"b\\\".ppm\\u000a\""
        );
    }

    #[test]
    fn a_ppm_reads_as_packed_pixels_and_hashes_as_the_kernel_does() {
        let mut ppm = b"P6\n2 1\n255\n".to_vec();
        ppm.extend([0x0e, 0x17, 0x26, 0x64, 0x63, 0x62]);
        let picture = Picture::from_ppm(&ppm).unwrap();
        assert_eq!((picture.width, picture.height), (2, 1));
        assert_eq!(picture.pixels, [0x000e_1726, 0x0064_6362]);
        let one = Picture {
            width: 1,
            height: 1,
            pixels: vec![0x6463_6261],
        };
        assert_eq!(one.hash(), 0xfc17_9f83_ee07_24dd);
    }

    #[test]
    fn a_ppm_cut_short_or_of_another_kind_is_refused() {
        assert!(Picture::from_ppm(b"P6\n2 1\n255\n\x01\x02").is_err());
        assert!(Picture::from_ppm(b"P5\n1 1\n255\n\x01").is_err());
        assert!(Picture::from_ppm(b"P6\n1 1\n65535\n\x01\x02\x03").is_err());
        assert!(Picture::from_ppm(b"P6\n1").is_err());
    }
}
