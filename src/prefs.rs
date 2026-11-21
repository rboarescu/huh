//! Persistent user preferences for the unified `huh` pane (`$XDG_STATE_HOME/huh/prefs.toml` or
//! `~/.local/state/huh/prefs.toml`), plus pane mode resolution (`--mode` > `config.toml` > `prefs`).

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

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
    pub tips: Option<bool>,
    pub last_mode: Option<String>,
}

/// `$XDG_STATE_HOME/huh/prefs.toml`, or `~/.local/state/huh/prefs.toml`.
pub fn state_path(xdg_state: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    match xdg_state.filter(|v| !v.is_empty()) {
        Some(dir) => Some(Path::new(dir).join("huh/prefs.toml")),
        None => Some(Path::new(home.filter(|v| !v.is_empty())?).join(".local/state/huh/prefs.toml")),
    }
}

pub fn default_state_path() -> Option<PathBuf> {
    state_path(std::env::var("XDG_STATE_HOME").ok().as_deref(), std::env::var("HOME").ok().as_deref())
}

pub fn load_from(path: &Path) -> Prefs {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Prefs::default();
    };
    toml::from_str::<Prefs>(&text).unwrap_or_default()
}

pub fn save_to(path: &Path, prefs: &Prefs) -> Result<()> {
    let dir = path.parent().context("prefs path has no parent directory")?;
    if !dir.as_os_str().is_empty() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let body = toml::to_string_pretty(prefs).context("cannot serialize prefs")?;
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("prefs.toml");
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

pub fn load_default() -> Prefs {
    default_state_path().map_or_else(Prefs::default, |p| load_from(&p))
}

pub fn save_default(prefs: &Prefs) -> Result<()> {
    let path = default_state_path().context("cannot find state directory ($XDG_STATE_HOME or $HOME)")?;
    save_to(&path, prefs)
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
    }

    #[test]
    fn state_path_uses_xdg_or_home() {
        assert_eq!(state_path(Some("/s"), Some("/h")).unwrap(), Path::new("/s/huh/prefs.toml"));
        assert_eq!(state_path(None, Some("/h")).unwrap(), Path::new("/h/.local/state/huh/prefs.toml"));
        assert_eq!(state_path(Some(""), Some("/h")).unwrap(), Path::new("/h/.local/state/huh/prefs.toml"));
        assert_eq!(state_path(None, None), None);
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
            tips: Some(false),
            last_mode: Some("lab".into()),
        };
        save_to(&path, &prefs).unwrap();
        assert_eq!(load_from(&path), prefs);

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

        let _ = std::fs::remove_dir_all(&dir);
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
