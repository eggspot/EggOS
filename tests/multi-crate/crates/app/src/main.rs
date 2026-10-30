use eggos_multi_crate_checksums::crc32;
use std::io::Write;

const PARTS: usize = 4;

fn checksums_in_threads(bytes: &[u8]) -> Vec<u32> {
    let length = bytes.len().div_ceil(PARTS);
    std::thread::scope(|scope| {
        let handles: Vec<_> = bytes
            .chunks(length)
            .map(|part| scope.spawn(move || crc32(part)))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("a checksum thread panicked"))
            .collect()
    })
}

fn main() -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    write!(out, "{}", eggos_multi_crate_report::render())?;
    let parts = checksums_in_threads(eggos_multi_crate_report::TEXT.as_bytes());
    let parts: Vec<String> = parts.iter().map(|crc| format!("{crc:08x}")).collect();
    writeln!(out, "threads: {}", parts.join(" "))?;
    writeln!(out, "{} {}", std::env::consts::OS, std::env::consts::ARCH)?;
    out.flush()
}
