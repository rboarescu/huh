//! Typed values never vanish silently. After a line is built, every concrete value the request
//! typed (a path, a number, an address, a name, a hash, quoted text, ...) must show up in the
//! line in some form, or the suggestion says `not included: <the phrase around it>`. A value that
//! fits the one placeholder the line has left is put there instead.
//!
//! This is a check on the finished line, not part of any command: it holds for every catalog
//! entry, including ones added later.

use crate::assemble::{Suggestion, quote};
use crate::candidates::STOPWORDS;
use crate::catalog::{Command, SlotKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Path,
    Number,
    Ip,
    Url,
    /// `user@host`, optionally with `:path`.
    Email,
    /// `@server`.
    Handle,
    /// A file name with an extension, a host name, or `.ext`.
    File,
    Hash,
    /// A name given after a label ("named web", "user bob") or in quotes.
    Name,
}

#[derive(Debug, Clone)]
pub struct Typed {
    pub value: String,
    /// The words around the value as the user wrote them: `into /opt/api`, `depth 1`.
    pub phrase: String,
    pub kind: Kind,
    /// The unit that follows or is glued to a number ("24h", "2 weeks").
    unit: String,
}

struct Tok {
    text: String,
    quoted: bool,
}

fn tokenize(request: &str) -> Vec<Tok> {
    let chars: Vec<char> = request.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        if matches!(chars[i], '"' | '\'' | '`') {
            let q = chars[i];
            let close = (i + 1..chars.len()).find(|&j| {
                chars[j] == q && chars.get(j + 1).is_none_or(|c| c.is_whitespace() || ",.;:!?)".contains(*c))
            });
            if let Some(j) = close {
                let inner: String = chars[i + 1..j].iter().collect();
                if !inner.trim().is_empty() {
                    out.push(Tok { text: inner.trim().to_string(), quoted: true });
                }
                i = j + 1;
                continue;
            }
        }
        let start = i;
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        let raw: String = chars[start..i].iter().collect();
        let text = raw
            .trim_start_matches(['(', '[', '{', '"', '\'', '`'])
            .trim_end_matches([',', ';', ':', '!', '?', ')', ']', '}', '"', '\'', '`'])
            .trim_end_matches('.')
            .to_string();
        if !text.is_empty() {
            out.push(Tok { text, quoted: false });
        }
    }
    out
}

const UNITS: &[&str] = &[
    "s", "ms", "sec", "secs", "m", "min", "mins", "h", "hr", "hrs", "d", "w", "k", "kb", "mb", "gb", "tb", "b", "g",
    "t", "kib", "mib", "gib", "x", "%", "am", "pm", "mbps", "gbps",
];

/// Words after a number that are its unit: "2 weeks", "100 MB".
const UNIT_WORDS: &[&str] = &[
    "lines",
    "line",
    "days",
    "day",
    "hours",
    "hour",
    "minutes",
    "minute",
    "mins",
    "min",
    "seconds",
    "second",
    "secs",
    "sec",
    "weeks",
    "week",
    "months",
    "month",
    "years",
    "year",
    "mb",
    "gb",
    "kb",
    "tb",
    "mib",
    "gib",
    "bytes",
    "megabytes",
    "gigabytes",
    "kilobytes",
    "files",
    "times",
    "commits",
    "entries",
    "items",
    "packets",
    "results",
    "percent",
    "ago",
    "processes",
    "levels",
    "threads",
    "cores",
];

/// Not a value: "and/or", "read/write", names of projects that contain a dot.
const NOT_VALUES: &[&str] = &[
    "and/or",
    "w/o",
    "n/a",
    "i/o",
    "24/7",
    "tcp/udp",
    "read/write",
    "yes/no",
    "y/n",
    "on/off",
    "either/or",
    "he/she",
    "input/output",
    "true/false",
    "up/down",
    "start/stop",
    "enable/disable",
    "allow/deny",
    "upload/download",
    "send/receive",
    "in/out",
    "pass/fail",
    "files/folders",
    "file/folder",
    "folders/files",
    "dirs/files",
    "e.g",
    "i.e",
    "etc",
    "vs",
    "node.js",
    "vue.js",
    "next.js",
    "react.js",
    "express.js",
    "asp.net",
    "a.m",
    "p.m",
    "approx",
    "w/",
    "b/w",
    "tcp/ip",
    "ip/port",
    "cpu/ram",
    "ram/cpu",
    "cpu/memory",
    "memory/cpu",
    "client/server",
    "user/group",
    "group/user",
];

/// Words that follow a label without being its value.
const NOT_NAMES: &[&str] = &[
    "it",
    "all",
    "them",
    "each",
    "every",
    "one",
    "both",
    "is",
    "be",
    "was",
    "not",
    "no",
    "interactive",
    "default",
    "current",
    "running",
    "active",
    "new",
    "old",
    "same",
    "other",
    "any",
    "some",
    "my",
];

fn is_ip(t: &str) -> bool {
    let (host, rest) = match t.find(['/', ':']) {
        Some(i) => (&t[..i], &t[i + 1..]),
        None => (t, "1"),
    };
    let octets: Vec<&str> = host.split('.').collect();
    octets.len() == 4
        && octets.iter().all(|o| !o.is_empty() && o.len() <= 3 && o.chars().all(|c| c.is_ascii_digit()))
        && !rest.is_empty()
        && rest.chars().all(|c| c.is_ascii_digit())
}

fn is_hash(t: &str) -> bool {
    (7..=40).contains(&t.len())
        && t.chars().all(|c| c.is_ascii_hexdigit())
        && t.chars().any(|c| c.is_ascii_digit())
        && t.chars().any(|c| c.is_ascii_alphabetic())
}

/// The digit runs of a number-like token and its glued unit: `8080:80`, `24h`, `1920x1080`.
fn number_parts(t: &str) -> Option<(Vec<u64>, String)> {
    let s = t.trim_start_matches('+');
    let chars: Vec<char> = s.chars().collect();
    if !chars.first()?.is_ascii_digit() {
        return None;
    }
    let (mut runs, mut cur, mut i) = (Vec::new(), String::new(), 0);
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
            cur.push(c);
        } else if matches!(c, '.' | ':' | ',' | '-' | 'x')
            && !cur.is_empty()
            && chars.get(i + 1).is_some_and(char::is_ascii_digit)
        {
            runs.push(cur.parse::<u64>().ok()?);
            cur.clear();
        } else {
            break;
        }
        i += 1;
    }
    runs.push(cur.parse::<u64>().ok()?);
    let unit: String = chars[i..].iter().collect::<String>().to_lowercase();
    (unit.is_empty() || UNITS.contains(&unit.as_str())).then_some((runs, unit))
}

/// A file name or host name: dotted, ending in an alphabetic part ("notes.txt", "api.example.com").
fn is_dotted_name(t: &str) -> bool {
    let parts: Vec<&str> = t.split('.').collect();
    parts.len() >= 2
        && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || "-_".contains(c)))
        && parts.last().is_some_and(|l| l.len() <= 10 && l.chars().all(|c| c.is_ascii_alphabetic()))
        && t.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
}

fn classify(t: &str) -> Option<Kind> {
    let low = t.to_lowercase();
    if t.starts_with('-') || NOT_VALUES.contains(&low.as_str()) {
        return None;
    }
    if t.contains("://") {
        return Some(Kind::Url);
    }
    if t.starts_with('@') && t.len() > 1 {
        return Some(Kind::Handle);
    }
    if let Some((l, r)) = t.split_once('@')
        && !l.is_empty()
        && !r.is_empty()
        && !t.contains('/') | t.contains(':')
    {
        return Some(Kind::Email);
    }
    if is_ip(t) {
        return Some(Kind::Ip);
    }
    if t.contains('/') {
        let ratio = t.chars().all(|c| c.is_ascii_digit() || c == '/');
        return (t.len() > 1 && !ratio).then_some(Kind::Path);
    }
    if t.starts_with('~') || t.starts_with("./") || t.starts_with("../") {
        return Some(Kind::Path);
    }
    if is_hash(t) {
        return Some(Kind::Hash);
    }
    if number_parts(t).is_some() {
        return Some(Kind::Number);
    }
    if let Some(ext) = t.strip_prefix('.')
        && (1..=8).contains(&ext.len())
        && ext.chars().all(|c| c.is_ascii_alphanumeric())
        && ext.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
    {
        return Some(Kind::File);
    }
    is_dotted_name(t).then_some(Kind::File)
}

/// Words that introduce a name: "named web", "comment ops@lab", "onto release".
const NAME_LABELS: &[&str] =
    &["named", "called", "comment", "message", "msg", "name", "onto", "hostname", "host", "author"];
/// Words that may sit between a label and its value.
const FILLERS: &[&str] = &["the", "a", "an", "as", "is", "to"];
/// Words not worth repeating at the front of a phrase.
const SKIP_BEFORE: &[&str] = &["the", "a", "an", "and", "or", "my", "me", "please", "all", "then", "it", "also"];

/// Every concrete value the request types, in order, each with the words around it.
pub fn typed_values(request: &str) -> Vec<Typed> {
    let toks = tokenize(request);
    let mut out: Vec<Typed> = Vec::new();
    let mut add = |t: Typed| {
        if !out.iter().any(|o| o.value.eq_ignore_ascii_case(&t.value)) {
            out.push(t);
        }
    };
    let phrase_for = |i: usize, with_unit: bool| -> String {
        let mut words: Vec<&str> = Vec::new();
        if let Some(prev) = i.checked_sub(1).map(|p| &toks[p]).filter(|p| !p.quoted) {
            let low = prev.text.to_lowercase();
            if prev.text.chars().all(|c| c.is_ascii_alphabetic()) && !SKIP_BEFORE.contains(&low.as_str()) {
                if low == "than"
                    && let Some(pp) = i.checked_sub(2).map(|p| &toks[p])
                {
                    words.push(&pp.text);
                }
                words.push(&prev.text);
            }
        }
        let shown = if toks[i].quoted { format!("\"{}\"", toks[i].text) } else { toks[i].text.clone() };
        let mut phrase = words.join(" ");
        if !phrase.is_empty() {
            phrase.push(' ');
        }
        phrase.push_str(&shown);
        if with_unit
            && let Some(next) = toks.get(i + 1).filter(|n| !n.quoted)
            && UNIT_WORDS.contains(&next.text.to_lowercase().as_str())
        {
            phrase.push(' ');
            phrase.push_str(&next.text);
        }
        phrase
    };
    for (i, tok) in toks.iter().enumerate() {
        if tok.quoted {
            add(Typed { value: tok.text.clone(), phrase: phrase_for(i, false), kind: Kind::Name, unit: String::new() });
            continue;
        }
        if let Some(kind) = classify(&tok.text) {
            let unit = match number_parts(&tok.text) {
                Some((_, glued)) if !glued.is_empty() => glued,
                _ => toks
                    .get(i + 1)
                    .filter(|n| !n.quoted)
                    .map(|n| n.text.to_lowercase())
                    .filter(|n| UNIT_WORDS.contains(&n.as_str()))
                    .unwrap_or_default(),
            };
            add(Typed { value: tok.text.clone(), phrase: phrase_for(i, kind == Kind::Number), kind, unit });
            continue;
        }
        // "named web", "comment ops@lab": the next word that can be a value.
        let by_someone = tok.text.eq_ignore_ascii_case("by")
            && i > 0
            && matches!(
                toks[i - 1].text.to_lowercase().as_str(),
                "commits" | "commit" | "changes" | "files" | "written" | "made" | "pushed" | "owned" | "authored"
            );
        if by_someone || NAME_LABELS.contains(&tok.text.to_lowercase().as_str()) {
            let mut j = i + 1;
            while toks.get(j).is_some_and(|n| !n.quoted && FILLERS.contains(&n.text.to_lowercase().as_str())) {
                j += 1;
            }
            if let Some(next) = toks.get(j).filter(|n| !n.quoted) {
                let low = next.text.to_lowercase();
                if !STOPWORDS.contains(&low.as_str())
                    && !NOT_NAMES.contains(&low.as_str())
                    && !NAME_LABELS.contains(&low.as_str())
                    && classify(&next.text).is_none()
                    && next.text.chars().all(|c| c.is_ascii_alphanumeric() || "-_".contains(c))
                {
                    add(Typed {
                        value: next.text.clone(),
                        phrase: format!("{} {}", tok.text, next.text),
                        kind: Kind::Name,
                        unit: String::new(),
                    });
                }
            }
        }
    }
    // "user bob", "group docker", "branch main": the matcher already knows these labels.
    for (label, value) in crate::matcher::labelled_values(request) {
        if value.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            && !NOT_NAMES.contains(&value.to_lowercase().as_str())
        {
            add(Typed { phrase: format!("{label} {value}"), value, kind: Kind::Name, unit: String::new() });
        }
    }
    out
}

/// Digit runs of a line, as numbers (leading zeros ignored).
fn digit_runs(line: &str) -> Vec<u64> {
    let mut runs = Vec::new();
    let mut cur = String::new();
    for c in line.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_digit() {
            cur.push(c);
        } else if !cur.is_empty() {
            runs.extend(cur.parse::<u64>());
            cur.clear();
        }
    }
    runs
}

/// A number is in the line when it, the next number (`head -n 11` for "top 10") or its value in
/// another unit (14 for "2 weeks", 120 for "2 hours") is there.
fn number_present(line: &str, runs: &[u64], unit: &str) -> bool {
    let have = digit_runs(line);
    let factors: &[u64] = match unit {
        "" => &[],
        "w" | "week" | "weeks" => &[7, 1, 24, 168, 10080],
        "d" | "day" | "days" => &[24, 1440, 86_400, 7, 30],
        "h" | "hr" | "hrs" | "hour" | "hours" => &[60, 3600, 24, 3_600_000],
        "m" | "min" | "mins" | "minute" | "minutes" => &[60, 1000, 60_000],
        "month" | "months" => &[30, 31, 4, 365],
        "year" | "years" => &[365, 12, 52],
        "s" | "sec" | "secs" | "second" | "seconds" | "ms" => &[1000],
        "k" | "kb" | "mb" | "gb" | "tb" | "kib" | "mib" | "gib" | "g" | "t" | "megabytes" | "gigabytes"
        | "kilobytes" => &[1024, 1000, 1_048_576, 1_000_000],
        _ => &[],
    };
    runs.iter().all(|&n| {
        have.contains(&n) || have.contains(&(n + 1)) || have.iter().any(|h| factors.iter().any(|f| *h == n * f))
    })
}

/// Whether the value shows up in the line in any form.
pub fn present(line: &str, t: &Typed) -> bool {
    let line_l = line.to_lowercase();
    let value = t.value.to_lowercase();
    let has = |s: &str| !s.is_empty() && line_l.contains(s);
    match t.kind {
        Kind::Number => {
            has(&value) || number_parts(&t.value).is_some_and(|(runs, _)| number_present(line, &runs, &t.unit))
        }
        Kind::Ip => has(&value) || value.split(['/', ':']).all(&has),
        Kind::Path => {
            let plain = value.trim_end_matches('/');
            let home = value.strip_prefix("~/").map(|rest| format!("$home/{rest}"));
            let dot = value.strip_prefix("./");
            has(&value)
                || has(plain)
                || home.as_deref().is_some_and(has)
                || dot.is_some_and(has)
                || (value == "~" && has("$home"))
        }
        Kind::Email => has(&value) || value.split(['@', ':']).all(&has),
        Kind::Handle => has(&value[1..]),
        Kind::Hash => has(&value) || has(&value[..7]),
        Kind::Url | Kind::File | Kind::Name => has(&value),
    }
}

/// The most confidence a line may show when more than one typed value is missing from it.
pub const LOST_CONFIDENCE: f64 = 0.5;

/// The typed values neither the line nor its notes (other than "not included" ones) contain; a note
/// can carry a value the line cannot, such as the entry to add in `crontab -e`.
pub fn lost(request: &str, line: &str, notes: &[String]) -> Vec<Typed> {
    let mut haystack = line.to_string();
    for note in notes.iter().filter(|n| !n.starts_with("not included")) {
        haystack.push('\n');
        haystack.push_str(note);
    }
    typed_values(request).into_iter().filter(|t| !present(&haystack, t)).collect()
}

/// Whether a stated modifier's value ("older than 2 weeks" has the value "2") is in the line:
/// as written, or as the same amount in another unit (`-mtime +14`).
pub fn modifier_present(line: &str, phrase: &str, value: &str) -> bool {
    if line.to_lowercase().contains(value) {
        return true;
    }
    match value.parse::<u64>() {
        Ok(n) => number_present(line, &[n], phrase.rsplit(' ').next().unwrap_or_default()),
        Err(_) => false,
    }
}

/// A placeholder in a line: `<target>`.
fn placeholders(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('<') {
        let Some(len) = rest[start..].find('>') else { break };
        let id = &rest[start + 1..start + len];
        if !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            out.push(id);
        }
        rest = &rest[start + len + 1..];
    }
    out
}

/// A lost value goes into the one placeholder of a compatible slot the line has left, if exactly
/// one value is lost and exactly one placeholder is open. Dangerous lines are never completed this
/// way: they need the evidence rules, not a guess.
pub fn fill_open_placeholder(suggestion: &mut Suggestion, command: &Command, lost: &mut Vec<Typed>) {
    if suggestion.destructive || lost.len() != 1 {
        return;
    }
    let line = suggestion.line.clone();
    let open = placeholders(&line);
    let [id] = open.as_slice() else { return };
    let Some(slot) = command.slots.iter().find(|s| s.id == *id) else { return };
    let fits = match slot.kind {
        SlotKind::Path => {
            matches!(lost[0].kind, Kind::Path | Kind::File) && !lost[0].value.starts_with('.')
                || lost[0].value.starts_with("../")
        }
        SlotKind::Text => !matches!(lost[0].kind, Kind::Number),
        SlotKind::Choice => false,
    };
    if !fits {
        return;
    }
    let value = lost.remove(0).value;
    let shown = slot.format.as_ref().map_or_else(|| quote(&value), |f| f.replace("{}", &quote(&value)));
    suggestion.line = suggestion.line.replacen(&format!("<{id}>"), &shown, 1);
    suggestion.notes.push(format!("used `{value}` for <{id}>: check that it is what that part of the line needs"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(request: &str) -> Vec<String> {
        typed_values(request).into_iter().map(|t| t.value).collect()
    }

    #[test]
    fn finds_the_values_a_request_types() {
        assert_eq!(found("df human readable for /var"), ["/var"]);
        assert_eq!(
            found("git clone https://github.com/acme/api.git into /opt/api depth 1"),
            ["https://github.com/acme/api.git", "/opt/api", "1"]
        );
        assert_eq!(found("docker prune unused images older than 24h"), ["24h"]);
        assert_eq!(found("run nginx on port 8080:80 detached name web"), ["8080:80", "web"]);
        assert_eq!(found("ssh key with comment ops@lab"), ["ops@lab"]);
        assert_eq!(found("cherry-pick abc1234 onto release"), ["abc1234", "release"]);
        assert_eq!(found("commit with message 'fix login bug'"), ["fix login bug"]);
        assert_eq!(found("ping 10.0.0.5/24 and read/write e.g. notes.txt."), ["10.0.0.5/24", "notes.txt"]);
        // Plain words are not values.
        assert!(found("show me the biggest files").is_empty());
    }

    #[test]
    fn a_value_is_present_in_any_form() {
        let check = |request: &str, line: &str| lost(request, line, &[]).is_empty();
        assert!(check("last 10 commits", "git log -n10"));
        assert!(check("top 10 processes", "ps aux | head -n 11"));
        assert!(check("files older than 2 weeks", "find . -mtime +14"));
        assert!(check("files older than 2 hours", "find . -mmin +120"));
        assert!(check("run in ~/Documents", "ls $HOME/Documents"));
        assert!(check("login as bob@lab", "ssh bob@lab"));
        assert!(check("at 03:30 daily", "30 3 * * * cmd"));
        assert!(!check("last 10 commits", "git log"));
        assert!(!check("list /var", "ls"));
    }

    #[test]
    fn an_open_placeholder_takes_the_one_lost_value() {
        let catalog = crate::catalog::load_embedded().unwrap();
        let command = catalog.iter().find(|c| c.name == "wc").unwrap();
        let mut suggestion = Suggestion {
            command: "wc".into(),
            line: "wc -l <target>".into(),
            probability: 0.8,
            description: String::new(),
            destructive: false,
            warning: None,
            notes: vec![],
            parts: vec![],
        };
        let mut lost = typed_values("count lines in notes.txt");
        fill_open_placeholder(&mut suggestion, command, &mut lost);
        assert_eq!(suggestion.line, "wc -l notes.txt");
        assert!(lost.is_empty());
    }
}
