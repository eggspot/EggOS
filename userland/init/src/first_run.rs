//! First run's place in init (D224, docs/ux/first-run-accessibility-v0.3.md section 1.1): on an
//! image that was never installed, booted with a disk whose settings hold no profile yet, init
//! starts `settings --first-run` right after the login program, without waiting for it. An
//! installed system asked its questions in the installer (`/etc/eggos/installed`); without a
//! disk the answers could not reach the next boot; without a display there is no window; and
//! a machine an agent drives (`/dev/agent`, `--mcp`) is set up through `settings_set`, so
//! none of them opens it. Nor does a boot whose kernel command line holds [`SKIP_WORD`], as the
//! host's automated checks pass unless they test the first run.

/// The program and its argument, each with its terminating NUL.
pub const FIRST_RUN: [&[u8]; 2] = [b"/bin/settings\0", b"--first-run\0"];

/// The file whose presence says the system was installed, with its terminating NUL.
pub const INSTALLED_MARK: &[u8] = b"/etc/eggos/installed\0";

/// The first disk's device, whose presence says a disk is attached, with its terminating NUL.
pub const DISK_DEVICE: &[u8] = b"/dev/vda\0";

/// The settings file (`eggos_settings_file::PATH`), with its terminating NUL.
pub const SETTINGS_FILE: &[u8] = b"/home/root/.config/eggos/settings\0";

/// The kernel command line, with its terminating NUL.
pub const COMMAND_LINE: &[u8] = b"/proc/cmdline\0";

/// The kernel command line's word that says not to ask.
pub const SKIP_WORD: &[u8] = b"eggos.first_run=skip";

/// The longest kernel command line init reads.
pub const COMMAND_LINE_BYTES: usize = 4096;

/// Whether `command_line` holds [`SKIP_WORD`] as a word of its own.
pub fn skipped_by(command_line: &[u8]) -> bool {
    command_line
        .split(|byte| byte.is_ascii_whitespace() || *byte == 0)
        .any(|word| word == SKIP_WORD)
}

/// What init finds on the machine at boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Found {
    /// `/dev/window` opens.
    pub display: bool,
    /// `/dev/vda` exists.
    pub disk: bool,
    /// `/dev/agent` exists.
    pub agent_channel: bool,
    /// `/etc/eggos/installed` exists.
    pub installed: bool,
    /// The kernel command line holds [`SKIP_WORD`].
    pub skipped: bool,
}

/// Whether init starts the first run, given what it found and the settings file's bytes
/// (`None` when there is no file).
pub fn wanted(found: Found, settings: Option<&[u8]>) -> bool {
    found.display
        && found.disk
        && !found.agent_channel
        && !found.installed
        && !found.skipped
        && settings.is_none_or(|file| {
            eggos_settings_file::value(file, eggos_settings_file::SHELL_PROFILE).is_none()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRESH: Found = Found {
        display: true,
        disk: true,
        agent_channel: false,
        installed: false,
        skipped: false,
    };

    #[test]
    fn the_paths_are_c_strings_and_the_settings_file_is_the_one_settings_writes() {
        for text in [
            FIRST_RUN[0],
            FIRST_RUN[1],
            INSTALLED_MARK,
            DISK_DEVICE,
            SETTINGS_FILE,
            COMMAND_LINE,
        ] {
            assert_eq!(
                text.iter().position(|&byte| byte == 0),
                Some(text.len() - 1)
            );
        }
        assert_eq!(
            &SETTINGS_FILE[..SETTINGS_FILE.len() - 1],
            eggos_settings_file::PATH.as_bytes()
        );
    }

    #[test]
    fn a_fresh_disk_without_a_profile_asks_and_a_saved_profile_does_not() {
        assert!(wanted(FRESH, None));
        assert!(wanted(
            FRESH,
            Some(b"eggos-settings 1\nkeyboard.layout=de\n")
        ));
        assert!(!wanted(
            FRESH,
            Some(b"eggos-settings 1\nshell.profile=windows\n")
        ));
    }

    #[test]
    fn no_disk_no_display_an_agent_or_an_installed_system_never_asks() {
        for found in [
            Found {
                disk: false,
                ..FRESH
            },
            Found {
                display: false,
                ..FRESH
            },
            Found {
                agent_channel: true,
                ..FRESH
            },
            Found {
                installed: true,
                ..FRESH
            },
            Found {
                skipped: true,
                ..FRESH
            },
        ] {
            assert!(!wanted(found, None), "{found:?}");
        }
    }

    #[test]
    fn the_skip_word_counts_only_as_a_whole_word_of_the_command_line() {
        assert!(skipped_by(b"eggos.first_run=skip"));
        assert!(skipped_by(
            b"eggos.display=offscreen eggos.first_run=skip\n"
        ));
        assert!(skipped_by(b"eggos.first_run=skip\0"));
        assert!(!skipped_by(b""));
        assert!(!skipped_by(b"eggos.first_run=ask"));
        assert!(!skipped_by(b"eggos.first_run=skipped"));
        assert!(!skipped_by(b"xeggos.first_run=skip"));
    }
}
