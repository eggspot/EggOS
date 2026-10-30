use std::collections::BTreeMap;
use std::io::Write;

const WORDS: &str = "the quick brown fox jumps over the lazy dog the end";

const PARTS: u64 = 4;

const PART_LENGTH: u64 = 1000;

fn word_counts(text: &str) -> BTreeMap<&str, u32> {
    let mut counts = BTreeMap::new();
    for word in text.split_whitespace() {
        *counts.entry(word).or_insert(0) += 1;
    }
    counts
}

fn sums_in_threads() -> Vec<u64> {
    let handles: Vec<_> = (0..PARTS)
        .map(|part| {
            std::thread::spawn(move || {
                (part * PART_LENGTH + 1..=(part + 1) * PART_LENGTH).sum::<u64>()
            })
        })
        .collect();
    handles
        .into_iter()
        .map(|handle| handle.join().expect("a summing thread panicked"))
        .collect()
}

fn written_and_read_back() -> std::io::Result<String> {
    let path = std::env::temp_dir().join(format!("hello-crate-{}.txt", std::process::id()));
    std::fs::write(&path, "written and read back\n")?;
    let read = std::fs::read_to_string(&path)?;
    std::fs::remove_file(&path)?;
    Ok(read)
}

fn main() -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "Hello, world!")?;
    for (word, count) in word_counts(WORDS) {
        writeln!(out, "{word} {count}")?;
    }
    let sums = sums_in_threads();
    writeln!(out, "sums {sums:?} total {}", sums.iter().sum::<u64>())?;
    write!(out, "{}", written_and_read_back()?)?;
    writeln!(out, "{} {}", std::env::consts::OS, std::env::consts::ARCH)?;
    out.flush()
}
