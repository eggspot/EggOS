use crate::output::Output;
use crate::system::System;
use core::ffi::CStr;

pub fn echo<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let (newline, words) = match arguments {
        [first, rest @ ..] if first.to_bytes() == b"-n" => (false, rest),
        _ => (true, arguments),
    };
    for (index, word) in words.iter().enumerate() {
        if index > 0 {
            output.write(b" ");
        }
        output.write(word.to_bytes());
    }
    if newline {
        output.write(b"\n");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn run(arguments: &[&CStr]) -> String {
        let mut output = Output::new(Fake::default(), b"echo");
        assert_eq!(echo(&mut output, arguments), 0);
        output.flush();
        output.system.output()
    }

    #[test]
    fn words_are_joined_by_spaces_and_ended_by_a_newline_unless_dash_n() {
        assert_eq!(run(&[]), "\n");
        assert_eq!(run(&[c"a", c"b  c", c""]), "a b  c \n");
        assert_eq!(run(&[c"-n", c"x", c"y"]), "x y");
        assert_eq!(run(&[c"-n"]), "");
        assert_eq!(run(&[c"x", c"-n"]), "x -n\n");
        assert_eq!(run(&[c"-e", c"\\n"]), "-e \\n\n");
    }
}
