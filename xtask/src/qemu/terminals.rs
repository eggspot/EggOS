//! The terminals boot test (D223): one boot of the release kernel with its display. The shell
//! on the serial console opens two terminal windows, as the launcher's Terminal does; both are
//! titled after the home folder (`~ - Terminal`, the shell's `OSC 0`), which `wm list` is asked
//! for again every 3 s until it shows, ten times at most. The second is resized
//! with `wm size`; `stty size` typed into it with `wm type` and `wm press` prints the rows and
//! columns its grid has, the same as `wm show`'s `screen` value (`<columns>x<rows>`), which
//! differs from the first size; `wm show` lists its `folder` (the shell's `OSC 7`) as the home
//! folder, its `row 1` as `/etc/motd`'s line and its `row 2` as the prompt; and `wm capture`
//! saves its content.

use super::settings::Boot;
use super::*;

const TIMEOUT: Duration = Duration::from_secs(180);
const OPEN: &[u8] = b"echo EGGOS-HOME $HOME; terminal; terminal; sleep 3; wm list\r";
const LIST_AGAIN: &[u8] = b"sleep 3; wm list\r";
const LISTINGS: usize = 10;
const LISTING_HEADER: &str = "ID STATE X Y WIDTH HEIGHT CLIENT TITLE";
const TITLE: &str = "~ - Terminal";
const TITLE_SUFFIX: &str = " - Terminal";
const NEW_SIZE: (u32, u32) = (840, 560);
const FIRST_GRID: &str = "80x24";
const HOME_PREFIX: &str = "EGGOS-HOME ";
const SHOWN_PREFIX: &str = "EGGOS-TERMINALS shown ";
const CAPTURED_PREFIX: &str = "EGGOS-TERMINALS capture ";
const POWER_OFF: &[u8] = b"poweroff\r";
const MOTD: &str = include_str!("../../../initramfs/etc/motd");

fn type_line(boot: &mut Boot<'_>, line: &[u8]) -> Result<(), String> {
    boot.keyboard
        .write_all(line)
        .and_then(|()| boot.keyboard.flush())
        .map_err(|e| format!("{}: could not type on the console: {e}", boot.arch.name))
}

fn console(boot: &mut Boot<'_>) -> String {
    boot.console.extend(boot.output.try_iter().flatten());
    String::from_utf8_lossy(&boot.console).into_owned()
}

fn terminal_windows(console: &str) -> Vec<(u32, String)> {
    let last_listing = console
        .rfind(LISTING_HEADER)
        .map_or(console, |at| &console[at..]);
    last_listing
        .lines()
        .filter_map(|line| {
            let mut words = line.trim_end().splitn(8, ' ');
            let id = words.next()?.parse().ok()?;
            let title = words.nth(6)?;
            title
                .ends_with(TITLE_SUFFIX)
                .then(|| (id, title.to_owned()))
        })
        .collect()
}

fn value_of<'a>(console: &'a str, name: &str) -> Option<&'a str> {
    let named = format!("\"{name}\" = \"");
    console.lines().find_map(|line| {
        let start = line.find(&named)? + named.len();
        line[start..].strip_suffix('"')
    })
}

fn grid_of(screen: &str) -> Option<(u16, u16)> {
    let (columns, rows) = screen.split_once('x')?;
    Some((columns.parse().ok()?, rows.parse().ok()?))
}

fn number_after(console: &str, prefix: &str) -> Option<i32> {
    console
        .lines()
        .find_map(|line| line.trim_end().strip_prefix(prefix)?.parse().ok())
}

fn verdict(arch: &str, console: &str, windows: &[(u32, String)]) -> Result<(), String> {
    if windows.len() != 2 || windows.iter().any(|(_, title)| title != TITLE) {
        return Err(format!(
            "{arch}: `wm list` shows {windows:?}, not two windows titled `{TITLE}`"
        ));
    }
    let screen = value_of(console, "screen")
        .ok_or_else(|| format!("{arch}: `wm show` lists no `screen` element"))?;
    let (columns, rows) = grid_of(screen)
        .ok_or_else(|| format!("{arch}: the `screen` value `{screen}` is no grid"))?;
    if screen == FIRST_GRID {
        return Err(format!(
            "{arch}: the resized terminal's grid is still {screen}"
        ));
    }
    let stty = format!("{rows} {columns}");
    let rows_text: Vec<&str> = console
        .lines()
        .filter(|line| line.contains("\"row "))
        .collect();
    if !console.contains(&format!("= \"{stty}\"")) {
        return Err(format!(
            "{arch}: `stty size` in the terminal did not print `{stty}`, its grid ({screen}): \
             {rows_text:?}"
        ));
    }
    let home = console
        .lines()
        .find_map(|line| line.trim_end().strip_prefix(HOME_PREFIX))
        .ok_or_else(|| format!("{arch}: the console has no `{HOME_PREFIX}` line"))?;
    match value_of(console, "folder") {
        Some(folder) if folder == home => {}
        other => {
            return Err(format!(
                "{arch}: `wm show` lists the folder {other:?}, not the home folder `{home}`"
            ));
        }
    }
    let motd = MOTD.trim_end();
    if value_of(console, "row 1") != Some(motd)
        || !value_of(console, "row 2").is_some_and(|row| row.starts_with("eggsh:"))
    {
        return Err(format!(
            "{arch}: `wm show`'s `row 1` and `row 2` are not `{motd}` and the prompt"
        ));
    }
    for prefix in [SHOWN_PREFIX, CAPTURED_PREFIX] {
        if number_after(console, prefix) != Some(0) {
            return Err(format!("{arch}: the console has no `{prefix}0` line"));
        }
    }
    Ok(())
}

/// Runs the terminals boot test (see the module documentation) on `kernel`, the release kernel
/// for `arch`.
pub fn check(arch: &Arch, kernel: &Path) -> Result<(), String> {
    let mut boot = Boot::start(arch, kernel, &[])?.with_timeout(TIMEOUT);
    boot.wait_for_prompts(1)?;
    type_line(&mut boot, OPEN)?;
    let mut prompts = 2;
    boot.wait_for_prompts(prompts)?;
    let mut windows = terminal_windows(&console(&mut boot));
    while windows.len() < 2 && prompts <= LISTINGS {
        type_line(&mut boot, LIST_AGAIN)?;
        prompts += 1;
        boot.wait_for_prompts(prompts)?;
        windows = terminal_windows(&console(&mut boot));
    }
    let Some(&(second, _)) = windows.iter().max_by_key(|(id, _)| *id) else {
        return Err(boot.fail(format!("{}: `wm list` shows no terminal window", arch.name)));
    };
    let (width, height) = NEW_SIZE;
    let drive = format!(
        "wm size {second} {width} {height}; sleep 2; wm type {second} 'stty size'; \
         wm press {second} enter; sleep 2; wm show {second}; echo {SHOWN_PREFIX}$?; \
         wm capture {second} -o /tmp/terminal.png; echo {CAPTURED_PREFIX}$?\r"
    );
    type_line(&mut boot, drive.as_bytes())?;
    boot.wait_for_prompts(prompts + 1)?;
    type_line(&mut boot, POWER_OFF)?;
    let (console, clean_exit) = boot.finish()?;
    verdict(arch.name, &console, &windows)?;
    if !clean_exit {
        return Err(format!(
            "{}: QEMU did not exit cleanly after the terminals test",
            arch.name
        ));
    }
    eprintln!("xtask: {}: terminals boot test passed", arch.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOWN: &str = "EGGOS-HOME /root\n\
        ID STATE X Y WIDTH HEIGHT CLIENT TITLE\n\
        64 normal 0 32 1280 768 3 Console\n\
        65 normal 40 72 658 430 7 ~ - Terminal\n\
        66 normal 80 112 658 430 9 ~ - Terminal\n\
        1 window 0,0 824x528\n\
        \x20 2 group 8,8 808x512 \"screen\" = \"101x32\"\n\
        \x20   3 text 8,8 808x16 \"row 1\" = \"Welcome to EggOS. Type help. Apps: the egg, or Super.\"\n\
        \x20   4 text 8,24 808x16 \"row 2\" = \"eggsh:/root# stty size\"\n\
        \x20   5 text 8,40 808x16 \"row 3\" = \"32 101\"\n\
        \x20   36 text 8,8 808x512 \"folder\" = \"/root\"\n\
        EGGOS-TERMINALS shown 0\n\
        EGGOS-TERMINALS capture 0\n";

    #[test]
    fn two_titled_windows_a_matching_stty_size_and_the_folder_pass() {
        let windows = terminal_windows(SHOWN);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[1].0, 66);
        assert_eq!(verdict("x86_64", SHOWN, &windows), Ok(()));
    }

    #[test]
    fn a_size_that_does_not_match_the_grid_fails() {
        let wrong = SHOWN.replace("\"32 101\"", "\"24 80\"");
        let windows = terminal_windows(&wrong);
        assert!(verdict("x86_64", &wrong, &windows).is_err());
        let elsewhere = SHOWN.replace("\"folder\" = \"/root\"", "\"folder\" = \"/\"");
        assert!(verdict("x86_64", &elsewhere, &windows).is_err());
        assert!(verdict("x86_64", SHOWN, &windows[..1]).is_err());
        let no_banner = SHOWN.replace("Welcome to EggOS.", "Hello.");
        assert!(verdict("x86_64", &no_banner, &windows).is_err());
    }
}
