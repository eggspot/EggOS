//! The processes `/proc/ps` lists (D168), for the Toolhelp snapshots (D178): one line per
//! process, `id parent group session state name`, the name cut to 15 bytes as Linux's `comm`;
//! the whole name is then the program's file name in its command line.

/// What a line of `/proc/ps` says of a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Listed<'a> {
    pub id: u32,
    pub parent: u32,
    pub name: &'a [u8],
}

/// The longest name `/proc/ps` gives whole: longer ones are cut to it.
pub const CUT_NAME: usize = 15;

fn number(word: &[u8]) -> Option<u32> {
    core::str::from_utf8(word).ok()?.parse().ok()
}

/// The process line `line` (without its newline) describes.
pub fn parse_line(line: &[u8]) -> Option<Listed<'_>> {
    let mut rest = line;
    let mut fields = [0u32; 2];
    for index in 0..5 {
        let space = rest.iter().position(|byte| *byte == b' ')?;
        let word = &rest[..space];
        if index < 2 {
            fields[index] = number(word)?;
        }
        rest = &rest[space + 1..];
    }
    Some(Listed {
        id: fields[0],
        parent: fields[1],
        name: rest,
    })
}

/// The whole name of a process whose listed name is `cut`, from its command line `arguments`
/// (each ended by a NUL): the file name of the first argument that starts with it.
pub fn whole_name<'a>(cut: &[u8], arguments: &'a [u8]) -> Option<&'a [u8]> {
    arguments
        .split(|byte| *byte == 0)
        .map(
            |argument| match argument.iter().rposition(|byte| *byte == b'/') {
                Some(slash) => &argument[slash + 1..],
                None => argument,
            },
        )
        .find(|name| name.len() > cut.len() && name.starts_with(cut))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_give_ids_and_names() {
        assert_eq!(
            parse_line(b"42 1 42 1 R AutoHotkey64.ex"),
            Some(Listed {
                id: 42,
                parent: 1,
                name: b"AutoHotkey64.ex"
            })
        );
        assert_eq!(
            parse_line(b"7 0 7 7 Z my tool").map(|listed| listed.name),
            Some(&b"my tool"[..])
        );
        assert_eq!(parse_line(b"x 1 1 1 R a"), None);
        assert_eq!(parse_line(b"1 1"), None);
    }

    #[test]
    fn cut_names_are_made_whole_from_the_command_line() {
        let arguments = b"/usr/lib/eggos/windows\0/home/root/AutoHotkey64.exe\0test.ahk\0";
        assert_eq!(
            whole_name(b"AutoHotkey64.ex", arguments),
            Some(&b"AutoHotkey64.exe"[..])
        );
        assert_eq!(whole_name(b"other", arguments), None);
    }
}
