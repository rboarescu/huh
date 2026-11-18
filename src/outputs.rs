//! What the output of common commands means (`lsblk`, `df -h`, `git status` ...).
//! The hand-written table is `outputs.txt` (format notes at its top), compiled in.
//! Recognition is by header line or shape and is strict on purpose: when unsure, say nothing.
//! Used by `cmd | huh`, `huh explain` on piped output, and `huh tutor`. Reads text only.

use crate::breakdown::Row;
use std::sync::OnceLock;

#[derive(Debug)]
enum Rule {
    /// The first line starts with these words (any one of the alternatives).
    First(Vec<Vec<String>>),
    /// Some line starts with this text (any alternative).
    Line(Vec<String>),
    /// The output contains this text (any alternative).
    Has(Vec<String>),
    Shape(String),
}

#[derive(Debug)]
struct Field {
    cond: Option<String>,
    key: String,
    text: String,
}

#[derive(Debug)]
pub struct Entry {
    pub name: String,
    title: String,
    rules: Vec<Rule>,
    header_columns: bool,
    fields: Vec<Field>,
    dynamic_perms: bool,
    notes: Vec<String>,
}

/// A recognised output, ready to print.
pub struct Described {
    /// The entry's name and the keys of the explained fields (read by the evals).
    #[cfg_attr(not(test), allow(dead_code))]
    pub name: &'static str,
    #[cfg_attr(not(test), allow(dead_code))]
    pub fields: Vec<String>,
    pub text: String,
}

fn parse(source: &str) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    for raw in source.lines() {
        let line = raw.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(head) = line.strip_prefix('@') {
            let (name, title) = head.split_once(" | ").expect("entry needs `@name | title`");
            entries.push(Entry {
                name: name.trim().to_string(),
                title: title.trim().to_string(),
                rules: Vec::new(),
                header_columns: false,
                fields: Vec::new(),
                dynamic_perms: false,
                notes: Vec::new(),
            });
            continue;
        }
        let entry = entries.last_mut().expect("line before the first entry");
        let alternatives = |rest: &str| rest.split(" | ").map(|a| a.trim().to_string()).collect::<Vec<_>>();
        if let Some(rest) = line.strip_prefix("first:") {
            let words = |a: &String| a.split_whitespace().map(str::to_string).collect::<Vec<_>>();
            entry.rules.push(Rule::First(alternatives(rest).iter().map(words).collect()));
        } else if let Some(rest) = line.strip_prefix("line:") {
            entry.rules.push(Rule::Line(alternatives(rest)));
        } else if let Some(rest) = line.strip_prefix("has:") {
            entry.rules.push(Rule::Has(alternatives(rest)));
        } else if let Some(rest) = line.strip_prefix("shape:") {
            entry.rules.push(Rule::Shape(rest.trim().to_string()));
        } else if line.trim() == "columns: header" {
            entry.header_columns = true;
        } else if line.trim() == "dyn: perms" {
            entry.dynamic_perms = true;
        } else if let Some(rest) = line.strip_prefix("note:") {
            entry.notes.push(rest.trim().to_string());
        } else {
            let (mut left, text) = line.split_once(" | ").unwrap_or_else(|| panic!("bad line in outputs.txt: {line}"));
            let mut cond = None;
            if let Some(inner) = left.strip_prefix('[').and_then(|l| l.split_once(']')) {
                cond = Some(inner.0.to_string());
                left = inner.1.trim();
            }
            entry.fields.push(Field { cond, key: left.trim().to_string(), text: text.trim().to_string() });
        }
    }
    entries
}

fn entries() -> &'static [Entry] {
    static E: OnceLock<Vec<Entry>> = OnceLock::new();
    E.get_or_init(|| parse(include_str!("outputs.txt")))
}

// ---------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------

fn is_perm_token(token: &str) -> bool {
    let chars: Vec<char> = token.chars().collect();
    (chars.len() == 10 || (chars.len() == 11 && ".+@".contains(chars[10])))
        && "-dlcbps".contains(chars[0])
        && chars[1..10].iter().all(|c| "-rwxsStT".contains(*c))
}

fn is_perm_line(line: &str) -> bool {
    let mut words = line.split_whitespace();
    words.next().is_some_and(is_perm_token) && line.split_whitespace().count() >= 9
}

fn is_hex(token: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&token.len()) && token.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

fn longlisting(lines: &[&str]) -> bool {
    let rest: Vec<&&str> = lines.iter().filter(|l| !l.starts_with("total ") && !l.ends_with(':')).collect();
    !rest.is_empty() && rest.iter().all(|l| is_perm_line(l))
}

fn du(lines: &[&str]) -> bool {
    !lines.is_empty()
        && lines.iter().all(|line| {
            let Some((size, path)) = line.split_once('\t') else { return false };
            let number = size.trim_end_matches(['K', 'M', 'G', 'T', 'P', 'E']);
            !path.trim().is_empty()
                && !number.is_empty()
                && number.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',')
                && number.chars().next().is_some_and(|c| c.is_ascii_digit())
        })
}

fn ipaddr(lines: &[&str]) -> bool {
    let Some((index, rest)) = lines[0].split_once(": ") else { return false };
    index.chars().all(|c| c.is_ascii_digit())
        && !index.is_empty()
        && rest.split_once(": <").is_some_and(|(name, _)| !name.contains(' '))
        && lines.iter().any(|l| l.trim_start().starts_with("link/"))
}

fn ipbr(lines: &[&str]) -> bool {
    lines.iter().all(|line| {
        let mut words = line.split_whitespace();
        words.next().is_some()
            && words
                .next()
                .is_some_and(|s| matches!(s, "UP" | "DOWN" | "UNKNOWN" | "LOWERLAYERDOWN" | "DORMANT" | "NOTPRESENT"))
    })
}

fn gitoneline(lines: &[&str]) -> bool {
    lines.len() >= 2
        && lines.iter().all(|line| {
            let mut words = line.split_whitespace();
            words.next().is_some_and(|w| is_hex(w, 7, 10)) && words.next().is_some()
        })
}

fn gitlog(lines: &[&str]) -> bool {
    lines[0].strip_prefix("commit ").is_some_and(|rest| is_hex(rest.split_whitespace().next().unwrap_or(""), 40, 40))
        && lines.iter().any(|l| l.starts_with("Author:"))
        && lines.iter().any(|l| l.starts_with("Date:"))
}

fn gitbranch(lines: &[&str]) -> bool {
    lines.iter().any(|l| l.starts_with("* "))
        && lines.iter().all(|line| {
            let Some(rest) =
                line.strip_prefix("* ").or_else(|| line.strip_prefix("+ ")).or_else(|| line.strip_prefix("  "))
            else {
                return false;
            };
            let words: Vec<&str> = rest.split_whitespace().collect();
            words.len() >= 3 && is_hex(words[1], 7, 40)
        })
}

fn journal_line(line: &str) -> bool {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.len() < 6 || !MONTHS.contains(&words[0]) {
        return false;
    }
    let time: Vec<&str> = words[2].split(':').collect();
    words[1].len() <= 2
        && words[1].chars().all(|c| c.is_ascii_digit())
        && time.len() == 3
        && time.iter().all(|t| t.len() == 2 && t.chars().all(|c| c.is_ascii_digit()))
        && words[4].ends_with(':')
}

fn journal(lines: &[&str]) -> bool {
    let body: Vec<&&str> = lines.iter().filter(|l| !l.starts_with("-- ")).collect();
    // A log that reports trouble is left to the error doctor.
    let calm = |l: &str| {
        let lower = l.to_lowercase();
        !["error", "fail", "warn", "denied", "fatal", "panic", "crit", "segfault"].iter().any(|w| lower.contains(w))
    };
    let matching = body.iter().filter(|l| journal_line(l) && calm(l)).count();
    matching >= 1 && matching * 10 >= body.len() * 8
}

fn uname(lines: &[&str]) -> bool {
    let words: Vec<&str> = lines[0].split_whitespace().collect();
    lines.len() == 1
        && words.len() >= 6
        && words[0] == "Linux"
        && words[2].starts_with(|c: char| c.is_ascii_digit())
        && words.iter().any(|w| w.starts_with('#'))
}

fn uptime(lines: &[&str]) -> bool {
    lines.len() == 1 && lines[0].contains(" up ") && lines[0].contains("load average:")
}

fn who(lines: &[&str]) -> bool {
    lines.iter().all(|line| {
        let words: Vec<&str> = line.split_whitespace().collect();
        let date: Vec<&str> = words.get(2).map_or(vec![], |d| d.split('-').collect());
        words.len() >= 4
            && ["tty", "pts/", ":", "seat", "console"].iter().any(|p| words[1].starts_with(p))
            && date.len() == 3
            && date[0].len() == 4
            && date.iter().all(|p| p.chars().all(|c| c.is_ascii_digit()))
            && words[3].contains(':')
    })
}

fn shape(name: &str, lines: &[&str]) -> Option<bool> {
    Some(match name {
        "longlisting" => longlisting(lines),
        "du" => du(lines),
        "ipaddr" => ipaddr(lines),
        "ipbr" => ipbr(lines),
        "gitoneline" => gitoneline(lines),
        "gitlog" => gitlog(lines),
        "gitbranch" => gitbranch(lines),
        "journal" => journal(lines),
        "uname" => uname(lines),
        "uptime" => uptime(lines),
        "who" => who(lines),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Matching and rendering
// ---------------------------------------------------------------------------

fn matches(entry: &Entry, text: &str, lines: &[&str]) -> bool {
    entry.rules.iter().all(|rule| match rule {
        Rule::First(alternatives) => {
            let words: Vec<&str> = lines[0].split_whitespace().collect();
            alternatives.iter().any(|a| words.len() >= a.len() && a.iter().zip(&words).all(|(x, y)| x == y))
        }
        Rule::Line(alternatives) => lines.iter().any(|l| alternatives.iter().any(|a| l.trim_start().starts_with(a))),
        Rule::Has(alternatives) => alternatives.iter().any(|a| text.contains(a.as_str())),
        Rule::Shape(name) => shape(name, lines).unwrap_or(false),
    })
}

/// "-rwxr-xr--" in words: what the owner, the group and everyone else may do.
fn decode_perms(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    let kind = match chars[0] {
        '-' => "a file",
        'd' => "a folder",
        'l' => "a link",
        _ => "a special file",
    };
    let who = |from: usize| {
        let mut allowed = Vec::new();
        if chars[from] == 'r' {
            allowed.push("read");
        }
        if chars[from + 1] == 'w' {
            allowed.push("write");
        }
        if "xst".contains(chars[from + 2]) {
            allowed.push("run/enter");
        }
        if allowed.is_empty() { "nothing".to_string() } else { allowed.join(", ") }
    };
    format!("{kind}; owner: {}; group: {}; everyone else: {}", who(1), who(4), who(7))
}

fn shown_fields<'a>(entry: &'a Entry, text: &str, lines: &[&str]) -> Vec<&'a Field> {
    let header: Vec<&str> = lines[0].split_whitespace().collect();
    entry
        .fields
        .iter()
        .filter(|f| match &f.cond {
            Some(cond) => text.contains(cond.as_str()),
            None if entry.header_columns => {
                let key: Vec<&str> = f.key.split_whitespace().collect();
                header.windows(key.len()).any(|w| w == key.as_slice())
            }
            None => true,
        })
        .collect()
}

fn render(entry: &'static Entry, text: &str, lines: &[&str]) -> Described {
    let mut rows: Vec<Row> = Vec::new();
    if entry.dynamic_perms
        && let Some(token) = lines.iter().find(|l| is_perm_line(l)).and_then(|l| l.split_whitespace().next())
    {
        rows.push(Row { piece: token.to_string(), meaning: decode_perms(token) });
    }
    let fields = shown_fields(entry, text, lines);
    rows.extend(fields.iter().map(|f| Row { piece: f.key.clone(), meaning: f.text.clone() }));
    let mut out = crate::style::paint(
        crate::style::Role::Header,
        &format!("This looks like the output of `{}`: {}.", entry.name, entry.title),
    );
    out.push('\n');
    crate::render::rows(&mut out, &rows, 2);
    for note in &entry.notes {
        crate::render::hanging_as(&mut out, 0, ("* ", crate::style::Role::Dim), (note, crate::style::Role::Note));
    }
    Described { name: entry.name.as_str(), fields: rows.into_iter().map(|r| r.piece).collect(), text: out }
}

/// Recognises piped or recorded normal output and explains its fields. `None` when the text is
/// not recognised, or has any line shaped like an error (then the error doctor's turn).
pub fn describe(raw: &str) -> Option<Described> {
    let text = crate::session::clean(raw);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() || lines.len() > 5000 || crate::doctor::has_error_shaped_line(&text) {
        return None;
    }
    let entry = entries().iter().find(|e| matches(e, &text, &lines))?;
    Some(render(entry, &text, &lines))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct File {
        case: Vec<Case>,
    }

    #[derive(Deserialize)]
    struct Case {
        /// Entry name, or "none" when nothing may be recognised.
        expect: String,
        #[serde(default)]
        fields: Vec<String>,
        output: String,
    }

    fn cases() -> Vec<Case> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/evals/outputs.toml");
        toml::from_str::<File>(&std::fs::read_to_string(path).unwrap()).expect("parse outputs.toml").case
    }

    #[test]
    fn data_file_is_well_formed() {
        let all = entries();
        assert!(all.len() >= 28, "{} entries", all.len());
        for e in all {
            assert!(!e.rules.is_empty() && !e.fields.is_empty(), "{}", e.name);
            assert!(e.notes.len() <= 2, "{}: at most 2 notes", e.name);
            for rule in &e.rules {
                if let Rule::Shape(name) = rule {
                    assert!(shape(name, &["x"]).is_some(), "{}: unknown shape {name}", e.name);
                }
            }
        }
    }

    #[test]
    fn captured_outputs_are_recognised_with_their_fields() {
        let cases = cases();
        for (n, case) in cases.iter().enumerate().filter(|(_, c)| c.expect != "none") {
            let described =
                describe(&case.output).unwrap_or_else(|| panic!("case {n} ({}) not recognised", case.expect));
            assert_eq!(described.name, case.expect, "case {n}");
            for field in &case.fields {
                assert!(described.fields.contains(field), "case {n} ({}): field {field} missing", case.expect);
            }
        }
        for e in entries() {
            assert!(cases.iter().any(|c| c.expect == e.name), "no eval case for {}", e.name);
        }
    }

    #[test]
    fn other_text_is_left_alone() {
        let cases = cases();
        let none: Vec<&Case> = cases.iter().filter(|c| c.expect == "none").collect();
        assert!(none.len() >= 15, "{} negative cases", none.len());
        for (n, case) in none.iter().enumerate() {
            let found = describe(&case.output).map(|d| d.name);
            assert_eq!(found, None, "negative case {n} was recognised");
        }
    }

    #[test]
    fn permission_strings_are_decoded() {
        assert_eq!(
            decode_perms("-rwxr-xr--"),
            "a file; owner: read, write, run/enter; group: read, run/enter; everyone else: read"
        );
        assert_eq!(
            decode_perms("drwx------"),
            "a folder; owner: read, write, run/enter; group: nothing; everyone else: nothing"
        );
    }

    #[test]
    fn describing_needs_no_columns_that_are_absent() {
        let d = describe("Filesystem Size Used Avail Use% Mounted on\n/dev/sda1 10G 1G 9G 10% /\n").unwrap();
        assert!(d.fields.contains(&"Avail".to_string()) && !d.fields.contains(&"Available".to_string()));
    }
}
