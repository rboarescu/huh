//! Guided Omarchy-style setup (`huh setup`): configures theme, pane defaults, pet rendering,
//! typewriter speed, rotating header tips, and shell hooks without running any external commands
//! or altering existing `config.toml` files.

use crate::prefs::{self, Prefs};
use crate::session::{self, ShellKind};
use crate::tui::{self, SelectItem, SelectOutcome};
use anyhow::{Context, Result};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const RECORD_START: &str = "# >>> huh shell-init (record) >>>";
const RECORD_END: &str = "# <<< huh shell-init (record) <<<";
const HOOKS_START: &str = "# >>> huh shell-init (hooks) >>>";
const HOOKS_END: &str = "# <<< huh shell-init (hooks) <<<";

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Inputs for `huh setup` (purely injectable for deterministic testing).
pub struct SetupConfig<'a> {
    pub home: Option<&'a Path>,
    pub xdg_config: Option<&'a Path>,
    pub xdg_state: Option<&'a Path>,
    pub shell_env: Option<&'a str>,
    pub env: &'a dyn Fn(&str) -> Option<String>,
    pub interactive: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KittySupport {
    Supported(&'static str),
    ForeignTmux(&'static str),
    NotDetected,
}

fn detect_kitty(env: &dyn Fn(&str) -> Option<String>) -> KittySupport {
    let nonempty = |k: &str| env(k).is_some_and(|v| !v.trim().is_empty());
    let term_prog = env("TERM_PROGRAM").unwrap_or_default().trim().to_ascii_lowercase();
    let term = env("TERM").unwrap_or_default().trim().to_ascii_lowercase();
    let hint = if nonempty("GHOSTTY_RESOURCES_DIR") || term_prog == "ghostty" || term.contains("ghostty") {
        Some("Ghostty")
    } else if nonempty("KITTY_WINDOW_ID") || term_prog == "kitty" || term.contains("kitty") {
        Some("Kitty")
    } else if term_prog == "wezterm" {
        Some("WezTerm")
    } else {
        None
    };
    let Some(hint) = hint else {
        return KittySupport::NotDetected;
    };
    if let Some(tmux) = env("TMUX").filter(|v| !v.trim().is_empty()) {
        let socket = tmux.split(',').next().unwrap_or("");
        let socket_name = Path::new(socket).file_name().and_then(|n| n.to_str()).unwrap_or("");
        let forced = env("HUH_KITTY_GRAPHICS").is_some_and(|v| v.trim() == "1");
        if socket_name == "huh" || forced { KittySupport::Supported(hint) } else { KittySupport::ForeignTmux(hint) }
    } else {
        KittySupport::Supported(hint)
    }
}

fn kitty_summary(support: KittySupport) -> String {
    match support {
        KittySupport::Supported(term) => {
            format!("supported ({term} — 32x32 pixel sprites ready; auto recommended)")
        }
        KittySupport::ForeignTmux(term) => {
            format!("paused in outer tmux ({term} — ASCII here, 32x32 pixels in huh's own tmux; auto recommended)")
        }
        KittySupport::NotDetected => "not detected (using 4x10 ASCII pet fallback; auto recommended)".to_string(),
    }
}

fn clean_path(path: Option<&Path>) -> Option<&Path> {
    path.filter(|p| !p.as_os_str().is_empty())
}

fn state_dir(cfg: &SetupConfig<'_>) -> Option<PathBuf> {
    if let Some(xdg) = clean_path(cfg.xdg_state) {
        return Some(xdg.join("huh"));
    }
    clean_path(cfg.home).map(|h| h.join(".local/state/huh"))
}

fn config_dir(cfg: &SetupConfig<'_>) -> Option<PathBuf> {
    if let Some(xdg) = clean_path(cfg.xdg_config) {
        return Some(xdg.join("huh"));
    }
    clean_path(cfg.home).map(|h| h.join(".config/huh"))
}

fn resolve_rc_path(cfg: &SetupConfig<'_>, shell: ShellKind) -> Option<PathBuf> {
    let zdotdir = (cfg.env)("ZDOTDIR").filter(|s| !s.trim().is_empty()).map(PathBuf::from);
    if shell == ShellKind::Fish
        && let Some(xdg) = clean_path(cfg.xdg_config)
    {
        return Some(xdg.join("fish/config.fish"));
    }
    if shell == ShellKind::Zsh
        && let Some(dir) = &zdotdir
    {
        return Some(dir.join(".zshrc"));
    }
    clean_path(cfg.home).map(|h| session::rc_path_with_zdotdir(shell, cfg.xdg_config, zdotdir.as_deref(), h))
}

fn write_atomic(path: &Path, body: &str) -> Result<()> {
    let dir = path.parent().context("target path has no parent directory")?;
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{file_name}.tmp.{stamp}.{seq}"));
    std::fs::write(&tmp, body).with_context(|| format!("cannot write {}", tmp.display()))?;
    if let Err(err) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err).with_context(|| format!("cannot replace {}", path.display()));
    }
    Ok(())
}

fn prompt_choice_tty<W: Write>(writer: &mut W, title: &str, options: &[(&str, &str)]) -> Result<Option<String>> {
    let items: Vec<SelectItem> = options
        .iter()
        .enumerate()
        .map(|(idx, (_val, raw_label))| {
            let (label, detail) = match raw_label.split_once(" — ") {
                Some((l, d)) => (l.trim().to_string(), d.trim().to_string()),
                None => ((*raw_label).to_string(), String::new()),
            };
            let detail = if idx == 0 {
                if detail.is_empty() { "Recommended".to_string() } else { format!("{detail} (Recommended)") }
            } else {
                detail
            };
            SelectItem { key: (idx + 1).to_string(), label, detail, enabled: true }
        })
        .collect();
    match tui::select_menu(title, &items, 0, "↑/↓ move · Enter select · q/Esc cancel", &mut || None)? {
        SelectOutcome::Picked(idx) => {
            writeln!(writer, "\n{title} -> {}", options[idx].0)?;
            Ok(Some(options[idx].0.to_string()))
        }
        SelectOutcome::Cancelled | SelectOutcome::Eof | SelectOutcome::Slash(_) => Ok(None),
    }
}

fn prompt_choice_step<R: BufRead, W: Write>(
    use_tty: bool,
    reader: &mut R,
    writer: &mut W,
    title: &str,
    options: &[(&str, &str)],
    aliases: &[(&str, &str)],
) -> Result<Option<String>> {
    if use_tty {
        return prompt_choice_tty(writer, title, options);
    }
    prompt_choice(reader, writer, title, options, aliases).map(Some)
}

fn prompt_yes_no_step<R: BufRead, W: Write>(
    use_tty: bool,
    reader: &mut R,
    writer: &mut W,
    question: &str,
) -> Result<Option<bool>> {
    if use_tty {
        let items = [
            SelectItem {
                key: "y".into(),
                label: "Yes — update shell rc file".into(),
                detail: String::new(),
                enabled: true,
            },
            SelectItem {
                key: "n".into(),
                label: "No — leave rc file untouched".into(),
                detail: "default".into(),
                enabled: true,
            },
        ];
        return match tui::select_menu(question, &items, 1, "↑/↓ move · y/n or Enter · Esc skip", &mut || None)? {
            SelectOutcome::Picked(0) => {
                writeln!(writer, "{question} -> Yes")?;
                Ok(Some(true))
            }
            SelectOutcome::Picked(_) | SelectOutcome::Cancelled => {
                writeln!(writer, "{question} -> No")?;
                Ok(Some(false))
            }
            SelectOutcome::Eof | SelectOutcome::Slash(_) => Ok(None),
        };
    }
    prompt_yes_no(reader, writer, question).map(Some)
}

fn prompt_choice<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    title: &str,
    options: &[(&str, &str)],
    aliases: &[(&str, &str)],
) -> Result<String> {
    writeln!(writer, "\n{title}")?;
    for (idx, (value, label)) in options.iter().enumerate() {
        let num = idx + 1;
        if idx == 0 {
            writeln!(writer, "  [{num}] {label} (Recommended)")?;
        } else {
            writeln!(writer, "  [{num}] {label}")?;
        }
        let _ = value;
    }
    loop {
        write!(writer, "Choice [1]: ")?;
        writer.flush()?;
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Ok(options[0].0.to_string());
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Ok(options[0].0.to_string());
        }
        if let Ok(num) = trimmed.parse::<usize>()
            && (1..=options.len()).contains(&num)
        {
            return Ok(options[num - 1].0.to_string());
        }
        let lower = trimmed.to_ascii_lowercase();
        if let Some((value, _)) = options.iter().find(|(v, _)| *v == lower) {
            return Ok((*value).to_string());
        }
        if let Some((_, value)) = aliases.iter().find(|(alias, _)| *alias == lower) {
            return Ok((*value).to_string());
        }
        writeln!(writer, "  Enter 1-{} (or press Enter for [1]):", options.len())?;
    }
}

fn prompt_yes_no<R: BufRead, W: Write>(reader: &mut R, writer: &mut W, question: &str) -> Result<bool> {
    loop {
        write!(writer, "{question} [y/N] ")?;
        writer.flush()?;
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Ok(false);
        }
        match line.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => return Ok(true),
            "" | "n" | "no" => return Ok(false),
            _ => writeln!(writer, "  Please answer y or n (Enter for No).")?,
        }
    }
}

fn marked_block(start: &str, body: &str, end: &str) -> Vec<String> {
    vec![start.to_string(), body.to_string(), end.to_string()]
}

/// Replaces existing marked block (removing any duplicate blocks) or matching bare line.
/// Returns `(lines, replaced)`.
fn replace_block_or_bare(
    lines: Vec<String>,
    start: &str,
    end: &str,
    bare_line: &str,
    block: &[String],
) -> (Vec<String>, bool) {
    let mut out = Vec::with_capacity(lines.len() + 4);
    let mut replaced = false;
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == start
            && let Some(rel_end) = lines[i + 1..].iter().position(|l| l.trim() == end)
        {
            if !replaced {
                out.extend(block.iter().cloned());
                replaced = true;
            }
            i += rel_end + 2;
            continue;
        }
        out.push(lines[i].clone());
        i += 1;
    }
    if replaced {
        return (out, true);
    }

    let mut upgraded = Vec::with_capacity(out.len() + 2);
    for line in out {
        if !replaced && line.trim() == bare_line {
            upgraded.extend(block.iter().cloned());
            replaced = true;
        } else {
            upgraded.push(line);
        }
    }
    (upgraded, replaced)
}

fn is_guard_if_line(lower: &str, shell: ShellKind) -> bool {
    match shell {
        ShellKind::Bash | ShellKind::Zsh => {
            lower.starts_with("if ")
                && (lower.contains("*i*") || lower.contains("-o interactive") || lower.contains("$ps1"))
        }
        ShellKind::Fish => {
            lower.starts_with("if ") && (lower.contains("is-interactive") || lower.contains("--is-interactive"))
        }
    }
}

fn is_single_line_guard(lower: &str, shell: ShellKind) -> bool {
    match shell {
        ShellKind::Bash | ShellKind::Zsh => {
            let has_check = lower.contains("*i*") || lower.contains("-o interactive") || lower.contains("$ps1");
            let has_exit = lower.contains("return") || lower.contains("exit");
            has_check && has_exit && !lower.starts_with("case ")
        }
        ShellKind::Fish => {
            (lower.contains("is-interactive") || lower.contains("--is-interactive"))
                && (lower.contains("return") || lower.contains("exit") || lower.starts_with("if "))
        }
    }
}

fn insert_index_after_guard(lines: &[String], shell: ShellKind) -> usize {
    let mut i = 0;
    if lines.first().is_some_and(|l| l.starts_with("#!")) {
        i = 1;
    }
    let scan_limit = lines.len().min(60);
    while i < scan_limit {
        let trimmed = lines[i].trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1;
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if matches!(shell, ShellKind::Bash | ShellKind::Zsh) && lower.starts_with("case ") && lower.contains("$-") {
            if let Some(rel) = lines[i + 1..scan_limit].iter().position(|l| l.trim().eq_ignore_ascii_case("esac")) {
                return i + rel + 2;
            }
            return i + 1;
        }
        if is_guard_if_line(&lower, shell) {
            if shell == ShellKind::Fish
                && !lower.contains("not ")
                && !lower.contains("return")
                && !lower.contains("exit")
            {
                // Fish wraps the interactive rc body in `if status is-interactive ... end`:
                // place the record line right after the opening `if` so it runs inside the block.
                return i + 1;
            }
            let closer = if shell == ShellKind::Fish { "end" } else { "fi" };
            if lower.split_whitespace().any(|w| w == closer || w == format!("{closer};")) {
                return i + 1;
            }
            if let Some(rel) =
                lines[i + 1..scan_limit].iter().position(|l| l.split_whitespace().next().is_some_and(|w| w == closer))
            {
                return i + rel + 2;
            }
            return i + 1;
        }
        if is_single_line_guard(&lower, shell) {
            return i + 1;
        }
        // Keep scanning top header comments / blank lines; stop after non-guard code.
        break;
    }
    if lines.first().is_some_and(|l| l.starts_with("#!")) { 1 } else { 0 }
}

fn insert_block_at(lines: Vec<String>, idx: usize, block: &[String]) -> Vec<String> {
    let idx = idx.min(lines.len());
    let mut out = Vec::with_capacity(lines.len() + block.len() + 2);
    out.extend_from_slice(&lines[..idx]);
    if idx > 0 && out.last().is_some_and(|l| !l.trim().is_empty()) {
        out.push(String::new());
    }
    out.extend(block.iter().cloned());
    if idx < lines.len() && !lines[idx].trim().is_empty() {
        out.push(String::new());
    }
    out.extend_from_slice(&lines[idx..]);
    out
}

fn append_block_at_end(mut lines: Vec<String>, block: &[String]) -> Vec<String> {
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    if !lines.is_empty() {
        lines.push(String::new());
    }
    lines.extend(block.iter().cloned());
    lines
}

/// Inserts or updates the marked `(record)` and `(hooks)` blocks idempotently in `existing`.
pub fn upsert_rc_blocks(existing: &str, shell: ShellKind) -> String {
    let (record_line, hooks_line) = session::init_lines(shell);
    let record_block = marked_block(RECORD_START, record_line, RECORD_END);
    let hooks_block = marked_block(HOOKS_START, hooks_line, HOOKS_END);

    let initial_lines: Vec<String> =
        if existing.is_empty() { Vec::new() } else { existing.lines().map(str::to_string).collect() };

    let (lines, had_record) =
        replace_block_or_bare(initial_lines, RECORD_START, RECORD_END, record_line, &record_block);
    let (mut lines, had_hooks) = replace_block_or_bare(lines, HOOKS_START, HOOKS_END, hooks_line, &hooks_block);

    if !had_record {
        let idx = insert_index_after_guard(&lines, shell);
        lines = insert_block_at(lines, idx, &record_block);
    }
    if !had_hooks {
        lines = append_block_at_end(lines, &hooks_block);
    }

    let mut rendered = lines.join("\n");
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    rendered
}

fn has_rc_init(rc_path: Option<&Path>, shell: ShellKind) -> bool {
    let Some(path) = rc_path else { return false };
    let Ok(text) = std::fs::read_to_string(path) else { return false };
    let (record_line, hooks_line) = session::init_lines(shell);
    text.contains(record_line) && text.contains(hooks_line)
}

/// Runs `huh setup` using injectable environment, paths, and I/O streams.
#[cfg_attr(not(test), allow(dead_code))]
pub fn run_with_io<R: BufRead, W: Write>(cfg: &SetupConfig<'_>, reader: &mut R, writer: &mut W) -> Result<()> {
    run_with_io_mode(cfg, reader, writer, false)
}

fn run_with_io_mode<R: BufRead, W: Write>(
    cfg: &SetupConfig<'_>,
    reader: &mut R,
    writer: &mut W,
    use_tty: bool,
) -> Result<()> {
    let shell = session::detect_shell(cfg.shell_env);
    let rc_path = resolve_rc_path(cfg, shell);
    let kitty = detect_kitty(cfg.env);
    let prefs_path = state_dir(cfg).map(|d| d.join("prefs.toml"));
    let config_path = config_dir(cfg).map(|d| d.join("config.toml"));
    let (record_line, hooks_line) = session::init_lines(shell);

    writeln!(writer, "huh setup")?;
    writeln!(writer, "  Shell:          {}", shell.name())?;
    writeln!(
        writer,
        "  RC file:        {}",
        rc_path.as_ref().map_or_else(|| "(unavailable: HOME not set)".to_string(), |p| p.display().to_string())
    )?;
    writeln!(writer, "  Kitty graphics: {}", kitty_summary(kitty))?;

    if !cfg.interactive {
        let existing_prefs = prefs_path.as_deref().map(prefs::load_from);
        let hooks_state = if has_rc_init(rc_path.as_deref(), shell) { "installed" } else { "not installed" };
        writeln!(writer, "  Shell hooks:    {hooks_state}")?;
        if let Some(p) = &prefs_path {
            if let Some(prefs) = existing_prefs.filter(|p| *p != Prefs::default()) {
                writeln!(
                    writer,
                    "  Preferences:    {} (theme={}, pet_style={}, speed={}, tips={})",
                    p.display(),
                    prefs.theme.as_deref().unwrap_or("babyblue"),
                    prefs.pet_style.as_deref().unwrap_or("auto"),
                    prefs.speed.as_deref().unwrap_or("normal"),
                    if prefs.tips.unwrap_or(true) { "on" } else { "off" }
                )?;
            } else {
                writeln!(writer, "  Preferences:    {} (not saved yet)", p.display())?;
            }
        }
        if let Some(c) = &config_path {
            writeln!(writer, "  Config file:    {}", c.display())?;
        }
        writeln!(writer, "\nNon-interactive session detected. Run `huh setup` in a terminal for guided prompts,")?;
        writeln!(writer, "or add these lines to your {}:", shell.name())?;
        writeln!(writer, "  {record_line}")?;
        writeln!(writer, "  {hooks_line}")?;
        return Ok(());
    }

    let Some(default_mode) = prompt_choice_step(
        use_tty,
        reader,
        writer,
        "1. Default pane mode when launching `huh`:",
        &[
            ("watch", "watch — live command explanations as you work"),
            ("lab", "lab   — interactive lessons & hands-on practice"),
            ("last", "last  — reopen whichever mode you used last"),
        ],
        &[],
    )?
    else {
        writeln!(writer, "\nSetup cancelled (no files changed).")?;
        return Ok(());
    };

    let Some(theme) = prompt_choice_step(
        use_tty,
        reader,
        writer,
        "2. Colour theme:",
        &[
            ("babyblue", "babyblue   — crisp baby-blue accent"),
            ("auto", "auto       — Omarchy theme or terminal 16 colours"),
            ("omarchy", "omarchy    — follow live Omarchy theme"),
            ("nord", "nord       — cool arctic palette"),
            ("gruvbox", "gruvbox    — warm retro contrast"),
            ("catppuccin", "catppuccin — Catppuccin Mocha"),
            ("dark", "dark       — One Dark"),
            ("light", "light      — One Light"),
            ("mono", "mono       — bold, dim, and underline only"),
        ],
        &[],
    )?
    else {
        writeln!(writer, "\nSetup cancelled (no files changed).")?;
        return Ok(());
    };

    let Some(pet_style) = prompt_choice_step(
        use_tty,
        reader,
        writer,
        "3. Pet render style:",
        &[
            ("auto", "auto (32x32 + ASCII fallback)"),
            ("ascii", "ascii — always use 4x10 pure ASCII art"),
            ("off", "off   — hide the #! companion"),
        ],
        &[("pixel", "pixel")],
    )?
    else {
        writeln!(writer, "\nSetup cancelled (no files changed).")?;
        return Ok(());
    };

    let Some(speed) = prompt_choice_step(
        use_tty,
        reader,
        writer,
        "4. Typewriter animation speed:",
        &[
            ("normal", "normal (~2s)"),
            ("fast", "fast   (~0.8s)"),
            ("slow", "slow   (~3.2s)"),
            ("off", "off    — instant output"),
        ],
        &[],
    )?
    else {
        writeln!(writer, "\nSetup cancelled (no files changed).")?;
        return Ok(());
    };

    let Some(tips_choice) = prompt_choice_step(
        use_tty,
        reader,
        writer,
        "5. Rotating header tips:",
        &[
            ("on", "on  — show rotating usage tips in the header"),
            ("off", "off — hide header tips (toggle anytime with /tips)"),
        ],
        &[("true", "on"), ("yes", "on"), ("y", "on"), ("false", "off"), ("no", "off"), ("n", "off")],
    )?
    else {
        writeln!(writer, "\nSetup cancelled (no files changed).")?;
        return Ok(());
    };

    let prefs_file = prefs_path.context("cannot determine state directory (set HOME or XDG_STATE_HOME)")?;
    let mut prefs = prefs::load_from(&prefs_file);
    prefs.theme = Some(theme.clone());
    prefs.pet_style = Some(pet_style.clone());
    prefs.speed = Some(speed.clone());
    prefs.tips = Some(tips_choice == "on");
    if default_mode != "last" {
        prefs.last_mode = Some(default_mode.clone());
    } else if prefs.last_mode.is_none() {
        prefs.last_mode = Some("watch".to_string());
    }
    prefs::save_to(&prefs_file, &prefs)?;
    writeln!(writer, "\nSaved preferences to {}", prefs_file.display())?;

    if default_mode != "watch"
        && let Some(cfg_file) = &config_path
    {
        if cfg_file.exists() {
            writeln!(
                writer,
                "Existing config preserved at {}. Add this line if you want `{default_mode}` on every start:\n  default_mode = \"{default_mode}\"",
                cfg_file.display()
            )?;
        } else {
            let minimal = format!("# huh configuration\ndefault_mode = \"{default_mode}\"\n");
            write_atomic(cfg_file, &minimal)?;
            writeln!(writer, "Created {} with default_mode = \"{default_mode}\"", cfg_file.display())?;
        }
    }

    writeln!(writer, "\nShell integration preview for {}:", shell.name())?;
    writeln!(writer, "  Top (after interactive guard): {record_line}")?;
    writeln!(writer, "  End of file:                   {hooks_line}")?;

    if let Some(rc) = &rc_path {
        match prompt_yes_no_step(use_tty, reader, writer, &format!("Add these lines to {}?", rc.display()))? {
            Some(true) => {
                let existing = std::fs::read_to_string(rc).unwrap_or_default();
                let updated = upsert_rc_blocks(&existing, shell);
                write_atomic(rc, &updated)?;
                writeln!(writer, "Updated {} with marked `huh shell-init` blocks.", rc.display())?;
            }
            Some(false) => {
                writeln!(writer, "Skipped editing {}. You can add the two lines above anytime.", rc.display())?;
            }
            None => {
                writeln!(writer, "\nSetup cancelled (rc file left untouched).")?;
                return Ok(());
            }
        }
    } else {
        writeln!(writer, "Skipped shell rc update because HOME is not set.")?;
    }

    writeln!(writer, "\nSetup complete. Restart your shell, then run `huh`.")?;
    Ok(())
}

/// Interactive entry point for `huh setup`.
pub fn run() -> Result<()> {
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let home = std::env::var_os("HOME").filter(|v| !v.is_empty()).map(PathBuf::from);
    let xdg_config = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from);
    let xdg_state = std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()).map(PathBuf::from);
    let shell_env = std::env::var("SHELL").ok();
    let env_lookup = |key: &str| std::env::var(key).ok();
    let cfg = SetupConfig {
        home: home.as_deref(),
        xdg_config: xdg_config.as_deref(),
        xdg_state: xdg_state.as_deref(),
        shell_env: shell_env.as_deref(),
        env: &env_lookup,
        interactive,
    };
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    run_with_io_mode(&cfg, &mut reader, &mut writer, interactive && tui::is_interactive_tty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::Cursor;

    static TEST_DIR_SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(label: &str) -> PathBuf {
        let seq = TEST_DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("huh-setup-test-{label}-{stamp}-{seq}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn enter_accepts_recommended_defaults_and_declining_leaves_rc_untouched() {
        let root = temp_dir("defaults");
        let home = root.join("home");
        let xdg_cfg = root.join("cfg");
        let xdg_state = root.join("state");
        std::fs::create_dir_all(&home).unwrap();
        let rc = home.join(".bashrc");
        let original_rc = "# original bashrc\n[[ $- != *i* ]] && return\n";
        std::fs::write(&rc, original_rc).unwrap();

        let env_map: HashMap<&str, &str> = [("GHOSTTY_RESOURCES_DIR", "/usr/share/ghostty")].into();
        let env_fn = |k: &str| env_map.get(k).map(|v| (*v).to_string());
        let cfg = SetupConfig {
            home: Some(&home),
            xdg_config: Some(&xdg_cfg),
            xdg_state: Some(&xdg_state),
            shell_env: Some("/bin/bash"),
            env: &env_fn,
            interactive: true,
        };

        let mut input = Cursor::new(b"\n\n\n\n\nn\n".to_vec());
        let mut output = Vec::new();
        run_with_io(&cfg, &mut input, &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();

        assert!(text.contains("Shell:          bash"), "{text}");
        assert!(text.contains("Kitty graphics: supported (Ghostty"), "{text}");
        assert!(text.contains("Skipped editing"), "{text}");
        assert_eq!(std::fs::read_to_string(&rc).unwrap(), original_rc);
        assert!(!xdg_cfg.join("huh/config.toml").exists(), "watch default creates no config.toml");

        let prefs_raw = std::fs::read_to_string(xdg_state.join("huh/prefs.toml")).unwrap();
        let prefs: Prefs = toml::from_str(&prefs_raw).unwrap();
        assert_eq!(prefs.theme.as_deref(), Some("babyblue"));
        assert_eq!(prefs.pet_style.as_deref(), Some("auto"));
        assert_eq!(prefs.speed.as_deref(), Some("normal"));
        assert_eq!(prefs.tips, Some(true));
        assert_eq!(prefs.last_mode.as_deref(), Some("watch"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn interactive_choices_save_prefs_preserve_existing_config_and_install_rc_idempotently() {
        let root = temp_dir("custom");
        let home = root.join("home");
        let xdg_cfg = root.join("cfg");
        let xdg_state = root.join("state");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(xdg_cfg.join("huh")).unwrap();
        std::fs::create_dir_all(xdg_state.join("huh")).unwrap();

        let existing_config = "# custom config\nmode = \"expert\"\ntheme = \"nord\"\n";
        let config_file = xdg_cfg.join("huh/config.toml");
        std::fs::write(&config_file, existing_config).unwrap();

        let existing_prefs = "safety = \"expert\"\nquiet = true\n";
        let prefs_file = xdg_state.join("huh/prefs.toml");
        std::fs::write(&prefs_file, existing_prefs).unwrap();

        let rc = home.join(".zshrc");
        std::fs::write(&rc, "[[ ! -o interactive ]] && return\n\nalias ll='ls -la'\n").unwrap();

        let env_fn = |_: &str| None;
        let cfg = SetupConfig {
            home: Some(&home),
            xdg_config: Some(&xdg_cfg),
            xdg_state: Some(&xdg_state),
            shell_env: Some("/usr/bin/zsh"),
            env: &env_fn,
            interactive: true,
        };

        // mode=2 (lab), theme=4 (nord), pet_style=2 (ascii), speed=2 (fast), tips=2 (off), hooks=y
        let mut input = Cursor::new(b"2\n4\n2\n2\n2\ny\n".to_vec());
        let mut output = Vec::new();
        run_with_io(&cfg, &mut input, &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();

        // Existing config.toml must never be modified; prints 1-line snippet instead.
        assert_eq!(std::fs::read_to_string(&config_file).unwrap(), existing_config);
        assert!(text.contains("default_mode = \"lab\""), "{text}");

        // Existing safety/quiet preserved alongside new picks.
        let prefs: Prefs = toml::from_str(&std::fs::read_to_string(&prefs_file).unwrap()).unwrap();
        assert_eq!(prefs.theme.as_deref(), Some("nord"));
        assert_eq!(prefs.pet_style.as_deref(), Some("ascii"));
        assert_eq!(prefs.speed.as_deref(), Some("fast"));
        assert_eq!(prefs.tips, Some(false));
        assert_eq!(prefs.last_mode.as_deref(), Some("lab"));
        assert_eq!(prefs.safety.as_deref(), Some("expert"));
        assert_eq!(prefs.quiet, Some(true));

        let rc_after_first = std::fs::read_to_string(&rc).unwrap();
        let expected_top = "[[ ! -o interactive ]] && return\n\n# >>> huh shell-init (record) >>>\neval \"$(huh shell-init --shell zsh)\"\n# <<< huh shell-init (record) <<<\n\nalias ll='ls -la'";
        assert!(rc_after_first.starts_with(expected_top), "{rc_after_first}");
        assert!(rc_after_first.ends_with("# >>> huh shell-init (hooks) >>>\neval \"$(huh shell-init --shell zsh --hooks)\"\n# <<< huh shell-init (hooks) <<<\n"));

        // Re-running setup with hook approval is completely idempotent (zero duplicates).
        let mut input2 = Cursor::new(b"2\n4\n2\n2\n2\ny\n".to_vec());
        let mut output2 = Vec::new();
        run_with_io(&cfg, &mut input2, &mut output2).unwrap();
        let rc_after_second = std::fs::read_to_string(&rc).unwrap();
        assert_eq!(rc_after_second, rc_after_first);
        assert_eq!(rc_after_second.matches(RECORD_START).count(), 1);
        assert_eq!(rc_after_second.matches(HOOKS_START).count(), 1);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn creates_minimal_config_only_when_missing_and_mode_is_not_watch() {
        let root = temp_dir("newcfg");
        let home = root.join("home");
        let xdg_cfg = root.join("cfg");
        let xdg_state = root.join("state");
        std::fs::create_dir_all(&home).unwrap();

        let env_fn = |_: &str| None;
        let cfg = SetupConfig {
            home: Some(&home),
            xdg_config: Some(&xdg_cfg),
            xdg_state: Some(&xdg_state),
            shell_env: Some("/usr/bin/fish"),
            env: &env_fn,
            interactive: true,
        };

        // mode=2 (lab), rest defaults, hooks=y (creates config.fish too)
        let mut input = Cursor::new(b"2\n\n\n\n\ny\n".to_vec());
        let mut output = Vec::new();
        run_with_io(&cfg, &mut input, &mut output).unwrap();

        let cfg_text = std::fs::read_to_string(xdg_cfg.join("huh/config.toml")).unwrap();
        assert!(cfg_text.contains("default_mode = \"lab\""), "{cfg_text}");

        let fish_rc = std::fs::read_to_string(xdg_cfg.join("fish/config.fish")).unwrap();
        assert!(fish_rc.contains("huh shell-init --shell fish | source"), "{fish_rc}");
        assert!(fish_rc.contains("huh shell-init --shell fish --hooks | source"), "{fish_rc}");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn upsert_rc_handles_debian_case_guard_and_fish_interactive_block() {
        let debian = "# ~/.bashrc\ncase $- in\n    *i*) ;;\n      *) return;;\nesac\n\nexport PATH=$HOME/bin:$PATH\n";
        let out_bash = upsert_rc_blocks(debian, ShellKind::Bash);
        let esac_pos = out_bash.find("esac").unwrap();
        let rec_pos = out_bash.find(RECORD_START).unwrap();
        let export_pos = out_bash.find("export PATH").unwrap();
        let hooks_pos = out_bash.find(HOOKS_START).unwrap();
        assert!(esac_pos < rec_pos && rec_pos < export_pos && export_pos < hooks_pos, "{out_bash}");
        assert_eq!(upsert_rc_blocks(&out_bash, ShellKind::Bash), out_bash);

        let fish_rc = "if status is-interactive\n    set -g fish_greeting\nend\n";
        let out_fish = upsert_rc_blocks(fish_rc, ShellKind::Fish);
        let if_pos = out_fish.find("if status is-interactive").unwrap();
        let fish_rec = out_fish.find(RECORD_START).unwrap();
        let greeting = out_fish.find("fish_greeting").unwrap();
        let end_pos = out_fish.find("\nend").unwrap();
        let fish_hooks = out_fish.find(HOOKS_START).unwrap();
        assert!(if_pos < fish_rec && fish_rec < greeting && greeting < end_pos && end_pos < fish_hooks, "{out_fish}");
        assert_eq!(upsert_rc_blocks(&out_fish, ShellKind::Fish), out_fish);
    }

    #[test]
    fn non_interactive_mode_prints_summary_without_reading_or_writing() {
        let root = temp_dir("noninteractive");
        let home = root.join("home");
        let xdg_cfg = root.join("cfg");
        let xdg_state = root.join("state");
        std::fs::create_dir_all(&home).unwrap();

        let env_map: HashMap<&str, &str> = [("TMUX", "/tmp/tmux-1000/default,1,0"), ("KITTY_WINDOW_ID", "1")].into();
        let env_fn = |k: &str| env_map.get(k).map(|v| (*v).to_string());
        let cfg = SetupConfig {
            home: Some(&home),
            xdg_config: Some(&xdg_cfg),
            xdg_state: Some(&xdg_state),
            shell_env: Some("/bin/zsh"),
            env: &env_fn,
            interactive: false,
        };

        let mut empty_reader = Cursor::new(Vec::<u8>::new());
        let mut output = Vec::new();
        run_with_io(&cfg, &mut empty_reader, &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();

        assert!(text.contains("Shell:          zsh"), "{text}");
        assert!(text.contains("paused in outer tmux"), "{text}");
        assert!(text.contains("eval \"$(huh shell-init --shell zsh)\""), "{text}");
        assert!(text.contains("eval \"$(huh shell-init --shell zsh --hooks)\""), "{text}");
        assert!(!xdg_state.join("huh/prefs.toml").exists());
        assert!(!home.join(".zshrc").exists());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn zdotdir_is_respected_for_zsh_rc_path() {
        let root = temp_dir("zdotdir");
        let home = root.join("home");
        let zdotdir = root.join("zdot");
        let xdg_state = root.join("state");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&zdotdir).unwrap();

        let env_map: HashMap<&str, &str> = [("ZDOTDIR", zdotdir.to_str().unwrap())].into();
        let env_fn = |k: &str| env_map.get(k).map(|v| (*v).to_string());
        let cfg = SetupConfig {
            home: Some(&home),
            xdg_config: None,
            xdg_state: Some(&xdg_state),
            shell_env: Some("zsh"),
            env: &env_fn,
            interactive: true,
        };

        let mut input = Cursor::new(b"\n\n\n\n\ny\n".to_vec());
        let mut output = Vec::new();
        run_with_io(&cfg, &mut input, &mut output).unwrap();

        assert!(zdotdir.join(".zshrc").exists());
        assert!(!home.join(".zshrc").exists());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn upsert_rc_upgrades_bare_lines_and_handles_shebang_and_multiline_if() {
        let bare =
            "#!/bin/bash\nexport FOO=1\neval \"$(huh shell-init)\"\nalias x=1\neval \"$(huh shell-init --hooks)\"\n";
        let upgraded = upsert_rc_blocks(bare, ShellKind::Bash);
        assert_eq!(upgraded.matches(RECORD_START).count(), 1, "{upgraded}");
        assert_eq!(upgraded.matches(HOOKS_START).count(), 1, "{upgraded}");
        assert_eq!(upgraded.matches(r#"eval "$(huh shell-init)""#).count(), 1, "{upgraded}");
        assert_eq!(upgraded.matches(r#"eval "$(huh shell-init --hooks)""#).count(), 1, "{upgraded}");
        assert_eq!(upsert_rc_blocks(&upgraded, ShellKind::Bash), upgraded);

        let shebang_only = "#!/usr/bin/env bash\nexport A=1\n";
        let with_shebang = upsert_rc_blocks(shebang_only, ShellKind::Bash);
        assert!(with_shebang.starts_with("#!/usr/bin/env bash\n\n# >>> huh shell-init (record) >>>"), "{with_shebang}");

        let multiline = "if [[ $- != *i* ]]; then\n    return\nfi\nexport B=2\n";
        let with_multi = upsert_rc_blocks(multiline, ShellKind::Bash);
        let fi_pos = with_multi.find("\nfi\n").unwrap();
        let rec_pos = with_multi.find(RECORD_START).unwrap();
        let export_pos = with_multi.find("export B=2").unwrap();
        assert!(fi_pos < rec_pos && rec_pos < export_pos, "{with_multi}");
    }

    #[test]
    fn invalid_input_reprompts_and_named_choices_work() {
        let root = temp_dir("reprompt");
        let home = root.join("home");
        let xdg_cfg = root.join("cfg");
        let xdg_state = root.join("state");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(xdg_state.join("huh")).unwrap();
        std::fs::write(xdg_state.join("huh/prefs.toml"), "last_mode = \"lab\"\n").unwrap();

        let env_fn = |_: &str| None;
        let cfg = SetupConfig {
            home: Some(&home),
            xdg_config: Some(&xdg_cfg),
            xdg_state: Some(&xdg_state),
            shell_env: Some("/bin/bash"),
            env: &env_fn,
            interactive: true,
        };

        // Invalid choice "99", then "last", "gruvbox", "off", "slow", "off", invalid "maybe", then "n"
        let mut input = Cursor::new(b"99\nlast\ngruvbox\noff\nslow\noff\nmaybe\nn\n".to_vec());
        let mut output = Vec::new();
        run_with_io(&cfg, &mut input, &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("Enter 1-3"), "{text}");
        assert!(text.contains("Please answer y or n"), "{text}");

        let prefs: Prefs = toml::from_str(&std::fs::read_to_string(xdg_state.join("huh/prefs.toml")).unwrap()).unwrap();
        assert_eq!(prefs.theme.as_deref(), Some("gruvbox"));
        assert_eq!(prefs.pet_style.as_deref(), Some("off"));
        assert_eq!(prefs.speed.as_deref(), Some("slow"));
        assert_eq!(prefs.tips, Some(false));
        // Choosing `last` preserves existing last_mode.
        assert_eq!(prefs.last_mode.as_deref(), Some("lab"));
        assert!(xdg_cfg.join("huh/config.toml").exists());

        std::fs::remove_dir_all(&root).ok();
    }
}
