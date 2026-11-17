//! Turns answers into command lines. The decision logic lives here; the threshold
//! constants it uses are in `questions.rs`, next to the question wording.
//!
//! | judgment                         | below NO          | NO..YES                    | above YES |
//! |----------------------------------|-------------------|----------------------------|-----------|
//! | command fits the request         | drop the command  | keep                       | keep      |
//! | flag is wanted                   | leave out         | ask; else leave out + note | add       |
//! | request names a path/text value  | default or `<id>` | ask; else use pick + note  | use pick  |
//!
//! "ask" means the `decide` callback: it gets a yes/no question for the person at
//! the keyboard and returns their answer, or `None` when nobody can be asked.

use crate::backend::{Answers, choice, noul};
use crate::candidates::Candidates;
use crate::catalog::{Command, Slot, SlotKind};
use crate::questions::{
    COMMAND, MIN_COMMAND_PROBABILITY, NO, YES, fit_id, item_id, named_id,
};
use anyhow::{Result, anyhow};
use std::collections::BTreeMap;


#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub command: String,
    pub line: String,
    pub probability: f64,
    pub description: String,
    pub destructive: bool,
    pub notes: Vec<String>,
    pub parts: Vec<(String, String)>,
}

/// The commands worth a second look, best first; always at least the top one.
pub fn shortlist(answers: &Answers, max: usize) -> Result<Vec<(String, f64)>> {
    let (_, probabilities) = choice(answers, COMMAND)?;
    let mut ranked: Vec<(String, f64)> = probabilities.iter().map(|(name, p)| (name.clone(), *p)).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    let keep = ranked.iter().filter(|(_, p)| *p >= MIN_COMMAND_PROBABILITY).count().clamp(1, max);
    ranked.truncate(keep);
    Ok(ranked)
}

/// Builds the command line for one shortlisted command, or `None` if it does not fit.
pub fn build(
    command: &Command,
    probability: f64,
    answers: &Answers,
    candidates: &Candidates,
    decide: &mut dyn FnMut(&str) -> Option<bool>,
) -> Result<Option<Suggestion>> {
    if noul(answers, &fit_id(&command.name))? < NO {
        return Ok(None);
    }
    let mut values: BTreeMap<&str, String> = BTreeMap::new();
    let mut notes = Vec::new();
    let mut parts = Vec::new();
    for id in crate::catalog::FACTS {
        if let Some(value) = crate::catalog::fact(id) {
            values.insert(id, value);
        }
    }

    for flag in &command.flags {
        let wanted = noul(answers, &item_id(&command.name, &flag.id))?;
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
            parts.push((flag.value.clone(), second_person_statement(&flag.question)));
        }
        values.insert(&flag.id, if add { flag.value.clone() } else { String::new() });
    }
    for slot in &command.slots {
        let value = match slot.kind {
            SlotKind::Choice => {
                let (picked, _) = choice(answers, &item_id(&command.name, &slot.id))?;
                let option = slot
                    .options
                    .iter()
                    .find(|o| o.name == picked)
                    .ok_or_else(|| anyhow!("backend picked unknown option `{picked}` for `{}`", slot.id))?;
                if !option.value.is_empty() {
                    parts.push((option.value.clone(), option.description.clone()));
                }
                option.value.clone()
            }
            SlotKind::Path | SlotKind::Text => {
                let (value, is_placeholder) =
                    picked_value(command, slot, answers, candidates, &mut notes, decide)?;
                if !is_placeholder && !value.is_empty() {
                    parts.push((value.clone(), slot.what.clone().unwrap_or_default()));
                }
                value
            }
        };
        values.insert(&slot.id, value);
    }

    Ok(Some(Suggestion {
        command: command.name.clone(),
        line: fill(&command.template, &values),
        probability,
        description: command.description.clone(),
        destructive: command.destructive,
        notes,
        parts,
    }))
}

fn picked_value(
    command: &Command,
    slot: &Slot,
    answers: &Answers,
    candidates: &Candidates,
    notes: &mut Vec<String>,
    decide: &mut dyn FnMut(&str) -> Option<bool>,
) -> Result<(String, bool)> {
    let wrap = |value: &str| match &slot.format {
        Some(format) => format.replace("{}", value),
        None => value.to_string(),
    };
    let unnamed = || match &slot.default {
        Some(default) => (default.clone(), false),
        None => (wrap(&format!("<{}>", slot.id)), true),
    };
    // No candidates means the questions were never asked.
    if candidates.for_slot(slot.kind).is_empty() {
        return Ok(unnamed());
    }
    let named = noul(answers, &named_id(&command.name, &slot.id))?;
    if named < NO {
        return Ok(unnamed());
    }
    let (picked, _) = choice(answers, &item_id(&command.name, &slot.id))?;
    if named <= YES {
        let what = slot.what.as_deref().unwrap_or(&slot.id);
        match decide(&format!("Is `{picked}` {what}?")) {
            Some(true) => {}
            Some(false) => return Ok(unnamed()),
            None => notes.push(format!("not sure `{picked}` is {what}")),
        }
    }
    // Inside the template's own single quotes only a single quote needs care.
    let value = if slot.raw { picked.replace('\'', "'\\''") } else { quote(picked) };
    Ok((wrap(&value), false))
}

/// Catalog questions talk about "the user"; shown to that user they should say "you".
fn second_person(question: &str) -> String {
    question
        .replace("Does the user", "Do you")
        .replace("Is the user", "Are you")
        .replace("the user", "you")
}

/// Turns a catalog yes/no flag question into a concise plain-English statement.
pub fn second_person_statement(question: &str) -> String {
    let trimmed = question
        .trim()
        .trim_end_matches('?')
        .replace("the user's", "your")
        .replace("their ", "your ");

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
            "to" | "as" | "in" | "into" | "inside" | "first" | "off" | "on" | "up" | "down" | "by"
                | "with" | "from" | "that"
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
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let Some(len) = rest[start..].find('}') else { break };
        out.push_str(&rest[..start]);
        if let Some(value) = values.get(&rest[start + 1..start + len]) {
            out.push_str(value);
        }
        rest = &rest[start + len + 1..];
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Answer;

    fn chmod() -> Command {
        toml::from_str(include_str!("../catalog/chmod.toml")).unwrap()
    }

    fn yes_no(p: f64) -> Answer {
        Answer::Noul { noul: p }
    }

    fn pick(name: &str) -> Answer {
        Answer::Choice { choice: name.into(), probabilities: BTreeMap::from([(name.to_string(), 0.9)]) }
    }

    fn chmod_answers(fit: f64, recursive: f64, named: f64) -> Answers {
        Answers::from([
            ("chmod.fit".into(), yes_no(fit)),
            ("chmod.recursive".into(), yes_no(recursive)),
            ("chmod.mode".into(), pick("make_executable")),
            ("chmod.target?".into(), yes_no(named)),
            ("chmod.target".into(), pick("deploy.sh")),
        ])
    }

    fn candidates() -> Candidates {
        Candidates::new("make deploy.sh executable", vec![])
    }

    #[test]
    fn builds_the_plain_case() {
        let got = build(&chmod(), 0.9, &chmod_answers(0.95, 0.02, 0.97), &candidates(), &mut |_| None).unwrap().unwrap();
        assert_eq!(got.line, "chmod +x deploy.sh");
        assert!(got.notes.is_empty());
    }

    #[test]
    fn adds_a_wanted_flag() {
        let got = build(&chmod(), 0.9, &chmod_answers(0.95, 0.9, 0.97), &candidates(), &mut |_| None).unwrap().unwrap();
        assert_eq!(got.line, "chmod -R +x deploy.sh");
    }

    #[test]
    fn unsure_flag_is_left_out_but_mentioned() {
        let got = build(&chmod(), 0.9, &chmod_answers(0.95, 0.5, 0.97), &candidates(), &mut |_| None).unwrap().unwrap();
        assert_eq!(got.line, "chmod +x deploy.sh");
        assert_eq!(got.notes.len(), 1);
    }

    #[test]
    fn unsure_flag_follows_the_answer_when_someone_can_be_asked() {
        let answers = chmod_answers(0.95, 0.5, 0.97);
        let mut asked = Vec::new();
        let yes = build(&chmod(), 0.9, &answers, &candidates(), &mut |q| {
            asked.push(q.to_string());
            Some(true)
        });
        let yes = yes.unwrap().unwrap();
        assert_eq!(yes.line, "chmod -R +x deploy.sh");
        assert!(yes.notes.is_empty());
        assert_eq!(asked, ["Add -R? Do you want the change applied to everything inside a folder?"]);
        let no = build(&chmod(), 0.9, &answers, &candidates(), &mut |_| Some(false)).unwrap().unwrap();
        assert_eq!(no.line, "chmod +x deploy.sh");
    }

    #[test]
    fn unsure_value_can_be_rejected() {
        let answers = chmod_answers(0.95, 0.02, 0.5);
        let no = build(&chmod(), 0.9, &answers, &candidates(), &mut |_| Some(false)).unwrap().unwrap();
        assert_eq!(no.line, "chmod +x <target>");
        let unasked = build(&chmod(), 0.9, &answers, &candidates(), &mut |_| None).unwrap().unwrap();
        assert_eq!(unasked.line, "chmod +x deploy.sh");
        assert_eq!(unasked.notes.len(), 1);
    }

    #[test]
    fn unnamed_value_becomes_a_placeholder() {
        let got = build(&chmod(), 0.9, &chmod_answers(0.95, 0.02, 0.05), &candidates(), &mut |_| None).unwrap().unwrap();
        assert_eq!(got.line, "chmod +x <target>");
    }

    #[test]
    fn command_that_does_not_fit_is_dropped() {
        assert_eq!(build(&chmod(), 0.9, &chmod_answers(0.1, 0.02, 0.97), &candidates(), &mut |_| None).unwrap(), None);
    }

    #[test]
    fn missing_answer_is_an_error_not_a_no() {
        let mut answers = chmod_answers(0.95, 0.02, 0.97);
        answers.remove("chmod.recursive");
        assert!(build(&chmod(), 0.9, &answers, &candidates(), &mut |_| None).is_err());
    }

    #[test]
    fn shortlist_keeps_likely_commands_best_first() {
        let answers = Answers::from([
            (
                "command".into(),
                Answer::Choice {
                    choice: "cp".into(),
                    probabilities: BTreeMap::from([
                        ("cp".to_string(), 0.6),
                        ("mv".to_string(), 0.3),
                        ("ls".to_string(), 0.05),
                        ("rm".to_string(), 0.05),
                    ]),
                },
            ),
        ]);
        let names: Vec<String> = shortlist(&answers, 3).unwrap().into_iter().map(|(name, _)| name).collect();
        assert_eq!(names, ["cp", "mv"]);
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
        assert_eq!(
            second_person_statement("Does the user want hidden files included?"),
            "Include hidden files"
        );
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
        let got = build(&chmod(), 0.9, &chmod_answers(0.95, 0.9, 0.97), &candidates(), &mut |_| None)
            .unwrap()
            .unwrap();
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
        let got = build(&chmod(), 0.9, &chmod_answers(0.95, 0.02, 0.05), &candidates(), &mut |_| None)
            .unwrap()
            .unwrap();
        assert_eq!(got.line, "chmod +x <target>");
        assert_eq!(
            got.parts,
            vec![("+x".to_string(), "The file should be runnable as a program or script".to_string())]
        );

        let cut: Command = toml::from_str(include_str!("../catalog/cut.toml")).unwrap();
        let cut_answers = Answers::from([
            ("cut.fit".into(), yes_no(0.95)),
            ("cut.delimiter".into(), pick("tab_or_unknown")),
            ("cut.field?".into(), yes_no(0.95)),
            ("cut.field".into(), pick("2")),
            ("cut.file?".into(), yes_no(0.95)),
            ("cut.file".into(), pick("data.tsv")),
        ]);
        let cut_cand = Candidates::new("column 2 of data.tsv", vec![]);
        let got_cut = build(&cut, 0.9, &cut_answers, &cut_cand, &mut |_| None).unwrap().unwrap();
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
