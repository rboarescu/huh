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
        _ => None,
    }
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
    fn counts() {
        assert_eq!(read("count", "show the last 5 commits").as_deref(), Some("5"));
        assert_eq!(read("count", "show 3 commits").as_deref(), Some("3"));
        assert_eq!(read("count", "show the commit log"), None);
    }
}
