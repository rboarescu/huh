//! Normal or expert mode. Expert mode relaxes the beginner protections (the evidence needed for
//! a dangerous suggestion, the broad-target drop, the state-question action ban); warnings and
//! refusals never change.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Normal,
    Expert,
}

pub const VALID: &str = "normal, expert";

impl Mode {
    pub fn parse(value: &str) -> Result<Mode> {
        match value.trim().to_ascii_lowercase().as_str() {
            "normal" => Ok(Mode::Normal),
            "expert" => Ok(Mode::Expert),
            other => bail!("unknown mode \"{other}\"; the valid modes are: {VALID}"),
        }
    }

    pub fn is_expert(self) -> bool {
        self == Mode::Expert
    }
}

/// `mode = "expert"` from the text of a config file; other keys are ignored.
pub fn from_config_text(text: &str) -> Result<Option<Mode>> {
    let table: toml::Table = toml::from_str(text).map_err(|e| anyhow::anyhow!("invalid config file: {e}"))?;
    match table.get("mode") {
        None => Ok(None),
        Some(toml::Value::String(s)) => Mode::parse(s).map(Some),
        Some(_) => bail!("config file: mode must be a string; the valid modes are: {VALID}"),
    }
}

pub fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("huh").join("config.toml"))
}

/// Flags beat the environment, the environment beats the file, the file beats the default.
/// `--normal` and `--expert` given together is an error.
pub fn resolve(expert_flag: bool, normal_flag: bool, env: Option<&str>, file: Option<&Path>) -> Result<Mode> {
    if expert_flag && normal_flag {
        bail!("--expert and --normal cannot be used together");
    }
    if expert_flag {
        return Ok(Mode::Expert);
    }
    if normal_flag {
        return Ok(Mode::Normal);
    }
    if let Some(value) = env.filter(|v| !v.trim().is_empty()) {
        return Mode::parse(value).map_err(|e| anyhow::anyhow!("HUH_MODE: {e}"));
    }
    if let Some(path) = file
        && let Ok(text) = std::fs::read_to_string(path)
    {
        return Ok(from_config_text(&text)
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?
            .unwrap_or_default());
    }
    Ok(Mode::Normal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(name: &str, text: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(".scratch").join("mode-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn default_is_normal() {
        assert_eq!(resolve(false, false, None, None).unwrap(), Mode::Normal);
        let missing = Path::new(env!("CARGO_MANIFEST_DIR")).join(".scratch/no-such-config.toml");
        assert_eq!(resolve(false, false, None, Some(&missing)).unwrap(), Mode::Normal);
    }

    #[test]
    fn file_sets_the_mode() {
        let file = config("expert.toml", "mode = \"expert\"\n");
        assert_eq!(resolve(false, false, None, Some(&file)).unwrap(), Mode::Expert);
    }

    #[test]
    fn env_beats_file() {
        let file = config("expert2.toml", "mode = \"expert\"\n");
        assert_eq!(resolve(false, false, Some("normal"), Some(&file)).unwrap(), Mode::Normal);
        let file = config("normal.toml", "mode = \"normal\"\n");
        assert_eq!(resolve(false, false, Some("expert"), Some(&file)).unwrap(), Mode::Expert);
        assert_eq!(resolve(false, false, Some(""), Some(&file)).unwrap(), Mode::Normal);
    }

    #[test]
    fn flag_beats_env_and_file() {
        let file = config("expert3.toml", "mode = \"expert\"\n");
        assert_eq!(resolve(false, true, Some("expert"), Some(&file)).unwrap(), Mode::Normal);
        let file = config("normal2.toml", "mode = \"normal\"\n");
        assert_eq!(resolve(true, false, Some("normal"), Some(&file)).unwrap(), Mode::Expert);
        assert!(resolve(true, true, None, None).is_err());
    }

    #[test]
    fn unknown_values_name_the_valid_ones() {
        let err = resolve(false, false, Some("wizard"), None).unwrap_err().to_string();
        assert!(err.contains("normal, expert") && err.contains("wizard"), "{err}");
        let file = config("bad.toml", "mode = \"wizard\"\n");
        let err = resolve(false, false, None, Some(&file)).unwrap_err().to_string();
        assert!(err.contains("normal, expert"), "{err}");
        let file = config("bad2.toml", "mode = 3\n");
        assert!(resolve(false, false, None, Some(&file)).is_err());
        // Other keys are ignored.
        assert_eq!(from_config_text("other = 1\n").unwrap(), None);
    }
}
