//! The curated table of programs, subcommands, options and what they mean.
//! The data lives in `knowledge.txt` (see the format notes at its top) and is compiled in.

use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq)]
pub enum ArgKind {
    None,
    Required(String),
    /// Only attached to the option (`--color=auto`, `-i.bak`, `-j4`).
    Optional(String),
}

#[derive(Debug, Clone)]
pub struct OptSpec {
    pub names: Vec<String>,
    pub arg: ArgKind,
    pub meaning: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RoleKind {
    Plain,
    /// The rest are the words of a command (argv). The string says where it runs ("", "container").
    Argv(String),
    /// The rest is a command line given as text (joined). The string says where it runs.
    Shell(String),
}

#[derive(Debug, Clone)]
pub struct Role {
    pub text: String,
    pub kind: RoleKind,
    /// The last role repeats for any further words.
    pub repeat: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Section {
    pub path: Vec<String>,
    pub desc: String,
    pub single: bool,
    pub oldstyle: bool,
    pub first: bool,
    pub wrapper: bool,
    pub kv: bool,
    pub roles: Vec<Role>,
    pub opts: Vec<OptSpec>,
    /// Meanings for particular first words, like `restart` for systemctl.
    pub words: Vec<(String, String)>,
}

pub struct Knowledge {
    sections: Vec<Section>,
    index: HashMap<String, usize>,
}

fn parse_opt_line(line: &str) -> Option<OptSpec> {
    let (left, meaning) = line.split_once(" | ")?;
    let mut names = Vec::new();
    let mut arg = ArgKind::None;
    for piece in left.split(", ") {
        let mut tokens = piece.split_whitespace();
        let name = tokens.next()?;
        names.push(name.to_string());
        let label = tokens.collect::<Vec<_>>().join(" ");
        if !label.is_empty() && arg == ArgKind::None {
            arg = match label.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                Some(inner) => ArgKind::Optional(inner.to_string()),
                None => ArgKind::Required(label),
            };
        }
    }
    Some(OptSpec { names, arg, meaning: meaning.trim().to_string() })
}

fn parse_roles(list: &str) -> Vec<Role> {
    let parts: Vec<&str> = list.split(" ; ").map(str::trim).collect();
    let count = parts.len();
    parts
        .into_iter()
        .enumerate()
        .filter(|(_, p)| !p.is_empty())
        .map(|(i, part)| {
            let (text, repeat) = match part.strip_suffix("...") {
                Some(t) if i + 1 == count => (t.trim(), true),
                _ => (part, false),
            };
            let (kind, text) = if let Some(rest) = text.strip_prefix(">>") {
                (RoleKind::Shell(rest.trim().to_string()), "")
            } else if let Some(rest) = text.strip_prefix('>') {
                (RoleKind::Argv(rest.trim().to_string()), "")
            } else {
                (RoleKind::Plain, text)
            };
            Role { text: text.to_string(), kind, repeat }
        })
        .collect()
}

impl Knowledge {
    fn parse(data: &str) -> Knowledge {
        let mut sections: Vec<Section> = Vec::new();
        for raw in data.lines() {
            let line = raw.trim_end();
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(head) = line.strip_prefix('@') {
                let (path, desc) = head.split_once(" | ").unwrap_or((head, ""));
                sections.push(Section {
                    path: path.split_whitespace().map(str::to_string).collect(),
                    desc: desc.trim().to_string(),
                    ..Section::default()
                });
                continue;
            }
            let Some(section) = sections.last_mut() else { continue };
            match line {
                "!single" => section.single = true,
                "!oldstyle" => section.oldstyle = true,
                "!first" => section.first = true,
                "!wrapper" => section.wrapper = true,
                "!kv" => section.kv = true,
                _ => {
                    if let Some(list) = line.strip_prefix("pos:") {
                        section.roles = parse_roles(list);
                    } else if let Some(entry) = line.strip_prefix('=') {
                        if let Some((word, meaning)) = entry.split_once(" | ") {
                            section.words.push((word.trim().to_string(), meaning.trim().to_string()));
                        }
                    } else if let Some(opt) = parse_opt_line(line) {
                        section.opts.push(opt);
                    }
                }
            }
        }
        let mut index = HashMap::new();
        for (i, s) in sections.iter().enumerate() {
            index.insert(s.path.join(" "), i);
        }
        Knowledge { sections, index }
    }

    pub fn get() -> &'static Knowledge {
        static K: OnceLock<Knowledge> = OnceLock::new();
        K.get_or_init(|| Knowledge::parse(include_str!("knowledge.txt")))
    }

    pub fn section(&self, path: &[String]) -> Option<&Section> {
        self.index.get(&path.join(" ")).map(|&i| &self.sections[i])
    }

    /// Does the program have sections for subcommands (like `git commit`)?
    #[cfg(test)]
    pub fn has_subsections(&self, prog: &str) -> bool {
        self.sections.iter().any(|s| s.path.len() > 1 && s.path[0] == prog)
    }

    /// Is there a section below `path` (like `docker compose up` below `docker compose`)?
    pub fn has_children(&self, path: &[String]) -> bool {
        self.sections.iter().any(|s| s.path.len() > path.len() && s.path[..path.len()] == *path)
    }

    /// Looks `flag` up in the section for `path`, then in each parent section.
    pub fn find_opt(&self, path: &[String], flag: &str) -> Option<&OptSpec> {
        for end in (1..=path.len()).rev() {
            if let Some(section) = self.section(&path[..end])
                && let Some(opt) = section.opts.iter().find(|o| o.names.iter().any(|n| n == flag))
            {
                return Some(opt);
            }
        }
        None
    }

    /// The nearest section's flags that mention `flag` as a key=value operand.
    pub fn find_kv(&self, path: &[String], key: &str) -> Option<&OptSpec> {
        self.find_opt(path, key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_parses() {
        let k = Knowledge::get();
        let find = k.section(&["find".into()]).expect("find");
        assert!(find.single && find.first);
        let name = k.find_opt(&["find".into()], "-name").unwrap();
        assert_eq!(name.arg, ArgKind::Required("PATTERN".into()));
        let ssh = k.section(&["ssh".into()]).unwrap();
        assert_eq!(ssh.roles[1].kind, RoleKind::Shell("remote".into()));
        let commit = k.find_opt(&["git".into(), "commit".into()], "-m").unwrap();
        assert!(commit.names.contains(&"--message".to_string()));
        assert!(k.find_opt(&["git".into(), "commit".into()], "-C").is_some(), "inherits git -C");
        assert!(k.has_subsections("git") && !k.has_subsections("find"));
    }

    #[test]
    fn every_option_line_has_a_meaning() {
        for s in &Knowledge::get().sections {
            assert!(!s.path.is_empty());
            for o in &s.opts {
                assert!(!o.meaning.is_empty() && !o.names.is_empty(), "{:?}", s.path);
            }
        }
    }
}
