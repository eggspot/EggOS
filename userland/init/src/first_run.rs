//! First run's place in init (D223, docs/ux/first-run-accessibility-v0.3.md section 1.1): on an
//! image that was never installed, booted with a disk whose settings hold no profile yet, init
//! starts `settings --first-run` right after the login program, without waiting for it. An
//! installed system asked its questions in the installer (`/etc/eggos/installed`); without a
//! disk the answers could not reach the next boot; without a display there is no window; and
//! a machine an agent drives (`/dev/agent`, `--mcp`) is set up through `settings_set`, so
//! none of them opens it.

/// The program and its argument, each with its terminating NUL.
pub const FIRST_RUN: [&[u8]; 2] = [b"/bin/settings\0", b"--first-run\0"];

/// The file whose presence says the system was installed, with its terminating NUL.
pub const INSTALLED_MARK: &[u8] = b"/etc/eggos/installed\0";

/// The first disk's device, whose presence says a disk is attached, with its terminating NUL.
pub const DISK_DEVICE: &[u8] = b"/dev/vda\0";

/// The settings file (`eggos_settings_file::PATH`), with its terminating NUL.
pub const SETTINGS_FILE: &[u8] = b"/home/root/.config/eggos/settings\0";

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
}

/// Whether init starts the first run, given what it found and the settings file's bytes
/// (`None` when there is no file).
pub fn wanted(found: Found, settings: Option<&[u8]>) -> bool {
    found.display
        && found.disk
        && !found.agent_channel
        && !found.installed
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
    };

    #[test]
    fn the_paths_are_c_strings_and_the_settings_file_is_the_one_settings_writes() {
        for text in [FIRST_RUN[0], FIRST_RUN[1], INSTALLED_MARK, DISK_DEVICE, SETTINGS_FILE] {
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
        assert!(wanted(FRESH, Some(b"eggos-settings 1\nkeyboard.layout=de\n")));
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
        ] {
            assert!(!wanted(found, None), "{found:?}");
        }
    }
}
