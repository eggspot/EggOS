//! The positional parameters (D182): `$1`, `$2`, ... as the shell started with them and as
//! `set --`, `shift` and function calls change them. They are kept in fixed room, one frame per
//! function call on top of the shell's own, so a call's arguments go when it returns and its
//! caller's come back.

/// The most bytes the positional parameters of every frame take.
pub const ARGUMENTS_CAPACITY: usize = 8192;

/// The most positional parameters of every frame together.
pub const MAX_ARGUMENTS: usize = 256;

/// The most frames: the shell's own and one per function call running.
pub const MAX_FRAMES: usize = 65;

/// The parameters do not fit [`ARGUMENTS_CAPACITY`], [`MAX_ARGUMENTS`] or [`MAX_FRAMES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Full;

#[derive(Debug, Clone, Copy, Default)]
struct Frame {
    first: usize,
    shifted: usize,
    start: usize,
}

/// The positional parameters.
#[derive(Debug)]
pub struct Arguments {
    bytes: [u8; ARGUMENTS_CAPACITY],
    length: usize,
    ends: [u16; MAX_ARGUMENTS],
    count: usize,
    frames: [Frame; MAX_FRAMES],
    depth: usize,
}

impl Default for Arguments {
    fn default() -> Self {
        Self {
            bytes: [0; ARGUMENTS_CAPACITY],
            length: 0,
            ends: [0; MAX_ARGUMENTS],
            count: 0,
            frames: [Frame::default(); MAX_FRAMES],
            depth: 0,
        }
    }
}

impl Arguments {
    fn frame(&self) -> Frame {
        self.frames[self.depth]
    }

    /// How many there are: `$#`.
    pub fn count(&self) -> usize {
        let frame = self.frame();
        self.count - frame.first - frame.shifted
    }

    /// Parameter `number`, from 1.
    pub fn get(&self, number: usize) -> Option<&[u8]> {
        if number == 0 || number > self.count() {
            return None;
        }
        let frame = self.frame();
        let index = frame.first + frame.shifted + number - 1;
        let start = match index {
            0 => 0,
            _ => usize::from(self.ends[index - 1]),
        };
        Some(&self.bytes[start..usize::from(self.ends[index])])
    }

    /// Replaces this frame's parameters with `values` (`set -- values`); nothing changes if
    /// they do not fit.
    pub fn set<'v>(&mut self, values: impl Iterator<Item = &'v [u8]> + Clone) -> Result<(), Full> {
        let frame = self.frame();
        self.fits(frame.first, frame.start, values.clone())?;
        self.count = frame.first;
        self.length = frame.start;
        self.frames[self.depth].shifted = 0;
        self.append(values);
        Ok(())
    }

    /// Drops the first `number` parameters (`shift`): `false`, and nothing dropped, if there
    /// are fewer.
    pub fn shift(&mut self, number: usize) -> bool {
        if number > self.count() {
            return false;
        }
        self.frames[self.depth].shifted += number;
        true
    }

    /// Starts a function call's frame with `values` as its parameters.
    pub fn push_frame<'v>(
        &mut self,
        values: impl Iterator<Item = &'v [u8]> + Clone,
    ) -> Result<(), Full> {
        if self.depth + 1 == MAX_FRAMES {
            return Err(Full);
        }
        self.fits(self.count, self.length, values.clone())?;
        self.depth += 1;
        self.frames[self.depth] = Frame {
            first: self.count,
            shifted: 0,
            start: self.length,
        };
        self.append(values);
        Ok(())
    }

    /// Ends the function call's frame: its caller's parameters are back.
    pub fn pop_frame(&mut self) {
        if self.depth > 0 {
            let frame = self.frame();
            self.count = frame.first;
            self.length = frame.start;
            self.depth -= 1;
        }
    }

    fn fits<'v>(
        &self,
        count: usize,
        length: usize,
        values: impl Iterator<Item = &'v [u8]>,
    ) -> Result<(), Full> {
        let (number, bytes) = values.fold((count, length), |(number, bytes), value| {
            (number + 1, bytes + value.len())
        });
        match number <= MAX_ARGUMENTS && bytes <= ARGUMENTS_CAPACITY {
            true => Ok(()),
            false => Err(Full),
        }
    }

    fn append<'v>(&mut self, values: impl Iterator<Item = &'v [u8]>) {
        for value in values {
            let end = self.length + value.len();
            self.bytes[self.length..end].copy_from_slice(value);
            self.length = end;
            self.ends[self.count] = end as u16;
            self.count += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(arguments: &Arguments) -> Vec<&[u8]> {
        (1..=arguments.count())
            .map(|number| arguments.get(number).unwrap())
            .collect()
    }

    #[test]
    fn set_and_shift_change_the_parameters() {
        let mut arguments = Arguments::default();
        arguments.set([&b"a"[..], b"b c", b""].into_iter()).unwrap();
        assert_eq!(all(&arguments), [&b"a"[..], b"b c", b""]);
        assert_eq!(arguments.get(0), None);
        assert_eq!(arguments.get(4), None);
        assert!(arguments.shift(2));
        assert_eq!(all(&arguments), [&b""[..]]);
        assert!(!arguments.shift(2));
        arguments.set([&b"x"[..]].into_iter()).unwrap();
        assert_eq!(all(&arguments), [&b"x"[..]]);
    }

    #[test]
    fn a_call_s_frame_hides_its_caller_s_parameters_until_it_returns() {
        let mut arguments = Arguments::default();
        arguments.set([&b"outer"[..], b"two"].into_iter()).unwrap();
        arguments.shift(1);
        arguments.push_frame([&b"inner"[..]].into_iter()).unwrap();
        assert_eq!(all(&arguments), [&b"inner"[..]]);
        arguments
            .set([&b"changed"[..], b"too"].into_iter())
            .unwrap();
        assert_eq!(arguments.count(), 2);
        arguments.pop_frame();
        assert_eq!(all(&arguments), [&b"two"[..]]);
        arguments.pop_frame();
        assert_eq!(all(&arguments), [&b"two"[..]]);
    }

    #[test]
    fn what_does_not_fit_is_refused_and_changes_nothing() {
        let mut arguments = Arguments::default();
        arguments.set([&b"kept"[..]].into_iter()).unwrap();
        let big = [b'x'; ARGUMENTS_CAPACITY + 1];
        assert_eq!(arguments.set([&big[..]].into_iter()), Err(Full));
        assert_eq!(all(&arguments), [&b"kept"[..]]);
        let many = vec![&b"a"[..]; MAX_ARGUMENTS];
        assert_eq!(arguments.push_frame(many.into_iter()), Err(Full));
        assert_eq!(all(&arguments), [&b"kept"[..]]);
        for _ in 1..MAX_FRAMES {
            arguments.push_frame([].into_iter()).unwrap();
        }
        assert_eq!(arguments.push_frame([].into_iter()), Err(Full));
    }
}
