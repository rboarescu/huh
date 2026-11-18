//! Readers for values that depend on the wording of the request rather than on a single word:
//! "in 10 minutes" (`+10`), "older than 7 days" (`+7`) against "in the last 7 days" (`-7`), the
//! number after "port". A catalog slot names one with `extract = "..."`; it then gets exactly
//! what the reader finds, or its default when the reader finds nothing.

use crate::candidates::number_word;

/// The value the named reader finds in the request, if any.
pub fn extract(reader: &str, request: &str) -> Option<String> {
    let words = words(request);
    match reader {
        "shutdown_time" => shutdown_time(&words),
        "file_age" => file_age(&words),
        "file_glob" => file_glob(request, &words),
        "port" => port(&words),
        "count" => count(&words),
        "domain" => domain(request),
        "host" => host(request, &words),
        "service" => service(&words),
        "dns_server" => dns_server(request),
        "percent" => percent(request),
        "device" => device(request),
        "local_port" => forwarded_port(&words, 0),
        "remote_port" => forwarded_port(&words, 1),
        "json" => json_body(request),
        "context_dir" => context_dir(request),
        "user_name" => user_name(&words),
        "image_tag" => image_tag(&words),
        "rebase_target" => rebase_target(&words),
        "versioned_program" => versioned_program(&words),
        "filename" => filename(request),
        "mac" => mac_address(request),
        "chmod_mode" => chmod_mode(&words),
        _ => None,
    }
}

/// A permission mode the request spells out ("chmod 755", "make it 600") or describes ("only i can
/// read it" is 600, "only i can read it and nobody can change it" is 400, "everyone full access" is 777).
fn chmod_mode(words: &[String]) -> Option<String> {
    const MODES: &[&str] = &[
        "400", "440", "444", "500", "550", "555", "600", "640", "644", "660", "664", "666", "700", "750", "755", "770",
        "775", "777",
    ];
    if let Some(mode) = words.iter().find(|w| MODES.contains(&w.as_str())) {
        return Some(mode.clone());
    }
    let has = |w: &str| words.iter().any(|x| x == w);
    let phrase = |p: &str| {
        let p: Vec<&str> = p.split(' ').collect();
        words.windows(p.len()).any(|win| win.iter().zip(&p).all(|(a, b)| a == b))
    };
    let only_me = (has("only") && (has("i") || has("me") || has("myself") || phrase("the owner")))
        || phrase("nobody but me")
        || phrase("no one but me")
        || phrase("just me")
        || phrase("private to me");
    let me_can_read =
        only_me && ["read", "open", "see", "view", "access", "use", "readable", "private"].iter().any(|w| has(w));
    if me_can_read {
        let nobody_changes = (has("nobody") || phrase("no one") || has("noone") || phrase("nobody else"))
            && ["change", "edit", "modify", "write", "changes", "changed", "edited"].iter().any(|w| has(w))
            || phrase("read only")
            || has("read-only")
            || phrase("cant be changed")
            || phrase("can't be changed");
        return Some(if nobody_changes { "400" } else { "600" }.to_string());
    }
    let everyone = has("everyone") || has("anyone") || phrase("all users") || phrase("every user") || has("everybody");
    if everyone
        && (phrase("full access") || phrase("full permissions") || phrase("all permissions") || phrase("full control"))
    {
        return Some("777".to_string());
    }
    None
}

/// Lowercase words, with the edges of each trimmed of punctuation (a `:` inside stays, as in 23:00).
fn words(request: &str) -> Vec<String> {
    request
        .to_lowercase()
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '*' && c != '.' && c != '-').to_string())
        .map(|w| w.trim_end_matches(['.', ',']).to_string())
        .filter(|w| !w.is_empty())
        .collect()
}

fn number(word: &str) -> Option<u64> {
    if !word.is_empty() && word.chars().all(|c| c.is_ascii_digit()) {
        return word.parse().ok();
    }
    match word {
        "a" | "an" => Some(1),
        _ => number_word(word)?.parse().ok(),
    }
}

/// Length of a unit of time in minutes.
fn unit_minutes(word: &str) -> Option<u64> {
    match word.trim_end_matches('s') {
        "min" | "minute" | "mins" => Some(1),
        "hour" | "hr" | "h" => Some(60),
        "day" | "d" => Some(24 * 60),
        "week" | "wk" => Some(7 * 24 * 60),
        "month" => Some(30 * 24 * 60),
        "year" => Some(365 * 24 * 60),
        _ => None,
    }
}

/// What `shutdown` takes: `+MINUTES`, `HH:MM`, or nothing found.
fn shutdown_time(words: &[String]) -> Option<String> {
    for (i, word) in words.iter().enumerate() {
        let next = words.get(i + 1).map(String::as_str).unwrap_or_default();
        if word == "in" {
            if next == "half" && words.get(i + 2).is_some_and(|w| w == "an" || w == "a") {
                return Some("+30".into());
            }
            if let (Some(n), Some(unit)) = (number(next), words.get(i + 2).and_then(|u| unit_minutes(u)))
                && unit <= 60
            {
                return Some(format!("+{}", n * unit));
            }
        }
        if word == "at" || word == "by" {
            if next == "midnight" {
                return Some("00:00".into());
            }
            if let Some(time) = clock(next, words.get(i + 2).map(String::as_str).unwrap_or_default()) {
                return Some(time);
            }
        }
    }
    None
}

/// "23:00", "6:15", "6pm", "6 pm" as `HH:MM`.
fn clock(word: &str, following: &str) -> Option<String> {
    let (digits, suffix) = match word.find(|c: char| c.is_ascii_alphabetic()) {
        Some(at) => (&word[..at], &word[at..]),
        None => (word, following),
    };
    let (hour, minute) = match digits.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None => {
            // A bare number is a time only with am/pm.
            if !matches!(suffix, "am" | "pm") {
                return None;
            }
            (digits.parse::<u32>().ok()?, 0)
        }
    };
    let hour = match suffix {
        "pm" if hour < 12 => hour + 12,
        "am" if hour == 12 => 0,
        _ => hour,
    };
    (hour < 24 && minute < 60).then(|| format!("{hour:02}:{minute:02}"))
}

/// What `find -mtime` takes: `+N` for older than N days, `-N` for changed within N days.
fn file_age(words: &[String]) -> Option<String> {
    let has = |w: &str| words.iter().any(|x| x == w);
    for (i, word) in words.iter().enumerate() {
        let Some(n) = number(word).filter(|_| !matches!(word.as_str(), "a" | "an")) else { continue };
        let Some(minutes) = words.get(i + 1).and_then(|u| unit_minutes(u)) else { continue };
        // Whole days; fractions round the way that keeps the file in view ("48 hours old" is +1).
        let days = (n * minutes / (24 * 60)).max(if minutes < 24 * 60 { 1 } else { 0 });
        let before = |k: usize| i.checked_sub(k).map(|j| words[j].as_str()).unwrap_or_default();
        let after = |k: usize| words.get(i + k).map(String::as_str).unwrap_or_default();
        let older = matches!(before(1), "than" | "over" | "exceeding")
            && !matches!(before(2), "newer" | "less" | "fewer" | "within")
            || matches!(before(1), "least") && before(2) == "at"
            || (after(2) == "old" || after(2) == "ago") && matches!(before(1), "than" | "over")
            || (before(1) == "in" || before(1) == "for")
                && matches!(before(2), "modified" | "changed" | "touched")
                && words[..i].iter().any(|w| w == "not" || w == "untouched" || w == "unmodified");
        let newer = matches!(before(1), "last" | "past" | "within" | "previous")
            || matches!(before(2), "last" | "past" | "within") && matches!(before(1), "the")
            || matches!(before(2), "newer" | "less" | "fewer") && before(1) == "than"
            || matches!(before(1), "in") && (has("modified") || has("changed") || has("edited") || has("created"))
            || (has("recent") || has("recently")) && !older;
        if older {
            return Some(format!("+{days}"));
        }
        if newer {
            return Some(format!("-{}", days.max(1)));
        }
        // "files 30 days old": older. "files modified 3 days ago" means exactly that day, which
        // nobody wants; read it as within.
        if after(2) == "old" {
            return Some(format!("+{days}"));
        }
        if after(2) == "ago" {
            return Some(format!("-{}", days.max(1)));
        }
    }
    let phrase = |a: &str, b: &str| words.windows(2).any(|w| w[0] == a && w[1] == b);
    if phrase("this", "week") || phrase("last", "week") || phrase("past", "week") {
        return Some("-7".into());
    }
    if phrase("this", "month") || phrase("last", "month") || phrase("past", "month") {
        return Some("-30".into());
    }
    if has("today") {
        return Some("-1".into());
    }
    if has("yesterday") {
        return Some("-2".into());
    }
    None
}

/// Extensions that people name by saying "log files", "tmp files".
const EXTENSIONS: &[&str] = &[
    "log", "tmp", "bak", "txt", "pyc", "swp", "cache", "jpg", "jpeg", "png", "gif", "mp4", "mkv", "mp3", "zip", "gz",
    "tar", "iso", "pdf", "csv", "json", "md", "rs", "py", "js", "ts", "sh", "o", "orig", "old", "core", "lock",
];

/// A file name pattern: an explicit `*.log`, "named X", ".rs files" or "log files".
fn file_glob(request: &str, words: &[String]) -> Option<String> {
    if let Some(glob) =
        request.split_whitespace().map(|w| w.trim_matches(['"', '\'', ',', '?'])).find(|w| w.contains('*'))
    {
        return Some(glob.to_string());
    }
    for (i, word) in words.iter().enumerate() {
        if matches!(word.as_str(), "named" | "called")
            && let Some(name) = words.get(i + 1)
        {
            return Some(name.clone());
        }
        if matches!(word.as_str(), "file" | "files") && i > 0 {
            let before = words[i - 1].trim_start_matches('.');
            if words[i - 1].starts_with('.') && !before.is_empty() || EXTENSIONS.contains(&before) {
                return Some(format!("*.{before}"));
            }
        }
    }
    None
}

/// The port number: after "port", after ":" or after "on" (as in "listening on 5432").
fn port(words: &[String]) -> Option<String> {
    let valid =
        |w: &str| w.chars().all(|c| c.is_ascii_digit()) && w.parse::<u32>().is_ok_and(|n| (1..=65535).contains(&n));
    for (i, word) in words.iter().enumerate() {
        if word == "port"
            && let Some(next) = words.get(i + 1).filter(|w| valid(w))
        {
            return Some(next.clone());
        }
        if let Some(num) = word.strip_prefix("port").filter(|w| valid(w)) {
            return Some(num.to_string());
        }
        if matches!(word.as_str(), "on" | "at") && words.get(i + 1).is_some_and(|w| valid(w)) {
            return words.get(i + 1).cloned();
        }
        if let Some((_, tail)) = word.rsplit_once(':')
            && valid(tail)
            && !word.contains("//")
        {
            return Some(tail.to_string());
        }
    }
    // "nc web1 443": the last bare number, when the request talks about a port at all.
    if words.iter().any(|w| w == "port" || w == "ports") {
        return words.iter().rev().find(|w| valid(w)).cloned();
    }
    None
}

/// A dotted name that looks like a domain or an IP address: `example.com`, `1.1.1.1`.
fn is_domain(word: &str) -> bool {
    let labels: Vec<&str> = word.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        && labels.last().is_some_and(|tld| tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()))
        || labels.len() == 4 && labels.iter().all(|l| l.parse::<u8>().is_ok())
}

/// Words that introduce the DNS server to ask rather than the name to look up.
const SERVER_CUES: &[&str] = &["using", "via", "against", "server", "nameserver", "resolver"];

fn domain(request: &str) -> Option<String> {
    let words: Vec<&str> = request.split_whitespace().collect();
    words
        .iter()
        .enumerate()
        .filter(|(i, w)| {
            // "@8.8.8.8" and "using 1.1.1.1" name the server, not the domain.
            !w.starts_with('@')
                && !i.checked_sub(1).is_some_and(|j| SERVER_CUES.contains(&words[j].to_lowercase().as_str()))
        })
        .map(|(_, w)| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .map(|w| w.rsplit("://").next().unwrap_or(w).split('/').next().unwrap_or(w))
        .find(|w| is_domain(w))
        .map(str::to_string)
}

/// The server a DNS lookup should ask, as `dig` takes it: "@8.8.8.8", "using 1.1.1.1", "via ns1.example.com".
fn dns_server(request: &str) -> Option<String> {
    let words: Vec<&str> = request.split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        if let Some(rest) = word.strip_prefix('@') {
            let rest = rest.trim_matches(|c: char| !c.is_alphanumeric());
            if !rest.is_empty() {
                return Some(format!("@{rest}"));
            }
        }
        if SERVER_CUES.contains(&word.to_lowercase().as_str())
            && let Some(next) = words.get(i + 1).map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
            && is_domain(next)
        {
            return Some(format!("@{next}"));
        }
    }
    None
}

/// The machine a request is about: a domain or IP address, else the word after "on", "at" or "to".
fn host(request: &str, words: &[String]) -> Option<String> {
    if let Some(found) = domain(request) {
        return Some(found);
    }
    const NOT_HOSTS: &[&str] =
        &["this", "my", "the", "a", "an", "that", "your", "it", "here", "open", "port", "ports", "if", "is"];
    words
        .windows(2)
        .filter(|w| matches!(w[0].as_str(), "on" | "at" | "to" | "of"))
        .map(|w| &w[1])
        .find(|w| !NOT_HOSTS.contains(&w.as_str()) && number(w).is_none() && w.chars().any(|c| c.is_ascii_alphabetic()))
        .cloned()
}

/// The service a status question is about: "status of sshd", "is the nginx service running".
fn service(words: &[String]) -> Option<String> {
    const SKIP: &[&str] = &[
        "the", "a", "an", "my", "of", "for", "status", "state", "check", "show", "is", "are", "running", "service",
        "daemon", "unit", "up", "down", "on", "what", "whats", "current", "how", "doing", "alive", "working",
    ];
    let ok = |w: &String| !SKIP.contains(&w.as_str()) && w.chars().any(|c| c.is_ascii_alphabetic());
    for (i, word) in words.iter().enumerate() {
        if matches!(word.as_str(), "service" | "daemon" | "unit")
            && let Some(name) = i.checked_sub(1).map(|j| &words[j]).filter(|w| ok(w))
        {
            return Some(name.clone());
        }
    }
    for (i, word) in words.iter().enumerate() {
        if matches!(word.as_str(), "of" | "for" | "is" | "does")
            && let Some(name) = words[i + 1..].iter().find(|w| ok(w))
        {
            return Some(name.clone());
        }
    }
    None
}

/// "last 5 commits", "5 lines": a small count.
/// The ports of a forwarding request, in the order they are named: "forward local port 8080 to
/// port 80" gives 8080 then 80. A single port is both ends.
fn forwarded_port(words: &[String], index: usize) -> Option<String> {
    let ports: Vec<&String> =
        words.iter().filter(|w| !w.is_empty() && w.len() <= 5 && w.chars().all(|c| c.is_ascii_digit())).collect();
    ports.get(index).or(if index == 1 { ports.first() } else { None }).map(|p| p.to_string())
}

const NOT_NAMES: &[&str] = &[
    "the",
    "a",
    "an",
    "my",
    "this",
    "that",
    "user",
    "users",
    "account",
    "accounts",
    "named",
    "called",
    "and",
    "his",
    "her",
    "their",
    "its",
    "home",
    "folder",
    "directory",
    "dir",
    "from",
    "on",
    "of",
    "for",
    "to",
    "with",
    "too",
    "also",
    "please",
    "now",
    "it",
    "me",
    "i",
    "everything",
    "all",
    "files",
    "completely",
    "permanently",
    "entirely",
    "login",
    "password",
    "locally",
    "system",
    "machine",
    "computer",
    "so",
    "cannot",
    "can",
    "not",
];

/// The user a request is about: "delete user bob", "lock bob's account", "remove the account alice".
fn user_name(words: &[String]) -> Option<String> {
    let ok = |w: &str| {
        !NOT_NAMES.contains(&w) && w.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    };
    for (i, word) in words.iter().enumerate() {
        // "bob's account"
        if let Some(name) = word.strip_suffix("'s")
            && words.get(i + 1).is_some_and(|w| matches!(w.as_str(), "account" | "login" | "user"))
            && ok(name)
        {
            return Some(name.to_string());
        }
        // "user bob", "account alice", "named bob", "called bob", "account of alice"
        if matches!(word.as_str(), "user" | "users" | "account" | "named" | "called" | "of")
            && let Some(name) = words[i + 1..].iter().take(3).find(|w| ok(w))
            && !(matches!(word.as_str(), "of") && !words[..i].iter().any(|w| w == "account"))
        {
            return Some(name.clone());
        }
    }
    None
}

/// The name for a Docker image: "tagged myapp", "named web:1.0", "build image myapp".
fn image_tag(words: &[String]) -> Option<String> {
    const SKIP: &[&str] = &[
        "the",
        "a",
        "an",
        "my",
        "this",
        "from",
        "of",
        "in",
        "with",
        "using",
        "current",
        "folder",
        "directory",
        "dockerfile",
        "docker",
        "image",
        "to",
        "for",
        "here",
        "new",
        "called",
        "named",
        "tagged",
        "as",
        "tag",
        "it",
    ];
    for (i, word) in words.iter().enumerate() {
        if matches!(word.as_str(), "tagged" | "named" | "called" | "as" | "tag" | "-t")
            && let Some(name) = words[i + 1..].iter().take(2).find(|w| !SKIP.contains(&w.as_str()))
        {
            return Some(name.clone());
        }
    }
    if let Some(tagged) = words.iter().find(|w| {
        w.split_once(':').is_some_and(|(name, tag)| {
            !name.is_empty()
                && !tag.is_empty()
                && !name.contains('/')
                && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        })
    }) {
        return Some(tagged.clone());
    }
    let at = words.iter().position(|w| w == "image")?;
    words[at + 1..].iter().find(|w| !SKIP.contains(&w.as_str())).cloned()
}

/// What to rebase onto: "onto main", "on top of develop", "the last 3 commits" (`HEAD~3`).
fn rebase_target(words: &[String]) -> Option<String> {
    for (i, word) in words.iter().enumerate() {
        if matches!(word.as_str(), "onto" | "against" | "top")
            && let Some(name) = words[i + 1..].iter().find(|w| !matches!(w.as_str(), "of" | "the" | "branch"))
        {
            return Some(name.clone());
        }
    }
    let n = words.iter().position(|w| w == "commits" || w == "commit").and_then(|i| {
        let before = words.get(i.checked_sub(1)?)?;
        number(before)
    })?;
    Some(format!("HEAD~{n}"))
}

/// The folder a build reads from when the request gives one: `./app`, `~/proj`, `/srv/site`.
fn context_dir(request: &str) -> Option<String> {
    request
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| matches!(c, '"' | '\'' | ',' | '?' | '!' | ';')))
        .find(|w| (w.starts_with("./") || w.starts_with("~/") || w.starts_with('/')) && w.len() > 2)
        .map(str::to_string)
}

/// A JSON object written out in the request: `{"name": "x"}` (quotes around it are fine).
fn json_body(request: &str) -> Option<String> {
    let start = request.find('{')?;
    let end = request.rfind('}')?;
    (end > start).then(|| request[start..=end].to_string())
}

/// The program a version question is about: "version of node", "python version", "which node".
fn versioned_program(words: &[String]) -> Option<String> {
    const SKIP: &[&str] = &[
        "the",
        "a",
        "an",
        "my",
        "of",
        "is",
        "are",
        "do",
        "i",
        "have",
        "what",
        "which",
        "whats",
        "installed",
        "current",
        "check",
        "show",
        "get",
        "tell",
        "me",
        "version",
        "versions",
        "on",
        "this",
        "machine",
        "system",
        "computer",
        "running",
        "using",
        "use",
        "program",
        "command",
        "latest",
        "number",
        "for",
        "out",
        "find",
        "software",
        "app",
        "does",
        "it",
        "can",
        "you",
        "please",
        "how",
        "to",
        "see",
        "and",
        "that",
        "am",
        "now",
        "here",
    ];
    let ok = |w: &String| !SKIP.contains(&w.as_str()) && w.chars().any(|c| c.is_ascii_alphabetic());
    let at = words.iter().position(|w| w == "version" || w == "versions")?;
    // "version of node", "version of node do i have"
    if words.get(at + 1).is_some_and(|w| w == "of")
        && let Some(found) = words[at + 2..].iter().find(|w| ok(w))
    {
        return Some(found.clone());
    }
    // "node version", "check python version"
    words[..at].iter().rev().find(|w| ok(w)).cloned()
}

/// A device file named in the request: `/dev/sdb1`.
fn device(request: &str) -> Option<String> {
    request
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '/' && c != '-' && c != '_'))
        .find(|w| w.starts_with("/dev/") && w.len() > 5)
        .map(str::to_string)
}

/// A file named with an extension or a path: `backup.sh`, `./run`, `~/notes.txt`.
fn filename(request: &str) -> Option<String> {
    request
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| matches!(c, '"' | '\'' | ',' | '?' | '!' | ';')))
        .map(|w| w.trim_end_matches('.'))
        .find(|w| {
            if w.contains("://") || w.contains('@') {
                return false;
            }
            if w.starts_with("./") || w.starts_with("~/") || w.starts_with('/') {
                return w.len() > 2;
            }
            w.rsplit_once('.').is_some_and(|(stem, ext)| {
                !stem.is_empty() && (1..=5).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric())
            })
        })
        .map(str::to_string)
}

/// A Bluetooth address: six pairs of hex digits separated by colons.
fn mac_address(request: &str) -> Option<String> {
    request
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_ascii_hexdigit() && c != ':'))
        .find(|w| {
            let parts: Vec<&str> = w.split(':').collect();
            parts.len() == 6 && parts.iter().all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
        })
        .map(str::to_uppercase)
}

/// A level given as a percentage: "50%", "50 %" or "50 percent" gives `50%`.
fn percent(request: &str) -> Option<String> {
    let lower = request.to_lowercase();
    let tokens: Vec<&str> = lower.split_whitespace().collect();
    for (i, token) in tokens.iter().enumerate() {
        let token = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '%');
        if let Some(digits) = token.strip_suffix('%')
            && !digits.is_empty()
            && digits.chars().all(|c| c.is_ascii_digit())
        {
            return Some(format!("{digits}%"));
        }
        if !token.is_empty()
            && token.chars().all(|c| c.is_ascii_digit())
            && tokens.get(i + 1).is_some_and(|next| {
                next.trim_matches(|c: char| !c.is_alphanumeric() && c != '%') == "percent" || next.starts_with('%')
            })
        {
            return Some(format!("{token}%"));
        }
    }
    None
}

fn count(words: &[String]) -> Option<String> {
    for (i, word) in words.iter().enumerate() {
        let Some(n) = number(word).filter(|_| !matches!(word.as_str(), "a" | "an")) else { continue };
        let before = i.checked_sub(1).map(|j| words[j].as_str()).unwrap_or_default();
        let after = words.get(i + 1).map(String::as_str).unwrap_or_default();
        if matches!(before, "last" | "latest" | "recent" | "past" | "previous" | "first" | "top" | "n")
            || matches!(after.trim_end_matches('s'), "commit" | "line" | "entry" | "change")
        {
            return Some(n.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(reader: &str, request: &str) -> Option<String> {
        extract(reader, request)
    }

    #[test]
    fn shutdown_times() {
        assert_eq!(read("shutdown_time", "reboot in 10 minutes").as_deref(), Some("+10"));
        assert_eq!(read("shutdown_time", "reboot in ten minutes").as_deref(), Some("+10"));
        assert_eq!(read("shutdown_time", "shut down in 2 hours").as_deref(), Some("+120"));
        assert_eq!(read("shutdown_time", "in half an hour").as_deref(), Some("+30"));
        assert_eq!(read("shutdown_time", "shutdown at 23:00").as_deref(), Some("23:00"));
        assert_eq!(read("shutdown_time", "power off at 6pm").as_deref(), Some("18:00"));
        assert_eq!(read("shutdown_time", "reboot at 6:15").as_deref(), Some("06:15"));
        assert_eq!(read("shutdown_time", "power off at 12 am").as_deref(), Some("00:00"));
        assert_eq!(read("shutdown_time", "reboot now"), None);
        assert_eq!(read("shutdown_time", "reboot at 9"), None);
    }

    #[test]
    fn file_ages() {
        for (request, want) in [
            ("find files named *.log older than 7 days", "+7"),
            ("files more than 30 days old", "+30"),
            ("files not modified in 14 days", "+14"),
            ("files older than 2 weeks", "+14"),
            ("files modified in the last 3 days", "-3"),
            ("changed in the past 2 days", "-2"),
            ("files newer than 2 days", "-2"),
            ("files changed within 5 days", "-5"),
            ("files changed this week", "-7"),
            ("files changed today", "-1"),
            ("files modified in the last 24 hours", "-1"),
            ("files 90 days old", "+90"),
        ] {
            assert_eq!(read("file_age", request).as_deref(), Some(want), "{request}");
        }
        assert_eq!(read("file_age", "find files named x"), None);
    }

    #[test]
    fn file_globs() {
        assert_eq!(read("file_glob", "delete log files older than 7 days").as_deref(), Some("*.log"));
        assert_eq!(read("file_glob", "remove *.tmp files").as_deref(), Some("*.tmp"));
        assert_eq!(read("file_glob", "delete .pyc files").as_deref(), Some("*.pyc"));
        assert_eq!(read("file_glob", "remove files older than 30 days in /tmp"), None);
    }

    #[test]
    fn ports() {
        assert_eq!(read("port", "kill the process on port 3000").as_deref(), Some("3000"));
        assert_eq!(read("port", "what's listening on 5432").as_deref(), Some("5432"));
        assert_eq!(read("port", "check if port 443 open on web1").as_deref(), Some("443"));
        assert_eq!(read("port", "is port 22 open on 192.168.0.5").as_deref(), Some("22"));
        assert_eq!(read("port", "who uses localhost:8080").as_deref(), Some("8080"));
        assert_eq!(read("port", "what is using the network"), None);
    }

    #[test]
    fn hosts_and_domains() {
        assert_eq!(read("domain", "dig mx example.com").as_deref(), Some("example.com"));
        assert_eq!(read("domain", "dns lookup https://github.com/x").as_deref(), Some("github.com"));
        assert_eq!(read("domain", "look up 1.1.1.1").as_deref(), Some("1.1.1.1"));
        assert_eq!(read("domain", "dig mx"), None);
        assert_eq!(read("host", "check if port 443 open on web1").as_deref(), Some("web1"));
        assert_eq!(read("host", "is port 22 open on 192.168.0.5").as_deref(), Some("192.168.0.5"));
        assert_eq!(read("host", "is port 22 open on this machine"), None);
    }

    #[test]
    fn dns_servers() {
        assert_eq!(read("dns_server", "dig example.com @8.8.8.8").as_deref(), Some("@8.8.8.8"));
        assert_eq!(read("dns_server", "dig mx example.org using 1.1.1.1").as_deref(), Some("@1.1.1.1"));
        assert_eq!(read("dns_server", "look up example.org via ns1.example.net").as_deref(), Some("@ns1.example.net"));
        assert_eq!(read("dns_server", "dig example.com"), None);
        // The server is never the name to look up.
        assert_eq!(read("domain", "dig @8.8.8.8 example.com").as_deref(), Some("example.com"));
        assert_eq!(read("domain", "dig example.org using 1.1.1.1").as_deref(), Some("example.org"));
    }

    #[test]
    fn services() {
        assert_eq!(read("service", "check status of docker").as_deref(), Some("docker"));
        assert_eq!(read("service", "show status of postgresql").as_deref(), Some("postgresql"));
        assert_eq!(read("service", "is the nginx service running").as_deref(), Some("nginx"));
        assert_eq!(read("service", "is sshd running").as_deref(), Some("sshd"));
        assert_eq!(read("service", "status"), None);
    }

    #[test]
    fn filenames() {
        assert_eq!(read("filename", "how do i run backup.sh?").as_deref(), Some("backup.sh"));
        assert_eq!(read("filename", "copy ~/notes.txt to the clipboard").as_deref(), Some("~/notes.txt"));
        assert_eq!(read("filename", "copy text to clipboard"), None);
        assert_eq!(read("filename", "open example.com/page"), None);
    }

    #[test]
    fn forwarding_and_versions() {
        let w = |t: &str| words(t);
        assert_eq!(forwarded_port(&w("forward local port 8080 to port 80 on myserver"), 0).as_deref(), Some("8080"));
        assert_eq!(forwarded_port(&w("forward local port 8080 to port 80 on myserver"), 1).as_deref(), Some("80"));
        assert_eq!(forwarded_port(&w("tunnel port 5432"), 1).as_deref(), Some("5432"));
        assert_eq!(json_body("post {\"a\": 1} to https://x.io").as_deref(), Some("{\"a\": 1}"));
        assert_eq!(user_name(&w("delete the user bob and his home folder")).as_deref(), Some("bob"));
        assert_eq!(user_name(&w("lock bob's account")).as_deref(), Some("bob"));
        assert_eq!(user_name(&w("remove the account alice")).as_deref(), Some("alice"));
        assert_eq!(user_name(&w("delete a user")), None);
        assert_eq!(image_tag(&w("build a docker image tagged myapp from here")).as_deref(), Some("myapp"));
        assert_eq!(image_tag(&w("build docker image myapp:1.0")).as_deref(), Some("myapp:1.0"));
        assert_eq!(rebase_target(&w("rebase onto main")).as_deref(), Some("main"));
        assert_eq!(rebase_target(&w("squash the last 3 commits")).as_deref(), Some("HEAD~3"));
        assert_eq!(versioned_program(&w("what version of node do i have")).as_deref(), Some("node"));
        assert_eq!(versioned_program(&w("check python version")).as_deref(), Some("python"));
        assert_eq!(versioned_program(&w("which version of docker is installed")).as_deref(), Some("docker"));
    }

    #[test]
    fn percentages() {
        assert_eq!(read("percent", "set volume to 50%").as_deref(), Some("50%"));
        assert_eq!(read("percent", "brightness 30 percent").as_deref(), Some("30%"));
        assert_eq!(read("percent", "turn the volume up"), None);
    }

    #[test]
    fn counts() {
        assert_eq!(read("count", "show the last 5 commits").as_deref(), Some("5"));
        assert_eq!(read("count", "show 3 commits").as_deref(), Some("3"));
        assert_eq!(read("count", "show the commit log"), None);
    }
}
