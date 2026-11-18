//! "Never mislead": checks run around the matcher. A wrong answer is worse than "No suggestion",
//! so these refuse requests huh must not answer with a command, drop suggestions that rest on weak
//! evidence, and make sure what is shown tells the truth (warnings, confidence).

use crate::assemble::{NO, Suggestion};
use crate::candidates::{Candidates, EntryKind};
use crate::catalog::{Command, SlotKind};
use crate::matcher::{Details, SlotPick};

fn words(request: &str) -> Vec<String> {
    request
        .to_lowercase()
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Requests huh must not answer with a command at all, with the reason to show.
pub fn refusal(request: &str) -> Option<String> {
    let w = words(request);
    let has = |word: &str| w.iter().any(|x| x == word);
    let any = |list: &[&str]| list.iter().any(|x| has(x));
    let hacking = w.iter().any(|x| x.starts_with("hack") || x.starts_with("phish"))
        || any(&["ddos", "keylogger", "exploit", "exploits"])
        || (any(&["crack", "cracking", "steal", "bypass"])
            && any(&["password", "passwords", "wifi", "wpa", "account", "login"]))
        || (has("break") && has("into") && any(&["account", "wifi", "network", "server", "computer"]))
        || (has("spy") && has("on"));
    if hacking {
        return Some(
            "that asks for access to someone else's system or account, and huh does not suggest commands for that."
                .into(),
        );
    }
    let whole_drive = any(&["disk", "drive", "usb", "partition", "stick", "pendrive", "ssd", "hdd", "nvme"])
        || (has("sd") && has("card"))
        || (has("flash") && has("drive"));
    let wipes = any(&["format", "reformat", "wipe", "erase", "nuke", "destroy", "zero", "zeroize"])
        || has("empty") && has("disk");
    if whole_drive && wipes && !any(&["cache", "history", "logs", "log", "trash", "space"]) {
        return Some(
            "formatting or wiping a drive erases everything on it, and a wrong device name destroys the wrong disk. \
             Use your desktop's disk tool (for example GNOME Disks or KDE Partition Manager), where you can see which drive is which."
                .into(),
        );
    }
    let installs = any(&["install", "download", "setup", "reinstall"]);
    let system = any(&["windows", "macos", "osx"]) || (has("mac") && has("os"));
    if installs && system && !any(&["terminal", "wsl", "subsystem", "font", "theme", "manager"]) {
        return Some("installing an operating system is not a terminal command; huh suggests commands for programs and packages.".into());
    }
    None
}

/// The command a safety question is about: "is rm -rf / dangerous", "is it safe to run X",
/// "should i run X". `known` says whether a word is a program. Such a question is answered by
/// `huh explain`, never by the command itself.
pub fn command_in_safety_question<'a>(request: &'a str, known: &dyn Fn(&str) -> bool) -> Option<&'a str> {
    let trimmed = request.trim().trim_end_matches(['?', '.', '!']).trim();
    let lower = trimmed.to_ascii_lowercase();
    const PREFIXES: &[&str] = &[
        "is it safe to run ",
        "is it safe to use ",
        "is it safe to type ",
        "is it safe to execute ",
        "is it safe to ",
        "is it ok to run ",
        "is it okay to run ",
        "is it dangerous to run ",
        "is it a bad idea to run ",
        "is it a good idea to run ",
        "would it be safe to run ",
        "can i safely run ",
        "should i run ",
        "should i use ",
        "should i type ",
        "should i execute ",
        "is it bad to run ",
        "what happens if i run ",
        "what happens when i run ",
        "what would happen if i run ",
    ];
    const SUFFIXES: &[&str] = &[
        " dangerous",
        " safe",
        " safe to run",
        " safe to use",
        " risky",
        " harmful",
        " bad",
        " ok",
        " okay",
        " ok to run",
        " okay to run",
        " destructive",
        " a bad idea",
        " a good idea",
        " going to break my system",
        " going to delete everything",
        " dangerous to run",
    ];
    let inner = if let Some(prefix) = PREFIXES.iter().find(|p| lower.starts_with(**p)) {
        &trimmed[prefix.len()..]
    } else {
        let rest = lower.strip_prefix("is ")?;
        let suffix = SUFFIXES.iter().find(|s| rest.ends_with(**s))?;
        &trimmed[3..3 + rest.len() - suffix.len()]
    };
    let inner = inner.trim().trim_matches(['`', '"', '\'']).trim();
    let mut parts = inner.split_whitespace();
    let mut first = parts.next()?;
    if first == "sudo" {
        first = parts.next()?;
    }
    let flagged = inner.split_whitespace().any(|w| w.len() > 1 && w.starts_with('-') || w.contains('='));
    (known(first) || flagged && first.chars().all(|c| c.is_ascii_lowercase())).then_some(inner)
}

/// Whether the request gives a dangerous suggestion something specific to act on: a value the
/// request names (a file, folder, pattern, port, ...) or a mode it spells out. A destructive
/// command with only placeholders rests on the verb alone ("delete stuff") and must not be offered.
pub fn has_evidence(command: &Command, details: &Details, request: &str) -> bool {
    // The request may name what kind of thing the command acts on ("close a program", "delete
    // everything in this folder"): the line is then a template with a placeholder, which cannot
    // run as it stands. "delete stuff" and "remove it" name nothing.
    let asked = words(request);
    let stem = |w: &str| w.trim_end_matches('s').to_string();
    let names_the_kind = command.slots.iter().filter(|s| s.kind != SlotKind::Choice).any(|slot| {
        let what = slot.what.as_deref().unwrap_or_default();
        let noun_phrase = what.rsplit_once(" to ").map_or(what, |(before, _)| before);
        noun_phrase
            .split_whitespace()
            .filter(|w| w.len() >= 4 && !matches!(*w, "name" | "number" | "that" | "this" | "only" | "with"))
            .any(|w| asked.iter().any(|a| stem(a) == stem(w)))
    });
    if names_the_kind {
        return true;
    }
    let mut has_value_slot = false;
    for (slot, pick) in command.slots.iter().zip(&details.slots) {
        match (slot.kind, pick) {
            (SlotKind::Choice, SlotPick::Value { .. }) => return true,
            (SlotKind::Choice, _) => {}
            (_, SlotPick::Value { named, value }) if *named >= NO && !value.is_empty() => return true,
            // A slot with a default can stay unnamed: it is not the object of the action.
            _ if slot.default.is_none() => has_value_slot = true,
            _ => {}
        }
    }
    !has_value_slot
}

/// A copy or move onto a file that exists overwrites it.
pub fn overwrite_warning(command: &Command, details: &Details, candidates: &Candidates) -> Option<String> {
    if !matches!(command.name.as_str(), "cp" | "mv") {
        return None;
    }
    let dest = command.slots.iter().zip(&details.slots).find_map(|(slot, pick)| match pick {
        SlotPick::Value { named, value } if slot.id == "dest" && *named >= NO => Some(value.clone()),
        _ => None,
    })?;
    (candidates.entry_kind(&dest) == EntryKind::File)
        .then(|| format!("{dest} already exists and would be overwritten by the copy; its old contents are lost."))
}

/// A line made only of placeholders says nothing about what the person wants.
pub fn only_placeholders(command: &Command, details: &Details) -> bool {
    let mut any_slot = false;
    for (slot, pick) in command.slots.iter().zip(&details.slots) {
        if slot.kind == SlotKind::Choice {
            if matches!(pick, SlotPick::Value { .. }) {
                return false;
            }
            continue;
        }
        if slot.default.is_some() {
            continue;
        }
        any_slot = true;
        if matches!(pick, SlotPick::Value { named, .. } if *named >= NO) {
            return false;
        }
    }
    any_slot
}

/// The most confidence a line of placeholders may claim.
pub const PLACEHOLDER_CONFIDENCE: f64 = 0.6;

/// Marks a suggestion that overwrites a file, and caps the confidence of placeholder-only lines.
pub fn tell_the_truth(suggestion: &mut Suggestion, command: &Command, details: &Details, candidates: &Candidates) {
    if let Some(why) = overwrite_warning(command, details, candidates) {
        suggestion.warning.get_or_insert(why);
        suggestion.destructive = true;
    }
    if only_placeholders(command, details) {
        suggestion.probability = suggestion.probability.min(PLACEHOLDER_CONFIDENCE);
    }
}
