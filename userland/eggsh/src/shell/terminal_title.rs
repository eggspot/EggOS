const OSC_TITLE: &[u8] = b"\x1b]0;";
const OSC_FOLDER: &[u8] = b"\x1b]7;file://";
const BEL: &[u8] = b"\x07";
const HEX: &[u8; 16] = b"0123456789ABCDEF";

pub(super) const TERM_PROGRAM: &[u8] = b"TERM_PROGRAM";

fn keeps(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/')
}

pub(super) fn percent_encoded(path: &[u8], push: &mut impl FnMut(&[u8])) {
    for &byte in path {
        if keeps(byte) {
            push(&[byte]);
        } else {
            push(&[
                b'%',
                HEX[usize::from(byte >> 4)],
                HEX[usize::from(byte & 0xf)],
            ]);
        }
    }
}

fn push_printable(text: &[u8], push: &mut impl FnMut(&[u8])) {
    for part in text.split(|&byte| byte < 0x20 || byte == 0x7f) {
        push(part);
    }
}

pub(super) fn folder_title(folder: &[u8], home: Option<&[u8]>, push: &mut impl FnMut(&[u8])) {
    let home = home.filter(|home| !home.is_empty() && *home != &b"/"[..]);
    let rest = home.and_then(|home| folder.strip_prefix(home));
    match rest {
        Some(rest) if rest.is_empty() || rest.starts_with(b"/") => {
            push(b"~");
            push_printable(rest, push);
        }
        _ => push_printable(folder, push),
    }
}

pub(super) fn announce_folder(folder: &[u8], home: Option<&[u8]>, push: &mut impl FnMut(&[u8])) {
    push(OSC_TITLE);
    folder_title(folder, home, push);
    push(BEL);
    push(OSC_FOLDER);
    percent_encoded(folder, push);
    push(BEL);
}

pub(super) fn announce_command(name: &[u8], push: &mut impl FnMut(&[u8])) {
    let start = name
        .iter()
        .rposition(|&byte| byte == b'/')
        .map_or(0, |slash| slash + 1);
    push(OSC_TITLE);
    push_printable(&name[start..], push);
    push(BEL);
}
