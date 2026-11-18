//! Readers for the sysadmin entries: values that sit in the wording of the request ("between main
//! and develop", "with the message wip-auth", "as deploy", "every day at 3am") instead of in a
//! single word the matcher could offer. A catalog slot names one with `extract = "sys_..."`;
//! `extract::extract` hands those names to `read` here, so they are applied like every other
//! reader. `refine` only holds whole-command wording fixes (tar). `cron_note` writes the line to
//! paste into `crontab -e`.

use crate::catalog::Command;
use crate::matcher::{Details, SlotPick};

/// Words of the request, with quotes, commas and sentence dots taken off. A quoted phrase stays one
/// word. A lone `.` stays (it names the current folder: "copy it to .").
fn words(request: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut quoted = String::new();
    let mut quote: Option<char> = None;
    for raw in request.split_whitespace() {
        if let Some(q) = quote {
            if let Some(stripped) = raw.strip_suffix(q) {
                quoted.push(' ');
                quoted.push_str(stripped);
                out.push(std::mem::take(&mut quoted));
                quote = None;
            } else {
                quoted.push(' ');
                quoted.push_str(raw);
            }
            continue;
        }
        if let Some(first) = raw.chars().next().filter(|c| matches!(c, '"' | '\'' | '`'))
            && !raw[1..].ends_with(first)
        {
            quote = Some(first);
            quoted = raw[1..].to_string();
            continue;
        }
        let word = raw.trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | ',' | ';' | '?' | '!' | '(' | ')'));
        let word = if word.len() > 1 { word.trim_end_matches('.') } else { word };
        if !word.is_empty() {
            out.push(word.to_string());
        }
    }
    if quote.is_some() && !quoted.is_empty() {
        out.push(quoted);
    }
    out
}

fn lower(words: &[String]) -> Vec<String> {
    words.iter().map(|w| w.to_lowercase()).collect()
}

/// The word after the first of `markers`, skipping `skip` words.
fn after(words: &[String], markers: &[&str], skip: &[&str]) -> Option<String> {
    let low = lower(words);
    let at = low.iter().position(|w| markers.contains(&w.as_str()))?;
    words[at + 1..].iter().find(|w| !skip.contains(&w.to_lowercase().as_str())).cloned()
}

fn looks_like_ip(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| !p.is_empty() && p.len() <= 3 && p.chars().all(|c| c.is_ascii_digit()))
}

/// A path or file name: it has a slash, a dot-extension, or is `.` / `~`.
fn path_like(word: &str) -> bool {
    if word == "." || word == ".." || word == "~" {
        return true;
    }
    word.contains('/')
        || word.starts_with('~')
        || word.rsplit_once('.').is_some_and(|(stem, ext)| {
            !stem.is_empty()
                && (1..=8).contains(&ext.len())
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
                && !word.chars().all(|c| c.is_ascii_digit() || c == '.')
        })
}

pub fn is_interface_name(s: &str) -> bool {
    let lower = s.to_lowercase();
    lower == "lo"
        || [
            "eth",
            "enp",
            "eno",
            "ens",
            "wlan",
            "wlp",
            "wlo",
            "br",
            "docker",
            "veth",
            "tun",
            "tap",
            "virbr",
            "bond",
            "tailscale",
        ]
        .iter()
        .any(|p| lower.starts_with(p) && lower[p.len()..].chars().all(|c| c.is_ascii_alphanumeric()))
}

fn is_commit_hash(s: &str) -> bool {
    (7..=40).contains(&s.len())
        && s.chars().all(|c| c.is_ascii_hexdigit())
        && s.chars().any(|c| c.is_ascii_digit())
        && s.chars().any(|c| c.is_ascii_alphabetic())
}

/// The two names after "between X and Y", "compare X and Y", "compare X with Y" or "diff X Y".
pub fn ref_pair(request: &str) -> Option<(String, String)> {
    const FILLER: &[&str] = &[
        "the", "of", "branch", "branches", "tag", "tags", "commit", "commits", "and", "with", "to", "against", "vs",
        "versus", "from", "between", "changes", "changed",
    ];
    let words = words(request);
    for (i, word) in words.iter().enumerate() {
        if !matches!(word.to_lowercase().as_str(), "between" | "compare" | "comparing" | "diff" | "difference") {
            continue;
        }
        let mut rest = words[i + 1..].iter().filter(|w| !FILLER.contains(&w.to_lowercase().as_str()));
        if let (Some(a), Some(b)) = (rest.next(), rest.next()) {
            return Some((a.clone(), b.clone()));
        }
    }
    None
}

/// The text that names a stash: quoted, or the word after "message", "named", "called".
fn stash_message(request: &str) -> Option<String> {
    let words = words(request);
    after(
        &words,
        &["message", "msg", "named", "called", "label", "labelled", "labeled", "description", "comment"],
        &["the", "a", "an", "is", "of", "as", "to", "it", "this"],
    )
}

/// "push for the first time", "set upstream": the `-u origin HEAD` flag already names remote and branch.
fn first_push(request: &str) -> bool {
    let low = request.to_lowercase();
    ["first time", "upstream", "track", "set up tracking"].iter().any(|p| low.contains(p))
}

fn push_remote(request: &str) -> Option<String> {
    if first_push(request) {
        return None;
    }
    let words = words(request);
    words.iter().find(|w| matches!(w.to_lowercase().as_str(), "origin" | "upstream")).cloned()
}

fn push_branch(request: &str) -> Option<String> {
    if first_push(request) {
        return None;
    }
    let words = words(request);
    let low = lower(&words);
    if let Some(at) = low.iter().position(|w| matches!(w.as_str(), "origin" | "upstream")) {
        let next = words.get(at + 1)?;
        let stop = ["and", "with", "please", "for", "now", "again", "first", "to", "then", "so", "because"];
        if stop.contains(&next.to_lowercase().as_str()) {
            return None;
        }
        if matches!(next.to_lowercase().as_str(), "branch" | "the" | "my") {
            return words.get(at + 2).filter(|w| !stop.contains(&w.to_lowercase().as_str())).cloned();
        }
        return Some(next.clone());
    }
    after(&words, &["branch"], &["called", "named", "the", "my", "to", "origin"])
        .filter(|b| !matches!(b.to_lowercase().as_str(), "to" | "on" | "now" | "please"))
}

/// "from main", "based on develop", "starting from v1.0".
fn base_branch(request: &str) -> Option<String> {
    let words = words(request);
    after(&words, &["from", "off", "on"], &["the", "branch", "a", "my", "current"])
        .filter(|b| !matches!(b.to_lowercase().as_str(), "scratch" | "here" | "now" | "there"))
}

fn pid(request: &str) -> Option<String> {
    let words = words(request);
    let low = lower(&words);
    let digits = |w: &str| !w.is_empty() && w.len() <= 8 && w.chars().all(|c| c.is_ascii_digit());
    for (i, w) in low.iter().enumerate() {
        if matches!(w.as_str(), "pid" | "process" | "id")
            && let Some(n) = words[i + 1..].iter().take(2).find(|n| digits(n))
        {
            return Some(n.clone());
        }
    }
    words.iter().enumerate().find(|(i, w)| digits(w) && (*i == 0 || low[*i - 1] != "port")).map(|(_, w)| w.clone())
}

/// A word with a slash or tilde that names a file rather than a folder (`notes/a.txt`).
fn path_is_file_like(word: &str) -> bool {
    word.rsplit('/').next().is_some_and(|last| last.contains('.') && !last.starts_with('.') && !last.ends_with('.'))
}

/// A published port as `host:container`: "8080:80", or "port 6379" for the same number on both sides.
fn port_map(request: &str) -> Option<String> {
    let words = words(request);
    let port = |p: &str| (1..=5).contains(&p.len()) && p.chars().all(|c| c.is_ascii_digit());
    let mapped =
        words.iter().find(|w| w.split_once(':').is_some_and(|(host, container)| port(host) && port(container)));
    mapped.cloned().or_else(|| {
        let low = lower(&words);
        let at = low.iter().position(|w| w == "port" || w == "ports")?;
        words.get(at + 1).filter(|n| port(n)).map(|n| format!("{n}:{n}"))
    })
}

/// The image of a `docker run` request: the first word that is not part of the run wording, a port
/// or the name given to the container.
fn run_image(request: &str) -> Option<String> {
    const WORDING: &[&str] = &[
        "docker",
        "run",
        "start",
        "launch",
        "create",
        "new",
        "container",
        "image",
        "the",
        "a",
        "an",
        "from",
        "with",
        "on",
        "in",
        "as",
        "and",
        "to",
        "of",
        "my",
        "detached",
        "detach",
        "background",
        "interactive",
        "interactively",
        "shell",
        "port",
        "ports",
        "name",
        "named",
        "called",
        "please",
        "using",
        "use",
        "up",
        "spin",
        "an",
        "it",
        "that",
        "can",
        "i",
        "me",
        "want",
        "need",
        "then",
        "tty",
        "terminal",
        "session",
    ];
    let words = words(request);
    let low = lower(&words);
    let mut skip_next = false;
    for (i, word) in words.iter().enumerate() {
        if std::mem::take(&mut skip_next) {
            continue;
        }
        let lw = low[i].as_str();
        if matches!(lw, "name" | "named" | "called" | "port" | "ports") {
            skip_next = true;
            continue;
        }
        let mapped = word.split_once(':').is_some_and(|(a, b)| a.chars().all(|c| c.is_ascii_digit()) && !b.is_empty());
        if WORDING.contains(&lw) || mapped || word.chars().all(|c| c.is_ascii_digit()) || word.starts_with('-') {
            continue;
        }
        return Some(word.clone());
    }
    None
}

/// An age in the form docker's `until=` filter takes: "older than 24h" is `24h`, "3 days" is `72h`.
fn until_age(request: &str) -> Option<String> {
    let words = lower(&words(request));
    let hours = |n: u64, unit: &str| match unit {
        "h" | "hr" | "hrs" | "hour" | "hours" => Some(n),
        "d" | "day" | "days" => Some(n * 24),
        "w" | "week" | "weeks" => Some(n * 24 * 7),
        _ => None,
    };
    for (i, word) in words.iter().enumerate() {
        let digits = word.trim_end_matches(|c: char| c.is_ascii_alphabetic());
        let Ok(n) = digits.parse::<u64>() else { continue };
        let unit = if digits.len() < word.len() { &word[digits.len()..] } else { words.get(i + 1).map_or("", |u| u) };
        if let Some(h) = hours(n, unit) {
            return Some(format!("{h}h"));
        }
    }
    None
}

/// A path named in the request, the first that has a slash or an extension.
fn first_path(request: &str) -> Option<String> {
    words(request).into_iter().find(|w| path_like(w) && !looks_like_ip(w) && w != "." && w != "..")
}

fn ordinal(word: &str) -> Option<u32> {
    const NAMES: &[&str] =
        &["first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth", "ninth", "tenth"];
    if let Some(i) = NAMES.iter().position(|n| *n == word) {
        return Some(i as u32 + 1);
    }
    let digits = word.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let suffix = &word[digits.len()..];
    (matches!(suffix, "st" | "nd" | "rd" | "th") && !digits.is_empty()).then(|| digits.parse().ok()).flatten()
}

/// "second column", "3rd column", "column 2", "field 4".
fn column_number(request: &str) -> Option<String> {
    let low = lower(&words(request));
    for (i, w) in low.iter().enumerate() {
        if !matches!(w.as_str(), "column" | "columns" | "field" | "fields") {
            continue;
        }
        if let Some(n) = i.checked_sub(1).and_then(|j| ordinal(&low[j])) {
            return Some(n.to_string());
        }
        if let Some(n) = low.get(i + 1).filter(|n| n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty()) {
            return Some(n.clone());
        }
    }
    None
}

/// The command to run detached: "run sleep 600 in background with nohup" gives `sleep 600`.
fn background_command(request: &str) -> Option<String> {
    const MARKERS: &[&str] = &[
        " in the background",
        " in background",
        " in the bg",
        " in a background",
        " with nohup",
        " using nohup",
        " and keep",
        " so it",
        " so that",
        " detached",
        " nohup",
        " in a way",
        " in the backround",
        " and let",
        " even after",
        " after i log",
        " after logout",
        " in the terminal",
    ];
    let text = request.trim().trim_end_matches(['.', '!', '?']);
    let low = text.to_lowercase();
    let mut start = 0;
    for verb in ["please ", "run ", "execute ", "start ", "launch ", "keep ", "nohup "] {
        if low[start..].starts_with(verb) {
            start += verb.len();
        }
    }
    let end = MARKERS.iter().filter_map(|m| low[start..].find(m)).min().map_or(text.len(), |at| start + at);
    let mut command = text[start..end].trim().to_string();
    if let Some(rest) = command.strip_prefix("nohup ") {
        command = rest.trim().to_string();
    }
    if command.is_empty() {
        return None;
    }
    let script = [".sh", ".bash", ".py", ".pl", ".rb"].iter().any(|ext| command.ends_with(ext));
    if script && !command.contains(' ') && !command.starts_with(['/', '.', '~']) {
        command = format!("./{command}");
    }
    Some(command)
}

/// A folder name people delete everywhere: node_modules, __pycache__, build, ...
fn dir_name(request: &str) -> Option<String> {
    const NAMES: &[&str] = &[
        "node_modules",
        "__pycache__",
        "pycache",
        ".cache",
        "build",
        "dist",
        "target",
        ".venv",
        "venv",
        ".pytest_cache",
        ".gradle",
        "vendor",
        ".next",
        ".tox",
    ];
    let words = words(request);
    words
        .iter()
        .find(|w| NAMES.contains(&w.to_lowercase().as_str()))
        .cloned()
        .or_else(|| after(&words, &["named", "called"], &["the", "a"]))
}

/// "as deploy", "as user deploy": the account to log in as.
fn as_user(request: &str) -> Option<String> {
    let words = words(request);
    let found = after(&words, &["as"], &["user", "the", "a", "an", "account"])?;
    let name = found.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        && found.chars().next().is_some_and(char::is_alphabetic)
        && !matches!(found.to_lowercase().as_str(), "well" | "soon" | "possible" | "needed" | "usual" | "before");
    name.then_some(found)
}

fn container_name(request: &str) -> Option<String> {
    let words = words(request);
    after(&words, &["container", "containers"], &["named", "called", "the", "a", "id"])
        .filter(|c| !matches!(c.to_lowercase().as_str(), "to" | "at" | "from" | "into" | "in" | "and"))
}

/// The paths named in a `docker cp` request, in order, leaving out the container.
pub fn cp_paths(request: &str) -> Vec<String> {
    let container = container_name(request);
    words(request).into_iter().filter(|w| path_like(w) && Some(w) != container.as_ref() && !looks_like_ip(w)).collect()
}

fn cidr(request: &str) -> Option<String> {
    words(request).into_iter().find(|w| looks_like_ip(w.split('/').next().unwrap_or_default()))
}

fn interface(request: &str) -> Option<String> {
    let words = words(request);
    words
        .iter()
        .find(|w| is_interface_name(w))
        .cloned()
        .or_else(|| after(&words, &["interface", "dev", "nic"], &["the", "named", "called", "a"]))
}

/// "500M", "500mb", "2 gb": a size journalctl and du understand.
fn size_spec(request: &str) -> Option<String> {
    let words = words(request);
    for (i, w) in words.iter().enumerate() {
        let low = w.to_lowercase();
        let split = low.find(|c: char| !c.is_ascii_digit()).unwrap_or(low.len());
        let (number, unit) = low.split_at(split);
        let unit = if unit.is_empty() {
            words.get(i + 1).map(|u| u.to_lowercase()).unwrap_or_default()
        } else {
            unit.to_string()
        };
        if number.is_empty() {
            continue;
        }
        let letter = match unit.as_str() {
            "k" | "kb" | "kilobytes" | "kib" => 'K',
            "m" | "mb" | "megabytes" | "mib" => 'M',
            "g" | "gb" | "gigabytes" | "gib" => 'G',
            _ => continue,
        };
        return Some(format!("{number}{letter}"));
    }
    None
}

/// "replace A with B in F": the text to look for (`true`) or its replacement (`false`).
fn substitution(request: &str, from: bool) -> Option<String> {
    let words = words(request);
    let low = lower(&words);
    let verb = low.iter().position(|w| matches!(w.as_str(), "replace" | "substitute" | "swap" | "change"))?;
    let with = low.iter().skip(verb + 1).position(|w| matches!(w.as_str(), "with" | "by" | "for" | "to"))? + verb + 1;
    if from {
        let found = words[verb + 1..with]
            .iter()
            .filter(|w| {
                !matches!(
                    w.to_lowercase().as_str(),
                    "all" | "every" | "occurrences" | "of" | "the" | "word" | "text" | "string"
                )
            })
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        return (!found.is_empty()).then(|| escape_sed(&found));
    }
    let stop = ["in", "inside", "within", "across", "from", "throughout", "on", "everywhere"];
    let end =
        low.iter().skip(with + 1).position(|w| stop.contains(&w.as_str())).map_or(words.len(), |at| with + 1 + at);
    let found = words[with + 1..end]
        .iter()
        .filter(|w| !matches!(w.to_lowercase().as_str(), "the" | "word" | "text" | "string"))
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    (!found.is_empty()).then(|| escape_sed(&found))
}

/// The computer an ssh tunnel goes through: "on bastion", "via db1", "through user@host".
fn tunnel_host(request: &str) -> Option<String> {
    const NOT_HOSTS: &[&str] = &[
        "the",
        "a",
        "an",
        "my",
        "this",
        "that",
        "your",
        "it",
        "here",
        "port",
        "ports",
        "local",
        "localhost",
        "remote",
        "server",
        "ssh",
        "over",
        "tunnel",
        "me",
        "us",
        "if",
        "is",
        "and",
        "then",
        "also",
    ];
    let words = words(request);
    if let Some(with_user) = words.iter().find(|w| w.contains('@') && !w.contains('/')) {
        return Some(with_user.clone());
    }
    if let Some(domain) = words.iter().find(|w| {
        looks_like_ip(w)
            || (w.contains('.') && !w.contains('/') && w.chars().any(|c| c.is_ascii_alphabetic()) && !path_like_file(w))
    }) {
        return Some(domain.clone());
    }
    let low = lower(&words);
    words.windows(2).enumerate().find_map(|(i, pair)| {
        let marker = matches!(low[i].as_str(), "on" | "at" | "via" | "through" | "using" | "to" | "from");
        let next = pair[1].to_lowercase();
        (marker
            && !NOT_HOSTS.contains(&next.as_str())
            && !next.chars().all(|c| c.is_ascii_digit())
            && next.chars().any(|c| c.is_ascii_alphabetic())
            && next.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
        .then(|| pair[1].clone())
    })
}

/// A file name such as `app.log` (as opposed to a host name such as `db.example.com`).
fn path_like_file(word: &str) -> bool {
    word.rsplit_once('.').is_some_and(|(_, ext)| {
        matches!(ext, "log" | "txt" | "conf" | "yml" | "yaml" | "sh" | "py" | "json" | "md" | "env" | "cfg" | "ini")
    })
}

/// A value inside `s/.../.../g` needs its slashes and, in the replacement, ampersands escaped.
fn escape_sed(value: &str) -> String {
    value.replace('\\', "\\\\").replace('/', "\\/").replace('&', "\\&")
}

fn mount_point(request: &str) -> Option<String> {
    words(request).into_iter().find(|w| w.starts_with('/') && !w.starts_with("/dev/") && w.len() > 1)
}

/// The value a `sys_*` reader finds in the request.
pub fn read(reader: &str, request: &str) -> Option<String> {
    match reader {
        "sys_ref_first" => ref_pair(request).map(|(a, _)| a),
        "sys_ref_second" => ref_pair(request).map(|(_, b)| b),
        "sys_stash_message" => stash_message(request),
        "sys_push_remote" => push_remote(request),
        "sys_push_branch" => push_branch(request),
        "sys_base_branch" => base_branch(request),
        "sys_pid" => pid(request),
        "sys_port_map" => port_map(request),
        "sys_stats_container" => {
            // "for container redis", "stats of redis", "container named web": a name, never "each" or "all".
            const NOT_NAMES: &[&str] = &[
                "the",
                "a",
                "an",
                "my",
                "each",
                "every",
                "all",
                "any",
                "every",
                "using",
                "is",
                "are",
                "it",
                "them",
                "this",
                "that",
                "running",
                "docker",
                "container",
                "containers",
                "named",
                "called",
                "stats",
                "usage",
                "memory",
                "cpu",
                "ram",
                "processor",
                "resources",
                "live",
                "now",
                "right",
                "of",
                "for",
                "per",
            ];
            let words = words(request);
            let low = lower(&words);
            (0..words.len())
                .filter(|&i| matches!(low[i].as_str(), "container" | "containers" | "for" | "of" | "named" | "called"))
                .find_map(|i| {
                    let next = words
                        .get(i + 1..)?
                        .iter()
                        .find(|w| !matches!(w.to_lowercase().as_str(), "the" | "a" | "named" | "called"))?;
                    (!NOT_NAMES.contains(&next.to_lowercase().as_str()) && !next.chars().all(|c| c.is_ascii_digit()))
                        .then(|| next.clone())
                })
        }
        "sys_run_image" => run_image(request),
        "sys_under_dir" => {
            let words = words(request);
            let dir = |w: &str| w.contains('/') || w.starts_with('~') || w == "." || w == "..";
            const SKIP: &[&str] = &[
                "the",
                "my",
                "this",
                "a",
                "folder",
                "directory",
                "dir",
                "current",
                "here",
                "all",
                "every",
                "each",
                "files",
                "file",
            ];
            let low = lower(&words);
            words.iter().find(|w| dir(w) && !path_is_file_like(w)).cloned().or_else(|| {
                // The first word after "in", "under", ... that can be a folder name (not ".rs" or "3").
                (0..words.len())
                    .filter(|&i| matches!(low[i].as_str(), "in" | "under" | "inside" | "within" | "below"))
                    .find_map(|i| {
                        let next = words[i + 1..].iter().find(|w| !SKIP.contains(&w.to_lowercase().as_str()))?;
                        (!next.contains('.')
                            && !next.chars().all(|c| c.is_ascii_digit())
                            && !matches!(next.as_str(), "in" | "under" | "inside" | "within" | "below"))
                        .then(|| next.clone())
                    })
            })
        }
        "sys_name" => after(&words(request), &["named", "called", "name"], &["the", "a", "an", "it", "as", "is", "to"]),
        "sys_comment" => after(&words(request), &["comment", "label"], &["the", "a", "an", "is", "as", "to", "of"]),
        "sys_depth" => {
            let words = words(request);
            after(&words, &["depth"], &["of", "to", "is", "="])
                .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
                .or_else(|| lower(&words).iter().any(|w| w == "shallow").then(|| "1".to_string()))
        }
        "sys_clone_dir" => {
            let words = words(request);
            let remote = |w: &str| w.contains("://") || w.starts_with("git@");
            after(&words, &["into", "in", "to"], &["the", "a", "folder", "directory", "dir", "new"])
                .filter(|w| !remote(w) && !w.chars().all(|c| c.is_ascii_digit()))
                .or_else(|| words.iter().find(|w| !remote(w) && (w.contains('/') || w.starts_with('~'))).cloned())
        }
        "sys_until_age" => until_age(request),
        "sys_path" => first_path(request),
        "sys_host" => tunnel_host(request),
        "sys_compose_service" => {
            let words = words(request);
            after(
                &words,
                &["restart", "restarts"],
                &["the", "service", "services", "container", "docker", "compose", "my", "a"],
            )
            .filter(|w| {
                !matches!(
                    w.to_lowercase().as_str(),
                    "in" | "all" | "everything" | "again" | "now" | "please" | "with" | "using" | "for"
                )
            })
        }
        "sys_search_dir" => {
            let found =
                words(request).into_iter().find(|w| w.contains('/') || w.starts_with('~') || w.starts_with('.'));
            found.filter(|w| dir_name(request).as_deref() != Some(w.as_str())).or_else(|| {
                after(
                    &words(request),
                    &["in", "under", "inside", "below", "within"],
                    &[
                        "the",
                        "my",
                        "this",
                        "a",
                        "folder",
                        "directory",
                        "dir",
                        "current",
                        "here",
                        "all",
                        "every",
                        "each",
                    ],
                )
                .filter(|w| {
                    dir_name(request).as_ref() != Some(w)
                        && !matches!(
                            w.to_lowercase().as_str(),
                            "here" | "folder" | "directory" | "them" | "it" | "subfolders" | "subdirectories"
                        )
                })
            })
        }
        "sys_folder" => first_path(request).or_else(|| {
            after(
                &words(request),
                &["in", "inside", "under", "within"],
                &["the", "my", "this", "a", "folder", "directory", "dir", "that"],
            )
        }),
        "sys_column" => column_number(request),
        "sys_background_command" => background_command(request),
        "sys_dir_name" => dir_name(request),
        "sys_as_user" => as_user(request),
        "sys_container" => container_name(request),
        "sys_cp_first" => cp_paths(request).into_iter().next(),
        "sys_cp_last" => {
            let paths = cp_paths(request);
            (paths.len() >= 2).then(|| paths.last().cloned()).flatten()
        }
        "sys_cidr" => cidr(request),
        "sys_interface" => interface(request),
        "sys_size" => size_spec(request),
        "sys_sub_from" => substitution(request, true),
        "sys_sub_to" => substitution(request, false),
        "sys_mount_point" => mount_point(request),
        "sys_commit" => words(request).into_iter().find(|w| is_commit_hash(w)),
        _ => None,
    }
}

/// Fills the slots whose `extract` is a `sys_*` reader. Slots the reader finds nothing for stay empty
/// (the default or a placeholder).
/// Wording fixes that need a whole command's slots at once. Readers named by `extract = "sys_..."`
/// are applied with all the others, through `extract::extract`.
pub fn refine(command: &Command, request: &str, details: &mut Details) {
    if command.name == "tar" {
        tar_inputs_from_wording(command, request, details);
    }
}

/// "tar.gz of src excluding node_modules": the word `tar.gz` names the archive type, not what goes
/// into it. When the binder took it for the input, what follows "of" / "from" is the input.
fn tar_inputs_from_wording(command: &Command, request: &str, details: &mut Details) {
    let Some(at) = command.slots.iter().position(|s| s.id == "inputs") else { return };
    let SlotPick::Value { value, .. } = &details.slots[at] else { return };
    let lower = value.to_lowercase();
    if !(lower == "tar.gz" || lower == "targz" || lower == "tgz" || lower == "tar") {
        return;
    }
    let words = words(request);
    let named = after(&words, &["of", "from", "for"], &["the", "a", "an", "my", "folder", "directory", "dir"])
        .filter(|w| !matches!(w.to_lowercase().as_str(), "tar.gz" | "excluding" | "exclude" | "except" | "without"));
    details.slots[at] = match &named {
        Some(value) => SlotPick::Value { named: 0.9, value: value.clone() },
        None => SlotPick::Unnamed,
    };
    // "tar.gz of src" names the archive too: src.tar.gz.
    let archive = command.slots.iter().position(|s| s.id == "archive");
    if let (Some(archive), Some(input)) =
        (archive, named.filter(|n| n.chars().all(|c| c.is_ascii_alphanumeric() || "_./-".contains(c))))
        && !matches!(details.slots[archive], SlotPick::Value { .. })
    {
        let base = input.trim_end_matches('/').rsplit('/').next().unwrap_or_default().trim_start_matches('.');
        if !base.is_empty() {
            details.slots[archive] = SlotPick::Value { named: 0.9, value: format!("{base}.tar.gz") };
        }
    }
}

/// The `crontab -e` line for "run backup.sh every day at 3am": `0 3 * * * /path/to/backup.sh`.
pub fn cron_line(request: &str) -> Option<String> {
    let words = lower(&words(request));
    let originals = self::words(request);
    let has = |w: &str| words.iter().any(|x| x == w);
    let pair = |a: &str, b: &str| words.windows(2).any(|w| w[0] == a && w[1] == b);

    // Time of day: 3am, 3:30pm, 15:00, noon, midnight.
    let mut hm: Option<(u32, u32)> = None;
    for (i, w) in words.iter().enumerate() {
        let next = words.get(i + 1).map(String::as_str).unwrap_or_default();
        let (clock, meridiem) = if let Some(c) = w.strip_suffix("am") {
            (c, Some(false))
        } else if let Some(c) = w.strip_suffix("pm") {
            (c, Some(true))
        } else if matches!(next, "am" | "pm") && w.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            (w.as_str(), Some(next == "pm"))
        } else if w.contains(':') {
            (w.as_str(), None)
        } else {
            continue;
        };
        let (h, m) = clock.split_once(':').unwrap_or((clock, "0"));
        let (Ok(mut h), Ok(m)) = (h.parse::<u32>(), m.parse::<u32>()) else { continue };
        match meridiem {
            Some(true) if h < 12 => h += 12,
            Some(false) if h == 12 => h = 0,
            _ => {}
        }
        if h < 24 && m < 60 {
            hm = Some((h, m));
            break;
        }
    }
    if hm.is_none() {
        if has("midnight") {
            hm = Some((0, 0));
        } else if has("noon") {
            hm = Some((12, 0));
        }
    }

    let weekday = ["sunday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday"]
        .iter()
        .position(|d| words.iter().any(|w| w == d || w.strip_suffix('s') == Some(*d)));
    let interval = |unit: &str| -> Option<u32> {
        words.windows(3).find_map(|w| {
            (w[0] == "every" && w[2].trim_end_matches('s') == unit).then(|| w[1].parse::<u32>().ok()).flatten()
        })
    };
    let (h, m) = hm.unwrap_or((0, 0));
    let spec = if has("minute") && pair("every", "minute") {
        "* * * * *".to_string()
    } else if let Some(n) = interval("minute") {
        format!("*/{n} * * * *")
    } else if pair("every", "hour") || has("hourly") {
        format!("{m} * * * *")
    } else if let Some(n) = interval("hour") {
        format!("{m} */{n} * * *")
    } else if let Some(day) = weekday {
        format!("{m} {h} * * {day}")
    } else if pair("every", "week") || has("weekly") {
        format!("{m} {h} * * 0")
    } else if pair("every", "month") || has("monthly") {
        format!("{m} {h} 1 * *")
    } else if pair("every", "day")
        || pair("every", "night")
        || pair("every", "morning")
        || pair("every", "evening")
        || has("daily")
        || has("nightly")
    {
        format!("{m} {h} * * *")
    } else {
        return None;
    };

    let command = originals
        .iter()
        .find(|w| [".sh", ".py", ".bash", ".pl", ".rb"].iter().any(|ext| w.to_lowercase().ends_with(ext)))
        .map(|script| if script.contains('/') { script.clone() } else { format!("/path/to/{script}") })
        .unwrap_or_else(|| "/path/to/command".to_string());
    Some(format!("{spec} {command}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_two_refs() {
        assert_eq!(ref_pair("show diff between main and develop"), Some(("main".into(), "develop".into())));
        assert_eq!(ref_pair("compare branches main and release"), Some(("main".into(), "release".into())));
        assert_eq!(ref_pair("what changed between v1.0 and v2.0"), Some(("v1.0".into(), "v2.0".into())));
    }

    #[test]
    fn reads_the_stash_message() {
        assert_eq!(stash_message("stash changes with message wip-auth").as_deref(), Some("wip-auth"));
        assert_eq!(stash_message("stash my work with the message wip-login").as_deref(), Some("wip-login"));
        assert_eq!(
            stash_message("stash my changes with message \"before refactor\"").as_deref(),
            Some("before refactor")
        );
    }

    #[test]
    fn reads_the_background_command() {
        assert_eq!(background_command("run sleep 600 in background with nohup").as_deref(), Some("sleep 600"));
        assert_eq!(background_command("run backup.sh in the background with nohup").as_deref(), Some("./backup.sh"));
        assert_eq!(background_command("nohup python3 serve.py &").as_deref(), Some("python3 serve.py &"));
    }

    #[test]
    fn reads_substitutions() {
        assert_eq!(substitution("replace foo with bar in nginx.conf", true).as_deref(), Some("foo"));
        assert_eq!(substitution("replace foo with bar in nginx.conf", false).as_deref(), Some("bar"));
        assert_eq!(substitution("replace http://a with https://b in x.conf", false).as_deref(), Some("https:\\/\\/b"));
    }

    #[test]
    fn writes_cron_lines() {
        let line = |r: &str| cron_line(r);
        assert_eq!(line("schedule backup.sh every day at 3am").as_deref(), Some("0 3 * * * /path/to/backup.sh"));
        assert_eq!(line("run cleanup.sh every hour").as_deref(), Some("0 * * * * /path/to/cleanup.sh"));
        assert_eq!(line("run /opt/x.sh every 15 minutes").as_deref(), Some("*/15 * * * * /opt/x.sh"));
        assert_eq!(line("run job.py every monday at 9:30am").as_deref(), Some("30 9 * * 1 /path/to/job.py"));
        assert_eq!(line("run a.sh every day at 11pm").as_deref(), Some("0 23 * * * /path/to/a.sh"));
        assert_eq!(line("list my cron jobs"), None);
    }

    #[test]
    fn reads_docker_cp_paths() {
        assert_eq!(cp_paths("copy app.log into container web01 at /tmp"), ["app.log", "/tmp"]);
        assert_eq!(cp_paths("copy /var/log/app.log from container api to ."), ["/var/log/app.log", "."]);
    }

    #[test]
    fn reads_sizes_and_columns() {
        assert_eq!(size_spec("limit the journal to 500M").as_deref(), Some("500M"));
        assert_eq!(size_spec("shrink journal to 2 gb").as_deref(), Some("2G"));
        assert_eq!(column_number("print second column of access.log").as_deref(), Some("2"));
        assert_eq!(column_number("print the 3rd column of data.txt").as_deref(), Some("3"));
        assert_eq!(column_number("show column 4 of x.log").as_deref(), Some("4"));
    }
}
