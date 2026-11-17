//! The command catalog: one TOML file per command.

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
    /// Set for commands that only exist on one family of systems: "arch",
    /// "debian", "fedora", "omarchy". Matched against /etc/os-release.
    pub platform: Option<String>,
    /// Package name when it differs from `program()`.
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
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct SlotOption {
    pub name: String,
    pub value: String,
    pub description: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Flag {
    pub id: String,
    pub value: String,
    /// Yes/no question, phrased so that "yes" means the flag is wanted. Its words
    /// help matching, and it is shown (in the second person) when huh asks or notes.
    pub question: String,
}

impl Command {
    /// The program the template runs, looking past `sudo`.
    pub fn program(&self) -> &str {
        self.template.split_whitespace().find(|word| *word != "sudo").unwrap_or_default()
    }
}

/// Placeholders that code fills in from the machine, not from the request.
pub const FACTS: &[&str] = &["shell_rc"];

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

/// Shell built-ins have no file on PATH but are always there.
const BUILTINS: &[&str] = &["cd", "history", "pwd", "echo", "alias", "export", "source", "type", "jobs", "fg", "bg"];

const INSTALLERS: &[(&str, &str)] = &[
    ("omarchy", "omarchy pkg add"),
    ("arch", "sudo pacman -S"),
    ("debian", "sudo apt install"),
    ("fedora", "sudo dnf install"),
];

pub fn installed(program: &str) -> bool {
    if BUILTINS.contains(&program) {
        return true;
    }
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// Suggests an install command for the program behind `command`, or `None` for
/// shell built-ins and unrecognized platforms.
pub fn install_command(command: &Command, platforms: &[String]) -> Option<String> {
    let program = command.program();
    if program.is_empty() || BUILTINS.contains(&program) {
        return None;
    }
    let package = command.package.as_deref().unwrap_or(program);
    INSTALLERS
        .iter()
        .find(|(platform, _)| platforms.iter().any(|p| p == platform))
        .map(|(_, prefix)| format!("{prefix} {package}"))
}

/// The `ID` and `ID_LIKE` words of an os-release file, e.g. ["omarchy", "arch"].
pub fn platforms_from(os_release: &str) -> Vec<String> {
    os_release
        .lines()
        .filter_map(|line| line.strip_prefix("ID=").or_else(|| line.strip_prefix("ID_LIKE=")))
        .flat_map(|value| value.trim_matches(['"', '\'']).split_whitespace())
        .map(str::to_string)
        .collect()
}

pub fn this_platform() -> Vec<String> {
    platforms_from(&std::fs::read_to_string("/etc/os-release").unwrap_or_default())
}

/// Keeps the commands that make sense here: right platform and, when
/// `only_installed` is set, a program that is actually on PATH.
pub fn usable(commands: Vec<Command>, platforms: &[String], only_installed: bool) -> Vec<Command> {
    commands
        .into_iter()
        .filter(|c| c.platform.as_ref().is_none_or(|p| platforms.contains(p)))
        .filter(|c| !only_installed || installed(c.program()))
        .collect()
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
    Ok(commands)
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
    Ok(commands)
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
    let used: BTreeSet<&str> = placeholders(&command.template).into_iter().filter(|id| !FACTS.contains(id)).collect();
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
    fn reads_platforms_from_os_release() {
        assert_eq!(platforms_from("NAME=\"Omarchy\"\nID=omarchy\nID_LIKE=arch\n"), ["omarchy", "arch"]);
        assert_eq!(platforms_from("ID=ubuntu\nID_LIKE=\"debian\"\n"), ["ubuntu", "debian"]);
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
    fn builds_install_command_for_platform_and_package() {
        let mut smart = command("sudo smartctl -a {disk}", None);
        smart.package = Some("smartmontools".into());
        assert_eq!(
            install_command(&smart, &["arch".into(), "omarchy".into()]),
            Some("omarchy pkg add smartmontools".into())
        );
        assert_eq!(
            install_command(&smart, &["omarchy".into(), "arch".into()]),
            Some("omarchy pkg add smartmontools".into())
        );
        assert_eq!(install_command(&smart, &["arch".into()]), Some("sudo pacman -S smartmontools".into()));
        assert_eq!(
            install_command(&smart, &["ubuntu".into(), "debian".into()]),
            Some("sudo apt install smartmontools".into())
        );
        assert_eq!(install_command(&smart, &["fedora".into()]), Some("sudo dnf install smartmontools".into()));
        assert_eq!(install_command(&smart, &["unknown".into()]), None);

        let plain = command("rg {pattern}", None);
        assert_eq!(install_command(&plain, &["arch".into()]), Some("sudo pacman -S rg".into()));

        let builtin = command("cd {target}", None);
        assert_eq!(install_command(&builtin, &["omarchy".into(), "arch".into()]), None);
        let echo_cmd = command("echo 'x' >> {shell_rc}", None);
        assert_eq!(install_command(&echo_cmd, &["debian".into()]), None);
    }
}
