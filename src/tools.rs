//! `huh tools`: every program on `$PATH` with a one-line description. Only reads directories and
//! man page files; no program is ever run (no `--help`, no `whatis`, no `man`).

use crate::catalog::Command;
use crate::knowledge::Knowledge;
use crate::manpage;
use crate::platform;
use crate::style::{Role, paint};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Enough of a page for the NAME section; long comment headers fit too.
const HEAD_BYTES: u64 = 16 * 1024;
pub const NO_DESCRIPTION: &str = "(no description found)";

/// Executable files in these directories, by name; the first directory holding a name wins.
pub fn scan_path(dirs: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let mut seen: HashMap<String, PathBuf> = HashMap::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else { continue };
            if seen.contains_key(&name) {
                continue;
            }
            let path = entry.path();
            if platform::is_executable_file(&path) {
                seen.insert(name, path);
            }
        }
    }
    let mut names: Vec<(String, PathBuf)> = seen.into_iter().collect();
    names.sort();
    names
}

/// Man page files by program name, from `man1`, `man8` and `man6` of each man folder
/// (first found wins). `.xz` and `.zst` pages are skipped: no decoder for them is built in.
pub fn man_index(dirs: &[PathBuf]) -> HashMap<String, PathBuf> {
    let mut index = HashMap::new();
    for dir in dirs {
        for section in ["1", "8", "6"] {
            let Ok(entries) = std::fs::read_dir(dir.join(format!("man{section}"))) else { continue };
            for entry in entries.flatten() {
                let Ok(file) = entry.file_name().into_string() else { continue };
                let stem = file.strip_suffix(".gz").unwrap_or(&file);
                let Some((name, ext)) = stem.rsplit_once('.') else { continue };
                if ext != section || name.is_empty() {
                    continue;
                }
                index.entry(name.to_string()).or_insert_with(|| entry.path());
            }
        }
    }
    index
}

/// The text after `\-` in the NAME section of a page's roff source.
pub fn name_summary(src: &str) -> Option<String> {
    let summary = manpage::parse_roff(src).summary?;
    let mut summary = summary.trim();
    // A page written with a plain "-" keeps its list of names ("zstd, zstdmt - Compress files").
    if let Some((names, rest)) = summary.split_once(" - ")
        && names.split(", ").all(|n| !n.is_empty() && !n.contains(' '))
    {
        summary = rest.trim();
    }
    (!summary.is_empty()).then(|| summary.to_string())
}

/// One-line description from a man page file, following one `.so` redirect.
pub fn man_description(path: &Path, index: &HashMap<String, PathBuf>) -> Option<String> {
    let head = manpage::read_head(path, HEAD_BYTES)?;
    if let Some(target) = manpage::so_target(&head)
        && let Some(other) = index.get(&target)
    {
        return name_summary(&manpage::read_head(other, HEAD_BYTES)?);
    }
    name_summary(&head)
}

pub struct Tool {
    pub name: String,
    pub description: String,
}

/// Longest description shown; some NAME lines list a whole family of programs.
const MAX_DESC_CHARS: usize = 100;

fn shorten(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= MAX_DESC_CHARS {
        return text;
    }
    let cut: String = text.chars().take(MAX_DESC_CHARS - 3).collect();
    format!("{}...", cut.trim_end())
}

/// Describes each program (name and where it was found): catalog entry first, then the curated
/// table, then the man page. A program with no description of its own that is a link to another
/// program (`zstdmt` to `zstd`) is described like the target.
pub fn describe(found: &[(String, PathBuf)], catalog: &[Command], man_dirs: &[PathBuf], with_desc: bool) -> Vec<Tool> {
    let mut from_catalog: HashMap<&str, &str> = HashMap::new();
    for command in catalog {
        let program = command.program();
        if !program.is_empty() && !command.description.is_empty() {
            from_catalog.entry(program).or_insert(command.description.as_str());
        }
    }
    let index = if with_desc { man_index(man_dirs) } else { HashMap::new() };
    let knowledge = Knowledge::get();
    let known = |name: &str| -> Option<String> {
        if let Some(d) = from_catalog.get(name) {
            return Some((*d).to_string());
        }
        if let Some(section) = knowledge.section(&[name.to_string()])
            && !section.desc.is_empty()
        {
            return Some(section.desc.clone());
        }
        index.get(name).and_then(|path| man_description(path, &index))
    };
    found
        .iter()
        .map(|(name, path)| {
            let description = if !with_desc {
                String::new()
            } else {
                known(name)
                    .or_else(|| {
                        let target = std::fs::read_link(path).ok()?;
                        let target = target.file_name()?.to_str()?;
                        if target == name { None } else { known(target) }
                    })
                    .map_or_else(|| NO_DESCRIPTION.to_string(), |d| shorten(&d))
            };
            Tool { name: name.clone(), description }
        })
        .collect()
}

/// Keeps the tools whose name or description contains every word (case-insensitive).
pub fn filter<'a>(tools: &'a [Tool], words: &[String]) -> Vec<&'a Tool> {
    let words: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    tools
        .iter()
        .filter(|t| {
            let name = t.name.to_lowercase();
            let description = t.description.to_lowercase();
            words.iter().all(|w| name.contains(w.as_str()) || description.contains(w.as_str()))
        })
        .collect()
}

/// The aligned lines, then the count line.
pub fn render(shown: &[&Tool], total: usize, filtered: bool) -> String {
    let width = shown.iter().map(|t| t.name.chars().count()).max().unwrap_or(0).min(32);
    let mut out = String::new();
    for tool in shown {
        if tool.description.is_empty() {
            out.push_str(&paint(Role::Part, &tool.name));
        } else {
            out.push_str(&paint(Role::Part, &format!("{:<width$}", tool.name)));
            out.push_str("  ");
            out.push_str(&paint(Role::Description, &tool.description));
        }
        out.push('\n');
    }
    if filtered {
        out.push_str(&paint(Role::Dim, &format!("{} of {total} programs match", shown.len())));
        out.push('\n');
    } else {
        out.push_str(&paint(Role::Dim, &format!("{total} programs")));
        out.push('\n');
    }
    out
}

/// Every program on `$PATH` with its description (what `huh tools` and `huh find` list).
pub fn load_all(catalog: &[Command]) -> Vec<Tool> {
    describe(&scan_path(&platform::path_dirs()), catalog, &manpage::man_dirs(), true)
}

/// `huh tools [word...]`. With `no_desc` only names are listed and matched.
pub fn run(words: &[String], catalog: &[Command], no_desc: bool) -> String {
    let found = scan_path(&platform::path_dirs());
    let tools = describe(&found, catalog, &manpage::man_dirs(), !no_desc);
    let shown = filter(&tools, words);
    render(&shown, tools.len(), !words.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(".scratch").join("tools-tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()
    }

    fn file(path: &Path, mode: u32) {
        std::fs::write(path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn man_name_line_of_man_and_mdoc_pages() {
        assert_eq!(name_summary(&fixture("sample.1")).as_deref(), Some("sample program for testing"));
        assert_eq!(name_summary(&fixture("sample-mdoc.1")).as_deref(), Some("remote login client"));
        assert_eq!(name_summary(".TH X 1\n.SH SYNOPSIS\nx\n"), None);
        assert_eq!(
            name_summary(".TH X 1\n.SH NAME\nzstd, zstdmt - Compress files\n").as_deref(),
            Some("Compress files")
        );
    }

    #[test]
    fn name_survives_a_truncated_page() {
        let mut text = fixture("sample.1");
        text.truncate(text.find(".SH OPTIONS").unwrap() + 20);
        assert_eq!(name_summary(&text).as_deref(), Some("sample program for testing"));
    }

    #[test]
    fn scans_path_first_wins_and_skips_non_executables() {
        let (a, b) = (scratch("path-a"), scratch("path-b"));
        file(&a.join("alpha"), 0o755);
        file(&a.join("notes.txt"), 0o644);
        file(&b.join("alpha"), 0o755);
        file(&b.join("beta"), 0o755);
        // Not executable in the first folder, so the later one provides it.
        file(&a.join("gamma"), 0o644);
        file(&b.join("gamma"), 0o755);
        std::fs::create_dir_all(a.join("somedir")).unwrap();
        let missing = a.join("no-such-dir");
        let names = scan_path(&[a, missing, b]);
        let names: Vec<&str> = names.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["alpha", "beta", "gamma"]);
    }

    #[test]
    fn describes_from_plain_and_gzip_man_pages() {
        use flate2::{Compression, write::GzEncoder};
        use std::io::Write;
        let root = scratch("man");
        let man1 = root.join("man1");
        std::fs::create_dir_all(&man1).unwrap();
        std::fs::write(man1.join("plainprog.1"), fixture("sample.1")).unwrap();
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(fixture("sample-mdoc.1").as_bytes()).unwrap();
        std::fs::write(man1.join("zipprog.1.gz"), gz.finish().unwrap()).unwrap();
        std::fs::write(man1.join("aliasprog.1"), ".so man1/plainprog.1\n").unwrap();
        std::fs::write(man1.join("packed.1.zst"), "not readable").unwrap();
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink("plainprog", bin.join("linkprog")).unwrap();
        let names: Vec<(String, PathBuf)> = ["plainprog", "zipprog", "aliasprog", "packed", "nopage", "linkprog"]
            .map(|n| (n.to_string(), bin.join(n)))
            .to_vec();
        let tools = describe(&names, &[], &[root], true);
        let got: Vec<&str> = tools.iter().map(|t| t.description.as_str()).collect();
        assert_eq!(
            got,
            [
                "sample program for testing",
                "remote login client",
                "sample program for testing",
                NO_DESCRIPTION,
                NO_DESCRIPTION,
                "sample program for testing"
            ]
        );
    }

    #[test]
    fn catalog_and_curated_table_come_before_man_pages() {
        let root = scratch("man-empty");
        let found = [("ssh".to_string(), root.join("ssh")), ("grep".to_string(), root.join("grep"))];
        let tools = describe(&found, &[], &[root], true);
        assert!(!tools[0].description.is_empty() && tools[0].description != NO_DESCRIPTION, "{}", tools[0].description);
        assert!(tools[1].description != NO_DESCRIPTION, "{}", tools[1].description);
    }

    #[test]
    fn filters_by_every_word_in_name_or_description() {
        let tools = [
            Tool { name: "gzip".into(), description: "Compress or expand files".into() },
            Tool { name: "ping".into(), description: "send ICMP echo to a network host".into() },
            Tool { name: "netstat".into(), description: "Network statistics".into() },
        ];
        let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let names = |w: &[&str]| filter(&tools, &words(w)).iter().map(|t| t.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&[]), ["gzip", "ping", "netstat"]);
        assert_eq!(names(&["COMPRESS"]), ["gzip"]);
        assert_eq!(names(&["network"]), ["ping", "netstat"]);
        assert_eq!(names(&["network", "host"]), ["ping"]);
        assert_eq!(names(&["netstat", "statistics"]), ["netstat"]);
        assert!(names(&["zzz"]).is_empty());
    }

    #[test]
    fn output_is_aligned_with_a_count_line() {
        let tools = [
            Tool { name: "a".into(), description: "first".into() },
            Tool { name: "longer".into(), description: "second".into() },
        ];
        let shown: Vec<&Tool> = tools.iter().collect();
        assert_eq!(render(&shown, 10, true), "a       first\nlonger  second\n2 of 10 programs match\n");
        assert_eq!(render(&shown, 2, false), "a       first\nlonger  second\n2 programs\n");
    }
}
