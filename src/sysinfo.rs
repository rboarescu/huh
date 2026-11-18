//! Read-only facts about this machine that tell a real value from a stray word: which names are
//! users, groups, login shells and service units. Nothing is ever run; only these files are read:
//!
//! - users: `/etc/passwd`
//! - groups: `/etc/group`
//! - login shells: `/etc/shells`
//! - service units: the `*.service` files in `/usr/lib/systemd/system`, `/etc/systemd/system`,
//!   `~/.config/systemd/user` and `/usr/lib/systemd/user`
//!
//! `HUH_SYSINFO_DIR=<folder>` reads the files `passwd`, `group`, `shells` and `units` (one unit file
//! name per line) from that folder instead. The evals and tests use the fixture in
//! `tests/fixtures/sysinfo` on their own thread (`use_fixture`), so results never depend on the
//! machine they run on. Docker containers and packages have no list here: asking `docker` would
//! mean running a command, which huh never does.

use std::cell::{OnceCell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// The kinds of name a slot can be declared as with `type = "..."` in the catalog.
pub const TYPES: &[&str] = &["service", "user", "group", "shell", "container", "package"];

/// Names read from the system.
#[derive(Default, Clone, Debug)]
pub struct Facts {
    users: Vec<String>,
    groups: Vec<String>,
    /// Base name (`bash`) to full path (`/bin/bash`).
    shells: BTreeMap<String, String>,
    services: Vec<String>,
}

thread_local! {
    static REAL: OnceCell<Rc<Facts>> = const { OnceCell::new() };
    static FIXTURE: RefCell<Option<Rc<Facts>>> = const { RefCell::new(None) };
}

/// Names before the first `:` of each line (`passwd`, `group`).
pub fn parse_names(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split(':').next())
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .collect()
}

/// Login shells from `/etc/shells`, keyed by base name.
pub fn parse_shells(text: &str) -> BTreeMap<String, String> {
    let mut shells = BTreeMap::new();
    for line in text.lines().map(str::trim).filter(|l| l.starts_with('/')) {
        if let Some(base) = line.rsplit('/').next() {
            // `/bin/bash` and `/usr/bin/bash` can both be listed: keep the first.
            shells.entry(base.to_string()).or_insert_with(|| line.to_string());
        }
    }
    shells
}

/// Unit names from file names: `sshd.service` is `sshd`. Templates (`getty@.service`) are skipped.
pub fn parse_units<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut units: Vec<String> = names
        .into_iter()
        .map(str::trim)
        .filter_map(|n| n.strip_suffix(".service"))
        .filter(|n| !n.is_empty() && !n.ends_with('@'))
        .map(str::to_string)
        .collect();
    units.sort();
    units.dedup();
    units
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn from_folder(dir: &Path) -> Facts {
    let units = read(&dir.join("units"));
    Facts {
        users: parse_names(&read(&dir.join("passwd"))),
        groups: parse_names(&read(&dir.join("group"))),
        shells: parse_shells(&read(&dir.join("shells"))),
        services: parse_units(units.lines().filter(|l| !l.starts_with('#'))),
    }
}

fn from_system() -> Facts {
    let mut files = Vec::new();
    let mut dirs: Vec<PathBuf> = vec!["/usr/lib/systemd/system".into(), "/etc/systemd/system".into()];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(Path::new(&home).join(".config/systemd/user"));
    }
    dirs.push("/usr/lib/systemd/user".into());
    for dir in dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            files.extend(entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()));
        }
    }
    Facts {
        users: parse_names(&read(Path::new("/etc/passwd"))),
        groups: parse_names(&read(Path::new("/etc/group"))),
        shells: parse_shells(&read(Path::new("/etc/shells"))),
        services: parse_units(files.iter().map(String::as_str)),
    }
}

/// The facts for this thread: the fixture when one is set, else `HUH_SYSINFO_DIR`, else the system.
pub fn facts() -> Rc<Facts> {
    if let Some(fixture) = FIXTURE.with(|f| f.borrow().clone()) {
        return fixture;
    }
    REAL.with(|real| {
        real.get_or_init(|| {
            Rc::new(match std::env::var_os("HUH_SYSINFO_DIR") {
                Some(dir) => from_folder(Path::new(&dir)),
                None => from_system(),
            })
        })
        .clone()
    })
}

/// For the evals and tests: on this thread, answer from `tests/fixtures/sysinfo`. `false` ends it.
pub fn use_fixture(on: bool) {
    let facts = on.then(|| {
        Rc::new(Facts {
            users: parse_names(include_str!("../tests/fixtures/sysinfo/passwd")),
            groups: parse_names(include_str!("../tests/fixtures/sysinfo/group")),
            shells: parse_shells(include_str!("../tests/fixtures/sysinfo/shells")),
            services: parse_units(
                include_str!("../tests/fixtures/sysinfo/units").lines().filter(|l| !l.starts_with('#')),
            ),
        })
    });
    FIXTURE.with(|f| *f.borrow_mut() = facts);
}

impl Facts {
    /// Whether `word` is a name of this `type` on the machine. `container` and `package` have no
    /// list, so they never validate a word.
    pub fn knows(&self, value_type: &str, word: &str) -> bool {
        match value_type {
            "user" => self.users.iter().any(|n| n == word),
            "group" => self.groups.iter().any(|n| n == word),
            "service" => self.services.iter().any(|n| n.eq_ignore_ascii_case(word)),
            "shell" => self.shell_path(word).is_some(),
            _ => false,
        }
    }

    /// The full path of a login shell given by name (`bash` is `/bin/bash`) or by path.
    pub fn shell_path(&self, word: &str) -> Option<String> {
        let word = word.trim();
        if word.starts_with('/') {
            return self.shells.values().find(|p| p.as_str() == word).cloned();
        }
        self.shells.get(&word.to_lowercase()).cloned()
    }
}

/// Whether the type has a list to check against.
pub fn has_list(value_type: &str) -> bool {
    matches!(value_type, "user" | "group" | "service" | "shell")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_names_before_the_colon() {
        let text = "root:x:0:0:root:/root:/bin/bash\n# comment\n\nbob:x:1003:1003::/home/bob:/bin/zsh\n";
        assert_eq!(parse_names(text), vec!["root", "bob"]);
    }

    #[test]
    fn maps_shell_names_to_the_paths_in_etc_shells() {
        let shells = parse_shells("# valid shells\n/bin/sh\n/bin/bash\n/usr/bin/bash\n/usr/bin/zsh\n");
        assert_eq!(shells["bash"], "/bin/bash");
        assert_eq!(shells["zsh"], "/usr/bin/zsh");
        assert!(!shells.contains_key("fish"));
    }

    #[test]
    fn strips_the_service_suffix_and_skips_templates() {
        let units = parse_units(["sshd.service", "getty@.service", "sshd.socket", "nginx.service", "sshd.service"]);
        assert_eq!(units, vec!["nginx", "sshd"]);
    }

    #[test]
    fn fixture_answers_for_every_type() {
        use_fixture(true);
        let facts = facts();
        assert!(facts.knows("user", "bob"));
        assert!(!facts.knows("user", "docker"));
        assert!(facts.knows("group", "docker"));
        assert!(facts.knows("service", "sshd"));
        assert!(facts.knows("service", "NetworkManager"));
        assert!(!facts.knows("service", "older"));
        assert_eq!(facts.shell_path("bash").as_deref(), Some("/bin/bash"));
        assert_eq!(facts.shell_path("/usr/bin/fish").as_deref(), Some("/usr/bin/fish"));
        assert_eq!(facts.shell_path("tcsh"), None);
        assert!(!facts.knows("container", "web"));
        use_fixture(false);
    }
}
