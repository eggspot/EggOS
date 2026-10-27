use crate::cat::cat;
use crate::changes::{cp, mkdir, mv, rm, touch};
use crate::echo::echo;
use crate::grep::grep;
use crate::head::{head, tail};
use crate::ls::{Listing, ls};
use crate::output::{FAILURE, Output};
use crate::power::{HALT, POWER_OFF, RESTART, power};
use crate::processes::{free, ps, uptime};
use crate::sleep::sleep;
use crate::system::{STANDARD_ERROR, STANDARD_OUTPUT, System};
use crate::uname::uname;
use crate::wc::wc;
use core::ffi::CStr;

pub const MAX_ARGUMENTS: usize = 256;

pub const PROGRAM_NAME: &[u8] = b"coreutils";

const UTILITY_NOT_FOUND: i32 = 127;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Utility {
    Cat,
    Cp,
    Echo,
    False,
    Free,
    Grep,
    Halt,
    Head,
    Ls,
    Mkdir,
    Mv,
    Poweroff,
    Ps,
    Reboot,
    Rm,
    Sleep,
    Tail,
    Touch,
    True,
    Uname,
    Uptime,
    Wc,
}

impl Utility {
    pub const ALL: [Self; 22] = [
        Self::Cat,
        Self::Cp,
        Self::Echo,
        Self::False,
        Self::Free,
        Self::Grep,
        Self::Halt,
        Self::Head,
        Self::Ls,
        Self::Mkdir,
        Self::Mv,
        Self::Poweroff,
        Self::Ps,
        Self::Reboot,
        Self::Rm,
        Self::Sleep,
        Self::Tail,
        Self::Touch,
        Self::True,
        Self::Uname,
        Self::Uptime,
        Self::Wc,
    ];

    pub fn name(self) -> &'static [u8] {
        match self {
            Self::Cat => b"cat",
            Self::Cp => b"cp",
            Self::Echo => b"echo",
            Self::False => b"false",
            Self::Free => b"free",
            Self::Grep => b"grep",
            Self::Halt => b"halt",
            Self::Head => b"head",
            Self::Ls => b"ls",
            Self::Mkdir => b"mkdir",
            Self::Mv => b"mv",
            Self::Poweroff => b"poweroff",
            Self::Ps => b"ps",
            Self::Reboot => b"reboot",
            Self::Rm => b"rm",
            Self::Sleep => b"sleep",
            Self::Tail => b"tail",
            Self::Touch => b"touch",
            Self::True => b"true",
            Self::Uname => b"uname",
            Self::Uptime => b"uptime",
            Self::Wc => b"wc",
        }
    }

    pub fn named(name: &[u8]) -> Option<Self> {
        Self::ALL.into_iter().find(|utility| utility.name() == name)
    }

    fn run<S: System>(self, output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
        match self {
            Self::Cat => cat(output, arguments),
            Self::Cp => cp(output, arguments),
            Self::Echo => echo(output, arguments),
            Self::False => FAILURE,
            Self::Free => free(output, arguments),
            Self::Grep => grep(output, arguments),
            Self::Halt => power(output, arguments, HALT),
            Self::Head => head(output, arguments),
            Self::Ls => ls(output, &mut Listing::new(), arguments),
            Self::Mkdir => mkdir(output, arguments),
            Self::Mv => mv(output, arguments),
            Self::Poweroff => power(output, arguments, POWER_OFF),
            Self::Ps => ps(output, arguments),
            Self::Reboot => power(output, arguments, RESTART),
            Self::Rm => rm(output, arguments),
            Self::Sleep => sleep(output, arguments),
            Self::Tail => tail(output, arguments),
            Self::Touch => touch(output, arguments),
            Self::True => 0,
            Self::Uname => uname(output, arguments),
            Self::Uptime => uptime(output, arguments),
            Self::Wc => wc(output, arguments),
        }
    }
}

pub fn base_name(path: &[u8]) -> &[u8] {
    path.rsplit(|&byte| byte == b'/').next().unwrap_or(path)
}

pub fn run<S: System>(mut system: S, arguments: &[&CStr]) -> i32 {
    let invoked = arguments
        .first()
        .map_or(PROGRAM_NAME, |program| base_name(program.to_bytes()));
    let mut rest = arguments.get(1..).unwrap_or(&[]);
    let name = match (invoked, rest.first()) {
        (PROGRAM_NAME, None) => {
            write_usage(&mut system, STANDARD_ERROR);
            return FAILURE;
        }
        (PROGRAM_NAME, Some(first)) if first.to_bytes() == b"--list" => {
            for utility in Utility::ALL {
                let _ = system.write(STANDARD_OUTPUT, utility.name());
                let _ = system.write(STANDARD_OUTPUT, b"\n");
            }
            return 0;
        }
        (PROGRAM_NAME, Some(first)) => {
            rest = &rest[1..];
            first.to_bytes()
        }
        (invoked, _) => invoked,
    };
    let Some(utility) = Utility::named(name) else {
        for part in [PROGRAM_NAME, b": ", name, b": no such utility\n"] {
            let _ = system.write(STANDARD_ERROR, part);
        }
        return UTILITY_NOT_FOUND;
    };
    let mut output = Output::new(system, utility.name());
    let status = utility.run(&mut output, rest);
    output.finish(status)
}

fn write_usage<S: System>(system: &mut S, descriptor: usize) {
    let _ = system.write(
        descriptor,
        b"usage: coreutils <utility> [arguments], or a link to coreutils named after the utility\n\
          utilities:",
    );
    for utility in Utility::ALL {
        let _ = system.write(descriptor, b" ");
        let _ = system.write(descriptor, utility.name());
    }
    let _ = system.write(descriptor, b"\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Fake;

    fn run_with(fake: &mut Fake, arguments: &[&CStr]) -> i32 {
        run(fake, arguments)
    }

    #[test]
    fn the_utility_is_the_last_component_of_the_name_it_was_run_by() {
        let mut fake = Fake::default();
        assert_eq!(run_with(&mut fake, &[c"/bin/echo", c"hi"]), 0);
        assert_eq!(run_with(&mut fake, &[c"uname"]), 0);
        assert_eq!(run_with(&mut fake, &[c"./bin/false"]), 1);
        assert_eq!(run_with(&mut fake, &[c"true", c"--anything"]), 0);
        assert_eq!(fake.output(), "hi\nLinux\n");
    }

    #[test]
    fn as_coreutils_the_first_argument_names_the_utility() {
        let mut fake = Fake::default();
        assert_eq!(
            run_with(&mut fake, &[c"/bin/coreutils", c"echo", c"a", c"b"]),
            0
        );
        assert_eq!(run_with(&mut fake, &[c"coreutils", c"--list"]), 0);
        let names: Vec<String> = Utility::ALL
            .iter()
            .map(|utility| String::from_utf8_lossy(utility.name()).into_owned())
            .collect();
        assert_eq!(fake.output(), format!("a b\n{}\n", names.join("\n")));
        assert_eq!(run_with(&mut fake, &[c"coreutils"]), 1);
        assert!(fake.errors().starts_with("usage: coreutils <utility>"));
        assert!(run_with(&mut fake, &[]) == 1);
    }

    #[test]
    fn an_unknown_utility_is_not_found() {
        let mut fake = Fake::default();
        assert_eq!(run_with(&mut fake, &[c"/bin/frobnicate"]), 127);
        assert_eq!(run_with(&mut fake, &[c"coreutils", c"/bin/ls"]), 127);
        assert_eq!(
            fake.errors(),
            "coreutils: frobnicate: no such utility\ncoreutils: /bin/ls: no such utility\n"
        );
    }

    #[test]
    fn every_utility_has_a_distinct_name_and_is_found_by_it() {
        for utility in Utility::ALL {
            assert_eq!(Utility::named(utility.name()), Some(utility));
        }
        assert_eq!(base_name(b"/a/b/"), b"");
        assert_eq!(base_name(b"ls"), b"ls");
    }
}
