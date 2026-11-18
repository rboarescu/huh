//! The pointer printed when no command fits confidently: the closest tool, what it is for, and
//! where to read more. It is a hint, never a command line: it names a manual page and a few flags,
//! so nobody can copy a wrong command out of it. Only reads directories and files; nothing is run.

use crate::catalog::Command;
use crate::knowledge::Knowledge;
use crate::manpage;
use crate::platform;
use crate::tools;
use std::path::PathBuf;

/// The table of tools to point at for a topic (see the notes at the top of `hints.txt`).
struct Topic {
    tool: &'static str,
    triggers: Vec<Vec<&'static str>>,
    description: &'static str,
    flags: Vec<&'static str>,
    man: Option<&'static str>,
}

fn topics() -> Vec<Topic> {
    include_str!("hints.txt")
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|line| {
            let f: Vec<&str> = line.split('|').map(str::trim).collect();
            if f.len() < 4 {
                return None;
            }
            Some(Topic {
                tool: f[0],
                triggers: f[1].split(',').map(|t| t.trim().split('+').collect()).collect(),
                description: f[2],
                flags: f[3].split(", ").filter(|s| !s.is_empty()).collect(),
                man: f.get(4).copied().filter(|m| !m.is_empty()),
            })
        })
        .collect()
}

/// Words that say nothing about the task.
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "to", "of", "in", "on", "at", "for", "with", "from", "by", "my", "me", "i", "it",
    "this", "that", "these", "those", "is", "are", "be", "do", "does", "can", "could", "would", "should", "how",
    "what", "which", "who", "where", "when", "why", "please", "some", "any", "all", "get", "make", "show", "give",
    "want", "need", "use", "using", "into", "out", "up", "down", "new", "one", "file", "files", "folder", "command",
    "tool", "program", "thing", "things", "way", "about", "than", "then", "so", "if", "not", "no", "you", "your",
    "our", "we", "like", "just", "also", "set", "put", "run", "see", "check", "tell", "find", "list", "help", "change",
];

/// The request as lowercase words with the punctuation around them removed.
fn words(request: &str) -> Vec<String> {
    request.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

/// Like `words`, but a name such as `yt-dlp` or `systemd-analyze` stays in one piece.
fn name_words(request: &str) -> Vec<String> {
    request
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .filter(|w| !w.is_empty())
        .collect()
}

/// A rough stem, so "pdfs", "resizing" and "resize" meet.
fn stem(word: &str) -> String {
    let mut s = word.to_lowercase();
    for suffix in ["ing", "ed", "es", "s"] {
        if let Some(rest) = s.strip_suffix(suffix)
            && rest.chars().count() >= 3
        {
            s = rest.to_string();
            break;
        }
    }
    if s.chars().count() > 3
        && let Some(rest) = s.strip_suffix('e')
    {
        s = rest.to_string();
    }
    s
}

fn content_stems(words: &[String]) -> Vec<String> {
    words.iter().filter(|w| w.chars().count() >= 3 && !STOPWORDS.contains(&w.as_str())).map(|w| stem(w)).collect()
}

fn has_word(stems: &[String], word: &str) -> bool {
    let wanted = stem(word);
    stems.contains(&wanted)
}

/// The first table entry whose trigger is in the request.
fn topic_for(request: &str) -> Option<Topic> {
    let ws = words(request);
    let stems: Vec<String> = ws.iter().map(|w| stem(w)).collect();
    topics().into_iter().find(|t| t.triggers.iter().any(|group| group.iter().all(|w| has_word(&stems, w))))
}

fn on_path(name: &str) -> Option<PathBuf> {
    if !manpage::safe_page_name(name) {
        return None;
    }
    platform::path_dirs().into_iter().map(|d| d.join(name)).find(|p| platform::is_executable_file(p))
}

/// A man page file for `name` in the given section ("" = the sections `find_page` knows).
fn man_page(name: &str, section: Option<&str>) -> Option<PathBuf> {
    let Some(section) = section else { return manpage::find_page(name) };
    if !manpage::safe_page_name(name) || !section.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    manpage::man_dirs().into_iter().find_map(|dir| {
        ["", ".gz"]
            .iter()
            .map(|suffix| dir.join(format!("man{section}")).join(format!("{name}.{section}{suffix}")))
            .find(|p| p.is_file())
    })
}

/// The NAME-line summary of a program's man page, following one `.so` redirect.
fn man_summary(name: &str) -> Option<String> {
    let path = manpage::find_page(name)?;
    let head = manpage::read_head(&path, 16 * 1024)?;
    if let Some(target) = manpage::so_target(&head)
        && let Some(other) = manpage::find_page(&target)
    {
        return tools::name_summary(&manpage::read_head(&other, 16 * 1024)?);
    }
    tools::name_summary(&head)
}

/// What the program is for: the curated table, then its man page, then the catalog's own words.
fn describe(program: &str, catalog_text: Option<&str>) -> Option<String> {
    if let Some(section) = Knowledge::get().section(&[program.to_string()])
        && !section.desc.is_empty()
    {
        return Some(section.desc.clone());
    }
    man_summary(program).or_else(|| catalog_text.map(str::to_string))
}

fn tidy(text: &str, max: usize) -> String {
    let text = text.replace(" (", ", ").replace(')', "");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= max {
        return text;
    }
    let cut: String = text.chars().take(max - 3).collect();
    format!("{}...", cut.trim_end())
}

/// Up to two options of the program whose meaning shares words with the request.
fn relevant_flags(program: &str, request: &str) -> Vec<String> {
    let wanted = content_stems(&words(request));
    if wanted.is_empty() {
        return vec![];
    }
    let overlap = |text: &str| {
        let have = content_stems(&words(text));
        wanted.iter().filter(|w| have.contains(w)).count()
    };
    let mut scored: Vec<(usize, String)> = Vec::new();
    if let Some(section) = Knowledge::get().section(&[program.to_string()]) {
        for opt in &section.opts {
            let n = overlap(&opt.meaning);
            if n > 0 {
                scored.push((
                    n,
                    format!("{} ({})", opt.names.first().cloned().unwrap_or_default(), tidy(&opt.meaning, 48)),
                ));
            }
        }
    }
    if scored.is_empty()
        && let Some(page) = manpage::load(program)
    {
        for opt in &page.options {
            let n = overlap(&opt.desc);
            if n > 0 {
                let name =
                    opt.names.iter().find(|n| n.starts_with('-')).or(opt.names.first()).cloned().unwrap_or_default();
                scored.push((n, format!("{name} ({})", tidy(&opt.desc, 48))));
            }
        }
    }
    // Stable: ties keep the order of the table or page.
    scored.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    scored.into_iter().take(2).map(|(_, s)| s).collect()
}

/// "see `man x`" when the page is on disk, else the program's own help, else a note that it is missing.
fn where_to_read(program: &str, man: Option<&str>) -> String {
    let (section, page) = match man.and_then(|m| m.split_once(' ')) {
        Some((section, page)) => (Some(section), page),
        None => (None, program),
    };
    if man_page(page, section).is_some() {
        return format!("see `man {}`", man.unwrap_or(program));
    }
    if on_path(program).is_some() {
        format!("see `{program} --help`")
    } else {
        format!("it does not look installed here; once it is, see `man {program}`")
    }
}

fn render(program: &str, description: &str, flags: &[String], man: Option<&str>, lead: &str) -> String {
    let mut text =
        format!("hint: {lead}{program} ({}) \u{2014} {}", tidy(description, 80), where_to_read(program, man));
    if !flags.is_empty() {
        text.push_str(&format!(", especially {}", flags.join(", ")));
    }
    text.push('.');
    text
}

/// Words that join a program name to the request: "convert it with ffmpeg", "using sox".
const NAMING_WORDS: &[&str] = &["with", "using", "via", "use", "through"];
const NAME_AFTER: &[&str] = &["command", "tool", "program", "utility"];

/// A program the request names outright that exists on this machine or in the curated table.
fn named_program(request: &str, catalog: &[&Command]) -> Option<String> {
    let ws = name_words(request);
    let known = |name: &str| {
        manpage::safe_page_name(name)
            && (Knowledge::get().section(&[name.to_string()]).is_some()
                || catalog.iter().any(|c| c.program() == name)
                || on_path(name).is_some())
    };
    ws.iter().enumerate().find_map(|(i, w)| {
        let by_preposition = i > 0 && NAMING_WORDS.contains(&ws[i - 1].as_str());
        let by_suffix = ws.get(i + 1).is_some_and(|n| NAME_AFTER.contains(&n.as_str()));
        ((by_preposition || by_suffix) && !STOPWORDS.contains(&w.as_str()) && known(w)).then(|| w.clone())
    })
}

/// Whether a command has enough in common with the request to be called close: a word of its name
/// ("tar", "docker", "sort"), or two different content words of its name, description or keywords.
/// One shared ordinary word ("write", "order") is not enough: it would tie a poem to `chmod`.
fn lexical_support(command: &Command, stems: &[String]) -> bool {
    let first = command.name.split(' ').next().unwrap_or_default();
    let name = content_stems(&words(&format!("{first} {}", command.program())));
    if stems.iter().any(|s| name.contains(s)) {
        return true;
    }
    let text = format!("{} {}", command.description, command.keywords.join(" "));
    let have = content_stems(&words(&text));
    let mut shared: Vec<&String> = stems.iter().filter(|s| have.contains(s)).collect();
    shared.sort();
    shared.dedup();
    shared.len() >= 2
}

/// The best command the ranking rejected that still has something in common with the request.
fn closest_in_ranking<'a>(request: &str, ranked: &[(&'a Command, f64)]) -> Option<&'a Command> {
    let stems = content_stems(&words(request));
    ranked.iter().take(5).filter(|(_, p)| *p >= MIN_CLOSEST).find(|(c, _)| lexical_support(c, &stems)).map(|(c, _)| *c)
}

/// The least probability a rejected command must have to be offered as the closest tool.
const MIN_CLOSEST: f64 = 0.03;

/// Requests no tool is pointed out for: writing over or wiping drives, and getting into other
/// people's systems. huh declines these; a "closest tool" would read as help with them.
fn off_limits(request: &str) -> bool {
    let ws = words(request);
    let any = |list: &[&str]| ws.iter().any(|w| list.contains(&w.as_str()));
    let drive_write =
        any(&["burn", "flash", "format", "wipe", "erase", "partition", "bootable", "mkfs", "dd", "shred"])
            && any(&["usb", "disk", "drive", "stick", "sd", "device", "iso", "partition", "card", "ssd", "nvme"]);
    let intrusion = any(&[
        "hack",
        "hacking",
        "crack",
        "cracking",
        "exploit",
        "bruteforce",
        "ddos",
        "keylogger",
        "phishing",
        "spy",
        "steal",
        "intrude",
        "eavesdrop",
    ]);
    drive_write || intrusion
}

/// The least probability the command refused for want of an object must have to be named.
const MIN_REFUSED: f64 = 0.25;

/// The hint for a request nothing answered: a named program, a tool for the topic, or the closest
/// rejected catalog command. `None` when nothing honest can be said.
/// `refused` is the command that matched but was held back because the request does not say what
/// to act on; it is named first.
pub fn pointer(
    request: &str,
    ranked: &[(&Command, f64)],
    catalog: &[&Command],
    refused: Option<(&Command, f64)>,
) -> Option<String> {
    if off_limits(request) {
        return None;
    }
    let of_command = |command: &Command| {
        let program = command.program();
        if program.is_empty() || !manpage::safe_page_name(program) {
            return None;
        }
        let description = describe(program, Some(&command.description))?;
        Some(render(program, &description, &relevant_flags(program, request), None, "no exact match. Closest: "))
    };
    if let Some((command, probability)) = refused {
        return (probability >= MIN_REFUSED).then(|| of_command(command)).flatten();
    }
    if let Some(name) = named_program(request, catalog) {
        let catalog_text = catalog.iter().find(|c| c.program() == name).map(|c| c.description.as_str());
        let description = describe(&name, catalog_text)?;
        return Some(render(&name, &description, &relevant_flags(&name, request), None, ""));
    }
    if let Some(topic) = topic_for(request) {
        let mut flags: Vec<String> = topic.flags.iter().map(|f| f.to_string()).collect();
        let wanted = content_stems(&words(request));
        // The flags that mention the request's words first; the table's own order otherwise.
        flags
            .sort_by_key(|f| std::cmp::Reverse(content_stems(&words(f)).iter().filter(|w| wanted.contains(w)).count()));
        flags.truncate(2);
        return Some(render(topic.tool, topic.description, &flags, topic.man, "no exact match. Closest: "));
    }
    of_command(closest_in_ranking(request, ranked)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_meet() {
        assert_eq!(stem("pdfs"), stem("pdf"));
        assert_eq!(stem("resizing"), stem("resize"));
        assert_eq!(stem("converting"), stem("convert"));
    }

    #[test]
    fn table_parses_and_triggers_match() {
        assert!(topics().len() > 20);
        assert_eq!(topic_for("merge these PDFs please").unwrap().tool, "pdfunite");
        assert_eq!(topic_for("convert video.mp4 to a gif").unwrap().tool, "ffmpeg");
        assert!(topic_for("order a pizza").is_none());
        assert!(topic_for("recommend a good movie").is_none());
    }

    #[test]
    fn hint_is_a_pointer_not_a_command() {
        let text = pointer("pretty print a json file", &[], &[], None).unwrap();
        assert!(text.starts_with("hint: no exact match. Closest: jq ("), "{text}");
        assert!(text.contains("especially"), "{text}");
        assert!(!text.contains("jq ."), "{text}");
    }
}
