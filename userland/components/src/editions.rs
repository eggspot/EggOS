//! Editions as component presets (D18, D258): each edition is a list of components, installed
//! together by `component edition <name>`. Components a store does not offer for this machine
//! (the Windows layer on riscv64) are left out of the plan, not refused.

/// One edition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edition {
    pub name: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    pub components: &'static [&'static str],
}

/// The editions, smallest first.
pub const EDITIONS: [Edition; 4] = [
    Edition {
        name: "server",
        title: "Server",
        summary: "The base alone: no desktop and no foreign apps.",
        components: &[],
    },
    Edition {
        name: "personal",
        title: "Personal",
        summary: "Windows and macOS apps beside EggOS's own.",
        components: &["desktop", "builtin-apps", "windows-apps", "mac-apps"],
    },
    Edition {
        name: "professional",
        title: "Professional",
        summary: "Personal, with every tool for work that the store offers.",
        components: &["desktop", "builtin-apps", "windows-apps", "mac-apps"],
    },
    Edition {
        name: "enterprise",
        title: "Enterprise",
        summary: "Professional, ready for managed fleets.",
        components: &["desktop", "builtin-apps", "windows-apps", "mac-apps"],
    },
];

/// The edition called `name`.
pub fn edition(name: &str) -> Option<&'static Edition> {
    EDITIONS.iter().find(|edition| edition.name == name)
}

/// Where the edition last chosen is recorded, so editions with the same components today are
/// told apart (docs/ux/editions-v0.4.md section 9).
pub const RECORD: &str = "/var/lib/eggos/edition";

/// The edition the installed components make, preferring `recorded` while its list still
/// matches.
pub fn current_preferring<'a>(
    installed: impl Iterator<Item = &'a str> + Clone,
    recorded: Option<&str>,
) -> Option<&'static Edition> {
    let matches = |edition: &Edition| {
        let mut names = installed.clone();
        edition.components.iter().all(|wanted| installed.clone().any(|name| name == *wanted))
            && names.all(|name| edition.components.contains(&name))
    };
    recorded
        .and_then(edition)
        .filter(|edition| matches(edition))
        .or_else(|| EDITIONS.iter().rev().find(|edition| matches(edition)))
}

/// The edition whose components are exactly the installed ones, if any; the largest when
/// several are.
pub fn current<'a>(installed: impl Iterator<Item = &'a str> + Clone) -> Option<&'static Edition> {
    EDITIONS.iter().rev().find(|edition| {
        let mut names = installed.clone();
        edition.components.iter().all(|wanted| installed.clone().any(|name| name == *wanted))
            && names.all(|name| edition.components.contains(&name))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editions_are_found_by_name_and_by_what_is_installed() {
        assert_eq!(
            edition("personal").unwrap().components,
            &["desktop", "builtin-apps", "windows-apps", "mac-apps"]
        );
        assert!(edition("home").is_none());
        let none: [&str; 0] = [];
        assert_eq!(current(none.iter().copied()).unwrap().name, "server");
        let both = ["mac-apps", "windows-apps", "builtin-apps", "desktop"];
        assert_eq!(current(both.iter().copied()).unwrap().name, "enterprise");
        let one = ["mac-apps"];
        assert!(current(one.iter().copied()).is_none());
    }
}
