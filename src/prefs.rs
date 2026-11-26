//! Persistent user preferences for the unified `huh` pane (`$XDG_STATE_HOME/huh/prefs.toml` or
//! `~/.local/state/huh/prefs.toml`), plus pane mode resolution (`--mode` > `config.toml` > `prefs`).

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum NoiseLevel {
    #[default]
    All,
    New,
    Errors,
}

impl NoiseLevel {
    pub const VALID: &'static str = "all, new, errors";

    pub fn parse(value: &str) -> Result<NoiseLevel> {
        match value.trim().to_ascii_lowercase().as_str() {
            "all" => Ok(NoiseLevel::All),
            "new" => Ok(NoiseLevel::New),
            "errors" => Ok(NoiseLevel::Errors),
            other => bail!("unknown noise level \"{other}\"; valid levels: {}", Self::VALID),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            NoiseLevel::All => "all",
            NoiseLevel::New => "new",
            NoiseLevel::Errors => "errors",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum PaneMode {
    #[default]
    Watch,
    Lab,
}

impl PaneMode {
    pub const VALID: &'static str = "watch, lab";

    pub fn parse(value: &str) -> Result<PaneMode> {
        match value.trim().to_ascii_lowercase().as_str() {
            "watch" => Ok(PaneMode::Watch),
            "lab" => Ok(PaneMode::Lab),
            other => bail!("unknown pane mode \"{other}\"; valid modes: {}", Self::VALID),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            PaneMode::Watch => "watch",
            PaneMode::Lab => "lab",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DefaultMode {
    #[default]
    Watch,
    Lab,
    Last,
}

impl DefaultMode {
    pub const VALID: &'static str = "watch, lab, last";

    pub fn parse(value: &str) -> Result<DefaultMode> {
        match value.trim().to_ascii_lowercase().as_str() {
            "watch" => Ok(DefaultMode::Watch),
            "lab" => Ok(DefaultMode::Lab),
            "last" => Ok(DefaultMode::Last),
            other => bail!("unknown default mode \"{other}\"; valid modes: {}", Self::VALID),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn name(self) -> &'static str {
        match self {
            DefaultMode::Watch => "watch",
            DefaultMode::Lab => "lab",
            DefaultMode::Last => "last",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub theme: Option<String>,
    pub safety: Option<String>,
    pub speed: Option<String>,
    pub pet_style: Option<String>,
    pub quiet: Option<bool>,
    pub noise: Option<String>,
    pub tips: Option<bool>,
    pub last_mode: Option<String>,
    pub miss_log: Option<bool>,
}

impl Prefs {
    pub fn effective_noise(&self) -> NoiseLevel {
        if let Some(level) = self.noise.as_deref().and_then(|s| NoiseLevel::parse(s).ok()) {
            return level;
        }
        if self.quiet == Some(true) { NoiseLevel::New } else { NoiseLevel::All }
    }
}

fn state_dir(xdg_state: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    match xdg_state.filter(|v| !v.is_empty()) {
        Some(dir) => Some(Path::new(dir).join("huh")),
        None => Some(Path::new(home.filter(|v| !v.is_empty())?).join(".local/state/huh")),
    }
}

/// `$XDG_STATE_HOME/huh/prefs.toml`, or `~/.local/state/huh/prefs.toml`.
pub fn state_path(xdg_state: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    Some(state_dir(xdg_state, home)?.join("prefs.toml"))
}

pub fn default_state_path() -> Option<PathBuf> {
    state_path(std::env::var("XDG_STATE_HOME").ok().as_deref(), std::env::var("HOME").ok().as_deref())
}

/// `$XDG_STATE_HOME/huh/seen.txt`, or `~/.local/state/huh/seen.txt`.
pub fn seen_path(xdg_state: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    Some(state_dir(xdg_state, home)?.join("seen.txt"))
}

pub fn default_seen_path() -> Option<PathBuf> {
    seen_path(std::env::var("XDG_STATE_HOME").ok().as_deref(), std::env::var("HOME").ok().as_deref())
}

fn write_atomic(path: &Path, body: &str, label: &str) -> Result<()> {
    let dir = path.parent().context(format!("{label} path has no parent directory"))?;
    if !dir.as_os_str().is_empty() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or(label);
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!("{file_name}.tmp.{}.{seq}", std::process::id()));
    if let Err(err) = std::fs::write(&tmp, body) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err).with_context(|| format!("cannot write {}", tmp.display()));
    }
    if let Err(err) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err).with_context(|| format!("cannot replace {}", path.display()));
    }
    Ok(())
}

pub fn load_from(path: &Path) -> Prefs {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Prefs::default();
    };
    toml::from_str::<Prefs>(&text).unwrap_or_default()
}

pub fn save_to(path: &Path, prefs: &Prefs) -> Result<()> {
    let body = toml::to_string_pretty(prefs).context("cannot serialize prefs")?;
    write_atomic(path, &body, "prefs.toml")
}

pub fn load_default() -> Prefs {
    default_state_path().map_or_else(Prefs::default, |p| load_from(&p))
}

pub fn save_default(prefs: &Prefs) -> Result<()> {
    let path = default_state_path().context("cannot find state directory ($XDG_STATE_HOME or $HOME)")?;
    save_to(&path, prefs)
}

pub fn load_seen(path: &Path) -> HashSet<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashSet::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.chars().any(char::is_whitespace))
        .map(str::to_string)
        .collect()
}

pub fn save_seen(path: &Path, seen: &HashSet<String>) -> Result<()> {
    let mut sorted: Vec<&str> =
        seen.iter().map(String::as_str).filter(|s| !s.is_empty() && !s.chars().any(char::is_whitespace)).collect();
    sorted.sort_unstable();
    sorted.dedup();
    let mut body = sorted.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    write_atomic(path, &body, "seen.txt")
}

/// `default_mode = "watch" | "lab" | "last"` from the text of `config.toml`; other keys are ignored.
pub fn default_mode_from_config(text: &str) -> Result<Option<DefaultMode>> {
    let table: toml::Table = toml::from_str(text).map_err(|e| anyhow::anyhow!("invalid config file: {e}"))?;
    match table.get("default_mode") {
        None => Ok(None),
        Some(toml::Value::String(s)) => DefaultMode::parse(s).map(Some),
        Some(_) => bail!("config file: default_mode must be a string; valid modes: {}", DefaultMode::VALID),
    }
}

/// `noise = "all" | "new" | "errors"` from the text of `config.toml`; other keys are ignored.
pub fn noise_from_config(text: &str) -> Result<Option<NoiseLevel>> {
    let table: toml::Table = toml::from_str(text).map_err(|e| anyhow::anyhow!("invalid config file: {e}"))?;
    match table.get("noise") {
        None => Ok(None),
        Some(toml::Value::String(s)) => NoiseLevel::parse(s).map(Some),
        Some(_) => bail!("config file: noise must be a string; valid levels: {}", NoiseLevel::VALID),
    }
}

/// Resolves the watch noise level: CLI `--noise` > CLI `--quiet` > `config.toml` `noise`
/// > `prefs.noise` > `prefs.quiet` > `NoiseLevel::All`.
pub fn resolve_noise_level(
    cli_noise: Option<NoiseLevel>,
    cli_quiet: bool,
    config_text: Option<&str>,
    prefs: &Prefs,
) -> Result<NoiseLevel> {
    if let Some(level) = cli_noise {
        return Ok(level);
    }
    if cli_quiet {
        return Ok(NoiseLevel::New);
    }
    if let Some(text) = config_text
        && let Some(configured) = noise_from_config(text)?
    {
        return Ok(configured);
    }
    Ok(prefs.effective_noise())
}

/// Resolves the starting pane mode: CLI `--mode` beats `config.toml` `default_mode`,
/// and `default_mode = "last"` uses `prefs.last_mode` (defaulting to `Watch`).
pub fn resolve_initial_pane_mode(
    cli_mode: Option<PaneMode>,
    config_text: Option<&str>,
    prefs: &Prefs,
) -> Result<PaneMode> {
    if let Some(mode) = cli_mode {
        return Ok(mode);
    }
    let configured = match config_text {
        Some(text) => default_mode_from_config(text)?.unwrap_or_default(),
        None => DefaultMode::Watch,
    };
    Ok(match configured {
        DefaultMode::Watch => PaneMode::Watch,
        DefaultMode::Lab => PaneMode::Lab,
        DefaultMode::Last => prefs.last_mode.as_deref().and_then(|s| PaneMode::parse(s).ok()).unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parsing_and_names() {
        assert_eq!(PaneMode::parse("watch").unwrap(), PaneMode::Watch);
        assert_eq!(PaneMode::parse(" LAB ").unwrap(), PaneMode::Lab);
        assert_eq!(PaneMode::Watch.name(), "watch");
        assert_eq!(PaneMode::Lab.name(), "lab");
        assert!(PaneMode::parse("other").unwrap_err().to_string().contains(PaneMode::VALID));

        assert_eq!(DefaultMode::parse("watch").unwrap(), DefaultMode::Watch);
        assert_eq!(DefaultMode::parse("lab").unwrap(), DefaultMode::Lab);
        assert_eq!(DefaultMode::parse("LAST").unwrap(), DefaultMode::Last);
        assert_eq!(DefaultMode::Watch.name(), "watch");
        assert_eq!(DefaultMode::Lab.name(), "lab");
        assert_eq!(DefaultMode::Last.name(), "last");
        assert!(DefaultMode::parse("other").unwrap_err().to_string().contains(DefaultMode::VALID));

        assert_eq!(NoiseLevel::parse("all").unwrap(), NoiseLevel::All);
        assert_eq!(NoiseLevel::parse(" NEW ").unwrap(), NoiseLevel::New);
        assert_eq!(NoiseLevel::parse("Errors").unwrap(), NoiseLevel::Errors);
        assert_eq!(NoiseLevel::All.name(), "all");
        assert_eq!(NoiseLevel::New.name(), "new");
        assert_eq!(NoiseLevel::Errors.name(), "errors");
        assert!(NoiseLevel::parse("loud").unwrap_err().to_string().contains(NoiseLevel::VALID));
    }

    #[test]
    fn state_path_uses_xdg_or_home() {
        assert_eq!(state_path(Some("/s"), Some("/h")).unwrap(), Path::new("/s/huh/prefs.toml"));
        assert_eq!(state_path(None, Some("/h")).unwrap(), Path::new("/h/.local/state/huh/prefs.toml"));
        assert_eq!(state_path(Some(""), Some("/h")).unwrap(), Path::new("/h/.local/state/huh/prefs.toml"));
        assert_eq!(state_path(None, None), None);

        assert_eq!(seen_path(Some("/s"), Some("/h")).unwrap(), Path::new("/s/huh/seen.txt"));
        assert_eq!(seen_path(None, Some("/h")).unwrap(), Path::new("/h/.local/state/huh/seen.txt"));
        assert_eq!(seen_path(None, None), None);
    }

    #[test]
    fn atomic_round_trip_and_corrupt_recovery() {
        let dir = std::env::temp_dir().join(format!("huh-prefs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("prefs.toml");

        assert_eq!(load_from(&path), Prefs::default());

        let prefs = Prefs {
            theme: Some("nord".into()),
            safety: Some("expert".into()),
            speed: Some("fast".into()),
            pet_style: Some("ascii".into()),
            quiet: Some(true),
            noise: Some("errors".into()),
            tips: Some(false),
            last_mode: Some("lab".into()),
            miss_log: Some(true),
        };
        save_to(&path, &prefs).unwrap();
        assert_eq!(load_from(&path), prefs);

        // Legacy prefs.toml lacking `noise` and `miss_log` loads backward-compatibly.
        std::fs::write(&path, "theme = \"nord\"\nquiet = true\n").unwrap();
        let legacy = load_from(&path);
        assert_eq!(legacy.quiet, Some(true));
        assert_eq!(legacy.noise, None);
        assert_eq!(legacy.miss_log, None);
        assert_eq!(legacy.effective_noise(), NoiseLevel::New);

        // Unknown keys from newer versions are ignored without discarding valid prefs.
        std::fs::write(&path, "theme = \"gruvbox\"\nfuture_key = 42\n").unwrap();
        let partial = load_from(&path);
        assert_eq!(partial.theme.as_deref(), Some("gruvbox"));
        assert_eq!(partial.speed, None);
        assert_eq!(partial.tips, None);

        // Corrupt TOML or wrong field type recovers cleanly to defaults.
        std::fs::write(&path, "not valid toml [[[").unwrap();
        assert_eq!(load_from(&path), Prefs::default());
        std::fs::write(&path, "quiet = \"yes\"\n").unwrap();
        assert_eq!(load_from(&path), Prefs::default());
        std::fs::write(&path, "tips = \"yes\"\n").unwrap();
        assert_eq!(load_from(&path), Prefs::default());

        // seen.txt round-trip filters empty/whitespace entries and sorts deterministically.
        let seen_file = dir.join("seen.txt");
        assert!(load_seen(&seen_file).is_empty());
        let seen_set: HashSet<String> = ["ls".into(), "grep".into(), "tar".into()].into();
        save_seen(&seen_file, &seen_set).unwrap();
        assert_eq!(std::fs::read_to_string(&seen_file).unwrap(), "grep\nls\ntar\n");
        assert_eq!(load_seen(&seen_file), seen_set);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn noise_level_precedence_and_errors() {
        let quiet_only = Prefs { quiet: Some(true), ..Prefs::default() };
        let errors_pref = Prefs { quiet: Some(true), noise: Some("errors".into()), ..Prefs::default() };
        let bogus_pref = Prefs { noise: Some("bogus".into()), ..Prefs::default() };

        // CLI --noise > CLI --quiet > config.toml > prefs.noise > prefs.quiet > All.
        assert_eq!(
            resolve_noise_level(Some(NoiseLevel::All), true, Some("noise = \"errors\"\n"), &errors_pref).unwrap(),
            NoiseLevel::All
        );
        assert_eq!(
            resolve_noise_level(None, true, Some("noise = \"errors\"\n"), &Prefs::default()).unwrap(),
            NoiseLevel::New
        );
        assert_eq!(
            resolve_noise_level(None, false, Some("noise = \"errors\"\n"), &quiet_only).unwrap(),
            NoiseLevel::Errors
        );
        assert_eq!(resolve_noise_level(None, false, None, &errors_pref).unwrap(), NoiseLevel::Errors);
        assert_eq!(resolve_noise_level(None, false, None, &quiet_only).unwrap(), NoiseLevel::New);
        assert_eq!(resolve_noise_level(None, false, None, &bogus_pref).unwrap(), NoiseLevel::All);
        assert_eq!(resolve_noise_level(None, false, None, &Prefs::default()).unwrap(), NoiseLevel::All);

        let err =
            resolve_noise_level(None, false, Some("noise = \"loud\"\n"), &Prefs::default()).unwrap_err().to_string();
        assert!(err.contains(NoiseLevel::VALID), "{err}");
        assert!(resolve_noise_level(None, false, Some("noise = 7\n"), &Prefs::default()).is_err());
    }

    #[test]
    fn initial_pane_mode_precedence_and_errors() {
        let lab_pref = Prefs { last_mode: Some("lab".into()), ..Prefs::default() };
        let bad_pref = Prefs { last_mode: Some("bogus".into()), ..Prefs::default() };

        // CLI beats config and prefs.
        assert_eq!(
            resolve_initial_pane_mode(Some(PaneMode::Watch), Some("default_mode = \"lab\"\n"), &lab_pref).unwrap(),
            PaneMode::Watch
        );
        assert_eq!(
            resolve_initial_pane_mode(Some(PaneMode::Lab), Some("default_mode = \"watch\"\n"), &Prefs::default())
                .unwrap(),
            PaneMode::Lab
        );

        // Config watch/lab ignore prefs.last_mode.
        assert_eq!(
            resolve_initial_pane_mode(None, Some("default_mode = \"watch\"\n"), &lab_pref).unwrap(),
            PaneMode::Watch
        );
        assert_eq!(
            resolve_initial_pane_mode(None, Some("default_mode = \"lab\"\n"), &Prefs::default()).unwrap(),
            PaneMode::Lab
        );

        // Config "last" reads prefs.last_mode (and falls back cleanly to Watch if missing or corrupt).
        assert_eq!(
            resolve_initial_pane_mode(None, Some("default_mode = \"last\"\n"), &lab_pref).unwrap(),
            PaneMode::Lab
        );
        assert_eq!(
            resolve_initial_pane_mode(None, Some("default_mode = \"last\"\n"), &Prefs::default()).unwrap(),
            PaneMode::Watch
        );
        assert_eq!(
            resolve_initial_pane_mode(None, Some("default_mode = \"last\"\n"), &bad_pref).unwrap(),
            PaneMode::Watch
        );

        // Missing config or absent key defaults to Watch.
        assert_eq!(resolve_initial_pane_mode(None, None, &lab_pref).unwrap(), PaneMode::Watch);
        assert_eq!(resolve_initial_pane_mode(None, Some("theme = \"nord\"\n"), &lab_pref).unwrap(), PaneMode::Watch);

        // Invalid config value errors clearly.
        let err =
            resolve_initial_pane_mode(None, Some("default_mode = \"auto\"\n"), &lab_pref).unwrap_err().to_string();
        assert!(err.contains(DefaultMode::VALID), "{err}");
        assert!(resolve_initial_pane_mode(None, Some("default_mode = 7\n"), &lab_pref).is_err());
        assert!(resolve_initial_pane_mode(None, Some("not toml ["), &lab_pref).is_err());
    }
}
