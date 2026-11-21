//! Rotating, practical header tips for the `huh` companion pane and `tips` config precedence.

use anyhow::{Result, bail};

/// Target wall-clock cadence (seconds) between header tip rotations.
#[cfg_attr(not(test), allow(dead_code))]
pub const ROTATE_SECONDS: u64 = 15;

/// Event-loop ticks (~220 ms each) between header tip rotations (`68 * 220 ms ≈ 15 s`).
pub const ROTATE_TICKS: u64 = 68;

/// Short, practical tips rotated in the companion-pane header (each <= 74 chars).
pub const TIPS: &[&str] = &[
    "Type / to browse slash commands and fuzzy-match any setting.",
    "Press Tab to complete a slash command or step into its options.",
    "Use ↑/↓ to browse input history or move through selectors.",
    "Switch to /mode lab for interactive Linux lessons and sandboxes.",
    "Preview or switch colour themes live with /theme (e.g. /theme nord).",
    "Run /pet customize to export an editable 32x32 sprite sheet.",
    "Toggle beginner guardrails anytime with /safety normal or /safety expert.",
    "Adjust typewriter speed with /speed fast, slow, off, or ms.",
    "Use /quiet on to stop repeating explanations for seen programs.",
    "Search the command catalog and $PATH tools with /find or ?words.",
    "Hide or cycle these header tips anytime with /tips off or /tips next.",
    "In your shell, Alt+h or Alt+f turns plain words into a command.",
    "Press Alt+e on a shell command to explain its flags before running.",
    "Pipe failing command output directly: cmd 2>&1 | huh explains why.",
    "Run huh setup to install bash, zsh, or fish recording and key hooks.",
    "Press Enter on an empty line to replay the last watched command.",
    "Type why to diagnose why the last watched command exited non-zero.",
    "Nothing typed here is ever executed; huh only reads and explains.",
];

/// Returns the tip at `index`, wrapping seamlessly across `TIPS`.
pub fn tip_at(index: usize) -> &'static str {
    TIPS[index % TIPS.len()]
}

/// Number of built-in companion-pane tips.
#[cfg_attr(not(test), allow(dead_code))]
pub fn count() -> usize {
    TIPS.len()
}

/// `tips = true | false` from the text of `config.toml`; other keys are ignored.
pub fn tips_from_config(text: &str) -> Result<Option<bool>> {
    let table: toml::Table = toml::from_str(text).map_err(|e| anyhow::anyhow!("invalid config file: {e}"))?;
    match table.get("tips") {
        None => Ok(None),
        Some(toml::Value::Boolean(on)) => Ok(Some(*on)),
        Some(_) => bail!("config file: tips must be a boolean (true or false)"),
    }
}

/// Resolves whether header tips are enabled: `env_no_tips` > `config.toml` > `prefs.toml` > `true`.
pub fn resolve_tips_enabled(env_no_tips: bool, config_text: Option<&str>, pref_tips: Option<bool>) -> Result<bool> {
    if env_no_tips {
        return Ok(false);
    }
    if let Some(text) = config_text
        && let Some(configured) = tips_from_config(text)?
    {
        return Ok(configured);
    }
    Ok(pref_tips.unwrap_or(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tips_are_non_empty_single_line_within_width_and_wrap() {
        assert_eq!(count(), TIPS.len());
        assert!(count() >= 18);
        assert_eq!(ROTATE_SECONDS, 15);
        assert_eq!(ROTATE_TICKS, 68);

        for (idx, tip) in TIPS.iter().enumerate() {
            assert!(!tip.trim().is_empty(), "tip {idx} is empty");
            assert!(!tip.contains('\n') && !tip.contains('\r'), "tip {idx} is multi-line: {tip:?}");
            assert!(!tip.contains('\x1b'), "tip {idx} contains escape sequence");
            assert!(tip.chars().count() <= 74, "tip {idx} exceeds 74 chars ({}): {tip}", tip.chars().count());
            assert_eq!(tip_at(idx), *tip);
            assert_eq!(tip_at(idx + TIPS.len()), *tip);
        }

        let joined = TIPS.join("\n").to_ascii_lowercase();
        for topic in [
            "/",
            "tab",
            "↑/↓",
            "/mode lab",
            "/theme",
            "/pet customize",
            "/safety",
            "/speed",
            "/quiet",
            "/find",
            "/tips off",
            "alt+h",
            "alt+f",
            "alt+e",
            "| huh",
            "huh setup",
            "enter",
            "why",
            "ever executed",
        ] {
            assert!(joined.contains(&topic.to_ascii_lowercase()), "missing topic `{topic}` in TIPS");
        }
        assert!(joined.contains("nothing typed here is ever executed"));
    }

    #[test]
    fn config_parsing_handles_booleans_absent_keys_and_errors() {
        assert_eq!(tips_from_config("").unwrap(), None);
        assert_eq!(tips_from_config("theme = \"nord\"\n").unwrap(), None);
        assert_eq!(tips_from_config("tips = true\n").unwrap(), Some(true));
        assert_eq!(tips_from_config("tips = false\n").unwrap(), Some(false));

        let err = tips_from_config("tips = \"off\"\n").unwrap_err().to_string();
        assert!(err.contains("boolean"), "{err}");
        assert!(tips_from_config("tips = 0\n").is_err());
        assert!(tips_from_config("not valid toml [[[").is_err());
    }

    #[test]
    fn precedence_env_beats_config_beats_prefs_beats_default() {
        // Default is true when nothing is set.
        assert!(resolve_tips_enabled(false, None, None).unwrap());

        // Prefs alone overrides default.
        assert!(!resolve_tips_enabled(false, None, Some(false)).unwrap());
        assert!(resolve_tips_enabled(false, None, Some(true)).unwrap());

        // Config without `tips` falls back to prefs.
        assert!(!resolve_tips_enabled(false, Some("theme = \"nord\"\n"), Some(false)).unwrap());

        // Config `tips` overrides prefs in both directions.
        assert!(resolve_tips_enabled(false, Some("tips = true\n"), Some(false)).unwrap());
        assert!(!resolve_tips_enabled(false, Some("tips = false\n"), Some(true)).unwrap());

        // Env beats config and prefs, even if config would enable or error.
        assert!(!resolve_tips_enabled(true, Some("tips = true\n"), Some(true)).unwrap());
        assert!(!resolve_tips_enabled(true, Some("not valid toml [[["), Some(true)).unwrap());

        // Invalid config still surfaces when env_no_tips is false.
        assert!(resolve_tips_enabled(false, Some("tips = \"nope\"\n"), Some(true)).is_err());
    }
}
