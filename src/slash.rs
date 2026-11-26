//! Slash command definitions, live fuzzy completions, and submit-time fuzzy resolution.
//!
//! Covers all 12 pane slash commands (`/mode`, `/watch`, `/lab`, `/safety`, `/theme`, `/pet`,
//! `/speed`, `/noise`, `/quiet`, `/tips`, `/find`, `/help`) while guarding Unix paths
//! (`/etc/hosts`, `/usr/bin/env`) and non-matching word extensions (`/finder`, `/modeled`).

use crate::find;
use crate::pet::Species;
use crate::style;

/// Metadata for one built-in slash command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlashDef {
    pub name: &'static str,
    pub summary: &'static str,
    pub usage: &'static str,
}

/// All 12 companion-pane slash commands in display order.
pub const COMMANDS: &[SlashDef] = &[
    SlashDef { name: "/mode", summary: "Switch Watch / Lab mode", usage: "/mode [watch|lab]" },
    SlashDef { name: "/watch", summary: "Switch to Watch mode", usage: "/watch" },
    SlashDef {
        name: "/lab",
        summary: "Switch to Lab course & practice mode",
        usage: "/lab [unit|review|practice|progress]",
    },
    SlashDef { name: "/safety", summary: "Toggle normal / expert safeguards", usage: "/safety [normal|expert]" },
    SlashDef { name: "/theme", summary: "Preview or switch colour theme", usage: "/theme [name]" },
    SlashDef {
        name: "/pet",
        summary: "Inspect or customize Pebble",
        usage: "/pet [status|pebbles|name|species|style|customize]",
    },
    SlashDef { name: "/speed", summary: "Set typewriter speed", usage: "/speed [off|fast|normal|slow|<ms>]" },
    SlashDef { name: "/noise", summary: "Set watch noise level", usage: "/noise [all|new|errors]" },
    SlashDef { name: "/quiet", summary: "Toggle repeating command explanations", usage: "/quiet [on|off]" },
    SlashDef { name: "/tips", summary: "Toggle or cycle header tips", usage: "/tips [on|off|next]" },
    SlashDef { name: "/find", summary: "Fuzzy search catalog & tools", usage: "/find [words]" },
    SlashDef { name: "/help", summary: "Show pane commands", usage: "/help" },
];

/// One completion candidate shown in the live `/` dropdown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlashCompletion {
    pub insert: String,
    pub display: String,
    pub summary: String,
    pub auto_submit: bool,
}

/// Outcome of resolving a submitted line against slash commands and their sub-arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FuzzyResolution {
    NotSlash,
    Exact(String),
    Unique { resolved: String, note: String },
    Ambiguous(Vec<&'static SlashDef>),
    Palette(Vec<&'static SlashDef>),
    Unknown(String),
}

/// True when `line` starts with a single-segment `/` command candidate rather than a Unix path
/// such as `/etc/hosts` or `/usr/bin/env`.
pub fn is_slash_candidate(line: &str) -> bool {
    let Some(first) = line.split_whitespace().next() else {
        return false;
    };
    first.starts_with('/') && !first[1..].contains('/')
}

// ---------------------------------------------------------------------------
// Candidate scoring & fuzzy resolution helpers
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct OptionDef {
    name: &'static str,
    summary: &'static str,
}

const MODE_ARGS: &[OptionDef] = &[
    OptionDef { name: "watch", summary: "Live command explanations" },
    OptionDef { name: "lab", summary: "Interactive lessons & practice" },
];

const SAFETY_ARGS: &[OptionDef] = &[
    OptionDef { name: "normal", summary: "Standard beginner safeguards" },
    OptionDef { name: "expert", summary: "Fewer safeguards (warnings stay on)" },
];

const SPEED_ARGS: &[OptionDef] = &[
    OptionDef { name: "normal", summary: "2000ms typewriter budget" },
    OptionDef { name: "fast", summary: "800ms typewriter budget" },
    OptionDef { name: "slow", summary: "3200ms typewriter budget" },
    OptionDef { name: "off", summary: "Instant (no typewriter)" },
];

const NOISE_ARGS: &[OptionDef] = &[
    OptionDef { name: "all", summary: "Explain every finished command" },
    OptionDef { name: "new", summary: "Explain each program once" },
    OptionDef { name: "errors", summary: "Only failures and warnings" },
];

const QUIET_ARGS: &[OptionDef] = &[
    OptionDef { name: "on", summary: "Explain each program once" },
    OptionDef { name: "off", summary: "Explain every command" },
];

const TIPS_ARGS: &[OptionDef] = &[
    OptionDef { name: "on", summary: "Show rotating header tips" },
    OptionDef { name: "off", summary: "Hide header tips" },
    OptionDef { name: "next", summary: "Show next header tip" },
];

const PET_SUBCMDS: &[OptionDef] = &[
    OptionDef { name: "status", summary: "Show Pebble status card" },
    OptionDef { name: "pebbles", summary: "Show gifted shiny pebbles" },
    OptionDef { name: "name", summary: "Rename Pebble (needs name)" },
    OptionDef { name: "species", summary: "Switch unlocked pet species" },
    OptionDef { name: "style", summary: "Set auto / pixel / ascii / off" },
    OptionDef { name: "customize", summary: "Export editable 32x32 sprite" },
];

const PET_STYLES: &[OptionDef] = &[
    OptionDef { name: "auto", summary: "32x32 Kitty with ASCII fallback" },
    OptionDef { name: "pixel", summary: "Force 32x32 Kitty pixel sprite" },
    OptionDef { name: "ascii", summary: "Force 4x10 pure-ASCII art" },
    OptionDef { name: "off", summary: "Hide Pebble in header" },
];

const LAB_ARGS: &[OptionDef] = &[
    OptionDef { name: "progress", summary: "View XP, streaks, and badges" },
    OptionDef { name: "review", summary: "Spaced repetition review" },
    OptionDef { name: "practice", summary: "Hands-on live sandbox labs" },
    OptionDef { name: "1", summary: "Unit 1: Finding Your Way" },
    OptionDef { name: "2", summary: "Unit 2: Files & Folders" },
    OptionDef { name: "3", summary: "Unit 3: Searching & Pipes" },
    OptionDef { name: "4", summary: "Unit 4: Reading Permissions" },
    OptionDef { name: "5", summary: "Unit 5: Processes & Disk" },
    OptionDef { name: "6", summary: "Unit 6: Packages & Manuals" },
    OptionDef { name: "7", summary: "Unit 7: Archives & Links" },
    OptionDef { name: "8", summary: "Unit 8: Networking & Stay Safe" },
];

fn theme_summary(theme: style::Theme) -> &'static str {
    match theme {
        style::Theme::BabyBlue => "Default baby-blue palette",
        style::Theme::Auto => "Omarchy or terminal 16-colour",
        style::Theme::Omarchy => "Current Omarchy desktop theme",
        style::Theme::Dark => "One Dark palette",
        style::Theme::Light => "One Light palette",
        style::Theme::Nord => "Nord Arctic palette",
        style::Theme::Gruvbox => "Gruvbox warm palette",
        style::Theme::Catppuccin => "Catppuccin Mocha palette",
        style::Theme::Mono => "Bold, dim, and underline only",
    }
}

fn damerau_levenshtein(a: &[char], b: &[char]) -> usize {
    let n = a.len();
    let m = b.len();
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in dp.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in dp[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut val = (dp[i - 1][j] + 1).min(dp[i][j - 1] + 1).min(dp[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                val = val.min(dp[i - 2][j - 2] + 1);
            }
            dp[i][j] = val;
        }
    }
    dp[n][m]
}

const TIER_EXACT: u8 = 3;
const TIER_PREFIX: u8 = 2;
const TIER_FUZZY: u8 = 1;

fn candidate_score(query: &str, candidate: &str) -> Option<(u8, i32)> {
    let q = query.trim().to_ascii_lowercase();
    let c = candidate.to_ascii_lowercase();
    if q == c {
        return Some((TIER_EXACT, 0));
    }
    if c.starts_with(&q) {
        return Some((TIER_PREFIX, 0));
    }
    let sub = find::score(&q, &c, 2);
    let q_chars: Vec<char> = q.chars().collect();
    let c_chars: Vec<char> = c.chars().collect();
    let dl1 = q_chars.len() >= 3
        && (q_chars.len() as isize - c_chars.len() as isize).abs() <= 1
        && damerau_levenshtein(&q_chars, &c_chars) <= 1;
    match (sub, dl1) {
        (Some(s), true) => Some((TIER_FUZZY, s + 20)),
        (Some(s), false) => Some((TIER_FUZZY, s)),
        (None, true) => Some((TIER_FUZZY, 0)),
        (None, false) => None,
    }
}

fn rank_indices<'a, I>(keys: I, query: &str) -> Vec<usize>
where
    I: IntoIterator<Item = &'a str>,
{
    let q = query.trim();
    if q.is_empty() {
        return keys.into_iter().enumerate().map(|(i, _)| i).collect();
    }
    let mut scored: Vec<(usize, u8, i32)> = keys
        .into_iter()
        .enumerate()
        .filter_map(|(i, key)| {
            let (tier, score) = candidate_score(q, key)?;
            Some((i, tier, score))
        })
        .collect();
    let has_prefix_or_exact = scored.iter().any(|(_, tier, _)| *tier >= TIER_PREFIX);
    if has_prefix_or_exact {
        scored.retain(|(_, tier, _)| *tier >= TIER_PREFIX);
    }
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
    scored.into_iter().map(|(i, _, _)| i).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Resolved<T> {
    Exact(T),
    Unique(T),
    Ambiguous(Vec<T>),
    None,
}

fn resolve_from<'a, T: Copy>(items: &[T], key: impl Fn(&T) -> &'a str, query: &str) -> Resolved<T> {
    let q = query.trim();
    if q.is_empty() {
        return Resolved::None;
    }
    let mut scored: Vec<(T, u8, i32)> = items
        .iter()
        .filter_map(|&item| {
            let (tier, score) = candidate_score(q, key(&item))?;
            Some((item, tier, score))
        })
        .collect();
    let Some(max_tier) = scored.iter().map(|(_, tier, _)| *tier).max() else {
        return Resolved::None;
    };
    scored.retain(|(_, tier, _)| *tier == max_tier);
    if max_tier == TIER_EXACT {
        return Resolved::Exact(scored[0].0);
    }
    if max_tier == TIER_FUZZY {
        let max_score = scored.iter().map(|(_, _, s)| *s).max().unwrap_or(0);
        scored.retain(|(_, _, s)| *s == max_score);
    }
    if scored.len() == 1 {
        Resolved::Unique(scored[0].0)
    } else {
        Resolved::Ambiguous(scored.into_iter().map(|(item, _, _)| item).collect())
    }
}

fn resolve_command(cmd_token: &str) -> Resolved<&'static SlashDef> {
    let Some(query) = cmd_token.strip_prefix('/') else {
        return Resolved::None;
    };
    if query.is_empty() {
        return Resolved::None;
    }
    let refs: Vec<&'static SlashDef> = COMMANDS.iter().collect();
    resolve_from(&refs, |d| &d.name[1..], query)
}

// ---------------------------------------------------------------------------
// Stage 1 & Stage 2 completions
// ---------------------------------------------------------------------------

fn stage1_completion(def: &SlashDef) -> SlashCompletion {
    let zero_arg = matches!(def.name, "/watch" | "/help");
    SlashCompletion {
        insert: if zero_arg { def.name.to_string() } else { format!("{} ", def.name) },
        display: def.name.to_string(),
        summary: def.summary.to_string(),
        auto_submit: true,
    }
}

fn option_completions(cmd_name: &str, options: &[OptionDef], query: &str) -> Vec<SlashCompletion> {
    rank_indices(options.iter().map(|o| o.name), query)
        .into_iter()
        .map(|i| {
            let opt = options[i];
            SlashCompletion {
                insert: format!("{cmd_name} {}", opt.name),
                display: opt.name.to_string(),
                summary: opt.summary.to_string(),
                auto_submit: true,
            }
        })
        .collect()
}

fn theme_completions(query: &str) -> Vec<SlashCompletion> {
    rank_indices(style::ALL.iter().map(|t| t.name()), query)
        .into_iter()
        .map(|i| {
            let theme = style::ALL[i];
            let name = theme.name();
            SlashCompletion {
                insert: format!("/theme {name}"),
                display: name.to_string(),
                summary: theme_summary(theme).to_string(),
                auto_submit: true,
            }
        })
        .collect()
}

fn pet_subcmd_completions(query: &str) -> Vec<SlashCompletion> {
    rank_indices(PET_SUBCMDS.iter().map(|o| o.name), query)
        .into_iter()
        .map(|i| {
            let opt = PET_SUBCMDS[i];
            let (insert, auto_submit) = match opt.name {
                "name" => ("/pet name ".to_string(), false),
                "species" => ("/pet species ".to_string(), true),
                "style" => ("/pet style ".to_string(), true),
                other => (format!("/pet {other}"), true),
            };
            SlashCompletion { insert, display: opt.name.to_string(), summary: opt.summary.to_string(), auto_submit }
        })
        .collect()
}

fn pet_species_completions(query: &str, unlocked_species: &[Species]) -> Vec<SlashCompletion> {
    rank_indices(Species::ALL.iter().map(|s| s.name()), query)
        .into_iter()
        .map(|i| {
            let sp = Species::ALL[i];
            let unlocked = sp == Species::Penguin || unlocked_species.contains(&sp);
            let summary = if unlocked { sp.label().to_string() } else { format!("{} (locked)", sp.label()) };
            SlashCompletion {
                insert: format!("/pet species {}", sp.name()),
                display: sp.name().to_string(),
                summary,
                auto_submit: true,
            }
        })
        .collect()
}

/// Returns Stage 1 (command) or Stage 2/3 (sub-argument) completions for `line`.
pub fn completions(line: &str, unlocked_species: &[Species]) -> Vec<SlashCompletion> {
    if !is_slash_candidate(line) {
        return Vec::new();
    }
    let trimmed_start = line.trim_start();
    let Some(ws) = trimmed_start.find(char::is_whitespace) else {
        let query = &trimmed_start[1..];
        return rank_indices(COMMANDS.iter().map(|d| &d.name[1..]), query)
            .into_iter()
            .map(|i| stage1_completion(&COMMANDS[i]))
            .collect();
    };

    let cmd_tok = &trimmed_start[..ws];
    let def = match resolve_command(cmd_tok) {
        Resolved::Exact(d) | Resolved::Unique(d) => d,
        Resolved::Ambiguous(_) | Resolved::None => return Vec::new(),
    };
    let after_cmd = trimmed_start[ws..].trim_start();

    if def.name == "/pet" {
        let Some(sub_ws) = after_cmd.find(char::is_whitespace) else {
            return pet_subcmd_completions(after_cmd);
        };
        let sub_tok = &after_cmd[..sub_ws];
        let rest = after_cmd[sub_ws..].trim_start();
        if rest.contains(char::is_whitespace) {
            return Vec::new();
        }
        let sub = match resolve_from(PET_SUBCMDS, |o| o.name, sub_tok) {
            Resolved::Exact(o) | Resolved::Unique(o) => o.name,
            Resolved::Ambiguous(_) | Resolved::None => return Vec::new(),
        };
        return match sub {
            "species" => pet_species_completions(rest, unlocked_species),
            "style" => option_completions("/pet style", PET_STYLES, rest),
            _ => Vec::new(),
        };
    }

    if after_cmd.contains(char::is_whitespace) {
        return Vec::new();
    }
    match def.name {
        "/mode" => option_completions("/mode", MODE_ARGS, after_cmd),
        "/safety" => option_completions("/safety", SAFETY_ARGS, after_cmd),
        "/theme" => theme_completions(after_cmd),
        "/speed" => option_completions("/speed", SPEED_ARGS, after_cmd),
        "/noise" => option_completions("/noise", NOISE_ARGS, after_cmd),
        "/quiet" => option_completions("/quiet", QUIET_ARGS, after_cmd),
        "/tips" => option_completions("/tips", TIPS_ARGS, after_cmd),
        "/lab" => option_completions("/lab", LAB_ARGS, after_cmd),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Submit-time resolution
// ---------------------------------------------------------------------------

fn finish_resolution(all_exact: bool, resolved: String) -> FuzzyResolution {
    if all_exact {
        FuzzyResolution::Exact(resolved)
    } else {
        let note = format!("→ {resolved}");
        FuzzyResolution::Unique { resolved, note }
    }
}

/// Resolves `arg` against `options`, returning `Ok((canonical_arg, was_exact))` or `Err(Ambiguous)`.
/// Unknown args pass through unchanged with `was_exact = true` so command handlers print usage/errors.
fn resolve_option<'a>(
    cmd_def: &'static SlashDef,
    options: &[&'a str],
    arg: &'a str,
) -> Result<(&'a str, bool), FuzzyResolution> {
    match resolve_from(options, |s| s, arg) {
        Resolved::Exact(v) => Ok((v, true)),
        Resolved::Unique(v) => Ok((v, false)),
        Resolved::Ambiguous(_) => Err(FuzzyResolution::Ambiguous(vec![cmd_def])),
        Resolved::None => Ok((arg, true)),
    }
}

fn is_numeric_speed(arg: &str) -> bool {
    let lower = arg.to_ascii_lowercase();
    let digits = lower.strip_suffix("ms").unwrap_or(&lower);
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

fn resolve_pet_submit(cmd_def: &'static SlashDef, cmd_exact: bool, args: &[&str]) -> FuzzyResolution {
    if args.is_empty() {
        return finish_resolution(cmd_exact, "/pet".to_string());
    }
    let (sub, sub_exact) = match resolve_from(PET_SUBCMDS, |o| o.name, args[0]) {
        Resolved::Exact(o) => (o.name, true),
        Resolved::Unique(o) => (o.name, false),
        Resolved::Ambiguous(_) => return FuzzyResolution::Ambiguous(vec![cmd_def]),
        Resolved::None => return finish_resolution(cmd_exact, format!("/pet {}", args.join(" "))),
    };
    let head_exact = cmd_exact && sub_exact;
    match sub {
        "name" => {
            if args.len() == 1 {
                finish_resolution(head_exact, "/pet name".to_string())
            } else {
                finish_resolution(head_exact, format!("/pet name {}", args[1..].join(" ")))
            }
        }
        "species" => {
            if args.len() == 1 {
                finish_resolution(head_exact, "/pet species".to_string())
            } else if args.len() == 2 {
                let names: Vec<&'static str> = Species::ALL.iter().map(|s| s.name()).collect();
                match resolve_option(cmd_def, &names, args[1]) {
                    Ok((sp, sp_exact)) => finish_resolution(head_exact && sp_exact, format!("/pet species {sp}")),
                    Err(ambig) => ambig,
                }
            } else {
                finish_resolution(head_exact, format!("/pet species {}", args[1..].join(" ")))
            }
        }
        "style" => {
            if args.len() == 1 {
                finish_resolution(head_exact, "/pet style".to_string())
            } else if args.len() == 2 {
                let styles: Vec<&'static str> = PET_STYLES.iter().map(|o| o.name).collect();
                match resolve_option(cmd_def, &styles, args[1]) {
                    Ok((st, st_exact)) => finish_resolution(head_exact && st_exact, format!("/pet style {st}")),
                    Err(ambig) => ambig,
                }
            } else {
                finish_resolution(head_exact, format!("/pet style {}", args[1..].join(" ")))
            }
        }
        other => {
            if args.len() == 1 {
                finish_resolution(head_exact, format!("/pet {other}"))
            } else {
                finish_resolution(head_exact, format!("/pet {other} {}", args[1..].join(" ")))
            }
        }
    }
}

/// Resolves a submitted input line into `FuzzyResolution` without executing anything.
pub fn resolve_submit(line: &str) -> FuzzyResolution {
    if !is_slash_candidate(line) {
        return FuzzyResolution::NotSlash;
    }
    let trimmed = line.trim();
    if trimmed == "/" {
        return FuzzyResolution::Palette(COMMANDS.iter().collect());
    }
    let mut tokens = trimmed.split_whitespace();
    let cmd_tok = tokens.next().unwrap_or(trimmed);
    let args: Vec<&str> = tokens.collect();

    let (def, cmd_exact) = match resolve_command(cmd_tok) {
        Resolved::Exact(d) => (d, true),
        Resolved::Unique(d) => (d, false),
        Resolved::Ambiguous(defs) => return FuzzyResolution::Ambiguous(defs),
        Resolved::None => return FuzzyResolution::Unknown(cmd_tok.to_string()),
    };

    if args.is_empty() {
        return finish_resolution(cmd_exact, def.name.to_string());
    }

    match def.name {
        "/find" | "/lab" | "/watch" | "/help" => {
            finish_resolution(cmd_exact, format!("{} {}", def.name, args.join(" ")))
        }
        "/pet" => resolve_pet_submit(def, cmd_exact, &args),
        "/mode" => {
            let modes = ["watch", "lab"];
            match resolve_option(def, &modes, args[0]) {
                Ok((m, arg_exact)) => {
                    let resolved = if args.len() == 1 {
                        format!("/mode {m}")
                    } else {
                        format!("/mode {m} {}", args[1..].join(" "))
                    };
                    finish_resolution(cmd_exact && arg_exact, resolved)
                }
                Err(ambig) => ambig,
            }
        }
        "/safety" if args.len() == 1 => {
            let opts = ["normal", "expert"];
            match resolve_option(def, &opts, args[0]) {
                Ok((v, arg_exact)) => finish_resolution(cmd_exact && arg_exact, format!("/safety {v}")),
                Err(ambig) => ambig,
            }
        }
        "/theme" if args.len() == 1 => {
            let themes: Vec<&'static str> = style::ALL.iter().map(|t| t.name()).collect();
            match resolve_option(def, &themes, args[0]) {
                Ok((v, arg_exact)) => finish_resolution(cmd_exact && arg_exact, format!("/theme {v}")),
                Err(ambig) => ambig,
            }
        }
        "/speed" if args.len() == 1 => {
            if is_numeric_speed(args[0]) {
                finish_resolution(cmd_exact, format!("/speed {}", args[0].to_ascii_lowercase()))
            } else {
                let speeds = ["normal", "fast", "slow", "off"];
                match resolve_option(def, &speeds, args[0]) {
                    Ok((v, arg_exact)) => finish_resolution(cmd_exact && arg_exact, format!("/speed {v}")),
                    Err(ambig) => ambig,
                }
            }
        }
        "/noise" if args.len() == 1 => {
            let opts = ["all", "new", "errors"];
            match resolve_option(def, &opts, args[0]) {
                Ok((v, arg_exact)) => finish_resolution(cmd_exact && arg_exact, format!("/noise {v}")),
                Err(ambig) => ambig,
            }
        }
        "/quiet" if args.len() == 1 => {
            if matches!(args[0].to_ascii_lowercase().as_str(), "true" | "false" | "1" | "0") {
                finish_resolution(cmd_exact, format!("/quiet {}", args[0].to_ascii_lowercase()))
            } else {
                let opts = ["on", "off"];
                match resolve_option(def, &opts, args[0]) {
                    Ok((v, arg_exact)) => finish_resolution(cmd_exact && arg_exact, format!("/quiet {v}")),
                    Err(ambig) => ambig,
                }
            }
        }
        "/tips" if args.len() == 1 => {
            let opts = ["on", "off", "next"];
            match resolve_option(def, &opts, args[0]) {
                Ok((v, arg_exact)) => finish_resolution(cmd_exact && arg_exact, format!("/tips {v}")),
                Err(ambig) => ambig,
            }
        }
        _ => finish_resolution(cmd_exact, format!("{} {}", def.name, args.join(" "))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_table_lists_all_twelve_in_order() {
        let names: Vec<&str> = COMMANDS.iter().map(|c| c.name).collect();
        assert_eq!(
            names,
            [
                "/mode", "/watch", "/lab", "/safety", "/theme", "/pet", "/speed", "/noise", "/quiet", "/tips", "/find",
                "/help",
            ]
        );
        for def in COMMANDS {
            assert!(def.name.starts_with('/'));
            assert!(!def.summary.is_empty());
            assert!(def.usage.starts_with(def.name));
        }
    }

    #[test]
    fn unix_path_guardrail_rejects_multi_segment_paths_and_non_slash_inputs() {
        for ok in ["/", "  /  ", "/th", "/md", "/mode lab", "/find /etc/hosts", "/pet name a/b"] {
            assert!(is_slash_candidate(ok), "expected slash candidate: {ok:?}");
        }
        for nope in [
            "",
            "   ",
            "pet",
            "help",
            "ls -la",
            "cat /etc/hosts",
            "/etc/hosts",
            "/usr/bin/env",
            "/tmp/foo.sh",
            "/usr/local/bin/huh --help",
            "//etc/passwd",
        ] {
            assert!(!is_slash_candidate(nope), "expected non-slash: {nope:?}");
            assert!(completions(nope, &[]).is_empty(), "expected no completions for {nope:?}");
            assert_eq!(resolve_submit(nope), FuzzyResolution::NotSlash, "expected NotSlash for {nope:?}");
        }
    }

    #[test]
    fn stage1_completions_cover_palette_prefixes_and_fuzzy_commands() {
        let all = completions("/", &[]);
        assert_eq!(all.len(), 12);
        assert_eq!(all[0].display, "/mode");
        assert_eq!(all[0].insert, "/mode ");
        assert!(all[0].auto_submit);

        // Zero-arg commands keep no trailing space.
        let watch = all.iter().find(|c| c.display == "/watch").unwrap();
        assert_eq!(watch.insert, "/watch");
        assert!(watch.auto_submit);
        let help = all.iter().find(|c| c.display == "/help").unwrap();
        assert_eq!(help.insert, "/help");
        assert!(help.auto_submit);

        // Exact prefix wins cleanly over fuzzy middle matches.
        let th = completions("/th", &[]);
        assert_eq!(th.len(), 1);
        assert_eq!(th[0].display, "/theme");
        assert_eq!(th[0].insert, "/theme ");
        assert!(th[0].auto_submit);

        let p = completions("/p", &[]);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].display, "/pet");
        assert_eq!(p[0].insert, "/pet ");

        let no = completions("/no", &[]);
        assert_eq!(no.len(), 1);
        assert_eq!(no[0].display, "/noise");
        assert_eq!(no[0].insert, "/noise ");
        assert!(no[0].auto_submit);

        // Subsequence match and 1-edit Damerau-Levenshtein typo match.
        let md = completions("/md", &[]);
        assert_eq!(md.len(), 1);
        assert_eq!(md[0].display, "/mode");

        let thme = completions("/thme", &[]);
        assert_eq!(thme.len(), 1);
        assert_eq!(thme[0].display, "/theme");

        // Ambiguous prefixes preserve COMMANDS order.
        let s: Vec<String> = completions("/s", &[]).into_iter().map(|c| c.display).collect();
        assert_eq!(s, ["/safety", "/speed"]);
        let t: Vec<String> = completions("/t", &[]).into_iter().map(|c| c.display).collect();
        assert_eq!(t, ["/theme", "/tips"]);

        // Non-matching suffixes never fuzzy-match shorter commands.
        assert!(completions("/finder", &[]).is_empty());
        assert!(completions("/modeled", &[]).is_empty());
    }

    #[test]
    fn stage2_and_stage3_completions_cover_all_commands_and_species_locks() {
        let mode: Vec<String> = completions("/mode ", &[]).into_iter().map(|c| c.display).collect();
        assert_eq!(mode, ["watch", "lab"]);

        let safety: Vec<String> = completions("/safety ", &[]).into_iter().map(|c| c.display).collect();
        assert_eq!(safety, ["normal", "expert"]);

        let themes = completions("/theme ", &[]);
        let expected_themes: Vec<&str> = style::ALL.iter().map(|t| t.name()).collect();
        assert_eq!(themes.iter().map(|c| c.display.as_str()).collect::<Vec<_>>(), expected_themes);
        assert!(themes.iter().all(|c| c.auto_submit && c.insert == format!("/theme {}", c.display)));

        let speed: Vec<String> = completions("/speed ", &[]).into_iter().map(|c| c.display).collect();
        assert_eq!(speed, ["normal", "fast", "slow", "off"]);

        let noise: Vec<String> = completions("/noise ", &[]).into_iter().map(|c| c.display).collect();
        assert_eq!(noise, ["all", "new", "errors"]);

        let quiet: Vec<String> = completions("/quiet ", &[]).into_iter().map(|c| c.display).collect();
        assert_eq!(quiet, ["on", "off"]);

        let tips: Vec<String> = completions("/tips ", &[]).into_iter().map(|c| c.display).collect();
        assert_eq!(tips, ["on", "off", "next"]);

        let lab: Vec<String> = completions("/lab ", &[]).into_iter().map(|c| c.display).collect();
        assert_eq!(lab, ["progress", "review", "practice", "1", "2", "3", "4", "5", "6", "7", "8"]);

        // Fuzzy command + arg prefix completes to canonical command + arg.
        let th_cat = completions("/th cat", &[]);
        assert_eq!(th_cat.len(), 1);
        assert_eq!(th_cat[0].insert, "/theme catppuccin");
        assert!(th_cat[0].auto_submit);

        let no_err = completions("/no er", &[]);
        assert_eq!(no_err.len(), 1);
        assert_eq!(no_err[0].insert, "/noise errors");
        assert!(no_err[0].auto_submit);

        // /pet stage 2: name has trailing space and auto_submit: false; species/style have trailing space & auto_submit: true.
        let pet = completions("/pet ", &[]);
        assert_eq!(
            pet.iter().map(|c| c.display.as_str()).collect::<Vec<_>>(),
            ["status", "pebbles", "name", "species", "style", "customize",]
        );
        assert_eq!((pet[0].insert.as_str(), pet[0].auto_submit), ("/pet status", true));
        assert_eq!((pet[1].insert.as_str(), pet[1].auto_submit), ("/pet pebbles", true));
        assert_eq!((pet[2].insert.as_str(), pet[2].auto_submit), ("/pet name ", false));
        assert_eq!((pet[3].insert.as_str(), pet[3].auto_submit), ("/pet species ", true));
        assert_eq!((pet[4].insert.as_str(), pet[4].auto_submit), ("/pet style ", true));
        assert_eq!((pet[5].insert.as_str(), pet[5].auto_submit), ("/pet customize", true));

        // /pet species stage 3 shows unlocked vs (locked).
        let sp = completions("/pet species ", &[Species::Penguin, Species::Cat]);
        assert_eq!(
            sp.iter().map(|c| c.display.as_str()).collect::<Vec<_>>(),
            ["penguin", "cat", "beaver", "turtle", "dolphin", "bear", "snake", "bee", "owl"]
        );
        assert!(!sp[0].summary.contains("locked"));
        assert!(!sp[1].summary.contains("locked"));
        for locked in &sp[2..] {
            assert!(locked.summary.contains("(locked)"), "{:?}", locked);
        }

        let sp_fuzzy = completions("/pet sp tur", &[Species::Turtle]);
        assert_eq!(sp_fuzzy.len(), 1);
        assert_eq!(sp_fuzzy[0].insert, "/pet species turtle");
        assert!(!sp_fuzzy[0].summary.contains("locked"));

        let sp_bee = completions("/pet sp bee", &[Species::Bee]);
        assert_eq!(sp_bee[0].insert, "/pet species bee");
        assert!(!sp_bee[0].summary.contains("locked"));

        let st = completions("/pet style ", &[]);
        assert_eq!(st.iter().map(|c| c.display.as_str()).collect::<Vec<_>>(), ["auto", "pixel", "ascii", "off"]);

        // Free-form / zero-arg / trailing extra spaces return no dropdown noise.
        assert!(completions("/watch ", &[]).is_empty());
        assert!(completions("/help ", &[]).is_empty());
        assert!(completions("/find tar", &[]).is_empty());
        assert!(completions("/pet name Pip", &[]).is_empty());
        assert!(completions("/theme nord extra", &[]).is_empty());
        assert!(completions("/s ", &[]).is_empty());
    }

    #[test]
    fn resolve_submit_handles_palette_exact_unique_ambiguous_and_unknown() {
        // Bare `/` opens palette.
        assert_eq!(resolve_submit("/"), FuzzyResolution::Palette(COMMANDS.iter().collect()));

        // Exact commands & args normalize cleanly.
        assert_eq!(resolve_submit("/mode"), FuzzyResolution::Exact("/mode".into()));
        assert_eq!(resolve_submit("/THEME  Nord "), FuzzyResolution::Exact("/theme nord".into()));
        assert_eq!(resolve_submit("/find  tar   gz"), FuzzyResolution::Exact("/find tar gz".into()));
        assert_eq!(resolve_submit("/pet name  Éclair "), FuzzyResolution::Exact("/pet name Éclair".into()));
        assert_eq!(resolve_submit("/lab  practice  2"), FuzzyResolution::Exact("/lab practice 2".into()));
        assert_eq!(resolve_submit("/speed 500ms"), FuzzyResolution::Exact("/speed 500ms".into()));
        assert_eq!(resolve_submit("/noise errors"), FuzzyResolution::Exact("/noise errors".into()));

        // Fuzzy command + sub-arg unique matches.
        assert_eq!(
            resolve_submit("/saf"),
            FuzzyResolution::Unique { resolved: "/safety".into(), note: "→ /safety".into() }
        );
        assert_eq!(
            resolve_submit("/thme"),
            FuzzyResolution::Unique { resolved: "/theme".into(), note: "→ /theme".into() }
        );
        assert_eq!(
            resolve_submit("/tisp"),
            FuzzyResolution::Unique { resolved: "/tips".into(), note: "→ /tips".into() }
        );
        assert_eq!(
            resolve_submit("/th cat"),
            FuzzyResolution::Unique { resolved: "/theme catppuccin".into(), note: "→ /theme catppuccin".into() }
        );
        assert_eq!(
            resolve_submit("/no er"),
            FuzzyResolution::Unique { resolved: "/noise errors".into(), note: "→ /noise errors".into() }
        );
        assert_eq!(
            resolve_submit("/pet peb"),
            FuzzyResolution::Unique { resolved: "/pet pebbles".into(), note: "→ /pet pebbles".into() }
        );
        assert_eq!(
            resolve_submit("/pet sp tur"),
            FuzzyResolution::Unique { resolved: "/pet species turtle".into(), note: "→ /pet species turtle".into() }
        );
        assert_eq!(
            resolve_submit("/tips n"),
            FuzzyResolution::Unique { resolved: "/tips next".into(), note: "→ /tips next".into() }
        );
        assert_eq!(
            resolve_submit("/pet sty px"),
            FuzzyResolution::Unique { resolved: "/pet style pixel".into(), note: "→ /pet style pixel".into() }
        );

        // Ambiguous commands and ambiguous sub-args.
        assert_eq!(resolve_submit("/s"), FuzzyResolution::Ambiguous(vec![&COMMANDS[3], &COMMANDS[6]]));
        assert_eq!(resolve_submit("/t"), FuzzyResolution::Ambiguous(vec![&COMMANDS[4], &COMMANDS[9]]));
        assert_eq!(resolve_submit("/pet st"), FuzzyResolution::Ambiguous(vec![&COMMANDS[5]]));
        assert_eq!(resolve_submit("/quiet o"), FuzzyResolution::Ambiguous(vec![&COMMANDS[8]]));
        assert_eq!(resolve_submit("/tips o"), FuzzyResolution::Ambiguous(vec![&COMMANDS[9]]));
        assert_eq!(resolve_submit("/pet style a"), FuzzyResolution::Ambiguous(vec![&COMMANDS[5]]));

        // Additional DL transpositions, numeric speed, and /pet subcommands.
        assert_eq!(
            resolve_submit("/mdoe"),
            FuzzyResolution::Unique { resolved: "/mode".into(), note: "→ /mode".into() }
        );
        assert_eq!(
            resolve_submit("/hlep"),
            FuzzyResolution::Unique { resolved: "/help".into(), note: "→ /help".into() }
        );
        assert_eq!(
            resolve_submit("/safty exp"),
            FuzzyResolution::Unique { resolved: "/safety expert".into(), note: "→ /safety expert".into() }
        );
        assert_eq!(
            resolve_submit("/theme nodr"),
            FuzzyResolution::Unique { resolved: "/theme nord".into(), note: "→ /theme nord".into() }
        );
        assert_eq!(resolve_submit("/theme bogus"), FuzzyResolution::Exact("/theme bogus".into()));
        assert_eq!(
            resolve_submit("/theme drk"),
            FuzzyResolution::Unique { resolved: "/theme dark".into(), note: "→ /theme dark".into() }
        );
        assert_eq!(resolve_submit("/speed 500"), FuzzyResolution::Exact("/speed 500".into()));
        assert_eq!(
            resolve_submit("/spd 500"),
            FuzzyResolution::Unique { resolved: "/speed 500".into(), note: "→ /speed 500".into() }
        );
        assert_eq!(
            resolve_submit("/pet n Pip"),
            FuzzyResolution::Unique { resolved: "/pet name Pip".into(), note: "→ /pet name Pip".into() }
        );
        assert_eq!(resolve_submit("/pet species"), FuzzyResolution::Exact("/pet species".into()));
        assert_eq!(resolve_submit("/pet style"), FuzzyResolution::Exact("/pet style".into()));
        assert_eq!(resolve_submit("/pet pebbles"), FuzzyResolution::Exact("/pet pebbles".into()));

        // Guardrail: `/finder`, `/modeled`, and unrelated commands are Unknown, never fuzzy-matched.
        assert_eq!(resolve_submit("/finder"), FuzzyResolution::Unknown("/finder".into()));
        assert_eq!(resolve_submit("/modeled"), FuzzyResolution::Unknown("/modeled".into()));
        assert_eq!(resolve_submit("/pwd"), FuzzyResolution::Unknown("/pwd".into()));
        assert_eq!(resolve_submit("/zzzz"), FuzzyResolution::Unknown("/zzzz".into()));
    }
}
