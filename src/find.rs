//! `huh find`: a fuzzy finder over huh's catalog and every installed program.
//!
//! It only looks things up: names come from the catalog and `$PATH`, descriptions and flags from
//! the catalog, the built-in table and man pages on disk. With `fzf` installed it hands the list to
//! it (launch.rs); otherwise a small line-based finder does the job. Nothing selected is run.

use crate::catalog::{self, Command};
use crate::knowledge::{ArgKind, Knowledge};
use crate::manpage::{self, ManPage};
use crate::platform;
use crate::render;
use crate::style::{Role, paint};
use crate::tools::{self, NO_DESCRIPTION, Tool};
use anyhow::Result;
use std::path::PathBuf;

/// Where an item comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    Catalog,
    Installed,
}

impl Tag {
    pub fn label(self) -> &'static str {
        match self {
            Tag::Catalog => "[catalog]",
            Tag::Installed => "[installed]",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub description: String,
    pub tag: Tag,
    /// Extra search words (catalog keywords); never shown.
    pub keywords: String,
}

/// The separator between name and description: the first field of a line is the name.
pub const SEPARATOR: &str = " \u{2014} ";

/// `name — description [tag]`.
pub fn item_line(item: &Item) -> String {
    format!("{}{SEPARATOR}{} {}", item.name, item.description, item.tag.label())
}

/// The name at the start of a line made by `item_line`.
pub fn name_of_line(line: &str) -> &str {
    line.split(SEPARATOR).next().unwrap_or(line).trim()
}

/// Catalog entries first, then the installed programs that have no catalog entry (a program is
/// shown once: by its catalog entry when it has one).
pub fn build_items(catalog: &[Command], tools: &[Tool]) -> Vec<Item> {
    let mut items: Vec<Item> = catalog
        .iter()
        .map(|c| Item {
            name: c.name.clone(),
            description: c.description.clone(),
            tag: Tag::Catalog,
            keywords: c.keywords.join(" "),
        })
        .collect();
    let covered: std::collections::HashSet<&str> =
        catalog.iter().flat_map(|c| [c.name.as_str(), c.program()]).filter(|n| !n.is_empty()).collect();
    items.extend(tools.iter().filter(|t| !covered.contains(t.name.as_str())).map(|t| Item {
        name: t.name.clone(),
        description: t.description.clone(),
        tag: Tag::Installed,
        keywords: String::new(),
    }));
    items
}

// ---------------------------------------------------------------------------
// Fuzzy scoring
// ---------------------------------------------------------------------------

const NO_LIMIT: usize = usize::MAX;
/// Longest run of skipped characters allowed between two matched ones in a description, so that
/// a short word does not "match" letters scattered across a sentence.
const DESC_GAP: usize = 2;
/// Added to each matched word of a catalog entry.
const CATALOG_BONUS: i32 = 80;

fn word_start(chars: &[char], j: usize) -> bool {
    j == 0 || !chars[j - 1].is_alphanumeric()
}

/// Subsequence match of `query` (no spaces) in `text`, case-insensitive. Each matched character
/// scores 1, plus 8 at the start of a word and 6 when it follows the previous match directly;
/// starting late costs a little. `max_gap` limits the skipped characters between matches.
pub fn score(query: &str, text: &str, max_gap: usize) -> Option<i32> {
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    if q.is_empty() {
        return Some(0);
    }
    if q.len() > t.len() {
        return None;
    }
    const NEG: i32 = i32::MIN / 2;
    // best[j]: the best score with the current query character matched at text position j.
    let mut best = vec![NEG; t.len()];
    for (i, qc) in q.iter().enumerate() {
        let mut next = vec![NEG; t.len()];
        for j in 0..t.len() {
            if t[j] != *qc {
                continue;
            }
            let bonus = 1 + if word_start(&t, j) { 8 } else { 0 };
            let base = if i == 0 {
                -(j.min(3) as i32)
            } else {
                let mut from = NEG;
                for k in (0..j).rev().take(max_gap.saturating_add(1)) {
                    if best[k] == NEG {
                        continue;
                    }
                    let joined = if k + 1 == j { 6 } else { -(1 + (j - k - 1).min(4) as i32) };
                    from = from.max(best[k] + joined);
                }
                from
            };
            if base > NEG {
                next[j] = base + bonus;
            }
        }
        best = next;
    }
    best.into_iter().filter(|s| *s > NEG).max()
}

/// How well an item answers the query: every word must match the name, the description or the
/// keywords. A name match counts more than a description match. `None` when a word matches nowhere.
pub fn item_score(query: &str, item: &Item) -> Option<i32> {
    let name = item.name.to_lowercase();
    let mut total = 0;
    for word in query.split_whitespace() {
        let w = word.to_lowercase();
        let in_name = score(&w, &name, NO_LIMIT).map(|s| {
            let exact = if name == w {
                100
            } else if name.starts_with(&w) {
                30
            } else {
                0
            };
            s * 2 + exact
        });
        let in_desc = score(&w, &item.description, DESC_GAP);
        let in_keywords = score(&w, &item.keywords, DESC_GAP).map(|s| s - 3);
        total += [in_name, in_desc, in_keywords].into_iter().flatten().max()?;
        // A catalog entry is a curated answer: it comes first among equal matches.
        if item.tag == Tag::Catalog {
            total += CATALOG_BONUS;
        }
    }
    Some(total)
}

/// Indexes of the items that match, best first (catalog before installed, then by name, on ties).
/// An empty query matches everything in list order.
pub fn rank(items: &[Item], query: &str) -> Vec<usize> {
    let mut hits: Vec<(i32, usize)> =
        items.iter().enumerate().filter_map(|(i, item)| Some((item_score(query, item)?, i))).collect();
    if query.split_whitespace().next().is_some() {
        hits.sort_by(|a, b| {
            let (x, y) = (&items[a.1], &items[b.1]);
            b.0.cmp(&a.0).then((x.tag as u8).cmp(&(y.tag as u8))).then(x.name.cmp(&y.name))
        });
    }
    hits.into_iter().map(|(_, i)| i).collect()
}

// ---------------------------------------------------------------------------
// Details
// ---------------------------------------------------------------------------

/// Longest `huh find --describe` output, so that it fits a preview.
const MAX_LINES: usize = 25;
const MAX_FLAGS: usize = 6;
const FLAG_MEANING_CHARS: usize = 70;

/// Where details come from; the real one reads this machine, tests give their own.
pub struct Sources<'a> {
    pub catalog: &'a [Command],
    pub platforms: &'a [String],
    /// Where a program is installed.
    pub locate: &'a dyn Fn(&str) -> Option<PathBuf>,
    /// The program's man page.
    pub man: &'a dyn Fn(&str) -> Option<ManPage>,
    /// One-line description of an installed program (built-in table, else man page).
    pub summary: &'a dyn Fn(&str) -> Option<String>,
}

fn cut(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= max {
        return text;
    }
    let short: String = text.chars().take(max - 3).collect();
    format!("{}...", short.trim_end())
}

fn flag_label(names: &[String], arg: &str) -> String {
    let names = names.join(", ");
    if arg.is_empty() { names } else { format!("{names} {arg}") }
}

/// Up to `MAX_FLAGS` flags with their meaning: the built-in table first, then the man page.
fn common_flags(path: &[String], program: &str, src: &Sources) -> Vec<(String, String)> {
    let knowledge = Knowledge::get();
    let section = knowledge.section(path).or_else(|| knowledge.section(&[program.to_string()]));
    let mut flags: Vec<(String, Vec<String>, String)> = section
        .map(|s| {
            s.opts
                .iter()
                .take(MAX_FLAGS)
                .map(|o| {
                    let arg = match &o.arg {
                        ArgKind::None => String::new(),
                        ArgKind::Required(label) => label.clone(),
                        ArgKind::Optional(label) => format!("[{label}]"),
                    };
                    (flag_label(&o.names, &arg), o.names.clone(), o.meaning.clone())
                })
                .collect()
        })
        .unwrap_or_default();
    if flags.len() < MAX_FLAGS
        && let Some(page) = (src.man)(program)
    {
        for option in &page.options {
            if flags.len() >= MAX_FLAGS {
                break;
            }
            let taken = flags.iter().any(|(_, names, _)| names.iter().any(|n| option.names.contains(n)));
            if option.desc.is_empty() || taken {
                continue;
            }
            let label = flag_label(&option.names, option.arg.as_deref().unwrap_or(""));
            flags.push((label, option.names.clone(), option.desc.clone()));
        }
    }
    flags.into_iter().map(|(label, _, meaning)| (label, meaning)).collect()
}

/// The details of one catalog entry or program (what Enter shows and the fzf preview prints).
pub fn describe(name: &str, src: &Sources) -> String {
    let name = name.trim();
    let entry = src
        .catalog
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(name))
        .or_else(|| src.catalog.iter().find(|c| c.program() == name));
    let program = entry.map_or(name, |c| c.program());
    let shown_name = entry.map_or(name, |c| c.name.as_str());
    let path: Vec<String> = shown_name.split_whitespace().map(str::to_string).collect();
    let installed = (src.locate)(program);
    let builtin = catalog::is_builtin(program);
    let description = match entry {
        Some(c) => Some(c.description.clone()),
        None => (src.summary)(program).filter(|d| d != NO_DESCRIPTION),
    };
    let known = Knowledge::get().section(&[program.to_string()]).is_some();
    if entry.is_none() && installed.is_none() && description.is_none() && !known {
        return format!("No catalog entry or installed program called `{name}`.\n");
    }
    let tag = if entry.is_some() { Tag::Catalog } else { Tag::Installed };
    let mut head = format!("{}  {}\n", paint(Role::Header, shown_name), paint(Role::Dim, tag.label()));
    let description = description.unwrap_or_else(|| NO_DESCRIPTION.to_string());
    render::hanging_as(&mut head, 0, ("", Role::Dim), (&description, Role::Description));
    let status = match (&installed, builtin) {
        (Some(path), _) => paint(Role::Success, &format!("installed ({})", path.display())),
        (None, true) => paint(Role::Success, "built into the shell"),
        (None, false) => {
            let package = entry.and_then(|c| c.package.as_deref());
            let hint = match platform::install_advice(program, package, src.platforms) {
                Some(a) if a.installs => format!("Install: {}", a.command),
                Some(a) => format!("Find the package: {}", a.command),
                None => "no install hint for this system".to_string(),
            };
            format!("{} {}", paint(Role::Warning, "not installed."), paint(Role::Note, &hint))
        }
    };
    head.push_str(&format!("{} {status}\n", paint(Role::Dim, "Status:")));
    if let Some(c) = entry {
        head.push_str(&format!("{} {}\n", paint(Role::Dim, "Command:"), paint(Role::Command, &c.template)));
        if let Some(warning) = c.warning.as_deref().filter(|w| !w.is_empty()) {
            let tag = c.risk.map(|r| format!("[{}]", r.name())).unwrap_or_default();
            let text = format!("{tag} {warning}");
            render::hanging_tagged(&mut head, 0, ("WARNING ", Role::Warning), &tag, text.trim());
        }
    }
    let flags = common_flags(&path, program, src);
    let mut body: Vec<String> = Vec::new();
    if !flags.is_empty() {
        body.push(paint(Role::Dim, "Common flags:"));
        let width = flags.iter().map(|(l, _)| l.chars().count()).max().unwrap_or(0).min(24);
        for (label, meaning) in &flags {
            body.push(format!(
                "  {}  {}",
                paint(Role::Part, &format!("{label:<width$}")),
                paint(Role::Description, &cut(meaning, FLAG_MEANING_CHARS))
            ));
        }
    }
    // Keep it within a preview: a long header leaves room for fewer flags.
    body.truncate(MAX_LINES.saturating_sub(head.lines().count() + 1));
    let mut out = head;
    for line in body {
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str(&format!("{} man {program}\n", paint(Role::Dim, "More:")));
    out
}

/// The details as found on this machine.
pub fn describe_here(name: &str, everything: &[Command]) -> String {
    let platforms = catalog::this_platform();
    let usable = catalog::usable(everything.to_vec(), &platforms, false);
    let path = platform::path_dirs();
    let locate = |program: &str| match platform::locate_in(program, &path, &[]) {
        platform::Found::OnPath(p) | platform::Found::SbinOnly(p) => Some(p),
        platform::Found::Missing => None,
    };
    let summary = |program: &str| {
        let found = [(program.to_string(), PathBuf::new())];
        Some(tools::describe(&found, &[], &manpage::man_dirs(), true).pop()?.description)
    };
    let src =
        Sources { catalog: &usable, platforms: &platforms, locate: &locate, man: &manpage::load, summary: &summary };
    describe(name, &src)
}

// ---------------------------------------------------------------------------
// The built-in finder (no fzf)
// ---------------------------------------------------------------------------

/// Results shown per search.
pub const TOP: usize = 15;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Waiting for words to search for.
    Search,
    /// Waiting for a number; holds the item indexes shown.
    Pick(Vec<usize>),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Step {
    pub mode: Mode,
    pub out: String,
    pub done: bool,
}

pub fn prompt(mode: &Mode) -> &'static str {
    match mode {
        Mode::Search => "find> ",
        Mode::Pick(_) => "number (Enter: new search, q: quit)> ",
    }
}

fn colored_line(item: &Item) -> String {
    format!(
        "{}{SEPARATOR}{} {}",
        paint(Role::Part, &item.name),
        paint(Role::Description, &item.description),
        paint(Role::Dim, item.tag.label())
    )
}

/// One turn of the built-in finder: what was typed, and what happens. `details` describes an item.
pub fn step(mode: &Mode, line: &str, items: &[Item], details: &dyn Fn(&Item) -> String) -> Step {
    let line = line.trim();
    let stay = |mode: &Mode, out: String| Step { mode: mode.clone(), out, done: false };
    let note = |text: &str| format!("{}\n", paint(Role::Note, text));
    if line == "q" {
        return Step { mode: mode.clone(), out: String::new(), done: true };
    }
    if let Mode::Pick(shown) = mode {
        if line.is_empty() {
            return stay(&Mode::Search, String::new());
        }
        if let Ok(number) = line.parse::<usize>() {
            return match number.checked_sub(1).and_then(|n| shown.get(n)) {
                Some(&index) => stay(mode, format!("\n{}\n", details(&items[index]))),
                None => stay(mode, note(&format!("pick a number from 1 to {}", shown.len()))),
            };
        }
        // Words instead of a number: a new search.
    }
    if line.is_empty() {
        return stay(&Mode::Search, note("type some words to search for, or q to quit"));
    }
    let hits: Vec<usize> = rank(items, line).into_iter().take(TOP).collect();
    if hits.is_empty() {
        return stay(&Mode::Search, note("nothing matches; try other words"));
    }
    let width = hits.len().to_string().len() + 1;
    let mut out = String::new();
    for (n, &index) in hits.iter().enumerate() {
        out.push_str(&format!(
            "{} {}\n",
            paint(Role::Dim, &format!("{:>width$}", format!("{}.", n + 1))),
            colored_line(&items[index])
        ));
    }
    Step { mode: Mode::Pick(hits), out, done: false }
}

fn fallback(items: &[Item], words: &str, details: &dyn Fn(&Item) -> String) -> Result<()> {
    use std::io::{BufRead, Write};
    let mut mode = Mode::Search;
    if !words.trim().is_empty() {
        let s = step(&mode, words, items, details);
        print!("{}", s.out);
        mode = s.mode;
    }
    let stdin = std::io::stdin();
    loop {
        print!("{}", paint(Role::Dim, prompt(&mode)));
        std::io::stdout().flush()?;
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            println!();
            return Ok(());
        }
        let s = step(&mode, &line, items, details);
        print!("{}", s.out);
        if s.done {
            return Ok(());
        }
        mode = s.mode;
    }
}

// ---------------------------------------------------------------------------
// The command
// ---------------------------------------------------------------------------

/// `huh find [words]`: lists the matches when piped, uses fzf when installed, else the built-in finder.
pub fn run(words: &[String], everything: &[Command]) -> Result<()> {
    use std::io::IsTerminal;
    let platforms = catalog::this_platform();
    let usable = catalog::usable(everything.to_vec(), &platforms, false);
    let items = build_items(&usable, &tools::load_all(everything));
    let query = words.join(" ");
    if !std::io::stdout().is_terminal() {
        let mut out = String::new();
        for index in rank(&items, &query) {
            out.push_str(&item_line(&items[index]));
            out.push('\n');
        }
        print!("{out}");
        return Ok(());
    }
    let dirs = platform::path_dirs();
    if !matches!(platform::locate_in("fzf", &dirs, &[]), platform::Found::Missing) {
        let huh = std::env::current_exe()?;
        let argv = crate::launch::fzf_argv(&query, &huh, crate::style::enabled());
        let input: String = items.iter().map(|i| item_line(i) + "\n").collect();
        if let Some(line) = crate::launch::run_fzf(&argv, &input)? {
            println!("{}", describe_here(name_of_line(&line), everything));
        }
        return Ok(());
    }
    println!(
        "{}",
        paint(Role::Dim, "huh find: fzf is not installed, so this is the plain finder. Nothing here is run.")
    );
    fallback(&items, &query, &|item| describe_here(&item.name, everything))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn item(name: &str, description: &str, tag: Tag) -> Item {
        Item { name: name.into(), description: description.into(), tag, keywords: String::new() }
    }

    fn tool(name: &str, description: &str) -> Tool {
        Tool { name: name.into(), description: description.into() }
    }

    fn names(items: &[Item], query: &str) -> Vec<String> {
        rank(items, query).into_iter().map(|i| items[i].name.clone()).collect()
    }

    fn catalog() -> Vec<Command> {
        catalog::load_embedded().unwrap()
    }

    #[test]
    fn consecutive_and_word_start_matches_score_higher() {
        let s = |q: &str, t: &str| score(q, t, NO_LIMIT).unwrap();
        assert!(s("gz", "gzip") > s("gz", "big zebra"), "consecutive over scattered");
        assert!(s("ls", "ls") > s("ls", "tools"), "word start over the middle");
        assert!(s("fo", "foo-bar") > s("fo", "xfoo"), "an earlier start wins");
        assert!(s("pb", "paste-buffer") > s("pb", "palabra"), "word starts beat scattered letters");
        assert!(score("zip", "gzip", NO_LIMIT).is_some());
        assert_eq!(score("pig", "gzip", NO_LIMIT), None, "letters must come in order");
        assert_eq!(score("", "anything", NO_LIMIT), Some(0));
        assert_eq!(score("abcdef", "abc", NO_LIMIT), None);
        assert_eq!(score("TAR", "tar", NO_LIMIT), score("tar", "TAR", NO_LIMIT), "case does not matter");
    }

    #[test]
    fn a_gap_limit_keeps_short_words_out_of_long_sentences() {
        assert_eq!(score("tar", "show the amount of free space", DESC_GAP), None);
        assert!(score("tar", "show the amount of free space", NO_LIMIT).is_some());
        assert!(score("compress", "Compress or expand files", DESC_GAP).is_some());
    }

    #[test]
    fn name_matches_come_before_description_matches() {
        let items = [
            item("pack", "tar-like tool for bundles", Tag::Installed),
            item("tar", "Pack files into an archive", Tag::Installed),
            item("star", "something else entirely", Tag::Installed),
        ];
        assert_eq!(names(&items, "tar"), ["tar", "star", "pack"]);
        assert_eq!(names(&items, "tar archive"), ["tar"]);
        assert!(names(&items, "zzz").is_empty());
        assert_eq!(names(&items, ""), ["pack", "tar", "star"], "an empty query keeps the list order");
    }

    #[test]
    fn keywords_find_catalog_entries_and_ties_prefer_the_catalog() {
        let mut tarball = item("tar", "Pack files", Tag::Catalog);
        tarball.keywords = "tarball compress folder".into();
        let items = [item("gzip", "compress files", Tag::Installed), tarball, item("compress", "x", Tag::Installed)];
        let found = names(&items, "compress");
        assert_eq!(found[0], "compress", "an exact name first");
        assert!(found.contains(&"tar".to_string()), "{found:?}");
        let same = [item("b", "same words", Tag::Installed), item("a", "same words", Tag::Catalog)];
        assert_eq!(names(&same, "same"), ["a", "b"]);
    }

    #[test]
    fn real_catalog_answers_plain_requests() {
        let catalog = catalog::usable(catalog(), &["arch".to_string()], false);
        let items = build_items(&catalog, &[]);
        let found = names(&items, "compress");
        assert!(!found.is_empty());
        assert!(found.iter().take(15).any(|n| n == "tar" || n.contains("zip")), "{found:?}");
    }

    #[test]
    fn items_are_deduplicated_and_tagged() {
        let catalog = catalog();
        let tools = [
            tool("tar", "GNU tar from the man page"),
            tool("usermod", "modify a user account"),
            tool("zzz-only-installed", "a program with no catalog entry"),
        ];
        let items = build_items(&catalog, &tools);
        let count = |name: &str| items.iter().filter(|i| i.name == name).count();
        assert_eq!(count("tar"), 1, "a program with a catalog entry is shown once");
        assert!(items.iter().find(|i| i.name == "tar").is_some_and(|i| i.tag == Tag::Catalog));
        assert_eq!(count("usermod"), 0, "usermod is the program of `add user to group`");
        assert_eq!(count("add user to group"), 1);
        let only = items.iter().find(|i| i.name == "zzz-only-installed").unwrap();
        assert_eq!((only.tag, only.description.as_str()), (Tag::Installed, "a program with no catalog entry"));
        assert_eq!(items.iter().filter(|i| i.tag == Tag::Catalog).count(), catalog.len());
        let tar = items.iter().find(|i| i.name == "tar").unwrap();
        assert!(!tar.keywords.is_empty());
    }

    #[test]
    fn lines_have_a_name_a_dash_a_description_and_a_tag() {
        let i = item("tar", "Pack files", Tag::Catalog);
        assert_eq!(item_line(&i), "tar \u{2014} Pack files [catalog]");
        assert_eq!(item_line(&item("zip", "Pack", Tag::Installed)), "zip \u{2014} Pack [installed]");
        assert_eq!(name_of_line("add user to group \u{2014} Add [catalog]"), "add user to group");
        assert_eq!(name_of_line("bare"), "bare");
    }

    fn fixture_page() -> ManPage {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.1");
        manpage::parse_roff(&std::fs::read_to_string(path).unwrap())
    }

    #[test]
    fn describes_an_installed_program_from_its_man_page() {
        let catalog = catalog();
        let platforms = ["arch".to_string()];
        let locate = |p: &str| (p == "sample").then(|| PathBuf::from("/usr/bin/sample"));
        let man = |p: &str| (p == "sample").then(fixture_page);
        let summary = |p: &str| (p == "sample").then(|| "sample program for testing".to_string());
        let src = Sources { catalog: &catalog, platforms: &platforms, locate: &locate, man: &man, summary: &summary };
        let text = describe("sample", &src);
        assert!(text.starts_with("sample  [installed]\nsample program for testing\n"), "{text}");
        assert!(text.contains("Status: installed (/usr/bin/sample)"), "{text}");
        assert!(text.contains("Common flags:"), "{text}");
        assert!(text.contains("-x, --extract, --get") && text.contains("Extract files from an archive."), "{text}");
        assert!(text.contains("-f, --file ARCHIVE"), "{text}");
        assert!(!text.contains("Command:") && !text.contains("WARNING"), "{text}");
        assert!(text.ends_with("More: man sample\n"), "{text}");
        assert!(text.lines().count() <= MAX_LINES);
        let flag_lines = text.lines().filter(|l| l.starts_with("  -")).count();
        assert!((1..=MAX_FLAGS).contains(&flag_lines), "{flag_lines}");
    }

    #[test]
    fn describes_a_catalog_entry_with_its_warning_and_risk() {
        let catalog = catalog();
        let platforms = ["arch".to_string()];
        let none = |_: &str| None;
        let no_text = |_: &str| None::<String>;
        let man = |_: &str| None;
        let src = Sources { catalog: &catalog, platforms: &platforms, locate: &none, man: &man, summary: &no_text };
        let text = describe("add user to group", &src);
        assert!(text.starts_with("add user to group  [catalog]\n"), "{text}");
        assert!(text.contains("Status: not installed. "), "{text}");
        assert!(text.contains("Install: sudo pacman -S"), "{text}");
        assert!(text.contains("Command: sudo usermod -aG {group} {user}"), "{text}");
        assert!(text.contains("WARNING [gives away access] this gives the user every right"), "{text}");
        assert!(text.contains("-a, --append") || text.contains("-G"), "usermod flags come from the table\n{text}");
        assert!(text.ends_with("More: man usermod\n"), "{text}");
        assert!(text.lines().count() <= MAX_LINES, "{text}");
        // The program name finds the same entry.
        assert!(describe("usermod", &src).starts_with("add user to group  [catalog]"));
        let sh = |p: &str| (p == "usermod").then(|| PathBuf::from("/usr/bin/usermod"));
        let src2 = Sources { locate: &sh, ..src };
        assert!(describe("add user to group", &src2).contains("Status: installed (/usr/bin/usermod)"));
    }

    #[test]
    fn describes_the_unknown_and_the_built_in() {
        let catalog = catalog();
        let none = |_: &str| None;
        let no_text = |_: &str| None::<String>;
        let man = |_: &str| None;
        let src = Sources { catalog: &catalog, platforms: &[], locate: &none, man: &man, summary: &no_text };
        assert_eq!(
            describe("no-such-thing-xyz", &src),
            "No catalog entry or installed program called `no-such-thing-xyz`.\n"
        );
        assert!(describe("cd", &src).contains("built into the shell"));
        assert!(describe("tar", &src).contains("no install hint for this system"));
    }

    fn pick_items() -> Vec<Item> {
        vec![
            item("tar", "Pack files into an archive", Tag::Catalog),
            item("gzip", "Compress files", Tag::Installed),
            item("zip", "Package and compress files", Tag::Installed),
        ]
    }

    fn details(i: &Item) -> String {
        format!("details of {}", i.name)
    }

    #[test]
    fn the_finder_searches_then_picks_then_searches_again() {
        let items = pick_items();
        let s = step(&Mode::Search, "compress", &items, &details);
        assert!(!s.done);
        let Mode::Pick(shown) = &s.mode else { panic!("{s:?}") };
        assert_eq!(shown.len(), 2);
        assert!(s.out.starts_with(" 1. ") || s.out.starts_with("1. "), "{}", s.out);
        assert!(s.out.contains("gzip \u{2014} Compress files [installed]"), "{}", s.out);
        // A number shows the details and keeps the list.
        let d = step(&s.mode, "2", &items, &details);
        assert!(d.out.contains("details of") && !d.done);
        assert_eq!(d.mode, s.mode);
        let first = step(&s.mode, "1", &items, &details);
        assert!(first.out.contains(&format!("details of {}", items[shown[0]].name)));
        // Out of range and zero.
        for bad in ["0", "3", "99"] {
            let b = step(&s.mode, bad, &items, &details);
            assert!(b.out.contains("pick a number from 1 to 2") && b.mode == s.mode, "{bad}: {b:?}");
        }
        // Empty line goes back to searching.
        let again = step(&s.mode, "", &items, &details);
        assert_eq!((again.mode, again.out.as_str(), again.done), (Mode::Search, "", false));
        // Words instead of a number start a new search.
        let new = step(&s.mode, "archive", &items, &details);
        assert!(matches!(&new.mode, Mode::Pick(v) if v.len() == 1) && new.out.contains("tar"), "{new:?}");
    }

    #[test]
    fn the_finder_handles_empty_input_no_match_and_quit() {
        let items = pick_items();
        let empty = step(&Mode::Search, "  ", &items, &details);
        assert_eq!(empty.mode, Mode::Search);
        assert!(empty.out.contains("type some words") && !empty.done);
        let none = step(&Mode::Search, "qqqq", &items, &details);
        assert_eq!(none.mode, Mode::Search);
        assert!(none.out.contains("nothing matches"));
        for mode in [Mode::Search, Mode::Pick(vec![0])] {
            let q = step(&mode, "q\n", &items, &details);
            assert!(q.done && q.out.is_empty(), "{mode:?}");
        }
        assert!(prompt(&Mode::Search).starts_with("find"));
        assert!(prompt(&Mode::Pick(vec![])).contains("q: quit"));
    }

    #[test]
    fn the_finder_shows_at_most_fifteen_results() {
        let many: Vec<Item> = (0..40).map(|n| item(&format!("tool{n}"), "a tool", Tag::Installed)).collect();
        let s = step(&Mode::Search, "tool", &many, &details);
        assert_eq!(s.out.lines().count(), TOP);
        assert!(matches!(s.mode, Mode::Pick(v) if v.len() == TOP));
        assert!(s.out.lines().last().unwrap().trim_start().starts_with("15."));
    }
}
