//! What the user lets agents do (D21, D137), set on the host when EggOS starts
//! (`run-eggos.sh --mcp --mcp-allow write,install`, which puts `eggos.mcp_allow=write,install`
//! on the kernel command line) and fixed for the boot: the server reads it from
//! `/proc/cmdline` only, which nothing in the machine can change. Without it, agents only read.
//! `run` runs any program, which can do anything, so it implies `write` and `install`.

#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

/// The kernel command line's parameter holding the grants.
pub const PARAMETER: &[u8] = b"eggos.mcp_allow=";

/// Where the server reads the kernel command line.
pub const COMMAND_LINE_PATH: &str = "/proc/cmdline";

/// The most bytes of unknown names kept for the audit log.
pub const MAX_UNKNOWN_BYTES: usize = 128;

/// One thing the user can let agents do beyond reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    /// Write and delete files below `/home/root` (`tee`, `trash`).
    Write,
    /// Install, open and uninstall apps (`app_install`, `app_open`, `app_uninstall`).
    Install,
    /// Run any program (`run`): full control of the machine.
    Run,
}

impl Grant {
    /// Every grant, in the order the log names them.
    pub const ALL: [Grant; 3] = [Grant::Write, Grant::Install, Grant::Run];

    /// Its name on the command line and in messages.
    pub fn name(self) -> &'static str {
        match self {
            Grant::Write => "write",
            Grant::Install => "install",
            Grant::Run => "run",
        }
    }

    fn bit(self) -> u8 {
        match self {
            Grant::Write => 1,
            Grant::Install => 2,
            Grant::Run => 4,
        }
    }

    fn named(name: &[u8]) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|grant| grant.name().as_bytes() == name)
    }
}

/// The grants of this boot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Grants(u8);

impl Grants {
    /// Reading only.
    pub const NONE: Grants = Grants(0);

    /// Every grant.
    pub const ALL: Grants = Grants(7);

    /// These grants and `grant`, with what it implies: `run` brings `write` and `install`.
    pub fn with(self, grant: Grant) -> Self {
        match grant {
            Grant::Run => {
                Grants(self.0 | Grant::Run.bit() | Grant::Write.bit() | Grant::Install.bit())
            }
            grant => Grants(self.0 | grant.bit()),
        }
    }

    /// Whether `grant` is given.
    pub fn allows(self, grant: Grant) -> bool {
        self.0 & grant.bit() != 0
    }

    /// The grants as the log shows them: `read`, then each given grant, comma-separated.
    pub fn describe(self, out: &mut [u8]) -> usize {
        let mut length = 0;
        let mut put = |text: &[u8], length: &mut usize| {
            if let Some(place) = out.get_mut(*length..*length + text.len()) {
                place.copy_from_slice(text);
                *length += text.len();
            }
        };
        put(b"read", &mut length);
        for grant in Grant::ALL.into_iter().filter(|&grant| self.allows(grant)) {
            put(b",", &mut length);
            put(grant.name().as_bytes(), &mut length);
        }
        length
    }
}

/// Names on the command line that are not grants, comma-separated as given, for the audit log
/// (a typo such as `Write` grants nothing, and shows there).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unknown {
    bytes: [u8; MAX_UNKNOWN_BYTES],
    length: usize,
}

impl Default for Unknown {
    fn default() -> Self {
        Self {
            bytes: [0; MAX_UNKNOWN_BYTES],
            length: 0,
        }
    }
}

impl Unknown {
    /// The names, as given; at most [`MAX_UNKNOWN_BYTES`].
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..self.length).unwrap_or_default()
    }

    fn push(&mut self, name: &[u8]) {
        let separator: &[u8] = if self.length == 0 { b"" } else { b"," };
        for piece in [separator, name] {
            let room = MAX_UNKNOWN_BYTES - self.length;
            let taken = piece.get(..piece.len().min(room)).unwrap_or_default();
            if let Some(place) = self.bytes.get_mut(self.length..self.length + taken.len()) {
                place.copy_from_slice(taken);
                self.length += taken.len();
            }
        }
    }
}

/// The grants the kernel command line `line` gives: the value of its first
/// `eggos.mcp_allow=` word, comma-separated names; empty names are skipped, unknown ones grant
/// nothing and are returned.
pub fn from_command_line(line: &[u8]) -> (Grants, Unknown) {
    let mut grants = Grants::NONE;
    let mut unknown = Unknown::default();
    let Some(value) = line
        .split(u8::is_ascii_whitespace)
        .find_map(|word| word.strip_prefix(PARAMETER))
    else {
        return (grants, unknown);
    };
    for name in value
        .split(|&byte| byte == b',')
        .filter(|name| !name.is_empty())
    {
        match Grant::named(name) {
            Some(grant) => grants = grants.with(grant),
            None => unknown.push(name),
        }
    }
    (grants, unknown)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

    use super::*;

    fn described(grants: Grants) -> String {
        let mut out = [0u8; 64];
        let length = grants.describe(&mut out);
        String::from_utf8(out[..length].to_vec()).unwrap()
    }

    #[test]
    fn no_parameter_means_reading_only() {
        for line in [
            &b""[..],
            b"console=ttyS0",
            b"eggos.mcp_allowx=run",
            b"xeggos.mcp_allow=run",
        ] {
            let (grants, unknown) = from_command_line(line);
            assert_eq!(grants, Grants::NONE, "{line:?}");
            assert_eq!(unknown.as_bytes(), b"");
        }
        assert_eq!(described(Grants::NONE), "read");
    }

    #[test]
    fn named_grants_are_given_and_run_implies_the_others() {
        let (grants, _) = from_command_line(b"quiet eggos.mcp_allow=write,install\n");
        assert!(grants.allows(Grant::Write) && grants.allows(Grant::Install));
        assert!(!grants.allows(Grant::Run));
        assert_eq!(described(grants), "read,write,install");
        let (grants, _) = from_command_line(b"eggos.mcp_allow=run");
        assert_eq!(grants, Grants::ALL);
        assert_eq!(described(grants), "read,write,install,run");
        let (grants, _) = from_command_line(b"eggos.mcp_allow=install");
        assert!(!grants.allows(Grant::Write));
    }

    #[test]
    fn the_first_parameter_wins_and_odd_names_grant_nothing() {
        let (grants, unknown) = from_command_line(b"eggos.mcp_allow=write eggos.mcp_allow=run");
        assert_eq!(described(grants), "read,write");
        assert_eq!(unknown.as_bytes(), b"");
        let (grants, unknown) = from_command_line(b"eggos.mcp_allow=Write,,run2,install,");
        assert_eq!(described(grants), "read,install");
        assert_eq!(unknown.as_bytes(), b"Write,run2");
        let (grants, _) = from_command_line(b"eggos.mcp_allow=");
        assert_eq!(grants, Grants::NONE);
        let (grants, _) = from_command_line(b"eggos.mcp_allow=wr");
        assert_eq!(grants, Grants::NONE);
        let long = format!("eggos.mcp_allow={}", "x".repeat(300));
        let (_, unknown) = from_command_line(long.as_bytes());
        assert_eq!(unknown.as_bytes().len(), MAX_UNKNOWN_BYTES);
    }
}
