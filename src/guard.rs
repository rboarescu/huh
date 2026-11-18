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
    // A device path (/dev/sdb, /dev/nvme0n1p2) or a bare device name (sdb1, mmcblk0) is a drive too.
    let device_name = |x: &String| {
        let lettered = ["sd", "hd", "vd", "xvd"].iter().find_map(|p| x.strip_prefix(p)).is_some_and(|rest| {
            let mut chars = rest.chars();
            chars.next().is_some_and(|c| c.is_ascii_lowercase()) && chars.all(|c| c.is_ascii_digit())
        });
        lettered
            || (x.starts_with("nvme") && x[4..].starts_with(|c: char| c.is_ascii_digit()))
            || (x.starts_with("mmcblk") && x[6..].starts_with(|c: char| c.is_ascii_digit()))
    };
    let device_path = request.split_whitespace().any(|t| is_disk_device(t.trim_matches([',', ';', '.', '?', '!'])));
    let whole_drive = any(&["disk", "drive", "usb", "partition", "stick", "pendrive", "ssd", "hdd", "nvme"])
        || (has("sd") && has("card"))
        || (has("flash") && has("drive"))
        || device_path
        || w.iter().any(device_name);
    let wipes = any(&[
        "format",
        "reformat",
        "wipe",
        "erase",
        "nuke",
        "destroy",
        "zero",
        "zeroize",
        "zeroise",
        "shred",
        "scrub",
        "overwrite",
        "blank",
        "mkfs",
        "obliterate",
    ]) || w.iter().any(|x| x.starts_with("mkfs"))
        || has("empty") && has("disk")
        || (device_path && any(&["delete", "remove", "rm", "secure", "securely"]));
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

// --- requests that are themselves command lines ---------------------------------------------------

/// Words that make a request prose rather than a command line ("du -sh the folders in /var").
const PROSE_WORDS: &[&str] = &[
    "the",
    "a",
    "an",
    "in",
    "to",
    "for",
    "of",
    "with",
    "me",
    "my",
    "all",
    "and",
    "into",
    "on",
    "by",
    "from",
    "at",
    "is",
    "that",
    "it",
    "process",
    "owned",
    "named",
    "called",
    "except",
    "excluding",
    "without",
    "every",
    "each",
    "only",
    "please",
    "i",
    "want",
    "how",
    "what",
    "which",
    "where",
    "under",
    "inside",
    "older",
    "newer",
];

/// A request that is itself a shell command line ("tar -xzf backup.tar.gz", "ls | wc -l"): its first
/// word is a program and it has flags, `NAME=value` arguments or shell operators. Matching its words
/// against the catalog would guess; `huh explain` reads it. An instruction with a command inside
/// ("run ls -la in src") starts with a plain word and is not one.
pub fn command_line_request<'a>(request: &'a str, known: &dyn Fn(&str) -> bool) -> Option<&'a str> {
    let line = request.trim().trim_matches('`').trim();
    let mut words = line.split_whitespace().peekable();
    if words.peek() == Some(&"sudo") {
        words.next();
    }
    let first = words.next()?;
    if !known(first) || !first.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_.".contains(c)) {
        return None;
    }
    let rest: Vec<&str> = words.collect();
    let flagged = rest.iter().any(|w| {
        let mut chars = w.chars();
        (chars.next() == Some('-') && chars.next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '-'))
            || matches!(*w, "|" | "||" | "&&" | ";" | ">" | ">>" | "<" | "2>&1")
            || (w.contains('=') && w.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))
    });
    let prose = rest.iter().any(|w| PROSE_WORDS.contains(&w.to_lowercase().as_str()));
    (flagged && !prose).then_some(line)
}

/// Targets that are the whole system or a whole user's files.
const SYSTEM_TARGETS: &[&str] = &[
    "/", "/*", "~", "~/", "~/*", "$HOME", "$HOME/", "$HOME/*", "${HOME}", "/etc", "/usr", "/bin", "/sbin", "/lib",
    "/lib64", "/boot", "/var", "/home", "/root", "/opt", "/dev", "/sys", "/proc", "/etc/*", "/usr/*", "/var/*",
    "/home/*", "/boot/*", "/bin/*", "/lib/*",
];

/// A path under /dev that names a disk, partition or other block device.
pub fn is_disk_device(path: &str) -> bool {
    let Some(name) = path.trim_matches(['"', '\'']).strip_prefix("/dev/") else { return false };
    let name = name.strip_prefix("disk/").unwrap_or(name);
    ["sd", "hd", "vd", "xvd", "nvme", "mmcblk", "md", "dm-", "mapper", "loop", "disk", "sr", "by-"]
        .iter()
        .any(|p| name.starts_with(p))
}

/// Whether a suggested line deletes, shreds or truncates a disk device (`rm /dev/sdb`): never a
/// right answer, whatever the request said.
pub fn deletes_a_drive(line: &str) -> bool {
    let mut words = line.split_whitespace().skip_while(|w| matches!(*w, "sudo" | "doas"));
    words.next().is_some_and(|p| matches!(p, "rm" | "rmdir" | "unlink" | "shred" | "truncate" | "mv"))
        && words.any(|w| is_disk_device(w) || w.starts_with("/dev/") && w != "/dev/null")
}

/// Whether a command line would destroy the whole system or a whole drive: a recursive delete or
/// permission change of `/`, `~` or a system folder, raw writes to a disk, a filesystem made on a
/// device, or a fork bomb.
pub fn destroys_system(line: &str) -> bool {
    if line.contains(":(){") || line.contains(":() {") {
        return true;
    }
    let tokens: Vec<&str> = line.split_whitespace().map(|w| w.trim_matches([';', '"', '\''])).collect();
    // `> /dev/sda`, `cat x | tee /dev/sda`.
    if tokens.windows(2).any(|w| matches!(w[0], ">" | ">>" | "tee") && is_disk_device(w[1])) {
        return true;
    }
    let mut args = tokens.iter().copied().peekable();
    while args.peek().is_some_and(|w| matches!(*w, "sudo" | "doas" | "nohup" | "time" | "exec")) {
        args.next();
    }
    let Some(program) = args.next() else { return false };
    let args: Vec<&str> = args.collect();
    let program = program.rsplit('/').next().unwrap_or(program);
    let recursive = args.iter().any(|a| {
        *a == "--recursive" || (a.starts_with('-') && !a.starts_with("--") && a.chars().any(|c| matches!(c, 'r' | 'R')))
    });
    let system_target = args.iter().any(|a| SYSTEM_TARGETS.contains(a));
    match program {
        "rm" | "chmod" | "chown" | "chgrp" | "mv" | "shred" | "find" | "rsync" => {
            args.contains(&"--no-preserve-root")
                || (system_target && (recursive || matches!(program, "mv" | "shred" | "rsync")))
                || (program == "find" && system_target && args.iter().any(|a| matches!(*a, "-delete" | "-exec")))
        }
        "dd" => args.iter().any(|a| a.strip_prefix("of=").is_some_and(is_disk_device)),
        "wipefs" | "blkdiscard" | "sgdisk" => args.iter().any(|a| is_disk_device(a)),
        p if p.starts_with("mkfs") || p == "mkswap" => args.iter().any(|a| is_disk_device(a)),
        _ => false,
    }
}

// --- "show me" requests never get a destructive answer ----------------------------------------------

/// Whether the request only asks to look at something (show, inspect, list, ...) and says nothing
/// about changing it. A destructive command is never the answer to such a request.
pub fn asks_to_look(request: &str) -> bool {
    const LOOK: &[&str] = &[
        "show", "inspect", "list", "display", "get", "print", "check", "find", "what", "which", "view", "see", "tell",
        "whats", "lookup",
    ];
    const CHANGE: &[&str] = &[
        "delet",
        "remov",
        "kill",
        "stop",
        "eras",
        "drop",
        "prun",
        "clean",
        "wip",
        "terminat",
        "destroy",
        "purg",
        "clear",
        "uninstall",
        "discard",
        "reset",
        "overwrit",
        "truncat",
        "shred",
        "rid",
        "rm",
        "force",
        "unlink",
        "revert",
        "undo",
        "format",
        "restart",
        "reboot",
        "shutdown",
        "poweroff",
        "halt",
        "trash",
        "flush",
        "evict",
        "zero",
        "nuke",
        "reclaim",
        "rollback",
        "abort",
        "cancel",
        "disable",
        "unmount",
        "umount",
    ];
    let w = words(request);
    let leads = w.iter().take(3).any(|x| LOOK.contains(&x.as_str()));
    // "show deleted files" describes; only a word that asks for the change itself counts.
    let changes = w.iter().any(|x| (x == "shred" || !x.ends_with("ed")) && CHANGE.iter().any(|c| x.starts_with(c)));
    leads && !changes
}

// --- "everything in X" means the contents, not X ----------------------------------------------------

/// The folder a request names the contents of: "all files in Documents", "everything in ~/old",
/// "the contents of build". Not "this folder" or other words for the current one.
pub fn contents_target(request: &str) -> Option<String> {
    let toks: Vec<String> = request
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| matches!(c, ',' | ';' | '?' | '!' | '"' | '\'' | '`')).trim_end_matches('.'))
        .map(str::to_string)
        .collect();
    let low: Vec<String> = toks.iter().map(|t| t.to_lowercase()).collect();
    let is = |i: usize, set: &[&str]| low.get(i).is_some_and(|w| set.contains(&w.as_str()));
    const THINGS: &[&str] =
        &["files", "folders", "directories", "contents", "content", "stuff", "things", "items", "data"];
    const PREPS: &[&str] = &["in", "inside", "from", "under", "within", "of"];
    let mut start = None;
    for i in 0..low.len() {
        let found = if is(i, &["all", "every"]) {
            let mut j = i + 1;
            if is(j, &["the", "of"]) {
                j += 1;
            }
            (is(j, THINGS) && is(j + 1, PREPS)).then_some(j + 2)
        } else if is(i, &["everything"]) {
            is(i + 1, PREPS).then_some(i + 2)
        } else if is(i, &["contents", "content"]) {
            is(i + 1, &["of"]).then_some(i + 2)
        } else {
            None
        };
        if found.is_some() {
            start = found;
            break;
        }
    }
    let mut j = start?;
    while is(j, &["the", "my", "your", "our"]) {
        j += 1;
    }
    let value = toks.get(j)?;
    let lower = value.to_lowercase();
    let current = matches!(
        lower.as_str(),
        "this" | "that" | "here" | "it" | "them" | "current" | "." | "./" | "~" | "/" | "$home"
    ) || value.contains('*')
        || value.contains('?');
    (!current && !value.is_empty()).then(|| value.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(word: &str) -> bool {
        matches!(word, "ls" | "rm" | "dd" | "du" | "tar" | "find" | "chmod" | "docker" | "cat")
    }

    #[test]
    fn command_lines_are_told_from_instructions() {
        for line in ["tar -xzf backup.tar.gz", "du -sh /var | sort -h", "dd if=/dev/zero of=/dev/sda", "sudo rm -rf /"]
        {
            assert_eq!(command_line_request(line, &known), Some(line), "{line}");
        }
        for request in [
            "run ls -la in src",
            "ls",
            "find large files",
            "du -sh the folders in /var",
            "tar the folder with -z please",
            "show disk usage",
        ] {
            assert_eq!(command_line_request(request, &known), None, "{request}");
        }
    }

    #[test]
    fn destructive_lines_on_the_whole_system_are_recognised() {
        for line in [
            "rm -rf /",
            "rm -rf /*",
            "sudo rm -fr ~",
            "rm -rf --no-preserve-root /",
            "chmod -R 777 /",
            "chown -R nobody /etc",
            "dd if=/dev/zero of=/dev/sda bs=1M",
            "mkfs.ext4 /dev/sda1",
            "wipefs -a /dev/nvme0n1",
            "cat /dev/zero > /dev/sda",
            ":(){ :|:& };:",
        ] {
            assert!(destroys_system(line), "{line}");
        }
        for line in ["rm -rf build", "rm -r /tmp/x", "chmod -R 755 uploads", "dd if=a.img of=b.img", "ls /"] {
            assert!(!destroys_system(line), "{line}");
        }
    }

    #[test]
    fn deleting_a_device_is_never_suggested() {
        assert!(deletes_a_drive("rm /dev/sdb"));
        assert!(deletes_a_drive("sudo shred -n 1 /dev/nvme0n1"));
        assert!(!deletes_a_drive("sudo fsck -n /dev/sdc1"));
        assert!(!deletes_a_drive("rm notes.txt"));
    }

    #[test]
    fn wipes_of_devices_are_refused_in_any_wording() {
        for request in [
            "wipe /dev/sdb",
            "securely erase /dev/nvme0n1",
            "zero out /dev/mmcblk0",
            "delete /dev/sdb1",
            "overwrite sdb with zeros",
            "shred /dev/sdc",
        ] {
            assert!(refusal(request).is_some(), "{request}");
        }
        for request in ["mount /dev/sdb1 on /mnt/usb", "check filesystem on /dev/sdc1", "show partitions of /dev/sda"] {
            assert!(refusal(request).is_none(), "{request}");
        }
    }

    #[test]
    fn looking_requests_are_told_from_changing_ones() {
        for request in ["show process tree of pid 1234", "docker inspect ip of container api", "what is using port 80"]
        {
            assert!(asks_to_look(request), "{request}");
        }
        for request in ["show me how to delete a file", "find and delete old logs", "stop container api", "kill pid 1"]
        {
            assert!(!asks_to_look(request), "{request}");
        }
        assert!(asks_to_look("show deleted files"));
    }

    #[test]
    fn everything_in_a_folder_names_its_contents() {
        for (request, folder) in [
            ("delete all files in Documents", "Documents"),
            ("delete everything in ~/Downloads", "~/Downloads"),
            ("remove the contents of build", "build"),
            ("get rid of all the files from /tmp/cache", "/tmp/cache"),
            ("wipe all files inside the logs folder", "logs"),
        ] {
            assert_eq!(contents_target(request).as_deref(), Some(folder), "{request}");
        }
        for request in
            ["delete the Documents folder", "delete everything in this folder", "delete all files", "delete *.log"]
        {
            assert_eq!(contents_target(request), None, "{request}");
        }
    }
}
