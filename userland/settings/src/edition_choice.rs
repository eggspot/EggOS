//! First run's edition page's facts and words (D771, docs/ux/first-run-edition-v0.1.md
//! section 3): what each edition would install on this machine, read once when first run
//! opens, and the two lines under the choices for the edition chosen, as `component edition
//! --dry-run` would plan it.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use eggos_components::editions::{EDITIONS, Edition};
use eggos_components::index::{Component, Index};
use eggos_components::manager;
use eggos_components::messages::size_text;

/// Where the component store is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StoreReach {
    /// A store is attached: the plans have titles and sizes.
    Attached,
    /// No store, but a network is up: `component edition` downloads it first (D257).
    Downloadable,
    /// No store and no network.
    #[default]
    Offline,
}

/// A component an edition would install: its name, its title (the name without a store) and
/// its size in bytes (0 without a store).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub name: String,
    pub title: String,
    pub bytes: u64,
}

/// What choosing one edition would do here: the components it would install, in install
/// order, and whether installed components outside the edition stay (an edition only adds).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditionPlan {
    pub components: Vec<Planned>,
    pub keeps_others: bool,
}

impl EditionPlan {
    /// The plan's bytes in all.
    pub fn bytes(&self) -> u64 {
        self.components.iter().map(|planned| planned.bytes).sum()
    }
}

/// What the edition page knows: the edition recorded as chosen (an index of `EDITIONS`), each
/// edition's plan (in `EDITIONS`' order), the components volume's free bytes and the store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditionFacts {
    pub recorded: Option<usize>,
    pub plans: [EditionPlan; 4],
    pub free_bytes: Option<u64>,
    pub store: StoreReach,
}

/// `edition`'s plan on a machine with `installed`, from the store's `index` when one is
/// attached: each of its components not installed, with what it needs first, that the store
/// offers (without a store, each one not installed, by name).
pub fn plan(edition: &Edition, index: Option<&Index>, installed: &[Component]) -> EditionPlan {
    let is_installed = |name: &str| installed.iter().any(|component| component.name == name);
    let keeps_others = installed
        .iter()
        .any(|component| !edition.components.contains(&component.name.as_str()));
    let mut components: Vec<Planned> = Vec::new();
    let Some(index) = index else {
        components = edition
            .components
            .iter()
            .filter(|name| !is_installed(name))
            .map(|name| Planned {
                name: String::from(*name),
                title: String::from(*name),
                bytes: 0,
            })
            .collect();
        return EditionPlan {
            components,
            keeps_others,
        };
    };
    let mut now_installed: Vec<Component> = installed.to_vec();
    for wanted in edition.components {
        if index.component(wanted).is_none() {
            continue;
        }
        let Ok(needs) = manager::plan_install(index, &now_installed, wanted) else {
            continue;
        };
        let needs: Vec<Component> = needs.into_iter().cloned().collect();
        for need in needs {
            if components.iter().any(|planned| planned.name == need.name) {
                continue;
            }
            components.push(Planned {
                name: need.name.clone(),
                title: need.title.clone(),
                bytes: need.size_bytes(),
            });
            now_installed.push(need);
        }
    }
    EditionPlan {
        components,
        keeps_others,
    }
}

/// The size shown beside edition `index` of `EDITIONS`: its plan's bytes, `Installed` when
/// it adds nothing, nothing without a store.
pub fn size_line(facts: &EditionFacts, index: usize) -> String {
    let plan = &facts.plans[index];
    match (plan.components.is_empty(), facts.store) {
        (true, _) => String::from("Installed"),
        (false, StoreReach::Attached) => size_text(plan.bytes()),
        (false, _) => String::new(),
    }
}

/// Why choosing edition `index` of `EDITIONS` installs nothing at `Done` even though it adds
/// components, if it does: not enough space, or no store and no network.
pub fn held_back(facts: &EditionFacts, index: usize) -> bool {
    let plan = &facts.plans[index];
    if plan.components.is_empty() {
        return false;
    }
    match facts.store {
        StoreReach::Attached => facts.free_bytes.is_some_and(|free| free < plan.bytes()),
        StoreReach::Downloadable => false,
        StoreReach::Offline => true,
    }
}

/// The installs line and the line under it for edition `index` of `EDITIONS`, and whether the
/// first is a refusal (drawn in `text` rather than `text.dim`).
pub fn lines(facts: &EditionFacts, index: usize) -> (String, Option<String>, bool) {
    let edition = &EDITIONS[index];
    let plan = &facts.plans[index];
    if plan.components.is_empty() {
        return match plan.keeps_others {
            true => (
                String::from("Nothing to add."),
                Some(String::from(
                    "An edition only adds: remove components in Settings > Components.",
                )),
                false,
            ),
            false => (
                format!("{} is installed: nothing to add.", edition.title),
                None,
                false,
            ),
        };
    }
    let titles: Vec<&str> = plan
        .components
        .iter()
        .map(|planned| planned.title.as_str())
        .collect();
    let titles = titles.join(", ");
    match facts.store {
        StoreReach::Attached => {
            let needed = plan.bytes();
            match facts.free_bytes {
                Some(free) if free < needed => (
                    format!(
                        "Not enough space: {} needed, {} free.",
                        size_text(needed),
                        size_text(free)
                    ),
                    Some(String::from(
                        "Free some space, then choose the edition in Settings > Components.",
                    )),
                    true,
                ),
                _ => (
                    format!("Installs {titles}: {}.", size_text(needed)),
                    Some(String::from(
                        "Downloads start when you press Done; EggOS is ready to use meanwhile.",
                    )),
                    false,
                ),
            }
        }
        StoreReach::Downloadable => (
            format!("Installs {titles}."),
            Some(String::from(
                "Downloads the component store from the release first, when you press Done.",
            )),
            false,
        ),
        StoreReach::Offline => (
            format!("Installs {titles}."),
            Some(String::from(
                "No component store, and the network is offline: the installs wait in \
                 Settings > Components.",
            )),
            false,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planned(name: &str, title: &str, bytes: u64) -> Planned {
        Planned {
            name: String::from(name),
            title: String::from(title),
            bytes,
        }
    }

    fn facts() -> EditionFacts {
        let personal = EditionPlan {
            components: alloc::vec![
                planned("windows-apps", "Windows apps", 1_887_437),
                planned("mac-apps", "Mac apps", 1_468_007),
            ],
            keeps_others: false,
        };
        EditionFacts {
            recorded: None,
            plans: [
                EditionPlan {
                    components: Vec::new(),
                    keeps_others: true,
                },
                personal,
                EditionPlan::default(),
                EditionPlan::default(),
            ],
            free_bytes: Some(1 << 30),
            store: StoreReach::Attached,
        }
    }

    #[test]
    fn a_plan_with_a_store_names_its_titles_and_size() {
        let facts = facts();
        let (first, second, refused) = lines(&facts, 1);
        assert_eq!(first, "Installs Windows apps, Mac apps: 3.2 MB.");
        assert_eq!(
            second.as_deref(),
            Some("Downloads start when you press Done; EggOS is ready to use meanwhile.")
        );
        assert!(!refused);
        assert_eq!(size_line(&facts, 1), "3.2 MB");
        assert_eq!(size_line(&facts, 0), "Installed");
        assert!(!held_back(&facts, 1));
    }

    #[test]
    fn server_on_a_desktop_adds_nothing_and_says_an_edition_only_adds() {
        let facts = facts();
        let (first, second, _) = lines(&facts, 0);
        assert_eq!(first, "Nothing to add.");
        assert!(second.unwrap().starts_with("An edition only adds"));
        let (installed, none, _) = lines(&facts, 2);
        assert_eq!(installed, "Professional is installed: nothing to add.");
        assert_eq!(none, None);
    }

    #[test]
    fn too_little_space_or_no_store_offline_holds_the_installs_back() {
        let mut facts = facts();
        facts.free_bytes = Some(600 * 1024);
        let (first, _, refused) = lines(&facts, 1);
        assert_eq!(first, "Not enough space: 3.2 MB needed, 600 KB free.");
        assert!(refused);
        assert!(held_back(&facts, 1));
        facts.store = StoreReach::Offline;
        let (names, second, _) = lines(&facts, 1);
        assert_eq!(names, "Installs Windows apps, Mac apps.");
        assert!(second.unwrap().starts_with("No component store"));
        assert!(held_back(&facts, 1));
        assert_eq!(size_line(&facts, 1), "");
        facts.store = StoreReach::Downloadable;
        assert!(!held_back(&facts, 1));
    }

    #[test]
    fn without_a_store_the_plan_is_the_missing_names() {
        let plan = plan(&EDITIONS[1], None, &[]);
        let names: Vec<&str> = plan.components.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["desktop", "builtin-apps", "windows-apps", "mac-apps"]);
        assert!(!plan.keeps_others);
    }
}
