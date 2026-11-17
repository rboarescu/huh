//! What kind of system this is and how it installs software.
//!
//! One source of truth for the catalog (`{pkg_install}` and friends), the `NOTE:` line
//! under a suggestion and the install hints of `huh why`. The package managers and the
//! package names live in `data/platforms.toml` (embedded), which `~/.config/huh/platforms.toml`
//! can extend. huh only prints the commands it finds here; it never runs them.
//!
//! A platform is a list of words: the `ID` and `ID_LIKE` entries of os-release plus the
//! name of every family they belong to (e.g. `["elementary", "ubuntu", "debian"]`). When
//! os-release says nothing we know, the package manager binary found on PATH decides.

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const EMBEDDED: &str = include_str!("../data/platforms.toml");

/// Catalog placeholders that are filled from the family's package manager.
pub const PKG_FACTS: &[&str] = &["pkg_install", "pkg_remove", "pkg_search", "pkg_update", "pkg_owns", "pkg_provides"];

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(deny_unknown_fields)]
struct Family {
    /// os-release IDs that belong to this family (the family name itself always does).
    ids: Option<Vec<String>>,
    /// Package manager binaries; one of them on PATH marks an unrecognised system as this family.
    detect: Option<Vec<String>>,
    /// Inherit everything not set here from this family (omarchy extends arch).
    extends: Option<String>,
    install: Option<String>,
    remove: Option<String>,
    search: Option<String>,
    update: Option<String>,
    /// Which installed package owns a file.
    owns: Option<String>,
    /// Which package would provide a program, for programs huh has no package name for.
    provides: Option<String>,
}

#[derive(Deserialize, Default, Debug)]
#[serde(deny_unknown_fields)]
struct Data {
    #[serde(default)]
    same: Vec<String>,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    programs: BTreeMap<String, String>,
    #[serde(default)]
    packages: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    families: BTreeMap<String, Family>,
}

impl Data {
    fn merge(&mut self, other: Data) {
        self.same.extend(other.same);
        self.tools.extend(other.tools);
        self.programs.extend(other.programs);
        for (key, rows) in other.packages {
            self.packages.entry(key).or_default().extend(rows);
        }
        for (name, add) in other.families {
            let family = self.families.entry(name).or_default();
            macro_rules! take {
                ($($field:ident),*) => { $( if add.$field.is_some() { family.$field = add.$field; } )* };
            }
            take!(ids, detect, extends, install, remove, search, update, owns, provides);
        }
    }

    /// The family and the families it extends, most specific first.
    fn chain(&self, name: &str) -> Vec<&str> {
        let mut chain = Vec::new();
        let mut next = Some(name);
        while let Some(current) = next {
            if chain.contains(&current) {
                break;
            }
            let Some((key, family)) = self.families.get_key_value(current) else { break };
            chain.push(key.as_str());
            next = family.extends.as_deref();
        }
        chain
    }

    fn field<'a>(&'a self, name: &str, pick: impl Fn(&'a Family) -> Option<&'a String>) -> Option<&'a str> {
        self.chain(name).into_iter().find_map(|n| pick(&self.families[n])).map(String::as_str)
    }
}

fn config_file() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("HUH_PLATFORMS").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let config = std::env::var_os("XDG_CONFIG_HOME").filter(|s| !s.is_empty()).map(PathBuf::from).or_else(|| {
        std::env::var_os("HOME").filter(|s| !s.is_empty()).map(|home| PathBuf::from(home).join(".config"))
    })?;
    Some(config.join("huh/platforms.toml"))
}

fn load() -> Data {
    let mut data: Data = toml::from_str(EMBEDDED).expect("data/platforms.toml is valid");
    if let Some(path) = config_file()
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        match toml::from_str::<Data>(&text) {
            Ok(extra) => data.merge(extra),
            Err(error) => eprintln!("huh: ignoring {}: {error}", path.display()),
        }
    }
    data
}

fn data() -> &'static Data {
    static DATA: OnceLock<Data> = OnceLock::new();
    DATA.get_or_init(load)
}

// ---------------------------------------------------------------------------
// Which system is this?
// ---------------------------------------------------------------------------

/// The `ID` words, then the `ID_LIKE` words, of an os-release file, e.g. ["omarchy", "arch"].
pub fn ids_from(os_release: &str) -> Vec<String> {
    let words = |key: &str| -> Vec<String> {
        os_release
            .lines()
            .filter_map(|line| line.trim().strip_prefix(key))
            .flat_map(|value| value.trim().trim_matches(['"', '\'']).split_whitespace().map(str::to_lowercase))
            .collect()
    };
    let mut ids = words("ID=");
    ids.extend(words("ID_LIKE="));
    ids
}

/// os-release as the system documents it: /etc first, then /usr/lib.
pub fn read_os_release() -> String {
    ["/etc/os-release", "/usr/lib/os-release"]
        .iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default()
}

fn expand_with(data: &Data, ids: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |word: &str| {
        if !out.iter().any(|w| w == word) {
            out.push(word.to_string());
        }
    };
    for id in ids {
        push(id);
    }
    for id in ids {
        for (name, family) in &data.families {
            if name == id || family.ids.as_ref().is_some_and(|ids| ids.contains(id)) {
                for ancestor in data.chain(name) {
                    push(ancestor);
                }
            }
        }
    }
    out
}

/// Adds the family name of every known ID, so `["elementary", "ubuntu"]` also says `debian`.
pub fn expand(ids: &[String]) -> Vec<String> {
    expand_with(data(), ids)
}

/// The family whose package manager this platform uses. A family that extends another
/// (omarchy extends arch) wins when both are listed.
pub fn manager(platforms: &[String]) -> Option<String> {
    let data = data();
    let platforms = expand_with(data, platforms);
    let names: Vec<&String> = platforms.iter().filter(|p| data.families.contains_key(p.as_str())).collect();
    names
        .iter()
        .find(|name| data.families[name.as_str()].extends.as_ref().is_some_and(|parent| names.contains(&parent)))
        .or(names.first())
        .map(|name| name.to_string())
}

thread_local! {
    static PINNED: std::cell::RefCell<Option<Vec<String>>> = const { std::cell::RefCell::new(None) };
}

/// For `huh eval-errors` and tests: on this thread, pretend to be this platform with no program
/// installed, so recorded failures give the same answer on every machine.
pub fn pin_for_eval(ids: &[&str]) {
    let ids = expand(&ids.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    PINNED.with(|pinned| *pinned.borrow_mut() = Some(ids));
}

/// The names of all known families.
pub fn family_names() -> Vec<String> {
    data().families.keys().cloned().collect()
}

/// The platform words for this machine.
pub fn detect() -> Vec<String> {
    if let Some(pinned) = PINNED.with(|pinned| pinned.borrow().clone()) {
        return pinned;
    }
    detect_with(&read_os_release(), &|program| !matches!(locate(program), Found::Missing))
}

fn detect_with(os_release: &str, on_path: &dyn Fn(&str) -> bool) -> Vec<String> {
    let data = data();
    let ids = expand_with(data, &ids_from(os_release));
    if manager(&ids).is_some() {
        return ids;
    }
    // Unknown ID: whichever package manager is installed tells us the family.
    for (name, family) in &data.families {
        if family.detect.iter().flatten().any(|binary| on_path(binary)) {
            let mut ids = ids;
            ids.push(name.clone());
            return expand_with(data, &ids);
        }
    }
    ids
}

// ---------------------------------------------------------------------------
// Package manager commands and package names
// ---------------------------------------------------------------------------

/// The command for a package task (`pkg_install`, ...) on this platform. `{}` marks the
/// argument; without it the argument goes at the end (see `fill`).
pub fn fact(platforms: &[String], id: &str) -> Option<String> {
    let data = data();
    let family = manager(platforms)?;
    let template = match id {
        "pkg_install" => data.field(&family, |f| f.install.as_ref()),
        "pkg_remove" => data.field(&family, |f| f.remove.as_ref()),
        "pkg_search" => data.field(&family, |f| f.search.as_ref()),
        "pkg_update" => data.field(&family, |f| f.update.as_ref()),
        "pkg_owns" => data.field(&family, |f| f.owns.as_ref()),
        "pkg_provides" => data.field(&family, |f| f.provides.as_ref()),
        _ => None,
    }?;
    Some(template.to_string())
}

enum Name {
    Package(String),
    NotPackaged,
    Unknown,
}

fn package_name(data: &Data, family: &str, program: &str, package: Option<&str>) -> Name {
    let key = package.or_else(|| data.programs.get(program).map(String::as_str)).unwrap_or(program);
    if let Some(rows) = data.packages.get(key) {
        let name = data.chain(family).into_iter().find_map(|f| rows.get(f)).or_else(|| rows.get("*"));
        return match name {
            Some(name) if name.is_empty() => Name::NotPackaged,
            Some(name) => Name::Package(name.clone()),
            None => Name::Unknown,
        };
    }
    if data.same.iter().any(|s| s == key) { Name::Package(key.to_string()) } else { Name::Unknown }
}

/// True when data/platforms.toml says something about this program's package, so a missing
/// program of that name is a real tool to install rather than a probable typo.
pub fn is_known_program(program: &str) -> bool {
    let data = data();
    data.programs.contains_key(program) || data.packages.contains_key(program) || data.same.iter().any(|s| s == program)
}

/// Programs that belong to a distro or a package manager, such as `pacman` or `omarchy`.
pub fn is_system_tool(program: &str) -> bool {
    let data = data();
    let manager_words = data.families.values().flat_map(|family| {
        let commands =
            [&family.install, &family.remove, &family.search, &family.update, &family.owns, &family.provides];
        commands
            .into_iter()
            .flatten()
            .flat_map(|command| command.split("&&"))
            .filter_map(|part| part.split_whitespace().find(|word| *word != "sudo"))
            .chain(family.detect.iter().flatten().map(String::as_str))
    });
    let mut known: BTreeSet<&str> = manager_words.collect();
    known.extend(data.tools.iter().map(String::as_str));
    known.contains(program)
}

/// What to tell the user about a program that is not installed.
#[derive(Debug, PartialEq, Eq)]
pub struct Advice {
    pub command: String,
    /// True when `command` installs the program; false when it only searches for the package.
    pub installs: bool,
    /// The package `command` installs, when it does.
    pub package: Option<String>,
}

/// How to get `program` on this platform. `package` is the catalog's package name when it
/// differs from the program. `None` when nothing sensible can be said: package managers and
/// distro tools, packages this family does not have, and systems with no known package manager.
pub fn install_advice(program: &str, package: Option<&str>, platforms: &[String]) -> Option<Advice> {
    if program.is_empty() || is_system_tool(program) {
        return None;
    }
    let data = data();
    let family = manager(platforms)?;
    match package_name(data, &family, program, package) {
        Name::Package(name) => {
            Some(Advice { command: fill(&fact(platforms, "pkg_install")?, &name), installs: true, package: Some(name) })
        }
        Name::NotPackaged => None,
        Name::Unknown => {
            Some(Advice { command: fill(&fact(platforms, "pkg_provides")?, program), installs: false, package: None })
        }
    }
}

/// Puts `argument` where the template has `{}`, or at the end when it has none.
pub fn fill(template: &str, argument: &str) -> String {
    if template.contains("{}") { template.replace("{}", argument) } else { format!("{template} {argument}") }
}

// ---------------------------------------------------------------------------
// Finding programs
// ---------------------------------------------------------------------------

/// Where administration tools live. Debian-like systems keep `ip`, `modprobe`, `smartctl`
/// there and leave it off a normal user's PATH, but `sudo` finds them.
pub const SBIN_DIRS: &[&str] = &["/usr/local/sbin", "/usr/sbin", "/sbin"];

/// The directories of `$PATH`, with a leading `~` expanded and empty entries dropped.
pub fn path_dirs() -> Vec<PathBuf> {
    let home = std::env::var("HOME").ok();
    std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .filter(|dir| !dir.as_os_str().is_empty())
                .map(|dir| expand_tilde(&dir, home.as_deref()))
                .collect()
        })
        .unwrap_or_default()
}

pub fn expand_tilde(dir: &Path, home: Option<&str>) -> PathBuf {
    let Some(home) = home.filter(|h| !h.is_empty()) else { return dir.to_path_buf() };
    if dir == Path::new("~") {
        return PathBuf::from(home);
    }
    match dir.strip_prefix("~") {
        Ok(rest) => Path::new(home).join(rest),
        Err(_) => dir.to_path_buf(),
    }
}

/// A regular file (after following symlinks) with at least one execute bit.
pub fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Found {
    /// An executable on PATH.
    OnPath(PathBuf),
    /// Only in /usr/sbin or the like, which is not on PATH: fine under `sudo`, not without.
    SbinOnly(PathBuf),
    Missing,
}

pub fn locate_in(program: &str, path: &[PathBuf], sbin: &[PathBuf]) -> Found {
    if program.is_empty() || program.contains('/') {
        return if is_executable_file(Path::new(program)) {
            Found::OnPath(PathBuf::from(program))
        } else {
            Found::Missing
        };
    }
    let search = |dirs: &[PathBuf]| dirs.iter().map(|dir| dir.join(program)).find(|file| is_executable_file(file));
    if let Some(file) = search(path) {
        return Found::OnPath(file);
    }
    match search(sbin) {
        Some(file) => Found::SbinOnly(file),
        None => Found::Missing,
    }
}

pub fn locate(program: &str) -> Found {
    if PINNED.with(|pinned| pinned.borrow().is_some()) {
        return Found::Missing;
    }
    let sbin: Vec<PathBuf> = SBIN_DIRS.iter().map(PathBuf::from).collect();
    locate_in(program, &path_dirs(), &sbin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn words(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// Every distro the tool is meant to work on, as its os-release declares itself.
    const FIXTURES: &[(&str, &str, &str)] = &[
        ("ID=arch\n", "arch", "sudo pacman -S"),
        ("NAME=\"Omarchy\"\nID=omarchy\nID_LIKE=arch\n", "omarchy", "omarchy pkg add"),
        ("ID=artix\nBUILD_ID=rolling\n", "arch", "sudo pacman -S"),
        ("ID=manjaro\nID_LIKE=arch\n", "arch", "sudo pacman -S"),
        ("ID=cachyos\nID_LIKE=\"arch\"\n", "arch", "sudo pacman -S"),
        ("ID=debian\nVERSION_ID=\"12\"\n", "debian", "sudo apt install"),
        ("ID=ubuntu\nID_LIKE=debian\n", "debian", "sudo apt install"),
        ("ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n", "debian", "sudo apt install"),
        ("ID=elementary\nID_LIKE=ubuntu\n", "debian", "sudo apt install"),
        ("ID=zorin\nID_LIKE=\"ubuntu debian\"\n", "debian", "sudo apt install"),
        ("ID=pop\nID_LIKE=\"ubuntu debian\"\n", "debian", "sudo apt install"),
        ("ID=fedora\n", "fedora", "sudo dnf install"),
        ("ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n", "fedora", "sudo dnf install"),
        ("ID=\"almalinux\"\nID_LIKE=\"rhel centos fedora\"\n", "fedora", "sudo dnf install"),
        ("ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n", "suse", "sudo zypper install"),
        ("ID=\"opensuse-leap\"\nID_LIKE=\"suse opensuse\"\n", "suse", "sudo zypper install"),
        ("ID=sles\nID_LIKE=\"suse\"\n", "suse", "sudo zypper install"),
        ("ID=alpine\n", "alpine", "sudo apk add"),
        ("ID=void\n", "void", "sudo xbps-install -S"),
        ("ID=gentoo\n", "gentoo", "sudo emerge --ask"),
        ("ID=nixos\nID_LIKE=\"\"\n", "nixos", "nix profile install nixpkgs#{}"),
    ];

    #[test]
    fn embedded_data_is_valid() {
        let data = data();
        for name in ["arch", "omarchy", "debian", "fedora", "suse", "alpine", "void", "gentoo", "nixos"] {
            assert!(data.families.contains_key(name), "missing family {name}");
        }
        for (name, family) in &data.families {
            assert!(data.field(name, |f| f.install.as_ref()).is_some(), "{name} has no install command");
            assert!(data.field(name, |f| f.provides.as_ref()).is_some(), "{name} has no provides command");
            assert!(family.extends.as_ref().is_none_or(|p| data.families.contains_key(p)), "{name} extends nothing");
        }
        for (key, rows) in &data.packages {
            for family in rows.keys() {
                assert!(family == "*" || data.families.contains_key(family), "{key}: unknown family {family}");
            }
        }
        for key in data.programs.values() {
            assert!(data.packages.contains_key(key) || data.same.contains(key), "program key {key} has no package row");
        }
    }

    #[test]
    fn reads_ids_in_order() {
        assert_eq!(ids_from("NAME=\"Omarchy\"\nID=omarchy\nID_LIKE=arch\n"), ["omarchy", "arch"]);
        assert_eq!(ids_from("ID_LIKE=\"ubuntu debian\"\nID=linuxmint\n"), ["linuxmint", "ubuntu", "debian"]);
        assert_eq!(ids_from("VERSION_ID=\"12\"\nID='alpine'\n"), ["alpine"]);
        assert_eq!(ids_from(""), Vec::<String>::new());
    }

    #[test]
    fn every_listed_distro_gets_its_package_manager() {
        for (os_release, family, install) in FIXTURES {
            let platform = detect_with(os_release, &|_| false);
            assert_eq!(manager(&platform).as_deref(), Some(*family), "{os_release}");
            assert_eq!(fact(&platform, "pkg_install").as_deref(), Some(*install), "{os_release}");
        }
    }

    #[test]
    fn derivatives_that_only_name_a_parent_still_map_to_its_family() {
        let platform = detect_with("ID=elementary\nID_LIKE=ubuntu\n", &|_| false);
        assert!(platform.contains(&"debian".to_string()), "{platform:?}");
        let platform = detect_with("ID=artix\n", &|_| false);
        assert!(platform.contains(&"arch".to_string()), "{platform:?}");
    }

    #[test]
    fn unknown_ids_fall_back_to_the_package_manager_on_path() {
        let has = |wanted: &'static str| move |program: &str| program == wanted;
        for (binary, family) in [
            ("pacman", "arch"),
            ("apt", "debian"),
            ("dnf", "fedora"),
            ("zypper", "suse"),
            ("apk", "alpine"),
            ("xbps-install", "void"),
            ("emerge", "gentoo"),
            ("nix", "nixos"),
        ] {
            let platform = detect_with("ID=somethingnew\nID_LIKE=\"other\"\n", &has(binary));
            assert_eq!(manager(&platform).as_deref(), Some(family), "{binary}");
            assert!(platform.contains(&"somethingnew".to_string()));
        }
        // Nothing known and no package manager: no family, so no install advice.
        let platform = detect_with("ID=somethingnew\n", &|_| false);
        assert_eq!(manager(&platform), None);
        assert_eq!(install_advice("htop", None, &platform), None);
        // Missing os-release altogether behaves the same.
        assert_eq!(manager(&detect_with("", &has("apk"))).as_deref(), Some("alpine"));
        // A known ID is never second-guessed by what is on PATH.
        assert_eq!(manager(&detect_with("ID=debian\n", &has("pacman"))).as_deref(), Some("debian"));
    }

    #[test]
    fn omarchy_beats_arch_in_either_order() {
        assert_eq!(manager(&words(&["omarchy", "arch"])).as_deref(), Some("omarchy"));
        assert_eq!(manager(&words(&["arch", "omarchy"])).as_deref(), Some("omarchy"));
        assert_eq!(fact(&words(&["omarchy", "arch"]), "pkg_remove").as_deref(), Some("omarchy pkg drop"));
        // Overridden for omarchy; inherited from arch.
        assert_eq!(fact(&words(&["omarchy", "arch"]), "pkg_update").as_deref(), Some("omarchy update"));
        assert_eq!(fact(&words(&["omarchy", "arch"]), "pkg_search").as_deref(), Some("pacman -Ss"));
        assert_eq!(fact(&words(&["arch"]), "pkg_update").as_deref(), Some("sudo pacman -Syu"));
        assert_eq!(manager(&words(&["arch"])).as_deref(), Some("arch"));
    }

    #[test]
    fn package_names_differ_by_family() {
        let advice = |program: &str, ids: &[&str]| install_advice(program, None, &words(ids)).map(|a| a.command);
        let deb = ["debian"];
        let fed = ["fedora"];
        let arch = ["arch"];
        assert_eq!(advice("nmcli", &deb).as_deref(), Some("sudo apt install network-manager"));
        assert_eq!(advice("systemctl", &deb).as_deref(), Some("sudo apt install systemd"));
        assert_eq!(advice("ps", &deb).as_deref(), Some("sudo apt install procps"));
        assert_eq!(advice("ps", &fed).as_deref(), Some("sudo dnf install procps-ng"));
        assert_eq!(advice("lsusb", &deb).as_deref(), Some("sudo apt install usbutils"));
        assert_eq!(advice("lsblk", &deb).as_deref(), Some("sudo apt install util-linux"));
        assert_eq!(advice("tracepath", &deb).as_deref(), Some("sudo apt install iputils-tracepath"));
        assert_eq!(advice("usermod", &deb).as_deref(), Some("sudo apt install passwd"));
        assert_eq!(advice("usermod", &fed).as_deref(), Some("sudo dnf install shadow-utils"));
        assert_eq!(advice("modprobe", &deb).as_deref(), Some("sudo apt install kmod"));
        assert_eq!(advice("ip", &fed).as_deref(), Some("sudo dnf install iproute"));
        assert_eq!(advice("ip", &arch).as_deref(), Some("sudo pacman -S iproute2"));
        assert_eq!(advice("virsh", &deb).as_deref(), Some("sudo apt install libvirt-clients"));
        assert_eq!(advice("fd", &deb).as_deref(), Some("sudo apt install fd-find"));
        assert_eq!(advice("dig", &deb).as_deref(), Some("sudo apt install bind9-dnsutils"));
        assert_eq!(advice("dig", &fed).as_deref(), Some("sudo dnf install bind-utils"));
        assert_eq!(advice("dig", &arch).as_deref(), Some("sudo pacman -S bind"));
        assert_eq!(advice("nc", &deb).as_deref(), Some("sudo apt install netcat-openbsd"));
        assert_eq!(advice("nc", &arch).as_deref(), Some("sudo pacman -S openbsd-netcat"));
        assert_eq!(advice("python3", &deb).as_deref(), Some("sudo apt install python3"));
        assert_eq!(advice("python3", &fed).as_deref(), Some("sudo dnf install python3"));
        assert_eq!(advice("cargo", &deb).as_deref(), Some("sudo apt install cargo"));
        assert_eq!(advice("cargo", &arch).as_deref(), Some("sudo pacman -S rust"));
        assert_eq!(advice("rg", &["omarchy", "arch"]).as_deref(), Some("omarchy pkg add ripgrep"));
        assert_eq!(advice("htop", &["alpine"]).as_deref(), Some("sudo apk add htop"));
        assert_eq!(advice("htop", &["nixos"]).as_deref(), Some("nix profile install nixpkgs#htop"));
        assert_eq!(advice("htop", &["suse"]).as_deref(), Some("sudo zypper install htop"));
        assert_eq!(advice("htop", &["void"]).as_deref(), Some("sudo xbps-install -S htop"));
        assert_eq!(advice("htop", &["gentoo"]).as_deref(), Some("sudo emerge --ask htop"));
        // The catalog's own package name goes through the same table.
        assert_eq!(
            install_advice("virsh", Some("libvirt"), &words(&["debian"])).map(|a| a.command).as_deref(),
            Some("sudo apt install libvirt-clients")
        );
    }

    #[test]
    fn unknown_programs_get_a_provides_search_not_a_guess() {
        let cases = [
            ("arch", "pacman -F frobnicate"),
            ("debian", "apt-file search bin/frobnicate"),
            ("fedora", "dnf provides '*/bin/frobnicate'"),
            ("suse", "zypper search --provides frobnicate"),
            ("alpine", "apk search -e cmd:frobnicate"),
            ("void", "xlocate frobnicate"),
            ("gentoo", "e-file frobnicate"),
            ("nixos", "nix-locate bin/frobnicate"),
        ];
        for (family, command) in cases {
            let advice = install_advice("frobnicate", None, &words(&[family])).expect(family);
            assert_eq!(advice, Advice { command: command.to_string(), installs: false, package: None }, "{family}");
        }
    }

    #[test]
    fn never_installs_package_managers_or_distro_tools() {
        for program in
            ["pacman", "apt", "apt-get", "dnf", "zypper", "apk", "xbps-install", "emerge", "nix", "omarchy", "yay"]
        {
            assert_eq!(install_advice(program, None, &words(&["debian"])), None, "{program}");
            assert_eq!(install_advice(program, None, &words(&["arch"])), None, "{program}");
        }
        // Packages a family does not have are not guessed either.
        assert_eq!(install_advice("systemctl", None, &words(&["alpine"])), None);
    }

    #[test]
    fn package_tasks_fill_in_for_every_manager() {
        let plat = words(&["fedora"]);
        assert_eq!(fact(&plat, "pkg_provides").as_deref(), Some("dnf provides '*/bin/{}'"));
        assert_eq!(fact(&plat, "pkg_owns").as_deref(), Some("rpm -qf"));
        assert_eq!(fact(&words(&["nixos"]), "pkg_owns"), None);
        assert_eq!(fact(&words(&["debian"]), "pkg_nope"), None);
    }

    #[test]
    fn user_data_extends_the_embedded_data() {
        let mut data: Data = toml::from_str(EMBEDDED).unwrap();
        let extra: Data = toml::from_str(
            "same = [\"frobnicate\"]\n[families.mylinux]\nids = [\"mylinux\"]\ninstall = \"mypm add\"\nprovides = \"mypm who {}\"\n\
             [families.arch]\nsearch = \"pacman -Ssq\"\n[packages]\nhtop = { mylinux = \"htop-ng\" }\n",
        )
        .unwrap();
        data.merge(extra);
        let platform = expand_with(&data, &words(&["mylinux"]));
        assert_eq!(platform, ["mylinux"]);
        assert!(matches!(package_name(&data, "mylinux", "frobnicate", None), Name::Package(p) if p == "frobnicate"));
        assert!(matches!(package_name(&data, "mylinux", "htop", None), Name::Package(p) if p == "htop-ng"));
        assert_eq!(data.field("arch", |f| f.search.as_ref()), Some("pacman -Ssq"));
        assert_eq!(data.field("arch", |f| f.install.as_ref()), Some("sudo pacman -S"));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("huh-platform-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, name: &str, mode: u32) -> PathBuf {
        let file = dir.join(name);
        std::fs::write(&file, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
        file
    }

    #[test]
    fn path_detection_checks_the_exec_bit() {
        let dir = temp_dir("exec");
        write(&dir, "runs", 0o755);
        write(&dir, "plain-file", 0o644);
        std::fs::create_dir(dir.join("a-folder")).unwrap();
        let path = [dir.clone()];
        assert_eq!(locate_in("runs", &path, &[]), Found::OnPath(dir.join("runs")));
        assert_eq!(locate_in("plain-file", &path, &[]), Found::Missing);
        assert_eq!(locate_in("a-folder", &path, &[]), Found::Missing);
        assert_eq!(locate_in("nothing", &path, &[]), Found::Missing);
        assert_eq!(locate_in("", &path, &[]), Found::Missing);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn path_detection_follows_symlinks_but_not_broken_ones() {
        let dir = temp_dir("links");
        let real = write(&dir, "real-tool", 0o755);
        let data = write(&dir, "data-file", 0o644);
        std::os::unix::fs::symlink(&real, dir.join("alias")).unwrap();
        std::os::unix::fs::symlink(&data, dir.join("alias-to-data")).unwrap();
        std::os::unix::fs::symlink(dir.join("gone"), dir.join("dangling")).unwrap();
        let path = [dir.clone()];
        assert!(matches!(locate_in("alias", &path, &[]), Found::OnPath(_)));
        assert_eq!(locate_in("alias-to-data", &path, &[]), Found::Missing);
        assert_eq!(locate_in("dangling", &path, &[]), Found::Missing);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn path_detection_separates_sbin_from_path_and_prefers_path() {
        let bin = temp_dir("bin");
        let sbin = temp_dir("sbin");
        write(&sbin, "modprobe", 0o755);
        write(&sbin, "both", 0o755);
        write(&bin, "both", 0o755);
        assert_eq!(
            locate_in("modprobe", std::slice::from_ref(&bin), std::slice::from_ref(&sbin)),
            Found::SbinOnly(sbin.join("modprobe"))
        );
        assert_eq!(
            locate_in("both", std::slice::from_ref(&bin), std::slice::from_ref(&sbin)),
            Found::OnPath(bin.join("both"))
        );
        assert_eq!(locate_in("modprobe", &[bin.clone(), sbin.clone()], &[]), Found::OnPath(sbin.join("modprobe")));
        std::fs::remove_dir_all(&bin).ok();
        std::fs::remove_dir_all(&sbin).ok();
    }

    #[test]
    fn expands_a_leading_tilde_in_path_entries() {
        assert_eq!(expand_tilde(Path::new("~/bin"), Some("/home/u")), PathBuf::from("/home/u/bin"));
        assert_eq!(expand_tilde(Path::new("~"), Some("/home/u")), PathBuf::from("/home/u"));
        assert_eq!(expand_tilde(Path::new("/usr/bin"), Some("/home/u")), PathBuf::from("/usr/bin"));
        assert_eq!(expand_tilde(Path::new("~other/bin"), Some("/home/u")), PathBuf::from("~other/bin"));
        assert_eq!(expand_tilde(Path::new("~/bin"), None), PathBuf::from("~/bin"));

        let home = temp_dir("home");
        std::fs::create_dir(home.join("bin")).unwrap();
        write(&home.join("bin"), "mine", 0o755);
        let dir = expand_tilde(Path::new("~/bin"), home.to_str());
        assert!(matches!(locate_in("mine", &[dir], &[]), Found::OnPath(_)));
        std::fs::remove_dir_all(&home).ok();
    }
}
