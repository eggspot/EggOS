pub fn primes_up_to(limit: usize) -> Vec<usize> {
    let mut composite = vec![false; limit + 1];
    let mut primes = Vec::new();
    for candidate in 2..=limit {
        if composite[candidate] {
            continue;
        }
        primes.push(candidate);
        let mut multiple = candidate * candidate;
        while multiple <= limit {
            composite[multiple] = true;
            multiple += candidate;
        }
    }
    primes
}

pub fn fibonacci(count: usize) -> Vec<u64> {
    let mut sequence = Vec::with_capacity(count);
    let (mut current, mut next) = (0u64, 1u64);
    for _ in 0..count {
        sequence.push(current);
        (current, next) = (next, current.wrapping_add(next));
    }
    sequence
}

pub fn greatest_common_divisor(mut first: u64, mut second: u64) -> u64 {
    while second != 0 {
        (first, second) = (second, first % second);
    }
    first
}

pub fn collatz_steps(mut value: u64) -> u32 {
    let mut steps = 0;
    while value > 1 {
        value = match value % 2 {
            0 => value / 2,
            _ => 3 * value + 1,
        };
        steps += 1;
    }
    steps
}

pub fn longest_collatz_below(limit: u64) -> (u64, u32) {
    (1..limit)
        .map(|start| (start, collatz_steps(start)))
        .fold((1, 0), |best, found| match found.1 > best.1 {
            true => found,
            false => best,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_sequences_come_out() {
        assert_eq!(primes_up_to(20), [2, 3, 5, 7, 11, 13, 17, 19]);
        assert_eq!(fibonacci(8), [0, 1, 1, 2, 3, 5, 8, 13]);
        assert_eq!(greatest_common_divisor(84, 36), 12);
        assert_eq!(collatz_steps(27), 111);
        assert_eq!(longest_collatz_below(10), (9, 19));
    }
}
