use crate::output::{FAILURE, Output};
use crate::system::System;
use crate::window_input::{MAX_RETRIES, RETRY_PAUSE, parse};
use crate::wm::{
    CHOOSE_MENU, CHOOSE_MENU_SIZE, GET_MENU, GET_MENU_SIZE, USAGE_STATUS, number, open_device,
    refused, usage,
};
use core::ffi::CStr;
use eggos_runtime::errno::Errno;

pub const HELP: &[u8] = b"wm menu: lists a window's main menu (the menus a macOS app shows in \
the top bar): each menu's number and title, then each entry's number, label, key equivalent in \
parentheses and [disabled]; separators are ---- and count. wm menu --choose chooses an item as a \
click on it would: <menu> and <entry> are those numbers, or a title and a label (not \
case-sensitive; a word of digits is always a number); --label <label> refuses the choice unless \
the entry is labelled so (for numbers read earlier, when the app may have changed its menu).\n";

const POOL_AT: usize = 8;
const POOL_CAPACITY: usize = GET_MENU_SIZE - POOL_AT;
const LABEL_AT: usize = 16;
const LABEL_CAPACITY: usize = CHOOSE_MENU_SIZE - LABEL_AT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line<'a> {
    Menu(&'a [u8]),
    Item {
        label: &'a [u8],
        key: &'a [u8],
        enabled: bool,
    },
    Separator,
}

pub fn lines(pool: &[u8]) -> impl Iterator<Item = Line<'_>> {
    pool.split(|&byte| byte == b'\n')
        .filter_map(|line| match line.split_first()? {
            (&b'M', title) => Some(Line::Menu(title)),
            (&kind @ (b'I' | b'D'), rest) => {
                let tab = rest
                    .iter()
                    .position(|&byte| byte == b'\t')
                    .unwrap_or(rest.len());
                Some(Line::Item {
                    label: &rest[..tab],
                    key: rest.get(tab + 1..).unwrap_or(&[]),
                    enabled: kind == b'I',
                })
            }
            (&b'-', _) => Some(Line::Separator),
            _ => None,
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice<'a> {
    Item {
        menu: usize,
        entry: usize,
        title: &'a [u8],
        label: &'a [u8],
    },
    NoMenu,
    NoEntry,
    Separator,
    Disabled,
}

fn names(word: &[u8], index: usize, text: &[u8]) -> bool {
    match number(word) {
        Some(value) => usize::try_from(value).ok() == Some(index),
        None => word.eq_ignore_ascii_case(text),
    }
}

pub fn resolve<'a>(pool: &'a [u8], menu_word: &[u8], entry_word: &[u8]) -> Choice<'a> {
    let mut menus = 0;
    let mut chosen: Option<(usize, &'a [u8])> = None;
    let mut entry = 0;
    for line in lines(pool) {
        match (line, chosen) {
            (Line::Menu(_), Some(_)) => break,
            (Line::Menu(title), None) => {
                if names(menu_word, menus, title) {
                    chosen = Some((menus, title));
                }
                menus += 1;
            }
            (_, None) => {}
            (Line::Separator, Some(_)) => {
                if names(entry_word, entry, b"----") {
                    return Choice::Separator;
                }
                entry += 1;
            }
            (Line::Item { label, enabled, .. }, Some((menu, title))) => {
                if names(entry_word, entry, label) {
                    return match enabled {
                        true => Choice::Item {
                            menu,
                            entry,
                            title,
                            label,
                        },
                        false => Choice::Disabled,
                    };
                }
                entry += 1;
            }
        }
    }
    match chosen {
        Some(_) => Choice::NoEntry,
        None => Choice::NoMenu,
    }
}

fn list<S: System>(output: &mut Output<S>, pool: &[u8]) {
    if pool.is_empty() {
        output.write(b"no main menu\n");
        return;
    }
    let mut menus = 0;
    let mut entry = 0;
    for line in lines(pool) {
        match line {
            Line::Menu(title) => {
                output.write(b"menu ");
                output.write_number(menus, 0);
                output.write(b": ");
                output.write(title);
                menus += 1;
                entry = 0;
            }
            Line::Separator => {
                output.write(b"  ");
                output.write_number(entry, 0);
                output.write(b" ----");
                entry += 1;
            }
            Line::Item {
                label,
                key,
                enabled,
            } => {
                output.write(b"  ");
                output.write_number(entry, 0);
                output.write(b" ");
                output.write(label);
                if !key.is_empty() {
                    output.write(b" (");
                    output.write(key);
                    output.write(b")");
                }
                if !enabled {
                    output.write(b" [disabled]");
                }
                entry += 1;
            }
        }
        output.write(b"\n");
    }
}

pub fn menu<S: System>(output: &mut Output<S>, arguments: &[&CStr]) -> i32 {
    let Some(parsed) = parse(arguments, &[b"--choose", b"--label"]) else {
        return usage(output);
    };
    let wanted = match parsed.choose {
        true => 3,
        false => 1,
    };
    if parsed.count != wanted || (parsed.label.is_some() && !parsed.choose) {
        return usage(output);
    }
    let window_text = parsed.positionals[0];
    let Some(window) = number(window_text).and_then(|id| u32::try_from(id).ok()) else {
        output.error(&[b"invalid window id '", window_text, b"'"]);
        return USAGE_STATUS;
    };
    let Some(device) = open_device(output) else {
        return FAILURE;
    };
    let mut request = [0u8; GET_MENU_SIZE];
    request[..4].copy_from_slice(&window.to_ne_bytes());
    let status = match output.system.device_control(device, GET_MENU, &mut request) {
        Err(errno) => refused(output, window_text, errno),
        Ok(()) => {
            let length = u32::from_ne_bytes([request[4], request[5], request[6], request[7]]);
            let length = (length as usize).min(POOL_CAPACITY);
            let pool = &request[POOL_AT..POOL_AT + length];
            match parsed.choose {
                false => {
                    list(output, pool);
                    0
                }
                true => {
                    let chooser = Chooser {
                        device,
                        window,
                        window_text,
                        expected_label: parsed.label,
                    };
                    chooser.choose(output, pool, parsed.positionals[1], parsed.positionals[2])
                }
            }
        }
    };
    output.system.close(device);
    status
}

struct Chooser<'a> {
    device: usize,
    window: u32,
    window_text: &'a [u8],
    expected_label: Option<&'a [u8]>,
}

impl Chooser<'_> {
    fn choose<S: System>(
        &self,
        output: &mut Output<S>,
        pool: &[u8],
        menu_word: &[u8],
        entry_word: &[u8],
    ) -> i32 {
        let window_text = self.window_text;
        let (menu, entry, title, label) = match resolve(pool, menu_word, entry_word) {
            _ if pool.is_empty() => {
                output.error(&[window_text, b": the window has no main menu"]);
                return FAILURE;
            }
            Choice::Item {
                menu,
                entry,
                title,
                label,
            } => (menu, entry, title, label),
            Choice::NoMenu => {
                output.error(&[window_text, b": no menu '", menu_word, b"' (see wm menu)"]);
                return FAILURE;
            }
            Choice::NoEntry => {
                output.error(&[
                    window_text,
                    b": no entry '",
                    entry_word,
                    b"' in menu '",
                    menu_word,
                    b"' (see wm menu)",
                ]);
                return FAILURE;
            }
            Choice::Separator => {
                output.error(&[window_text, b": entry '", entry_word, b"' is a separator"]);
                return FAILURE;
            }
            Choice::Disabled => {
                output.error(&[window_text, b": entry '", entry_word, b"' is disabled"]);
                return FAILURE;
            }
        };
        if let Some(expected) = self.expected_label
            && !expected.eq_ignore_ascii_case(label)
        {
            output.error(&[
                window_text,
                b": entry '",
                entry_word,
                b"' of menu '",
                menu_word,
                b"' is '",
                label,
                b"', not '",
                expected,
                b"' (the menu changed; see wm menu)",
            ]);
            return FAILURE;
        }
        let mut request = [0u8; CHOOSE_MENU_SIZE];
        let length = label.len().min(LABEL_CAPACITY);
        for (index, value) in [self.window, menu as u32, entry as u32, length as u32]
            .into_iter()
            .enumerate()
        {
            request[index * 4..index * 4 + 4].copy_from_slice(&value.to_ne_bytes());
        }
        request[LABEL_AT..LABEL_AT + length].copy_from_slice(&label[..length]);
        match self.send(&mut output.system, &mut request) {
            Ok(()) => {
                output.write(b"chose ");
                output.write(title);
                output.write(b" > ");
                output.write(label);
                output.write(b" in window ");
                output.write(window_text);
                output.write(b"\n");
                0
            }
            Err(Errno::EINVAL) => {
                output.error(&[
                    window_text,
                    b": the item cannot be chosen now: the app shows a dialog, or its menu \
                      changed (see wm menu)",
                ]);
                FAILURE
            }
            Err(errno) => refused(output, window_text, errno),
        }
    }

    fn send<S: System>(
        &self,
        system: &mut S,
        request: &mut [u8; CHOOSE_MENU_SIZE],
    ) -> Result<(), Errno> {
        let mut retries = 0;
        loop {
            match system.device_control(self.device, CHOOSE_MENU, request) {
                Err(Errno::EAGAIN) if retries < MAX_RETRIES => {
                    retries += 1;
                    system.sleep(RETRY_PAUSE)?;
                }
                result => return result,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{Fake, FakeWindow};
    use crate::wm::{BUSY_STATUS, wm};
    use eggos_drivers::desktop::protocol::{
        MENU_LABEL_CAPACITY, choose_menu_request, get_menu_reply, request, size,
    };

    const MAC_PAINT: &[u8] = b"Mmac-paint\nIAbout mac-paint\t\n-\nIQuit mac-paint\tCmd+Q\n\
MFile\nIClear\tCmd+K\nDUndo\tCmd+Z\nIClose\tCmd+W\n";

    fn machine() -> Fake {
        let mut fake = Fake::default();
        for id in [129, 130] {
            fake.windows.push(FakeWindow {
                id,
                state: 0,
                rect: [0, 0, 100, 50],
                client: 67,
                title: b"window".to_vec(),
            });
        }
        fake.menus.insert(129, MAC_PAINT.to_vec());
        fake
    }

    fn run(arguments: &[&CStr], fake: Fake) -> (i32, Fake) {
        let mut output = Output::new(fake, b"wm");
        let status = wm(&mut output, arguments);
        let status = output.finish(status);
        (status, output.system)
    }

    #[test]
    fn the_requests_match_the_window_server_s() {
        assert_eq!(GET_MENU, request::GET_MENU);
        assert_eq!(CHOOSE_MENU, request::CHOOSE_MENU);
        assert_eq!(GET_MENU_SIZE, size::GET_MENU);
        assert_eq!(CHOOSE_MENU_SIZE, size::CHOOSE_MENU);
        assert_eq!(POOL_AT, get_menu_reply::POOL);
        assert_eq!(choose_menu_request::LABEL_LENGTH, 12);
        assert_eq!(LABEL_AT, choose_menu_request::LABEL);
        assert_eq!(LABEL_CAPACITY, MENU_LABEL_CAPACITY);
        assert_eq!(
            POOL_CAPACITY,
            eggos_drivers::desktop::protocol::MENU_POOL_CAPACITY
        );
    }

    #[test]
    fn the_list_numbers_each_menu_and_entry_with_its_key_and_state() {
        let (status, fake) = run(&[c"menu", c"129"], machine());
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        assert_eq!(
            fake.output(),
            "menu 0: mac-paint\n  0 About mac-paint\n  1 ----\n  2 Quit mac-paint (Cmd+Q)\n\
             menu 1: File\n  0 Clear (Cmd+K)\n  1 Undo (Cmd+Z) [disabled]\n  2 Close (Cmd+W)\n"
        );
        let (status, fake) = run(&[c"menu", c"--", c"130"], machine());
        assert_eq!((status, fake.output().as_str()), (0, "no main menu\n"));
    }

    #[test]
    fn choosing_takes_numbers_or_names_and_says_what_it_chose() {
        for (menu, entry) in [
            (c"1", c"0"),
            (c"file", c"CLEAR"),
            (c"File", c"0"),
            (c"1", c"clear"),
        ] {
            let (status, fake) = run(&[c"menu", c"--choose", c"129", menu, entry], machine());
            assert_eq!(
                (status, fake.errors().as_str()),
                (0, ""),
                "{menu:?} {entry:?}"
            );
            assert_eq!(fake.chosen_menus, [[129, 1, 0]]);
            assert_eq!(fake.chosen_labels, [b"Clear".to_vec()]);
            assert_eq!(fake.output(), "chose File > Clear in window 129\n");
        }
        let (status, fake) = run(
            &[c"menu", c"--choose", c"--", c"129", c"0", c"Quit mac-paint"],
            machine(),
        );
        assert_eq!(status, 0);
        assert_eq!(fake.chosen_menus, [[129, 0, 2]]);
        assert_eq!(fake.chosen_labels, [b"Quit mac-paint".to_vec()]);
    }

    #[test]
    fn a_label_given_with_numbers_must_be_the_entry_s() {
        let (status, fake) = run(
            &[
                c"menu",
                c"--choose",
                c"--label",
                c"clear",
                c"129",
                c"1",
                c"0",
            ],
            machine(),
        );
        assert_eq!((status, fake.errors().as_str()), (0, ""));
        assert_eq!(fake.chosen_menus, [[129, 1, 0]]);
        assert_eq!(fake.chosen_labels, [b"Clear".to_vec()]);
        let (status, fake) = run(
            &[
                c"menu",
                c"--choose",
                c"--label",
                c"Close",
                c"129",
                c"1",
                c"0",
            ],
            machine(),
        );
        assert_eq!(status, FAILURE);
        assert_eq!(
            fake.errors(),
            "wm: 129: entry '0' of menu '1' is 'Clear', not 'Close' (the menu changed; see wm \
             menu)\n"
        );
        assert!(fake.chosen_menus.is_empty());
        let (status, fake) = run(&[c"menu", c"--label", c"Clear", c"129"], machine());
        assert_eq!(status, 2);
        assert!(fake.chosen_menus.is_empty());
    }

    #[test]
    fn a_refused_choice_says_a_dialog_or_a_changed_menu_may_be_why() {
        let mut refusing = machine();
        refusing.input_results = vec![Some(Errno::EINVAL)];
        let (status, fake) = run(&[c"menu", c"--choose", c"129", c"1", c"0"], refusing);
        assert_eq!(status, FAILURE);
        assert_eq!(
            fake.errors(),
            "wm: 129: the item cannot be chosen now: the app shows a dialog, or its menu changed \
             (see wm menu)\n"
        );
    }

    #[test]
    fn what_cannot_be_chosen_is_named_and_nothing_is_sent() {
        for (window, menu, entry, error) in [
            (
                c"129",
                c"Edit",
                c"0",
                "wm: 129: no menu 'Edit' (see wm menu)\n",
            ),
            (c"129", c"2", c"0", "wm: 129: no menu '2' (see wm menu)\n"),
            (
                c"129",
                c"File",
                c"3",
                "wm: 129: no entry '3' in menu 'File' (see wm menu)\n",
            ),
            (c"129", c"0", c"1", "wm: 129: entry '1' is a separator\n"),
            (
                c"129",
                c"File",
                c"Undo",
                "wm: 129: entry 'Undo' is disabled\n",
            ),
            (c"130", c"0", c"0", "wm: 130: the window has no main menu\n"),
            (c"7", c"0", c"0", "wm: 7: no such window\n  try: wm list\n"),
        ] {
            let (status, fake) = run(&[c"menu", c"--choose", window, menu, entry], machine());
            assert_eq!(status, FAILURE, "{error}");
            assert_eq!(fake.errors(), error);
            assert!(fake.chosen_menus.is_empty());
        }
    }

    #[test]
    fn a_person_wins_and_a_full_queue_is_retried() {
        let mut busy = machine();
        busy.input_results = vec![Some(Errno::EBUSY)];
        let (status, fake) = run(&[c"menu", c"--choose", c"129", c"1", c"0"], busy);
        assert_eq!(status, BUSY_STATUS);
        assert_eq!(
            fake.errors(),
            "wm: 129: a person is using the machine; try again in a moment\n"
        );
        let mut waiting = machine();
        waiting.input_results = vec![Some(Errno::EAGAIN), Some(Errno::EAGAIN)];
        let (status, fake) = run(&[c"menu", c"--choose", c"129", c"1", c"0"], waiting);
        assert_eq!((status, fake.slept.len()), (0, 2));
        assert_eq!(fake.chosen_menus, [[129, 1, 0]]);
    }

    #[test]
    fn bad_menu_commands_are_usage_errors() {
        for arguments in [
            &[c"menu"][..],
            &[c"menu", c"129", c"1"][..],
            &[c"menu", c"--choose", c"129", c"1"][..],
            &[c"menu", c"--frob", c"129"][..],
            &[c"menu", c"--choose", c"129", c"1", c"0", c"2"][..],
        ] {
            let (status, fake) = run(arguments, machine());
            assert_eq!(status, 2, "{arguments:?}");
            assert!(fake.chosen_menus.is_empty());
        }
        let (status, fake) = run(&[c"menu", c"x"], machine());
        assert_eq!(status, 2);
        assert_eq!(fake.errors(), "wm: invalid window id 'x'\n");
    }

    #[test]
    fn a_pool_s_lines_are_menus_items_and_separators() {
        let parsed: Vec<Line<'_>> = lines(b"MA\nIOne\tCmd+1\nDTwo\t\n-\n").collect();
        assert_eq!(
            parsed,
            [
                Line::Menu(b"A"),
                Line::Item {
                    label: b"One",
                    key: b"Cmd+1",
                    enabled: true
                },
                Line::Item {
                    label: b"Two",
                    key: b"",
                    enabled: false
                },
                Line::Separator,
            ]
        );
        assert_eq!(resolve(b"", b"0", b"0"), Choice::NoMenu);
        assert_eq!(resolve(b"MA\n", b"-1", b"0"), Choice::NoMenu);
    }
}
