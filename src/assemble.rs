//! Turns the matcher's `Details` into command lines, using these thresholds:
//!
//! | judgment                         | below NO          | NO..YES                    | above YES |
//! |----------------------------------|-------------------|----------------------------|-----------|
//! | command fits the request         | drop the command  | keep                       | keep      |
//! | flag is wanted                   | leave out         | ask; else leave out + note | add       |
//! | request names a path/text value  | default or `<id>` | ask; else use pick + note  | use pick  |
//!
//! "ask" means the `decide` callback: it gets a yes/no question for the person at
//! the keyboard and returns their answer, or `None` when nobody can be asked.

use crate::catalog::{Command, Slot, SlotKind};
use crate::matcher::{Details, SlotPick};
use anyhow::{Result, bail};
use std::collections::BTreeMap;

/// Above YES a judgment counts as yes, below NO as no; in between it is unsettled.
pub const YES: f64 = 0.7;
pub const NO: f64 = 0.3;
/// How many commands get a second look, and the least probability a command
/// needs to be one of them (the best one always is).
pub const MAX_SUGGESTIONS: usize = 3;
pub const MIN_COMMAND_PROBABILITY: f64 = 0.10;
/// The WARNING text of a destructive command that gives no reason of its own.
pub const DEFAULT_WARNING: &str = "this deletes or overwrites data.";

#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub command: String,
    pub line: String,
    pub probability: f64,
    pub description: String,
    pub destructive: bool,
    /// Why the line is dangerous; set exactly when `destructive` is.
    pub warning: Option<String>,
    pub notes: Vec<String>,
    pub parts: Vec<(String, String)>,
}

/// The commands worth a second look from a ranking that is best first;
/// always at least the top one.
pub fn shortlist<T>(mut ranked: Vec<(T, f64)>) -> Vec<(T, f64)> {
    let keep = ranked.iter().filter(|(_, p)| *p >= MIN_COMMAND_PROBABILITY).count().clamp(1, MAX_SUGGESTIONS);
    ranked.truncate(keep);
    ranked
}

/// Builds the command line for one shortlisted command, or `None` if it does not fit.
/// `details` must come from the matcher for this same command.
pub fn build(
    command: &Command,
    probability: f64,
    details: &Details,
    decide: &mut dyn FnMut(&str) -> Option<bool>,
) -> Result<Option<Suggestion>> {
    if details.flags.len() != command.flags.len() || details.slots.len() != command.slots.len() {
        bail!("details do not match the flags and slots of `{}`", command.name);
    }
    if details.fit < NO {
        return Ok(None);
    }
    let mut values: BTreeMap<&str, String> = BTreeMap::new();
    let mut notes = Vec::new();
    let mut parts = Vec::new();
    let mut warning = command.destructive.then(|| command.warning.clone().unwrap_or_else(|| DEFAULT_WARNING.into()));
    for id in crate::catalog::FACTS {
        if let Some(value) = crate::catalog::fact(id) {
            values.insert(id, value);
        }
    }

    for (flag, &wanted) in command.flags.iter().zip(&details.flags) {
        let add = if wanted > YES {
            true
        } else if wanted >= NO {
            let asked = decide(&format!("Add {}? {}", flag.value, second_person(&flag.question)));
            if asked.is_none() {
                notes.push(format!("maybe add {}: {}", flag.value, second_person(&flag.question)));
            }
            asked.unwrap_or(false)
        } else {
            false
        };
        if add {
            if let Some(why) = &flag.warning {
                warning.get_or_insert_with(|| why.clone());
            }
            parts.push((flag.value.clone(), second_person_statement(&flag.question)));
        }
        values.insert(&flag.id, if add { flag.value.clone() } else { String::new() });
    }
    for (slot, pick) in command.slots.iter().zip(&details.slots) {
        let value = match (slot.kind, pick) {
            (SlotKind::Choice, SlotPick::Option(index)) => {
                let Some(option) = slot.options.get(*index) else {
                    bail!("no option {index} in slot `{}` of `{}`", slot.id, command.name);
                };
                if let Some(why) = &option.warning {
                    warning.get_or_insert_with(|| why.clone());
                }
                if !option.value.is_empty() {
                    parts.push((option.value.clone(), option.description.clone()));
                }
                option.value.clone()
            }
            // A value typed in the request for a choice slot ("chmod 755").
            (SlotKind::Choice, SlotPick::Value { value, .. }) => {
                if let Some(why) = risky_mode_warning(&command.name, value) {
                    warning.get_or_insert(why);
                }
                parts.push((value.clone(), format!("the mode you asked for: {}", describe_mode(value))));
                value.clone()
            }
            (SlotKind::Path | SlotKind::Text, SlotPick::Value { .. } | SlotPick::Unnamed) => {
                let (value, is_placeholder) = picked_value(slot, pick, &mut notes, decide);
                if !is_placeholder && !value.is_empty() {
                    parts.push((value.clone(), slot.what.clone().unwrap_or_default()));
                }
                value
            }
            _ => bail!("slot `{}` of `{}` got the wrong kind of value", slot.id, command.name),
        };
        values.insert(&slot.id, value);
    }

    // `python` is Python 2 or nothing on most systems; the version question means Python 3.
    if command.name == "program version"
        && let Some(value) = values.get_mut("program")
        && value == "python"
    {
        *value = "python3".into();
    }
    let line =
        package_line(command, &mut values, &mut notes, &mut parts).unwrap_or_else(|| fill(&command.template, &values));

    Ok(Some(Suggestion {
        command: command.name.clone(),
        line,
        probability,
        description: command.description.clone(),
        destructive: warning.is_some(),
        warning,
        notes,
        parts,
    }))
}

/// The warning for a `chmod` mode that lets every user on the machine change the file.
fn risky_mode_warning(command: &str, mode: &str) -> Option<String> {
    if command != "chmod" || mode.len() < 3 || !mode.chars().all(|c| ('0'..='7').contains(&c)) {
        return None;
    }
    let others = mode.chars().last()?.to_digit(8)?;
    match (others & 2 != 0, others & 1 != 0) {
        (true, true) => {
            Some("everyone can change and run it; on a shared or system file that is a security hole.".into())
        }
        (true, false) => Some("everyone can change it; on a shared or system file that is a security hole.".into()),
        _ => None,
    }
}

/// Reads an octal mode aloud: 600 is "owner read+write, nobody else".
fn describe_mode(mode: &str) -> String {
    let who = ["owner", "group", "others"];
    let digits: Vec<u32> = mode.chars().rev().take(3).filter_map(|c| c.to_digit(8)).collect();
    let rights = |d: u32| match d {
        0 => "nothing".to_string(),
        d => [(4, "read"), (2, "write"), (1, "run")]
            .iter()
            .filter(|(bit, _)| d & bit != 0)
            .map(|(_, n)| *n)
            .collect::<Vec<_>>()
            .join("+"),
    };
    digits.iter().rev().zip(who).map(|(d, w)| format!("{w} {}", rights(*d))).collect::<Vec<_>>().join(", ")
}

/// Package tasks: the person names a program ("install dig", "remove docker") and the system
/// wants a package name (`bind9-dnsutils`, `docker.io`). Puts the package name into the slot
/// and adds the remark about releases that lack it. Returns a whole replacement line when the
/// program has no package here (search for what provides it instead). Names data/platforms.toml
/// knows nothing about are used as typed.
fn package_line<'a>(
    command: &'a Command,
    values: &mut BTreeMap<&'a str, String>,
    notes: &mut Vec<String>,
    parts: &mut [(String, String)],
) -> Option<String> {
    use crate::platform::{self, Wanted};
    let role = command.pkg_role.as_deref()?;
    let slot = command.slots.first()?;
    let value = values.get(slot.id.as_str())?.clone();
    let platforms = crate::catalog::this_platform();
    if role == "pkg_provides" {
        // A search by program name (`bin/{}`, `*/bin/{}`) cannot take a path: look for its file name.
        if value.contains('/') && command.template.contains(&format!("/{{{}}}", slot.id)) {
            let name = value.trim_end_matches('/').rsplit('/').next().unwrap_or_default().to_string();
            values.insert(slot.id.as_str(), name);
        }
        return None;
    }
    if !matches!(role, "pkg_install" | "pkg_remove" | "pkg_query") || value.starts_with('<') || value.contains(' ') {
        return None;
    }
    match platform::wanted_package(&value, &platforms) {
        Wanted::AsTyped => None,
        Wanted::Package { name, note } => {
            for part in parts.iter_mut().filter(|(piece, _)| *piece == value) {
                part.0 = name.clone();
            }
            values.insert(slot.id.as_str(), name);
            if role == "pkg_install" {
                notes.extend(note);
            }
            None
        }
        Wanted::NoPackage { search } => {
            if role != "pkg_install" {
                return None;
            }
            match search {
                Some(search) => {
                    notes.push(format!(
                        "`{value}` has no package in this system's repositories: this searches for what provides it."
                    ));
                    Some(search)
                }
                None => None,
            }
        }
    }
}

/// The text for a path or text slot, and whether it is a `<placeholder>`.
fn picked_value(
    slot: &Slot,
    pick: &SlotPick,
    notes: &mut Vec<String>,
    decide: &mut dyn FnMut(&str) -> Option<bool>,
) -> (String, bool) {
    let wrap = |value: &str| match &slot.format {
        // `{plus1}` in a format: the value plus one ("top 10" is `head -n 11`, the header line counts).
        Some(format) if format.contains("{plus1}") => match value.parse::<u64>() {
            Ok(n) => format.replace("{plus1}", "").replace("{}", &(n + 1).to_string()),
            Err(_) => value.to_string(),
        },
        // `{abs}` in a format: the number without the sign that the `find -mtime` readers add ("+7" is "7").
        Some(format) if format.contains("{abs}") => {
            let digits = value.trim_start_matches(['+', '-']);
            if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                format.replace("{abs}", "").replace("{}", digits)
            } else {
                value.to_string()
            }
        }
        Some(format) => format.replace("{}", value),
        None => value.to_string(),
    };
    let unnamed = || match &slot.default {
        Some(default) => (default.clone(), false),
        None => (wrap(&format!("<{}>", slot.id)), true),
    };
    let SlotPick::Value { named, value: picked } = pick else {
        return unnamed();
    };
    if *named < NO {
        return unnamed();
    }
    if *named <= YES {
        let what = slot.what.as_deref().unwrap_or(&slot.id);
        match decide(&format!("Is `{picked}` {what}?")) {
            Some(true) => {}
            Some(false) => return unnamed(),
            None => notes.push(format!("not sure `{picked}` is {what}")),
        }
    }
    // Inside the template's own single quotes only a single quote needs care.
    let value = if slot.raw { picked.replace('\'', "'\\''") } else { quote(picked) };
    (wrap(&value), false)
}

/// Catalog questions talk about "the user"; shown to that user they should say "you".
fn second_person(question: &str) -> String {
    question.replace("Does the user", "Do you").replace("Is the user", "Are you").replace("the user", "you")
}

/// Turns a catalog yes/no flag question into a concise plain-English statement.
pub fn second_person_statement(question: &str) -> String {
    let trimmed = question.trim().trim_end_matches('?').replace("the user's", "your").replace("their ", "your ");

    if let Some(rest) = trimmed.strip_prefix("Does the user want to ") {
        return capitalize(&replace_user_pronouns(rest));
    }
    if let Some(rest) = trimmed.strip_prefix("Does the user only want to ") {
        return format!("Only {}", replace_user_pronouns(rest));
    }
    if let Some(rest) = trimmed.strip_prefix("Does the user only want ") {
        return format!("Only {}", replace_user_pronouns(rest));
    }
    if let Some(rest) = trimmed.strip_prefix("Does the user want ") {
        let clause = replace_user_pronouns(rest);
        if let Some(rewritten) = rewrite_passive_want_clause(&clause) {
            return rewritten;
        }
        return capitalize(&clause);
    }
    if let Some(rest) = trimmed.strip_prefix("Does the user say ") {
        return capitalize(&replace_user_pronouns(rest));
    }
    if let Some(rest) = trimmed.strip_prefix("Is the user asking about ") {
        return format!("Focus on {}", replace_user_pronouns(rest));
    }
    if let Some(rest) = trimmed.strip_prefix("Is ") {
        let clause = replace_user_pronouns(rest);
        if let Some((subject, predicate)) = clause.split_once(" a ") {
            return format!("{} is a {predicate}", capitalize(subject));
        }
        if let Some((subject, predicate)) = clause.split_once(" an ") {
            return format!("{} is an {predicate}", capitalize(subject));
        }
        return capitalize(&clause);
    }
    capitalize(&second_person(trimmed.as_str()))
}

fn replace_user_pronouns(text: &str) -> String {
    text.replace("the user", "you")
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn rewrite_passive_want_clause(clause: &str) -> Option<String> {
    if let Some((obj, rest)) = clause.split_once(" to keep ") {
        return Some(format!("Keep {obj} {rest}"));
    }
    const PARTICIPLES: &[(&str, &str)] = &[
        ("applied", "Apply"),
        ("shown", "Show"),
        ("saved", "Save"),
        ("included", "Include"),
        ("created", "Create"),
        ("treated", "Treat"),
        ("searched", "Search"),
        ("listed", "List"),
        ("ordered", "Order"),
        ("reversed", "Reverse"),
        ("turned", "Turn"),
        ("removed", "Remove"),
        ("deleted", "Delete"),
        ("enabled", "Enable"),
        ("disabled", "Disable"),
        ("ignored", "Ignore"),
        ("kept", "Keep"),
        ("printed", "Print"),
        ("sorted", "Sort"),
        ("used", "Use"),
    ];
    let words: Vec<&str> = clause.split_whitespace().collect();
    for (idx, raw_word) in words.iter().enumerate() {
        let word = raw_word.trim_end_matches(',');
        let Some((_, verb)) = PARTICIPLES.iter().find(|(p, _)| *p == word) else {
            continue;
        };
        let next = words.get(idx + 1).copied().unwrap_or("");
        let is_end = idx + 1 == words.len();
        let has_comma = raw_word.ends_with(',');
        let post_trigger = matches!(
            next,
            "to" | "as"
                | "in"
                | "into"
                | "inside"
                | "first"
                | "off"
                | "on"
                | "up"
                | "down"
                | "by"
                | "with"
                | "from"
                | "that"
        );
        if (is_end || has_comma || post_trigger) && idx > 0 {
            let obj = words[..idx].join(" ");
            let mut tail = words[idx + 1..].join(" ");
            if has_comma && !tail.is_empty() {
                tail = format!(", {tail}");
            } else if !tail.is_empty() {
                tail = format!(" {tail}");
            }
            return Some(format!("{verb} {obj}{tail}"));
        }
    }
    None
}

/// Single-quotes a value unless it is made only of characters the shell leaves alone.
pub fn quote(value: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "_./~+-=:,@%".contains(c);
    if !value.is_empty() && value.chars().all(safe) {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

/// Replaces `{id}` placeholders in one pass and tidies the spacing.
fn fill(template: &str, values: &BTreeMap<&str, String>) -> String {
    let mut out = String::new();
    let bytes = template.as_bytes();
    let (mut i, mut copied) = (0, 0);
    while i < bytes.len() {
        match bytes[i] {
            // `\{` and `\}` are literal braces (Go templates): the backslash is dropped.
            b'\\' if matches!(bytes.get(i + 1), Some(b'{' | b'}')) => {
                out.push_str(&template[copied..i]);
                copied = i + 1;
                i += 2;
            }
            b'{' => {
                let Some(len) = template[i..].find('}') else { break };
                out.push_str(&template[copied..i]);
                if len == 1 {
                    // `{}` is find's own placeholder (`-exec … {} +`), kept as written.
                    out.push_str("{}");
                } else if let Some(value) = values.get(&template[i + 1..i + len]) {
                    out.push_str(value);
                }
                i += len + 1;
                copied = i;
            }
            _ => i += 1,
        }
    }
    out.push_str(&template[copied..]);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaped_braces_are_literal() {
        let template = r"docker inspect -f '\{\{range .Networks\}\}\{\{.IPAddress\}\}\{\{end\}\}' {container} {}";
        assert_eq!(crate::catalog::placeholders(template), vec!["container"]);
        let mut values = BTreeMap::new();
        values.insert("container", "api".to_string());
        assert_eq!(fill(template, &values), "docker inspect -f '{{range .Networks}}{{.IPAddress}}{{end}}' api {}");
    }

    fn chmod() -> Command {
        toml::from_str(include_str!("../catalog/chmod.toml")).unwrap()
    }

    /// chmod has one flag (`recursive`) and two slots: `mode` (option 0 is
    /// `make_executable`) and `target`, here filled with deploy.sh.
    fn chmod_details(fit: f64, recursive: f64, named: f64) -> Details {
        Details {
            fit,
            flags: vec![recursive],
            slots: vec![SlotPick::Option(0), SlotPick::Value { named, value: "deploy.sh".into() }],
        }
    }

    #[test]
    fn builds_the_plain_case() {
        let got = build(&chmod(), 0.9, &chmod_details(0.95, 0.02, 0.97), &mut |_| None).unwrap().unwrap();
        assert_eq!(got.line, "chmod +x deploy.sh");
        assert!(got.notes.is_empty());
    }

    #[test]
    fn adds_a_wanted_flag() {
        let got = build(&chmod(), 0.9, &chmod_details(0.95, 0.9, 0.97), &mut |_| None).unwrap().unwrap();
        assert_eq!(got.line, "chmod -R +x deploy.sh");
    }

    #[test]
    fn unsure_flag_is_left_out_but_mentioned() {
        let got = build(&chmod(), 0.9, &chmod_details(0.95, 0.5, 0.97), &mut |_| None).unwrap().unwrap();
        assert_eq!(got.line, "chmod +x deploy.sh");
        assert_eq!(got.notes.len(), 1);
    }

    #[test]
    fn unsure_flag_follows_the_answer_when_someone_can_be_asked() {
        let details = chmod_details(0.95, 0.5, 0.97);
        let mut asked = Vec::new();
        let yes = build(&chmod(), 0.9, &details, &mut |q| {
            asked.push(q.to_string());
            Some(true)
        });
        let yes = yes.unwrap().unwrap();
        assert_eq!(yes.line, "chmod -R +x deploy.sh");
        assert!(yes.notes.is_empty());
        assert_eq!(asked, ["Add -R? Do you want the change applied to everything inside a folder?"]);
        let no = build(&chmod(), 0.9, &details, &mut |_| Some(false)).unwrap().unwrap();
        assert_eq!(no.line, "chmod +x deploy.sh");
    }

    #[test]
    fn unsure_value_can_be_rejected() {
        let details = chmod_details(0.95, 0.02, 0.5);
        let no = build(&chmod(), 0.9, &details, &mut |_| Some(false)).unwrap().unwrap();
        assert_eq!(no.line, "chmod +x <target>");
        let unasked = build(&chmod(), 0.9, &details, &mut |_| None).unwrap().unwrap();
        assert_eq!(unasked.line, "chmod +x deploy.sh");
        assert_eq!(unasked.notes.len(), 1);
    }

    #[test]
    fn unnamed_value_becomes_a_placeholder() {
        let got = build(&chmod(), 0.9, &chmod_details(0.95, 0.02, 0.05), &mut |_| None).unwrap().unwrap();
        assert_eq!(got.line, "chmod +x <target>");
    }

    #[test]
    fn command_that_does_not_fit_is_dropped() {
        assert_eq!(build(&chmod(), 0.9, &chmod_details(0.1, 0.02, 0.97), &mut |_| None).unwrap(), None);
    }

    #[test]
    fn unnamed_slot_without_candidates_becomes_a_placeholder() {
        let mut details = chmod_details(0.95, 0.02, 0.97);
        details.slots[1] = SlotPick::Unnamed;
        let got = build(&chmod(), 0.9, &details, &mut |_| None).unwrap().unwrap();
        assert_eq!(got.line, "chmod +x <target>");
    }

    #[test]
    fn mismatched_details_are_an_error_not_a_no() {
        let mut missing_flag = chmod_details(0.95, 0.02, 0.97);
        missing_flag.flags.clear();
        assert!(build(&chmod(), 0.9, &missing_flag, &mut |_| None).is_err());

        let mut bad_option = chmod_details(0.95, 0.02, 0.97);
        bad_option.slots[0] = SlotPick::Option(99);
        assert!(build(&chmod(), 0.9, &bad_option, &mut |_| None).is_err());

        let mut wrong_kind = chmod_details(0.95, 0.02, 0.97);
        wrong_kind.slots[0] = SlotPick::Unnamed;
        assert!(build(&chmod(), 0.9, &wrong_kind, &mut |_| None).is_err());
    }

    #[test]
    fn shortlist_keeps_likely_commands_best_first() {
        let ranked = vec![("cp", 0.6), ("mv", 0.3), ("ls", 0.05), ("rm", 0.05)];
        let names: Vec<&str> = shortlist(ranked).into_iter().map(|(name, _)| name).collect();
        assert_eq!(names, ["cp", "mv"]);
        // The best command is kept even when nothing is likely.
        assert_eq!(shortlist(vec![("ls", 0.04), ("rm", 0.03)]).len(), 1);
    }

    #[test]
    fn quotes_only_when_needed() {
        assert_eq!(quote("src/main.rs"), "src/main.rs");
        assert_eq!(quote("hello world"), "'hello world'");
        assert_eq!(quote("*.txt"), "'*.txt'");
        assert_eq!(quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn converts_flag_questions_to_plain_statements() {
        assert_eq!(
            second_person_statement("Does the user want the change applied to everything inside a folder?"),
            "Apply the change to everything inside a folder"
        );
        assert_eq!(second_person_statement("Does the user want hidden files included?"), "Include hidden files");
        assert_eq!(
            second_person_statement("Does the user want to keep watching the file for new lines as they are written?"),
            "Keep watching the file for new lines as they are written"
        );
        assert_eq!(
            second_person_statement("Does the user want the most recently changed entries listed first?"),
            "List the most recently changed entries first"
        );
        assert_eq!(
            second_person_statement("Is the thing being copied a folder?"),
            "The thing being copied is a folder"
        );
        assert_eq!(
            second_person_statement("Does the user want their microphone or voice included in the recording?"),
            "Include your microphone or voice in the recording"
        );
    }

    #[test]
    fn populates_parts_for_flags_choices_and_filled_slots() {
        let got = build(&chmod(), 0.9, &chmod_details(0.95, 0.9, 0.97), &mut |_| None).unwrap().unwrap();
        assert_eq!(got.line, "chmod -R +x deploy.sh");
        assert_eq!(
            got.parts,
            vec![
                ("-R".to_string(), "Apply the change to everything inside a folder".to_string()),
                ("+x".to_string(), "The file should be runnable as a program or script".to_string()),
                ("deploy.sh".to_string(), "the file or folder whose permissions should change".to_string()),
            ]
        );
    }

    #[test]
    fn omits_unfilled_placeholder_and_empty_options_from_parts() {
        let got = build(&chmod(), 0.9, &chmod_details(0.95, 0.02, 0.05), &mut |_| None).unwrap().unwrap();
        assert_eq!(got.line, "chmod +x <target>");
        assert_eq!(
            got.parts,
            vec![("+x".to_string(), "The file should be runnable as a program or script".to_string())]
        );

        let cut: Command = toml::from_str(include_str!("../catalog/cut.toml")).unwrap();
        let cut_details = Details {
            fit: 0.95,
            flags: vec![],
            slots: vec![
                SlotPick::Option(0), // tab_or_unknown
                SlotPick::Value { named: 0.95, value: "2".into() },
                SlotPick::Value { named: 0.95, value: "data.tsv".into() },
            ],
        };
        let got_cut = build(&cut, 0.9, &cut_details, &mut |_| None).unwrap().unwrap();
        assert_eq!(got_cut.line, "cut -f2 data.tsv");
        assert_eq!(
            got_cut.parts,
            vec![
                ("2".to_string(), "the number of the column to show".to_string()),
                ("data.tsv".to_string(), "the file to read".to_string()),
            ]
        );
    }
}
