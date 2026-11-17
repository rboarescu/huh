//! `huh explain`: breaks a shell command line into its parts and says what each does.
//! Nothing is ever run.

use crate::catalog::{Command, SlotKind};

#[derive(Debug, Clone, PartialEq)]
enum ShellToken {
    Word { raw: String, unquoted: String, quoted: bool },
    Op(String),
}

impl ShellToken {
    fn as_word(&self) -> Option<(&str, &str, bool)> {
        match self {
            ShellToken::Word { raw, unquoted, quoted } => Some((raw, unquoted, *quoted)),
            ShellToken::Op(_) => None,
        }
    }
}

fn tokenize_shell(line: &str) -> Vec<ShellToken> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }

        // Check multi-character operators first (outside quotes).
        let rest: String = chars[i..].iter().take(4).collect();
        let matched_op = ["2>&1", "&>>", "&&", "||", ">>", "2>>", "2>", "&>", "|", ";", ">", "<"]
            .into_iter()
            .find(|op| rest.starts_with(op));
        if let Some(op) = matched_op {
            tokens.push(ShellToken::Op(op.to_string()));
            i += op.chars().count();
            continue;
        }

        let mut raw = String::new();
        let mut unquoted = String::new();
        let mut was_quoted = false;
        let mut quote: Option<char> = None;

        while i < chars.len() {
            let c = chars[i];
            match quote {
                Some('\'') => {
                    raw.push(c);
                    if c == '\'' {
                        quote = None;
                    } else {
                        unquoted.push(c);
                    }
                    i += 1;
                }
                Some('"') => {
                    raw.push(c);
                    if c == '\\' && i + 1 < chars.len() {
                        i += 1;
                        raw.push(chars[i]);
                        unquoted.push(chars[i]);
                        i += 1;
                    } else if c == '"' {
                        quote = None;
                        i += 1;
                    } else {
                        unquoted.push(c);
                        i += 1;
                    }
                }
                Some(_) => unreachable!(),
                None => {
                    if c.is_whitespace() {
                        break;
                    }
                    let lookahead: String = chars[i..].iter().take(4).collect();
                    if ["2>&1", "&>>", "&&", "||", ">>", "2>>", "2>", "&>", "|", ";", ">", "<"]
                        .iter()
                        .any(|op| lookahead.starts_with(op))
                    {
                        break;
                    }
                    if c == '\\' && i + 1 < chars.len() {
                        raw.push(c);
                        i += 1;
                        raw.push(chars[i]);
                        unquoted.push(chars[i]);
                        i += 1;
                    } else if c == '\'' || c == '"' {
                        was_quoted = true;
                        raw.push(c);
                        quote = Some(c);
                        i += 1;
                    } else {
                        raw.push(c);
                        unquoted.push(c);
                        i += 1;
                    }
                }
            }
        }
        tokens.push(ShellToken::Word { raw, unquoted, quoted: was_quoted });
    }
    tokens
}

fn split_pipeline_segments(tokens: &[ShellToken]) -> Vec<(Vec<ShellToken>, Option<String>)> {
    let mut segments = Vec::new();
    let mut current = Vec::new();
    for tok in tokens {
        match tok {
            ShellToken::Op(op) if matches!(op.as_str(), "|" | "&&" | "||" | ";") => {
                segments.push((std::mem::take(&mut current), Some(op.clone())));
            }
            other => current.push(other.clone()),
        }
    }
    if !current.is_empty() || segments.is_empty() {
        segments.push((current, None));
    }
    segments
}

fn explain_operator(op: &str) -> &'static str {
    match op {
        "|" => "pipes standard output from the previous command into the next command",
        "&&" => "runs the next command only if the previous command succeeded",
        "||" => "runs the next command only if the previous command failed",
        ";" => "runs the next command unconditionally after the previous command finishes",
        ">" => "overwrites the target file with standard output",
        ">>" => "appends standard output to the end of the target file",
        "<" => "reads standard input from the target file",
        "2>&1" => "merges standard error (stderr) into standard output (stdout)",
        "2>" => "overwrites the target file with standard error",
        "2>>" => "appends standard error to the target file",
        "&>" | "&>>" => "redirects both standard output and standard error to a file",
        _ => "shell operator",
    }
}

/// Converts a catalog flag's yes/no question into a crisp second-person/imperative statement.
fn question_to_statement(question: &str) -> String {
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
fn template_stage_prefix<'a>(template: &'a str, stage_program: &str) -> Option<(Vec<String>, &'a str, bool)> {
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

fn match_echo_command<'a>(
    catalog: &'a [Command],
    words: &[&ShellToken],
) -> Option<(&'a Command, usize, String, &'a str)> {
    let arg = words.get(1).and_then(|t| t.as_word()).map(|(_, u, _)| u).unwrap_or("");
    let target_name = if arg.contains("export PATH=") {
        "add to PATH"
    } else if arg.starts_with("alias ") {
        "add shell alias"
    } else if arg.starts_with("export ") {
        "set shell variable"
    } else {
        return None;
    };
    catalog.iter().find(|c| c.name == target_name).map(|c| (c, 1, "echo".to_string(), c.template.as_str()))
}

fn match_catalog_command<'a>(
    catalog: &'a [Command],
    words: &[&ShellToken],
) -> Option<(&'a Command, usize, String, &'a str)> {
    let (_, prog, _) = words.first()?.as_word()?;
    if prog == "echo" {
        return match_echo_command(catalog, words);
    }

    let word_strings: Vec<&str> = words.iter().filter_map(|t| t.as_word().map(|(_, u, _)| u)).collect();

    let mut best: Option<(&Command, usize, String, &str, bool)> = None;
    let mut primary_multiword_only = false;
    let mut primary_found = false;

    for cmd in catalog {
        let Some((prefix, stage, is_primary)) = template_stage_prefix(&cmd.template, prog) else {
            continue;
        };
        if is_primary {
            primary_found = true;
            if prefix.len() > 1 {
                primary_multiword_only = true;
            }
        }
        if word_strings.len() >= prefix.len() && prefix.iter().enumerate().all(|(i, p)| word_strings[i] == p) {
            let label = prefix.join(" ");
            let better = best.as_ref().is_none_or(|(_, len, _, _, best_primary)| {
                (is_primary && !*best_primary) || (is_primary == *best_primary && prefix.len() > *len)
            });
            if better {
                best = Some((cmd, prefix.len(), label, stage, is_primary));
            }
        }
    }

    if let Some((cmd, len, label, stage, _)) = best {
        return Some((cmd, len, label, stage));
    }
    if primary_found && !primary_multiword_only {
        return catalog.iter().find_map(|c| {
            let (prefix, stage, is_primary) = template_stage_prefix(&c.template, prog)?;
            (is_primary && prefix.first().is_some_and(|p| p == prog)).then(|| (c, 1, prog.to_string(), stage))
        });
    }
    None
}

fn explain_common_flag(program: &str, flag: &str) -> Option<&'static str> {
    // Program-specific well-known flags
    match (program, flag) {
        ("du" | "df" | "ls" | "free", "-h") => {
            return Some("human-readable: print sizes in easy-to-read units (e.g. 1K, 234M, 2G)");
        }
        ("sort", "-h") => return Some("human-numeric-sort: compare human-readable numbers such as 2K or 1G"),
        ("du", "-s") => return Some("summarize: display only a total for each argument"),
        ("ln", "-s") => return Some("create a symbolic (soft) link instead of a hard link"),
        ("curl", "-L") => return Some("follow HTTP redirects automatically"),
        ("curl", "-s") => return Some("silent mode: hide progress meter and error messages"),
        ("curl", "-S") => return Some("show errors even when silent mode (-s) is enabled"),
        ("curl", "-k") | ("curl", "--insecure") => {
            return Some("allow insecure TLS/SSL connections without verifying certificates");
        }
        ("curl", "-I") => return Some("fetch only the HTTP response headers"),
        ("rsync", "-a") => {
            return Some("archive mode: preserve permissions, timestamps, symlinks, and recurse into folders");
        }
        ("rsync", "-z") => return Some("compress file data during transfer"),
        ("tar", "-x") => return Some("extract files from an archive"),
        ("tar", "-c") => return Some("create a new archive"),
        ("tar", "-t") => return Some("list the contents of an archive"),
        ("tar", "-z") => return Some("filter the archive through gzip compression"),
        ("tar", "-j") => return Some("filter the archive through bzip2 compression"),
        ("tar", "-J") => return Some("filter the archive through xz compression"),
        ("tar", "-f") => return Some("specify the archive filename"),
        ("ss", "-t") => return Some("show TCP sockets"),
        ("ss", "-u") => return Some("show UDP sockets"),
        ("ss", "-l") => return Some("show only listening sockets"),
        ("ss", "-p") => return Some("show the process using each socket"),
        ("ss", "-n") => return Some("show numeric ports and addresses instead of resolving names"),
        ("ping", "-c") => return Some("stop after sending the specified number of packets"),
        ("head" | "tail" | "journalctl", "-n") => return Some("specify the number of lines to display"),
        ("tracepath", "-n") => return Some("print IP addresses numerically without resolving hostnames"),
        ("pgrep", "-a") => return Some("list both process IDs and full command lines"),
        ("pgrep", "-l") => return Some("list process IDs along with process names"),
        ("uname", "-a") => return Some("print all system and kernel information"),
        ("git", "-m") => return Some("provide the commit message directly on the command line"),
        ("docker", "-d") => return Some("detached mode: run containers in the background"),
        ("docker", "-i") => return Some("keep standard input open even if not attached"),
        ("docker", "-t") => return Some("allocate a pseudo-TTY terminal"),
        ("docker", "-p") => return Some("publish a container port to the host"),
        ("mkdir", "-p") => return Some("create parent directories as needed without error if they exist"),
        ("tee", "-a") => return Some("append to the file instead of overwriting it"),
        ("grep", "-E") => return Some("use extended regular expressions"),
        ("grep", "-F") => return Some("treat the pattern as a fixed literal string"),
        ("grep", "-v") => return Some("invert match: select lines that do not match"),
        ("grep", "-w") => return Some("match whole words only"),
        ("grep", "-c") => return Some("print only a count of matching lines"),
        ("cat", "-n") => return Some("number all output lines"),
        ("ps", "aux") => return Some("show all running processes for all users with user, CPU, and memory details"),
        ("tmux", "-s") => return Some("name the new tmux session"),
        ("tmux", "-t") => return Some("target a specific tmux session"),
        ("smartctl", "-a") => return Some("print all SMART health and diagnostic information for the device"),
        ("usermod", "-aG") => return Some("append the user to supplementary groups without removing existing ones"),
        ("usermod", "-a") => return Some("append the user to supplementary groups (used with -G)"),
        ("usermod", "-G") => return Some("specify supplementary group(s) for the user"),
        _ => {}
    }
    // General Unix flags
    match flag {
        "-h" | "--help" => Some("display help and usage information"),
        "-V" | "--version" => Some("display the program version"),
        "-v" | "--verbose" => Some("verbose output: show more detail about what is happening"),
        "-q" | "--quiet" | "--silent" => Some("quiet mode: suppress normal output"),
        "-f" | "--force" => Some("force the operation without prompting or failing on missing targets"),
        "-r" | "-R" | "--recursive" => Some("operate recursively on folders and everything inside them"),
        "-a" | "--all" => Some("include all entries, including hidden or inactive ones"),
        "-y" | "--yes" | "--noconfirm" => Some("answer yes automatically to confirmation prompts"),
        "--dry-run" => Some("preview what would happen without making changes"),
        "-i" | "--interactive" => Some("prompt for confirmation before modifying targets"),
        _ => None,
    }
}

fn explain_flag_in_command(cmd: &Command, program: &str, flag: &str) -> Option<String> {
    if let Some(f) = cmd.flags.iter().find(|f| f.value == flag) {
        return Some(question_to_statement(&f.question));
    }
    if let Some(desc) = explain_common_flag(program, flag) {
        return Some(desc.to_string());
    }
    None
}

fn split_segment_redirections(tokens: &[ShellToken]) -> (Vec<ShellToken>, Vec<(String, Option<String>)>) {
    let mut cmd_tokens = Vec::new();
    let mut redirs = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if let ShellToken::Op(op) = &tokens[i] {
            if matches!(op.as_str(), "2>&1") {
                redirs.push((op.clone(), None));
                i += 1;
                continue;
            }
            if matches!(op.as_str(), ">" | ">>" | "<" | "2>" | "2>>" | "&>" | "&>>") {
                let target = tokens.get(i + 1).and_then(|t| t.as_word()).map(|(raw, _, _)| raw.to_string());
                if target.is_some() {
                    i += 2;
                } else {
                    i += 1;
                }
                redirs.push((op.clone(), target));
                continue;
            }
        }
        cmd_tokens.push(tokens[i].clone());
        i += 1;
    }
    (cmd_tokens, redirs)
}

pub fn explain_command(catalog: &[Command], line: &str) -> Vec<(String, String)> {
    let tokens = tokenize_shell(line);
    let segments = split_pipeline_segments(&tokens);
    let mut out = Vec::new();
    let mut after_pipe = false;

    for (seg_tokens, next_op) in segments {
        explain_segment(catalog, &seg_tokens, after_pipe, &mut out);
        if let Some(op) = &next_op {
            out.push((op.clone(), explain_operator(op).to_string()));
        }
        after_pipe = next_op.as_deref() == Some("|");
    }
    out
}

fn explain_segment(catalog: &[Command], tokens: &[ShellToken], after_pipe: bool, out: &mut Vec<(String, String)>) {
    let (cmd_tokens, redirs) = split_segment_redirections(tokens);
    let mut idx = 0;
    while idx < cmd_tokens.len() {
        if let Some((_, unquoted, _)) = cmd_tokens[idx].as_word() {
            if unquoted == "sudo" {
                out.push(("sudo".into(), "run this command with administrator (root) privileges".into()));
                idx += 1;
                continue;
            }
            if unquoted.contains('=') && !unquoted.starts_with('-') && idx + 1 < cmd_tokens.len() {
                out.push((unquoted.to_string(), "set an environment variable for this command".into()));
                idx += 1;
                continue;
            }
        }
        break;
    }

    let words: Vec<&ShellToken> = cmd_tokens[idx..].iter().collect();
    if words.is_empty() {
        emit_redirs(&redirs, out);
        return;
    }

    let matched = match_catalog_command(catalog, &words);
    if let Some((cmd, consumed, label, stage)) = matched {
        let mut desc = if after_pipe && cmd.name == "grep" {
            "Filter matching lines from the piped output".to_string()
        } else {
            cmd.description.clone()
        };
        if cmd.destructive {
            desc.push_str(" — WARNING: deletes or overwrites data");
        }
        out.push((label, desc));

        let program = words[0].as_word().map_or("", |(_, u, _)| u);
        let pos_slots: Vec<&crate::catalog::Slot> = cmd
            .slots
            .iter()
            .filter(|s| {
                matches!(s.kind, SlotKind::Path | SlotKind::Text)
                    && s.format.is_none()
                    && stage.contains(&format!("{{{}}}", s.id))
                    && !stage.contains(&format!("-f{{{}}}", s.id))
                    && !stage.contains(&format!("-n {{{}}}", s.id))
            })
            .collect();
        let mut pos_idx = 0usize;
        let mut wi = consumed;

        while wi < words.len() {
            let Some((raw, unquoted, quoted)) = words[wi].as_word() else {
                wi += 1;
                continue;
            };

            // 1. Check 2-token choice option values like `-type f`, `-size +10M`.
            if !quoted
                && wi + 1 < words.len()
                && let Some((_, next_u, _)) = words[wi + 1].as_word()
            {
                let pair = format!("{unquoted} {next_u}");
                if let Some(opt) = cmd
                    .slots
                    .iter()
                    .filter(|s| s.kind == SlotKind::Choice)
                    .flat_map(|s| &s.options)
                    .find(|o| o.value == pair)
                {
                    out.push((pair, opt.description.clone()));
                    wi += 2;
                    continue;
                }
            }

            // 2. Check formatted slots like `-name {}`, `-mtime -{}`, `-u {}`, `-n {} --no-pager`.
            if !quoted
                && let Some(slot) = cmd.slots.iter().find(|s| {
                    s.format.as_deref().is_some_and(|fmt| {
                        let prefix = fmt.split_whitespace().next().unwrap_or("");
                        prefix == unquoted
                    })
                })
                && wi + 1 < words.len()
                && let Some((arg_raw, arg_u, _)) = words[wi + 1].as_word()
            {
                let what = slot.what.as_deref().unwrap_or(&slot.id);
                let val_clean = if slot.format.as_deref().is_some_and(|f| f.contains("-{}")) {
                    arg_u.trim_start_matches('-')
                } else {
                    arg_u
                };
                out.push((format!("{unquoted} {arg_raw}"), format!("{what} ({val_clean})")));
                wi += 2;
                // Consume trailing `--no-pager` if part of the slot's format.
                if slot.format.as_deref().is_some_and(|f| f.ends_with("--no-pager"))
                    && words.get(wi).and_then(|t| t.as_word()).is_some_and(|(_, u, _)| u == "--no-pager")
                {
                    out.push(("--no-pager".into(), "print output directly without opening a pager".into()));
                    wi += 1;
                }
                continue;
            }

            // 3. Check template inline `-f{field}` (e.g. `cut -f3` or `cut -f 3`).
            if !quoted
                && (unquoted == "-f" || (unquoted.starts_with("-f") && unquoted.len() > 2))
                && cmd.template.contains("-f{")
                && let Some(slot) = cmd.slots.iter().find(|s| s.id == "field")
            {
                let what = slot.what.as_deref().unwrap_or("field number");
                if unquoted == "-f"
                    && wi + 1 < words.len()
                    && let Some((_, next_u, _)) = words[wi + 1].as_word()
                {
                    out.push((format!("-f {next_u}"), format!("{what} ({next_u})")));
                    wi += 2;
                } else {
                    out.push((unquoted.to_string(), format!("{what} ({})", &unquoted[2..])));
                    wi += 1;
                }
                continue;
            }

            // 4. Check single-token choice option values (like `+x`, `600`, `-xzf`, `-l`, `-d,`, `start`, `status`).
            if !quoted
                && let Some(opt) = cmd
                    .slots
                    .iter()
                    .filter(|s| s.kind == SlotKind::Choice)
                    .flat_map(|s| &s.options)
                    .find(|o| !o.value.is_empty() && (o.value == unquoted || o.value == raw))
            {
                out.push((raw.to_string(), opt.description.clone()));
                wi += 1;
                continue;
            }

            // 5. Check exact flag in `cmd.flags` or `explain_common_flag`.
            if !quoted && (unquoted.starts_with('-') || (program == "ps" && unquoted == "aux")) {
                if let Some(desc) = explain_flag_in_command(cmd, program, unquoted) {
                    out.push((unquoted.to_string(), desc));
                    // Consume numeric value right after `-n` or `-c` if the template expects a count/arg.
                    if matches!(unquoted, "-n" | "-c")
                        && wi + 1 < words.len()
                        && let Some((_, next_u, _)) = words[wi + 1].as_word()
                        && next_u.chars().all(|c| c.is_ascii_digit())
                    {
                        if let Some(slot) = cmd.slots.iter().find(|s| s.id == "count") {
                            let what = slot.what.as_deref().unwrap_or("count");
                            out.push((next_u.to_string(), what.to_string()));
                        } else {
                            out.push((next_u.to_string(), "number / count argument".to_string()));
                        }
                        wi += 2;
                        continue;
                    }
                    wi += 1;
                    continue;
                }

                // 6. Combined short flags like `-la`, `-sh`, `-rf`, `-xzf`.
                if unquoted.starts_with('-')
                    && !unquoted.starts_with("--")
                    && unquoted.len() > 2
                    && !unquoted[1..].chars().all(|c| c.is_ascii_digit())
                {
                    let chars: Vec<char> = unquoted[1..].chars().collect();
                    let parts: Vec<(String, Option<String>)> = chars
                        .iter()
                        .map(|c| {
                            let single = format!("-{c}");
                            let desc = explain_flag_in_command(cmd, program, &single);
                            (single, desc)
                        })
                        .collect();
                    if parts.iter().any(|(_, d)| d.is_some()) {
                        for (single, desc) in parts {
                            out.push((single, desc.unwrap_or_else(|| "not in the catalog".to_string())));
                        }
                        wi += 1;
                        continue;
                    }
                }

                out.push((raw.to_string(), "not in the catalog".to_string()));
                wi += 1;
                continue;
            }

            // 7. Literal `$USER` in template (e.g. `sudo usermod -aG {group} $USER`).
            if unquoted == "$USER" && stage.split_whitespace().any(|w| w == "$USER") {
                out.push(("$USER".to_string(), "your current username (the user account)".to_string()));
                wi += 1;
                continue;
            }

            // 8. Positional argument -> match against remaining Path/Text slots.
            if pos_idx < pos_slots.len() {
                let slot = pos_slots[pos_idx];
                let what = slot.what.as_deref().unwrap_or(&slot.id);
                let detail = if unquoted == "." && slot.kind == SlotKind::Path {
                    format!("{what} (current folder)")
                } else if unquoted == "*" && slot.kind == SlotKind::Path {
                    format!("{what} (wildcard matching all items in the current folder)")
                } else if let Some((dir, "*")) = unquoted.rsplit_once('/')
                    && slot.kind == SlotKind::Path
                {
                    format!("{what} (wildcard matching all items inside {dir})")
                } else if unquoted.contains('*') && slot.kind == SlotKind::Path {
                    format!("{what} (wildcard pattern matching multiple items)")
                } else {
                    what.to_string()
                };
                out.push((unquoted.to_string(), detail));
                pos_idx += 1;
            } else if !pos_slots.is_empty() {
                // Additional file/target argument for the last slot (e.g., `rm a.txt b.txt`).
                let last_slot = pos_slots[pos_slots.len() - 1];
                let what = last_slot.what.as_deref().unwrap_or(&last_slot.id);
                out.push((unquoted.to_string(), what.to_string()));
            } else {
                out.push((unquoted.to_string(), "not in the catalog".to_string()));
            }
            wi += 1;
        }
    } else {
        // Command not in catalog (or builtin like `echo`)
        let program = words[0].as_word().map_or("", |(_, u, _)| u);
        if program == "echo" {
            out.push(("echo".into(), "print text to standard output (shell builtin)".into()));
            for tok in &words[1..] {
                if let Some((_, u, _)) = tok.as_word() {
                    out.push((u.to_string(), "text to print".into()));
                }
            }
        } else {
            out.push((program.to_string(), "not in the catalog".to_string()));
            for tok in &words[1..] {
                if let Some((raw, u, quoted)) = tok.as_word() {
                    if !quoted && u.starts_with('-') {
                        let desc = explain_common_flag(program, u).unwrap_or("not in the catalog");
                        out.push((raw.to_string(), desc.to_string()));
                    } else {
                        out.push((u.to_string(), "not in the catalog".to_string()));
                    }
                }
            }
        }
    }

    emit_redirs(&redirs, out);
}

fn emit_redirs(redirs: &[(String, Option<String>)], out: &mut Vec<(String, String)>) {
    for (op, target) in redirs {
        out.push((op.clone(), explain_operator(op).to_string()));
        if let Some(file) = target {
            let role = match op.as_str() {
                "<" => "input file to read from",
                ">>" | "2>>" | "&>>" => "output file to append to",
                _ => "output file to write to",
            };
            out.push((file.clone(), role.to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn load_test_catalog() -> Vec<Command> {
        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/catalog"));
        crate::catalog::load(dir).unwrap()
    }

    #[test]
    fn explains_ten_plus_command_lines() {
        let catalog = load_test_catalog();

        // 1. Combined flags on ls + directory
        let r1 = explain_command(&catalog, "ls -la src/");
        assert!(r1.iter().any(|(p, d)| p == "ls" && d.contains("List the files")));
        assert!(r1.iter().any(|(p, d)| p == "-l" && d.contains("details")));
        assert!(r1.iter().any(|(p, d)| p == "-a" && d.contains("hidden")));
        assert!(r1.iter().any(|(p, d)| p == "src/" && d.contains("folder")));

        // 2. Pipeline: du -sh . | sort -h
        let r2 = explain_command(&catalog, "du -sh . | sort -h");
        assert!(r2.iter().any(|(p, _)| p == "du"));
        assert!(r2.iter().any(|(p, d)| p == "-s" && d.contains("summarize")));
        assert!(r2.iter().any(|(p, d)| p == "-h" && d.contains("human-readable")));
        assert!(r2.iter().any(|(p, d)| p == "|" && d.contains("pipes")));
        assert!(r2.iter().any(|(p, _)| p == "sort"));

        // 3. sudo + pacman -S
        let r3 = explain_command(&catalog, "sudo pacman -S ripgrep");
        assert!(r3.iter().any(|(p, d)| p == "sudo" && d.contains("administrator")));
        assert!(r3.iter().any(|(p, d)| p == "pacman -S" && d.contains("Install")));
        assert!(r3.iter().any(|(p, d)| p == "ripgrep" && d.contains("program to install")));

        // 4. tar -xzf
        let r4 = explain_command(&catalog, "tar -xzf photos.tar.gz");
        assert!(r4.iter().any(|(p, _)| p == "tar"));
        assert!(r4.iter().any(|(p, d)| p == "-xzf" && d.contains("Unpack")));
        assert!(r4.iter().any(|(p, d)| p == "photos.tar.gz" && d.contains("archive")));

        // 5. git commit with quoted message containing pipe
        let r5 = explain_command(&catalog, r#"git commit -m "fix | pipe handling""#);
        assert_eq!(r5.iter().filter(|(p, _)| p == "|").count(), 0);
        assert!(r5.iter().any(|(p, _)| p == "git commit"));
        assert!(r5.iter().any(|(p, d)| p == "fix | pipe handling" && d.contains("commit message")));

        // 6. docker compose up -d
        let r6 = explain_command(&catalog, "docker compose up -d");
        assert!(r6.iter().any(|(p, d)| p == "docker compose up" && d.contains("Docker Compose")));
        assert!(r6.iter().any(|(p, d)| p == "-d" && d.contains("background")));

        // 7. Destructive command: rm -rf build/
        let r7 = explain_command(&catalog, "rm -rf build/");
        assert!(r7.iter().any(|(p, d)| p == "rm" && d.contains("WARNING: deletes or overwrites data")));
        assert!(r7.iter().any(|(p, _)| p == "-r"));
        assert!(r7.iter().any(|(p, d)| p == "-f" && d.contains("force")));

        // 8. Chained &&, ||, ; and redirections >, >>, 2>&1
        let r8 =
            explain_command(&catalog, "cargo test && echo ok > out.txt 2>&1 || echo fail >> err.log; ls --not-real");
        assert!(r8.iter().any(|(p, d)| p == "&&" && d.contains("succeeded")));
        assert!(r8.iter().any(|(p, d)| p == ">" && d.contains("overwrites")));
        assert!(r8.iter().any(|(p, d)| p == "2>&1" && d.contains("stderr")));
        assert!(r8.iter().any(|(p, d)| p == "||" && d.contains("failed")));
        assert!(r8.iter().any(|(p, d)| p == ">>" && d.contains("appends")));
        assert!(r8.iter().any(|(p, d)| p == ";" && d.contains("unconditionally")));
        assert!(r8.iter().any(|(p, d)| p == "--not-real" && d == "not in the catalog"));

        // 9. Unknown command
        let r9 = explain_command(&catalog, "frobnicate --mystery foo");
        assert!(r9.iter().any(|(p, d)| p == "frobnicate" && d == "not in the catalog"));
        assert!(r9.iter().any(|(p, d)| p == "--mystery" && d == "not in the catalog"));

        // 10. find with multi-token choice & formatted slots
        let r10 = explain_command(&catalog, "find . -type f -name '*.rs' -mtime -7 -size +50M");
        assert!(r10.iter().any(|(p, _)| p == "find"));
        assert!(r10.iter().any(|(p, d)| p == "-type f" && d == "Only files"));
        assert!(r10.iter().any(|(p, d)| p.starts_with("-name") && d.contains("pattern")));
        assert!(r10.iter().any(|(p, d)| p.starts_with("-mtime") && d.contains("days")));
        assert!(r10.iter().any(|(p, d)| p == "-size +50M" && d.contains("50M")));

        // 11. cut -d, -f3 users.csv
        let r11 = explain_command(&catalog, "cut -d, -f3 users.csv");
        assert!(r11.iter().any(|(p, d)| p == "-d," && d.contains("Commas")));
        assert!(r11.iter().any(|(p, d)| p == "-f3" && d.contains("column")));
        assert!(r11.iter().any(|(p, d)| p == "users.csv" && d.contains("file")));

        // 12. chmod -R +x deploy.sh
        let r12 = explain_command(&catalog, "chmod -R +x deploy.sh");
        assert!(r12.iter().any(|(p, d)| p == "-R" && d.contains("Apply the change")));
        assert!(r12.iter().any(|(p, d)| p == "+x" && d.contains("runnable")));
        assert!(r12.iter().any(|(p, d)| p == "deploy.sh" && d.contains("permissions")));

        // 13. head with and without -n 20
        let r13a = explain_command(&catalog, "head notes.txt");
        assert!(r13a.iter().any(|(p, d)| p == "notes.txt" && d.contains("file to show")));
        let r13b = explain_command(&catalog, "head -n 20 notes.txt");
        assert!(r13b.iter().any(|(p, d)| p == "-n" && d.contains("lines")));
        assert!(r13b.iter().any(|(p, d)| p == "20" && d.contains("number of lines")));
        assert!(r13b.iter().any(|(p, d)| p == "notes.txt" && d.contains("file to show")));

        // 14. omarchy theme set + echo | sudo tee -a
        let r14a = explain_command(&catalog, r#"omarchy theme set "Tokyo Night""#);
        assert!(r14a.iter().any(|(p, d)| p == "omarchy theme set" && d.contains("theme")));
        assert!(r14a.iter().any(|(p, d)| p == "Tokyo Night" && d.contains("theme to apply")));
        let r14b = explain_command(&catalog, "echo hello | sudo tee -a /etc/hosts");
        assert!(r14b.iter().any(|(p, d)| p == "tee" && d.contains("configuration file")));
        assert!(r14b.iter().any(|(p, d)| p == "-a" && d.contains("append")));
        assert!(r14b.iter().any(|(p, d)| p == "/etc/hosts" && d.contains("system file")));
    }

    #[test]
    fn explains_usermod_rsync_z_wildcards_and_piped_grep() {
        let catalog = load_test_catalog();

        let u = explain_command(&catalog, "sudo usermod -aG libvirt $USER");
        assert!(u.iter().any(|(p, d)| p == "libvirt" && d.contains("group")));
        assert!(u.iter().any(|(p, d)| p == "$USER" && d.contains("username")));

        let r = explain_command(&catalog, "rsync -avz ./dist/ user@host:/var/www/");
        assert!(r.iter().any(|(p, d)| p == "-z" && d.contains("compress")));

        let rm = explain_command(&catalog, "rm -rf folder");
        assert!(rm.iter().any(|(p, d)| p == "-r" && d.contains("Delete folders")));

        let glob = explain_command(&catalog, "du -sh * | sort -h");
        assert!(glob.iter().any(|(p, d)| p == "*" && d.contains("wildcard")));

        let piped = explain_command(&catalog, "ps aux | grep chrome");
        assert!(piped.iter().any(|(p, d)| p == "grep" && d.contains("piped output")));
    }
}
