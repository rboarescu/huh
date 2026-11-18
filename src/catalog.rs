//! The command catalog: one TOML file per command.

use crate::platform::{self, Found};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub name: String,
    /// What the user achieves with it, in plain words. The matcher matches against this.
    pub description: String,
    /// Command line with `{id}` placeholders for slots and flags.
    pub template: String,
    #[serde(default)]
    pub destructive: bool,
    /// The text of the WARNING line for a destructive command (a general one when unset).
    #[serde(default)]
    pub warning: Option<String>,
    /// Never suggested; only lets `huh explain` recognise another family's package commands.
    /// Made by `with_explain_variants`, not written in catalog files.
    #[serde(default)]
    pub explain_only: bool,
    /// The `pkg_*` fact this template was made from (`pkg_install`, ...), set when the catalog is
    /// made usable for a platform. Lets slot filling map program names to package names.
    #[serde(skip)]
    pub pkg_role: Option<String>,
    /// Set for commands that only exist on one family of systems: "arch", "debian",
    /// "fedora", "omarchy", ... Matched against the platform words (see platform.rs).
    pub platform: Option<String>,
    /// Package name when it differs from `program()`: a key of `[packages]` in data/platforms.toml.
    #[serde(default)]
    pub package: Option<String>,
    /// Natural-language synonyms and search phrases for matching.
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub slots: Vec<Slot>,
    #[serde(default)]
    pub flags: Vec<Flag>,
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SlotKind {
    /// One of a fixed list of values held in the catalog.
    Choice,
    /// A file or folder: picked from words in the request and entries in the current folder.
    Path,
    /// Any other value (pattern, number): picked from words in the request.
    Text,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Slot {
    pub id: String,
    pub kind: SlotKind,
    /// `choice` slots: the question whose answer is one of `options`; its words help matching.
    pub question: Option<String>,
    #[serde(default)]
    pub options: Vec<SlotOption>,
    /// `path` and `text` slots: noun phrase for the value, e.g. "the folder to list".
    pub what: Option<String>,
    /// Used when the request does not name a value. Without it the output shows `<id>`.
    pub default: Option<String>,
    /// Wrapper for a named value, e.g. `-name {}`.
    pub format: Option<String>,
    /// Set when the template already puts this value inside quotes, as in
    /// `echo 'export {name}={value}'`: the value is then inserted without quotes of its own.
    #[serde(default)]
    pub raw: bool,
    /// Name of a built-in reader (see extract.rs) that takes the value from the wording of the
    /// request instead of from the words offered as candidates, e.g. `shutdown_time`.
    #[serde(default)]
    pub extract: Option<String>,
    /// What kind of name the value is (`service`, `user`, `group`, `shell`, `container`, `package`):
    /// the label words in the request ("group docker") and, where the system has a list, the names
    /// found on this machine (see sysinfo.rs) decide which word fills the slot.
    #[serde(default, rename = "type")]
    pub value_type: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct SlotOption {
    pub name: String,
    pub value: String,
    pub description: String,
    /// Set when picking this option can lose work or change the system: the WARNING line says this.
    #[serde(default)]
    pub warning: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Flag {
    pub id: String,
    pub value: String,
    /// Yes/no question, phrased so that "yes" means the flag is wanted. Its words
    /// help matching, and it is shown (in the second person) when huh asks or notes.
    pub question: String,
    /// Set when adding this flag can lose work or change the system: the WARNING line says this.
    #[serde(default)]
    pub warning: Option<String>,
}

impl Command {
    /// The program the template runs, looking past `sudo`.
    pub fn program(&self) -> &str {
        self.template.split_whitespace().find(|word| *word != "sudo").unwrap_or_default()
    }
}

/// Placeholders that code fills in from the machine, not from the request. The `pkg_*`
/// ones become the package manager's command when the catalog is made usable for a platform.
pub const FACTS: &[&str] =
    &["shell_rc", "pkg_install", "pkg_remove", "pkg_search", "pkg_update", "pkg_owns", "pkg_provides"];

pub fn fact(id: &str) -> Option<String> {
    match id {
        "shell_rc" => Some(shell_rc(&std::env::var("SHELL").unwrap_or_default()).to_string()),
        _ => None,
    }
}

/// The startup file of a login shell, where exports and aliases go.
fn shell_rc(shell: &str) -> &'static str {
    match shell.rsplit('/').next().unwrap_or_default() {
        "bash" => "~/.bashrc",
        "zsh" => "~/.zshrc",
        "fish" => "~/.config/fish/config.fish",
        _ => "~/.profile",
    }
}

/// Words that run without a file on PATH: the bash built-ins (the catalog uses `kill`,
/// `cd`, `history`, ...) and shell keywords.
const BUILTINS: &[&str] = &[
    ".",
    ":",
    "alias",
    "bg",
    "bind",
    "builtin",
    "caller",
    "cd",
    "command",
    "compgen",
    "complete",
    "declare",
    "dirs",
    "disown",
    "echo",
    "enable",
    "eval",
    "exec",
    "exit",
    "export",
    "false",
    "fc",
    "fg",
    "getopts",
    "hash",
    "help",
    "history",
    "jobs",
    "kill",
    "let",
    "local",
    "logout",
    "mapfile",
    "popd",
    "printf",
    "pushd",
    "pwd",
    "read",
    "readarray",
    "readonly",
    "return",
    "set",
    "shift",
    "shopt",
    "source",
    "suspend",
    "test",
    "times",
    "trap",
    "true",
    "type",
    "typeset",
    "ulimit",
    "umask",
    "unalias",
    "unset",
    "wait",
];

pub fn is_builtin(program: &str) -> bool {
    BUILTINS.contains(&program)
}

impl Command {
    /// Whether the template runs through `sudo`.
    pub fn uses_sudo(&self) -> bool {
        self.template.split_whitespace().next() == Some("sudo")
    }

    /// Whether the program can run: on PATH, or in /usr/sbin and the like when `sudo` runs it
    /// (sudo's own PATH has those; a normal user's PATH on Debian does not).
    pub fn available(&self) -> bool {
        let program = self.program();
        is_builtin(program)
            || match platform::locate(program) {
                Found::OnPath(_) => true,
                Found::SbinOnly(_) => self.uses_sudo(),
                Found::Missing => false,
            }
    }
}

/// The text after `NOTE:` for a suggestion whose program is not available, or `None` when
/// it is. `line` is the suggested command line; `command` the catalog entry it came from.
pub fn missing_note(line: &str, command: Option<&Command>, platforms: &[String]) -> Option<String> {
    let mut words = line.split_whitespace();
    let first = words.next().unwrap_or_default();
    let sudo = first == "sudo";
    let program = if sudo { words.next().unwrap_or_default() } else { first };
    if program.is_empty() || is_builtin(program) {
        return None;
    }
    match platform::locate(program) {
        Found::OnPath(_) => None,
        Found::SbinOnly(_) if sudo => None,
        Found::SbinOnly(file) => Some(format!(
            "`{program}` is in {}, which is not on your PATH. Run it as {} or with sudo.",
            file.parent().map_or(String::new(), |dir| dir.display().to_string()),
            file.display()
        )),
        Found::Missing => {
            // A command written for another kind of system (--all-commands) gets no install advice.
            let foreign = command.and_then(|c| c.platform.as_ref()).is_some_and(|p| !platforms.contains(p));
            let package = command.and_then(|c| c.package.as_deref());
            match platform::install_advice(program, package, platforms).filter(|_| !foreign) {
                Some(advice) if advice.installs => {
                    let remark = advice.note.map(|note| format!(" {note}")).unwrap_or_default();
                    Some(format!("`{program}` is not installed. Install it with: {}{remark}", advice.command))
                }
                Some(advice) => Some(format!(
                    "`{program}` is not installed. Find the package that provides it with: {}",
                    advice.command
                )),
                None => Some(format!("`{program}` is not installed on this machine.")),
            }
        }
    }
}

/// The platform words for this machine.
pub fn this_platform() -> Vec<String> {
    platform::detect()
}

/// `pkg_role` of a package task on a system with no known package manager.
const NO_MANAGER: &str = "none";

impl Command {
    /// True for a package task kept only to recognise the request on a system whose package
    /// manager is unknown. Its template is not a command.
    pub fn without_manager(&self) -> bool {
        self.pkg_role.as_deref() == Some(NO_MANAGER)
    }
}

/// Fills the `{pkg_*}` placeholders of a command from the platform's package manager.
/// `None` when the platform has no such task (e.g. no "which package owns" on NixOS).
fn resolve_one(mut command: Command, platforms: &[String]) -> Option<Command> {
    for id in placeholders(&command.template.clone()) {
        if !platform::PKG_FACTS.contains(&id) {
            continue;
        }
        let Some(text) = platform::fact(platforms, id) else {
            // No package manager known at all: keep the task so the request is recognised as a
            // package request (and answered with "no package manager detected", see `without_manager`).
            if platform::manager(platforms).is_none() {
                command.pkg_role = Some(NO_MANAGER.to_string());
                return Some(command);
            }
            return None;
        };
        command.pkg_role.get_or_insert_with(|| id.to_string());
        let filled = match command.slots.first() {
            Some(slot) => platform::fill(&text, &format!("{{{}}}", slot.id)),
            None if text.contains("{}") => return None,
            None => text,
        };
        command.template = command.template.replace(&format!("{{{id}}}"), &filled);
    }
    Some(command)
}

/// Makes package tasks concrete for `platforms`, dropping the ones this platform cannot do.
/// Everything else is left alone.
pub fn resolve(commands: Vec<Command>, platforms: &[String]) -> Vec<Command> {
    let platforms = platform::expand(platforms);
    commands.into_iter().filter_map(|c| resolve_one(c, &platforms)).collect()
}

/// Adds, for every package task and every family, the concrete command (`apt install`,
/// `pacman -Rs`, ...) so `huh explain` can describe them on any machine. They are never suggested.
fn with_explain_variants(mut commands: Vec<Command>) -> Vec<Command> {
    let generic: Vec<Command> = commands
        .iter()
        .filter(|c| placeholders(&c.template).iter().any(|id| platform::PKG_FACTS.contains(id)))
        .cloned()
        .collect();
    for family in platform::family_names() {
        for command in &generic {
            if let Some(mut variant) = resolve_one(command.clone(), std::slice::from_ref(&family)) {
                variant.name = format!("{family} {}", command.name);
                variant.platform = Some(family.clone());
                variant.explain_only = true;
                commands.push(variant);
            }
        }
    }
    commands
}

/// Drops the commands that exist only for `huh explain`.
pub fn matchable(commands: Vec<Command>) -> Vec<Command> {
    commands.into_iter().filter(|c| !c.explain_only).collect()
}

/// Keeps the commands that make sense here: right platform, package tasks filled in for
/// this system's package manager and, when `only_installed` is set, a program that can run.
pub fn usable(commands: Vec<Command>, platforms: &[String], only_installed: bool) -> Vec<Command> {
    let platforms = platform::expand(platforms);
    let here: Vec<Command> =
        matchable(commands).into_iter().filter(|c| c.platform.as_ref().is_none_or(|p| platforms.contains(p))).collect();
    resolve(here, &platforms).into_iter().filter(|c| !only_installed || c.available()).collect()
}

include!(concat!(env!("OUT_DIR"), "/embedded_catalog.rs"));

fn resolve_override_dir(
    explicit: Option<PathBuf>,
    get_env: &dyn Fn(&str) -> Option<String>,
    is_dir: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    if let Some(dir) = explicit {
        return Some(dir);
    }
    if let Some(dir) = get_env("HUH_CATALOG").filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let config = get_env("XDG_CONFIG_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| get_env("HOME").filter(|s| !s.is_empty()).map(|home| PathBuf::from(home).join(".config")));
    if let Some(config) = config {
        let dir = config.join("huh/catalog");
        if is_dir(&dir) {
            return Some(dir);
        }
    }
    None
}

fn override_dir(explicit: Option<PathBuf>) -> Option<PathBuf> {
    resolve_override_dir(explicit, &|name| std::env::var(name).ok(), &Path::is_dir)
}

fn parse_command(label: &str, text: &str, names: &mut BTreeSet<String>) -> Result<Command> {
    let command: Command = toml::from_str(text).with_context(|| format!("invalid catalog file {label}"))?;
    validate(&command).with_context(|| format!("invalid catalog file {label}"))?;
    if !names.insert(command.name.clone()) {
        bail!("command `{}` is defined twice", command.name);
    }
    Ok(command)
}

pub fn load(dir: &Path) -> Result<Vec<Command>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("cannot read catalog folder {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    paths.sort();

    let mut commands = Vec::new();
    let mut names = BTreeSet::new();
    for path in paths {
        let text = std::fs::read_to_string(&path)?;
        commands.push(parse_command(&path.display().to_string(), &text, &mut names)?);
    }
    if commands.is_empty() {
        bail!("no catalog files found in {}", dir.display());
    }
    Ok(with_explain_variants(commands))
}

/// Loads and validates the catalog compiled into the binary.
pub fn load_embedded() -> Result<Vec<Command>> {
    let mut commands = Vec::new();
    let mut names = BTreeSet::new();
    for &(name, text) in EMBEDDED_CATALOG {
        commands.push(parse_command(name, text, &mut names)?);
    }
    if commands.is_empty() {
        bail!("no embedded catalog files found");
    }
    Ok(with_explain_variants(commands))
}

/// Loads from `--catalog`, `$HUH_CATALOG`, or user config directories, falling back to `load_embedded()`.
pub fn load_default(explicit: Option<PathBuf>) -> Result<Vec<Command>> {
    match override_dir(explicit) {
        Some(dir) => load(&dir),
        None => load_embedded(),
    }
}

/// The `{id}` placeholders in a template, in order.
pub fn placeholders(template: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let Some(len) = rest[start..].find('}') else { break };
        found.push(&rest[start + 1..start + len]);
        rest = &rest[start + len + 1..];
    }
    found
}

pub fn validate(command: &Command) -> Result<()> {
    let mut ids = BTreeSet::new();
    for id in command.slots.iter().map(|s| &s.id).chain(command.flags.iter().map(|f| &f.id)) {
        if !ids.insert(id.as_str()) {
            bail!("id `{id}` is used twice");
        }
    }
    // A `{pkg_*}` placeholder carries its argument (the one slot) once it is filled in.
    let package_task = placeholders(&command.template).iter().any(|id| platform::PKG_FACTS.contains(id));
    if package_task && command.slots.len() > 1 {
        bail!("a package task takes at most one slot");
    }
    let used: BTreeSet<&str> = if package_task {
        command.slots.iter().map(|s| s.id.as_str()).collect()
    } else {
        placeholders(&command.template).into_iter().filter(|id| !FACTS.contains(id)).collect()
    };
    if let Some(id) = used.difference(&ids).next() {
        bail!("template uses `{{{id}}}` but no slot or flag has that id");
    }
    if let Some(id) = ids.difference(&used).next() {
        bail!("`{id}` is defined but missing from the template");
    }
    for slot in &command.slots {
        match slot.kind {
            SlotKind::Choice => {
                if slot.question.is_none() || slot.options.len() < 2 {
                    bail!("choice slot `{}` needs a `question` and at least two options", slot.id);
                }
            }
            SlotKind::Path | SlotKind::Text => {
                if slot.what.is_none() {
                    bail!("slot `{}` needs `what` (e.g. \"the folder to list\")", slot.id);
                }
            }
        }
        if let Some(value_type) = &slot.value_type
            && !crate::sysinfo::TYPES.contains(&value_type.as_str())
        {
            bail!("slot `{}`: unknown type `{value_type}` (known: {})", slot.id, crate::sysinfo::TYPES.join(", "));
        }
        if slot.format.as_ref().is_some_and(|format| !format.contains("{}")) {
            bail!("slot `{}`: `format` must contain `{{}}`", slot.id);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn command(template: &str, platform: Option<&str>) -> Command {
        Command {
            name: template.into(),
            description: String::new(),
            template: template.into(),
            destructive: false,
            warning: None,
            explain_only: false,
            pkg_role: None,
            platform: platform.map(str::to_string),
            package: None,
            keywords: vec![],
            slots: vec![],
            flags: vec![],
        }
    }

    #[test]
    fn picks_the_startup_file_for_the_shell() {
        assert_eq!(shell_rc("/usr/bin/bash"), "~/.bashrc");
        assert_eq!(shell_rc("/bin/zsh"), "~/.zshrc");
        assert_eq!(shell_rc("/usr/bin/fish"), "~/.config/fish/config.fish");
        assert_eq!(shell_rc(""), "~/.profile");
    }

    #[test]
    fn program_looks_past_sudo() {
        assert_eq!(command("sudo pacman -S {package}", None).program(), "pacman");
        assert_eq!(command("ls {target}", None).program(), "ls");
    }

    #[test]
    fn filters_by_platform_and_by_what_is_installed() {
        let all = vec![
            command("sh -c true", None),
            command("sudo apt install {package}", Some("debian")),
            command("sudo pacman -S {package}", Some("arch")),
            command("no-such-program-xyz", None),
            command("cd {target}", None),
        ];
        let arch = ["arch".to_string()];
        let kept: Vec<String> = usable(all.clone(), &arch, false).into_iter().map(|c| c.name).collect();
        assert_eq!(kept, ["sh -c true", "sudo pacman -S {package}", "no-such-program-xyz", "cd {target}"]);
        let kept: Vec<String> = usable(all, &["debian".to_string()], true).into_iter().map(|c| c.name).collect();
        assert!(kept.contains(&"sh -c true".to_string()) && kept.contains(&"cd {target}".to_string()));
        assert!(!kept.contains(&"no-such-program-xyz".to_string()));
        assert!(!kept.contains(&"sudo pacman -S {package}".to_string()));
    }

    #[test]
    fn loads_and_validates_embedded_catalog() {
        let embedded = load_embedded().expect("embedded catalog should load and validate");
        let on_disk = load(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/catalog"))).unwrap();
        assert_eq!(embedded.len(), on_disk.len());
        assert_eq!(load_default(None).unwrap().len(), embedded.len());
        assert!(embedded.iter().all(|c| !c.keywords.is_empty()));

        let find = |name: &str| embedded.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("missing {name}"));
        assert_eq!(find("smartctl").package.as_deref(), Some("smartmontools"));
        assert_eq!(find("zbarimg").package.as_deref(), Some("zbar"));
        assert_eq!(find("battery status").package.as_deref(), Some("upower"));
        for net in ["ss", "ip", "ip neigh", "ip route", "ip route get"] {
            assert_eq!(find(net).package.as_deref(), Some("iproute2"), "{net}");
        }
        assert!(
            embedded.iter().filter(|c| c.name.starts_with("virsh ")).all(|c| c.package.as_deref() == Some("libvirt"))
        );

        for expected in ["tracepath", "ip route get", "omarchy refresh", "omarchy default", "omarchy pkg aur add"] {
            assert!(embedded.iter().any(|c| c.name == expected), "missing {expected}");
        }
        let restart = find("omarchy restart");
        let opts: Vec<&str> = restart.slots[0].options.iter().map(|o| o.name.as_str()).collect();
        for opt in ["wifi", "audio", "bluetooth", "shell", "terminal", "hyprctl", "trackpad", "hyprsunset"] {
            assert!(opts.contains(&opt), "missing omarchy restart option {opt}");
        }

        // Explicit dir override loads that folder and errors if missing.
        let tmp = std::env::temp_dir().join(format!("huh-catalog-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("demo.toml"), "name = \"demo\"\ndescription = \"d\"\ntemplate = \"echo ok\"\n")
            .unwrap();
        let custom = load_default(Some(tmp.clone())).unwrap();
        assert_eq!(custom.len(), 1);
        assert_eq!(custom[0].name, "demo");
        std::fs::remove_dir_all(&tmp).ok();
        assert!(load_default(Some(PathBuf::from("/nonexistent-huh-catalog-dir"))).is_err());
    }

    #[test]
    fn resolves_catalog_override_precedence() {
        // Explicit beats env and config dirs.
        let envs = HashMap::from([("HUH_CATALOG", "/env/huh"), ("HOME", "/home/u")]);
        let got =
            resolve_override_dir(Some(PathBuf::from("/explicit")), &|k| envs.get(k).map(|s| s.to_string()), &|_| true);
        assert_eq!(got, Some(PathBuf::from("/explicit")));

        // HUH_CATALOG beats config dirs.
        let got = resolve_override_dir(None, &|k| envs.get(k).map(|s| s.to_string()), &|_| true);
        assert_eq!(got, Some(PathBuf::from("/env/huh")));

        // Config dir is checked when no override is set, and only when it exists.
        let envs3 = HashMap::from([("HOME", "/home/u")]);
        let got = resolve_override_dir(None, &|k| envs3.get(k).map(|s| s.to_string()), &|_| true);
        assert_eq!(got, Some(PathBuf::from("/home/u/.config/huh/catalog")));

        let got = resolve_override_dir(None, &|k| envs3.get(k).map(|s| s.to_string()), &|_| false);
        assert_eq!(got, None);

        // XDG_CONFIG_HOME overrides HOME/.config.
        let envs4 = HashMap::from([("XDG_CONFIG_HOME", "/custom/cfg"), ("HOME", "/home/u")]);
        let got = resolve_override_dir(None, &|k| envs4.get(k).map(|s| s.to_string()), &|_| true);
        assert_eq!(got, Some(PathBuf::from("/custom/cfg/huh/catalog")));

        // Falls back to None (embedded) when nothing matches.
        let got = resolve_override_dir(None, &|k| envs3.get(k).map(|s| s.to_string()), &|_| false);
        assert_eq!(got, None);
    }

    #[test]
    fn notes_say_how_to_install_a_missing_program() {
        let note = |line: &str, cmd: &Command, ids: &[&str]| {
            let ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
            missing_note(line, Some(cmd), &ids)
        };
        let line = "no-such-tool-xyz --flag";
        let plain = command(line, None);
        assert_eq!(
            note(line, &plain, &["debian"]).as_deref(),
            Some(
                "`no-such-tool-xyz` is not installed. Find the package that provides it with: apt-file search bin/no-such-tool-xyz"
            )
        );
        assert_eq!(
            note(line, &plain, &["somethingnew-no-pm"]).as_deref(),
            Some("`no-such-tool-xyz` is not installed on this machine.")
        );
        // Built-ins are always there.
        assert_eq!(note("cd /tmp", &command("cd /tmp", None), &["debian"]), None);
        assert_eq!(note("kill 1", &command("kill {pid}", None), &["debian"]), None);
        // Package managers and distro tools are never installed with another distro's manager.
        let apt = command("sudo apt-nonexistent install {package}", Some("debian"));
        assert_eq!(
            note("sudo apt-nonexistent install x", &apt, &["arch"]).as_deref(),
            Some("`apt-nonexistent` is not installed on this machine.")
        );
        let pacman = command("pacman -Ss {term}", Some("arch"));
        assert_eq!(
            note("pacman -Ss x", &pacman, &["debian"]).filter(|n| n.contains("Install it")),
            None,
            "pacman is not installable on Debian"
        );
    }

    #[test]
    fn package_tasks_are_filled_in_per_package_manager() {
        let all = load_embedded().unwrap();
        let task = |name: &str, ids: &[&str]| {
            let ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
            usable(all.clone(), &ids, false).into_iter().find(|c| c.name == name).map(|c| c.template)
        };
        assert_eq!(task("package install", &["omarchy", "arch"]).as_deref(), Some("omarchy pkg add {package}"));
        assert_eq!(task("package install", &["arch"]).as_deref(), Some("sudo pacman -S {package}"));
        assert_eq!(task("package install", &["elementary", "ubuntu"]).as_deref(), Some("sudo apt install {package}"));
        assert_eq!(task("package install", &["alpine"]).as_deref(), Some("sudo apk add {package}"));
        assert_eq!(task("package install", &["nixos"]).as_deref(), Some("nix profile install nixpkgs#{package}"));
        assert_eq!(task("package remove", &["suse"]).as_deref(), Some("sudo zypper remove {package}"));
        assert_eq!(
            task("system update", &["gentoo"]).as_deref(),
            Some("sudo emerge --sync && sudo emerge --ask --update --deep --newuse @world")
        );
        assert_eq!(task("package owns file", &["fedora"]).as_deref(), Some("rpm -qf {file}"));
        assert_eq!(task("package owns file", &["nixos"]), None);
        // With no known package manager the task stays only to recognise the request.
        let none = usable(all.clone(), &["somethingnew-no-pm".to_string()], false);
        assert!(none.iter().find(|c| c.name == "package install").is_some_and(Command::without_manager));
        // Distro-specific entries stay with their distro.
        assert!(task("yay install", &["debian"]).is_none());
        assert!(task("omarchy update", &["arch"]).is_none());
        assert!(task("omarchy update", &["omarchy", "arch"]).is_some());
    }

    #[test]
    fn every_catalog_package_has_a_name_on_the_big_families() {
        for command in load_embedded().unwrap() {
            let Some(package) = command.package.as_deref() else { continue };
            for family in ["arch", "debian", "fedora", "suse"] {
                let advice = platform::install_advice(command.program(), Some(package), &[family.to_string()]);
                assert!(advice.is_some_and(|a| a.installs), "{} has no {family} package", command.name);
            }
        }
    }

    #[test]
    fn catalog_facts_match_the_platform_module() {
        for id in platform::PKG_FACTS {
            assert!(FACTS.contains(id), "{id}");
        }
        assert!(["kill", "cd", "history", "read"].iter().all(|word| is_builtin(word)));
    }
}
