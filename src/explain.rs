//! `huh explain`: breaks a shell command line into its parts and says what each
//! does, with warnings for risky parts. Nothing is ever run; see breakdown.rs
//! (engine), shell.rs (parser), manpage.rs, risk.rs and render.rs.

use crate::catalog::Command;

/// Explains a whole command line, part by part, with warnings. Runs nothing.
pub fn explain_command(catalog: &[Command], line: &str) -> crate::breakdown::Explanation {
    crate::breakdown::explain_line(catalog, line, crate::breakdown::Options::default())
}

/// Converts a catalog flag's yes/no question into a crisp second-person/imperative statement.
pub(crate) fn question_to_statement(question: &str) -> String {
    let q = question.trim().trim_end_matches('?').trim();
    if let Some(rest) = q.strip_prefix("Does the user want to ") {
        return capitalize(&second_person_pronouns(rest));
    }
    if let Some(rest) = q.strip_prefix("Does the user only want ") {
        return format!("Only show {}", second_person_pronouns(rest));
    }
    if let Some(rest) = q.strip_prefix("Does the user want ") {
        let rest = second_person_pronouns(rest);
        if let Some(before) = rest.strip_suffix(" included") {
            return format!("Include {before}");
        }
        if let Some((before, after)) = rest.split_once(" included in ") {
            return format!("Include {before} in {after}");
        }
        if let Some(before) = rest.strip_suffix(" shown") {
            return format!("Show {before}");
        }
        if let Some(before) = rest.strip_suffix(" turned off") {
            return format!("Turn {before} off");
        }
        if let Some(before) = rest.strip_suffix(" created") {
            return format!("Create {before}");
        }
        if let Some(before) = rest.strip_suffix(" searched") {
            return format!("Search {before}");
        }
        if let Some(before) = rest.strip_suffix(" listed first") {
            return format!("List {before} first");
        }
        if let Some(before) = rest.strip_suffix(" saved as a file") {
            return format!("Save {before} as a file");
        }
        if let Some((x, y)) = rest.split_once(" applied to ") {
            return format!("Apply {x} to {y}");
        }
        if let Some((x, y)) = rest.split_once(" ordered as ") {
            return format!("Order {x} as {y}");
        }
        if let Some(after) = rest.strip_prefix("the order reversed, ") {
            return format!("Reverse the order, {after}");
        }
        if let Some((x, y)) = rest.split_once(" treated as ") {
            return format!("Treat {x} as {y}");
        }
        if let Some((x, y)) = rest.split_once(" created inside ") {
            return format!("Create {x} inside {y}");
        }
        if let Some(subject) = rest.strip_suffix(" to keep running in the background") {
            return format!("Keep {subject} running in the background");
        }
        return format!("Include {rest}");
    }
    if let Some(rest) = q.strip_prefix("Does the user say ") {
        return format!("Used when {}", second_person_pronouns(rest));
    }
    if let Some(rest) = q.strip_prefix("Is the user asking about ") {
        return format!("Focus on {}", second_person_pronouns(rest));
    }
    if q == "Is the thing being deleted a folder" {
        return "Delete folders and everything inside them recursively".to_string();
    }
    if q == "Is the thing being copied a folder" {
        return "Copy folders and everything inside them recursively".to_string();
    }
    if q.starts_with("Is the thing being ") {
        return q.to_string();
    }
    capitalize(&second_person_pronouns(&q.replace("Does the user", "Do you").replace("Is the user", "Are you")))
}

fn second_person_pronouns(text: &str) -> String {
    text.replace("the user's", "your").replace("their", "your").replace("the user", "you")
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

/// Literal subcommand prefix words extracted from a catalog template's relevant pipeline stage.
/// Returns `(prefix_tokens, stage_text, is_primary_stage)`.
pub(crate) fn template_stage_prefix<'a>(
    template: &'a str,
    stage_program: &str,
) -> Option<(Vec<String>, &'a str, bool)> {
    for (stage_idx, stage) in template.split('|').enumerate() {
        let words: Vec<&str> = stage.split_whitespace().filter(|w| *w != "sudo").collect();
        let Some(&prog) = words.first() else { continue };
        if prog != stage_program {
            continue;
        }
        let mut prefix = Vec::new();
        for w in words {
            if w.starts_with('{') || w.contains('{') {
                break;
            }
            // Keep subcommand words (non-flags) and flag-discriminators for pacman/yay/systemctl.
            let is_subcommand_flag = matches!(prog, "pacman" | "yay") && w.starts_with('-')
                || (prog == "systemctl" && w == "--user")
                || (prog == "ip" && w == "-br");
            if w.starts_with('-') && !is_subcommand_flag {
                break;
            }
            // Stop before literal fixed options on single-command tools (e.g. `ping -c 4`, `virsh list --all`).
            if !prefix.is_empty() && w.chars().all(|c| c.is_ascii_digit()) {
                break;
            }
            prefix.push(w.to_string());
        }
        return Some((prefix, stage, stage_idx == 0));
    }
    None
}
