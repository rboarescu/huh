//! `huh explain`: breaks a shell command line into its parts and says what each
//! does, with warnings for risky parts. Nothing is ever run; see breakdown.rs
//! (engine), shell.rs (parser), manpage.rs, risk.rs and render.rs.

use crate::catalog::Command;

/// Explains a whole command line, part by part, with warnings. Runs nothing.
pub fn explain_command(catalog: &[Command], line: &str) -> crate::breakdown::Explanation {
    crate::breakdown::explain_line(catalog, line, crate::breakdown::Options::default())
}

/// Words a line of shell code can start with besides a program name.
const SHELL_STARTS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "function", "{", "}",
    "[[", "!", "time", "sudo", "env", "#!", "(", ")", ";;",
];

/// Piped text that is mostly not command lines, such as the output of `lsblk`: returns how many
/// of the lines that could start a command do (`plausible`, `total`). `None` when the text looks
/// like commands, or is too short to tell. Reads only PATH and the catalog; runs nothing.
pub fn looks_like_program_output(text: &str, catalog: &[Command]) -> Option<(usize, usize)> {
    let knowledge = crate::knowledge::Knowledge::get();
    let mut plausible = 0;
    let mut total = 0;
    let mut continued = false;
    let mut heredoc: Option<String> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(end) = &heredoc {
            if trimmed == end {
                heredoc = None;
            }
            continue;
        }
        if let Some(at) = trimmed.find("<<") {
            let delim = trimmed[at + 2..].trim_start_matches(['-', '~']).trim();
            let delim = delim.split_whitespace().next().unwrap_or("").trim_matches(['\'', '"']);
            if !delim.is_empty() {
                heredoc = Some(delim.to_string());
            }
        }
        let was_continued = continued;
        continued =
            trimmed.ends_with('\\') || trimmed.ends_with('|') || trimmed.ends_with("&&") || trimmed.ends_with("||");
        if trimmed.is_empty() || trimmed.starts_with('#') || was_continued {
            continue;
        }
        total += 1;
        let first = trimmed.split_whitespace().next().unwrap_or("");
        let is_assignment = first
            .split_once('=')
            .is_some_and(|(name, _)| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        let is_known = SHELL_STARTS.contains(&first)
            || is_assignment
            || crate::catalog::is_builtin(first)
            || catalog.iter().any(|c| c.program() == first)
            || knowledge.section(&[first.to_string()]).is_some()
            || (crate::manpage::safe_page_name(first)
                && !matches!(crate::platform::locate(first), crate::platform::Found::Missing))
            || (first.contains('/') && std::path::Path::new(first).is_file());
        if is_known {
            plausible += 1;
        }
    }
    (total >= 2 && plausible * 2 < total).then_some((plausible, total))
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
            let is_subcommand_flag = matches!(
                prog,
                "pacman" | "yay" | "paru" | "dpkg" | "rpm" | "apk" | "emerge" | "nix-env" | "xbps-query"
            ) && w.starts_with('-')
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

#[cfg(test)]
mod output_detection_tests {
    use super::*;

    fn catalog() -> Vec<Command> {
        crate::catalog::load_embedded().unwrap()
    }

    #[test]
    fn program_output_is_not_taken_for_a_script() {
        let lsblk = "NAME        MAJ:MIN RM   SIZE RO TYPE  MOUNTPOINTS
zram0       252:0    0  15.3G  0 disk  [SWAP]
nvme0n1     259:0    0 476.9G  0 disk
├─nvme0n1p1 259:1    0     2G  0 part  /boot
└─nvme0n1p2 259:2    0 474.9G  0 part
  └─root    253:0    0 474.9G  0 crypt /var/log
                                       /home
                                       /var/cache/pacman/pkg
                                       /
";
        assert!(looks_like_program_output(lsblk, &catalog()).is_some());
        let df = "Filesystem      Size  Used Avail Use% Mounted on\n/dev/nvme0n1p2  475G  120G  355G  26% /\ntmpfs           7.7G     0  7.7G   0% /dev/shm\n";
        assert!(looks_like_program_output(df, &catalog()).is_some());
    }

    #[test]
    fn scripts_are_still_scripts() {
        let script = "#!/usr/bin/env bash
set -euo pipefail
DIR=/tmp/build
mkdir -p \"$DIR\"
cd build && cmake .. \\
  -DCMAKE_BUILD_TYPE=Release
for f in *.log; do
  gzip \"$f\"
done
cat <<EOF > notes.txt
hello there
EOF
echo done
";
        assert_eq!(looks_like_program_output(script, &catalog()), None);
        assert_eq!(looks_like_program_output("ls -la", &catalog()), None);
    }
}
