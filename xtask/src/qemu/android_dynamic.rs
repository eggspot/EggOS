//! The dynamic Android programs boot test (D188): one boot of the release kernel with
//! `tests/android-console` linked dynamically by the NDK (`android_dynamic.rs`), Android's
//! `linker64` and the runtime module's libraries under `/system`, and `/system/build.prop`. The
//! session sets the property the program reads, runs the program as the static Android
//! programs boot test runs its static build (whose `loader` line now says `linker64`), runs it
//! again with `LD_LIBRARY_PATH` naming a folder that does not exist to exit with a status, and
//! powers off.

use super::settings::Boot;
use super::*;

/// The dynamically linked program's path in the image.
pub const PROGRAM_PATH: &str = "bin/android-console-dynamic";

const SESSION: [&[u8]; 3] = [
    b"setprop eggos.test.greeting hello; echo ANDROID-DYNAMIC status $?\r",
    b"GREETING=hi android-console-dynamic a 'b c'; echo ANDROID-CONSOLE status $?\r",
    b"LD_LIBRARY_PATH=/nowhere /bin/android-console-dynamic exit 42; \
      echo ANDROID-CONSOLE status $?\r",
];
const POWER_OFF: &[u8] = b"poweroff\r";

fn outputs() -> [Vec<Line>; SESSION.len()] {
    [
        vec![Line::Whole("ANDROID-DYNAMIC status 0".into())],
        android_console_lines("android-console-dynamic", "linker64"),
        vec![Line::Whole("ANDROID-CONSOLE status 42".into())],
    ]
}

fn verdict(arch: &str, console: &str, clean_exit: bool) -> Result<(), String> {
    session_verdict(
        arch,
        clean_exit,
        console,
        &SESSION,
        outputs(),
        "dynamic Android",
    )?;
    if !has_line(console, POWER_DOWN_LINE) {
        return Err(format!(
            "{arch}: the console has no `{POWER_DOWN_LINE}` line after the dynamic Android \
             program"
        ));
    }
    Ok(())
}

/// Runs the dynamic Android programs boot test (see the module documentation) on `kernel`,
/// the release kernel for `arch` with the program and Android's runtime in its initramfs.
pub fn check(arch: &Arch, kernel: &Path) -> Result<(), String> {
    let mut boot = Boot::start(arch, kernel, &[])?;
    for _ in SESSION {
        boot.type_next(&SESSION)?;
    }
    boot.wait_for_prompts(SESSION.len() + 1)?;
    boot.keyboard
        .write_all(POWER_OFF)
        .and_then(|()| boot.keyboard.flush())
        .map_err(|e| format!("{}: could not type on the console: {e}", arch.name))?;
    let (console, clean_exit) = boot.finish()?;
    verdict(arch.name, &console, clean_exit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn console() -> String {
        let mut console = String::new();
        for (typed, lines) in SESSION.iter().zip(outputs()) {
            let typed = String::from_utf8_lossy(typed);
            let command = typed.trim_end_matches('\r');
            console.push_str(&format!("{SHELL_PROMPT}# {command}\n"));
            for line in lines {
                match line {
                    Line::Whole(whole) => console.push_str(&whole),
                    Line::Around(start, end) => console.push_str(&format!("{start}x{end}")),
                }
                console.push('\n');
            }
        }
        console.push_str(&format!("{SHELL_PROMPT}# poweroff\n{POWER_DOWN_LINE}\n"));
        console
    }

    #[test]
    fn the_verdict_wants_the_linker_s_loader_line_and_a_clean_power_off() {
        let good = console();
        assert_eq!(verdict("aarch64", &good, true), Ok(()));
        assert!(verdict("aarch64", &good, false).is_err());
        let static_build = good.replace("loader linker64", "loader static");
        assert!(verdict("x86_64", &static_build, true).is_err());
        let failed = good.replacen("ANDROID-CONSOLE status 0", "ANDROID-CONSOLE status 127", 1);
        assert!(verdict("aarch64", &failed, true).is_err());
        let no_power_off = good.replace(POWER_DOWN_LINE, "");
        assert!(verdict("x86_64", &no_power_off, true).is_err());
    }

    #[test]
    fn the_session_runs_the_program_at_the_path_the_image_has() {
        let typed: String = SESSION
            .iter()
            .map(|line| String::from_utf8_lossy(line))
            .collect();
        let name = PROGRAM_PATH.rsplit('/').next().unwrap();
        assert!(typed.contains(&format!("GREETING=hi {name} ")));
        assert!(typed.contains(&format!(" /{PROGRAM_PATH} exit 42")));
    }
}
