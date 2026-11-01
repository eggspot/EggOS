use eggos_multi_crate_checksums::{adler32, crc32, fnv1a64};
use eggos_multi_crate_numbers::{
    fibonacci, greatest_common_divisor, longest_collatz_below, primes_up_to,
};
use eggos_multi_crate_shapes::{Shape, total_doubled_area};
use eggos_multi_crate_words::{longest, most_common, title_case};
use std::fmt::Write;

pub const TEXT: &str = "Cargo builds every crate of the workspace, and the crates that do not \
                        depend on each other build at the same time, one rustc for each CPU, \
                        sharing the jobserver's tokens. The workspace is small, and the build \
                        is the same on Linux and on EggOS.";

pub const SHAPES: [Shape; 4] = [
    Shape::Square { side: 7 },
    Shape::Rectangle {
        width: 3,
        height: 11,
    },
    Shape::RightTriangle {
        base: 5,
        height: 12,
    },
    Shape::RightTriangle {
        base: 8,
        height: 15,
    },
];

pub fn render() -> String {
    let mut text = String::new();
    let _ = writeln!(text, "words: {}", title_case("a multi crate workspace"));
    for (word, count) in most_common(TEXT, 5) {
        let _ = writeln!(text, "  {word} {count}");
    }
    let _ = writeln!(text, "  longest {}", longest(TEXT).unwrap_or_default());
    let primes = primes_up_to(10_000);
    let _ = writeln!(
        text,
        "numbers: {} primes below 10000, the last {}",
        primes.len(),
        primes.last().copied().unwrap_or(0)
    );
    let _ = writeln!(text, "  fibonacci {:?}", &fibonacci(12)[6..]);
    let _ = writeln!(text, "  gcd {}", greatest_common_divisor(1_071, 462));
    let (start, steps) = longest_collatz_below(10_000);
    let _ = writeln!(text, "  collatz {start} takes {steps} steps");
    let _ = writeln!(
        text,
        "shapes: total doubled area {}",
        total_doubled_area(&SHAPES)
    );
    for shape in SHAPES {
        let perimeter = shape
            .whole_perimeter()
            .map_or_else(|| "not whole".to_owned(), |perimeter| perimeter.to_string());
        let _ = writeln!(text, "  {} perimeter {perimeter}", shape.name());
    }
    let bytes = TEXT.as_bytes();
    let _ = writeln!(
        text,
        "checksums: crc32 {:08x} adler32 {:08x} fnv1a64 {:016x}",
        crc32(bytes),
        adler32(bytes),
        fnv1a64(bytes)
    );
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_section_is_rendered() {
        let report = render();
        for section in ["words: ", "numbers: ", "shapes: ", "checksums: "] {
            assert!(report.contains(section), "{section}");
        }
        assert!(report.contains("1229 primes below 10000, the last 9973"));
        assert!(report.contains("gcd 21"));
    }
}
