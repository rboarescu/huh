//! Reads option descriptions out of man pages on disk (roff source, plain or gzip).
//!
//! Only reads files; it never runs `man`, `--help` or anything else. Understands the common
//! layouts: `.TP` tagged paragraphs, `.IP "tag"`, `.PP` + bold option line (git's style),
//! and mdoc `.It Fl x Ar arg` (BSD and OpenSSH style).

use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_PAGE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_DESC_CHARS: usize = 190;

#[derive(Debug, Clone, PartialEq)]
pub struct OptEntry {
    /// Every spelling, like `-i` and `--identity`.
    pub names: Vec<String>,
    /// The argument it takes (as the page names it), if any.
    pub arg: Option<String>,
    pub desc: String,
}

#[derive(Debug, Clone, Default)]
pub struct ManPage {
    /// The one-line summary from the NAME section.
    pub summary: Option<String>,
    pub options: Vec<OptEntry>,
}

impl ManPage {
    /// Finds the entry for `flag` (like `-i` or `--since`).
    pub fn lookup(&self, flag: &str) -> Option<&OptEntry> {
        self.options.iter().find(|o| o.names.iter().any(|n| n == flag))
    }
}

// ---------------------------------------------------------------------------
// Finding and reading pages
// ---------------------------------------------------------------------------

fn man_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(path) = std::env::var("MANPATH") {
        dirs.extend(path.split(':').filter(|p| !p.is_empty()).map(PathBuf::from));
    }
    for d in ["/usr/share/man", "/usr/local/share/man", "/usr/local/man", "/opt/homebrew/share/man"] {
        let d = PathBuf::from(d);
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    dirs
}

/// The man page file for `name` (like `git-commit`), looking in sections 1 and 8.
pub fn find_page(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains('/') || name.starts_with('.') {
        return None;
    }
    for dir in man_dirs() {
        for section in ["1", "8", "6"] {
            for suffix in ["", ".gz"] {
                let candidate = dir.join(format!("man{section}")).join(format!("{name}.{section}{suffix}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

fn read_page_source(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    if path.extension().is_some_and(|e| e == "gz") {
        flate2::read::GzDecoder::new(file).take(MAX_PAGE_BYTES).read_to_end(&mut bytes).ok()?;
    } else {
        file.take(MAX_PAGE_BYTES).read_to_end(&mut bytes).ok()?;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Loads and parses the man page called `name`, following `.so` redirects.
pub fn load(name: &str) -> Option<ManPage> {
    let path = find_page(name)?;
    let mut src = read_page_source(&path)?;
    for _ in 0..3 {
        let redirect = src
            .lines()
            .find(|l| !l.trim().is_empty() && !l.starts_with(".\\\""))
            .and_then(|l| l.strip_prefix(".so "))
            .map(|t| t.trim().to_string());
        let Some(target) = redirect else { break };
        let stem = Path::new(&target).file_name()?.to_str()?;
        let stem = stem.rsplit_once('.').map_or(stem, |(a, _)| a);
        let again = find_page(stem)?;
        src = read_page_source(&again)?;
    }
    Some(parse_roff(&src))
}

// ---------------------------------------------------------------------------
// roff text clean-up
// ---------------------------------------------------------------------------

/// Turns roff text (font escapes, special characters) into plain text.
pub fn plain(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c != '\\' {
            out.push(c);
            i += 1;
            continue;
        }
        i += 1;
        let Some(&n) = chars.get(i) else { break };
        i += 1;
        match n {
            'f' | '*' | 'n' => {
                // `\fB`, `\f(CW`, `\f[CR]`, `\*(xx`, `\*[xx]`, `\*x`
                match chars.get(i) {
                    Some('(') => i += 3,
                    Some('[') => {
                        while i < chars.len() && chars[i] != ']' {
                            i += 1;
                        }
                        i += 1;
                    }
                    Some(_) => i += 1,
                    None => {}
                }
            }
            '(' => {
                let name: String = chars.iter().skip(i).take(2).collect();
                i += 2;
                out.push_str(special(&name));
            }
            '[' => {
                let mut name = String::new();
                while i < chars.len() && chars[i] != ']' {
                    name.push(chars[i]);
                    i += 1;
                }
                i += 1;
                out.push_str(special(&name));
            }
            // `\X'tty: link ...'`, `\w'..'`, `\h'..'`: escapes with a quoted argument that print nothing.
            'X' | 'w' | 'h' | 'v' | 'l' | 'L' | 'b' | 'o' | 'z' | 'D' if chars.get(i) == Some(&'\'') => {
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    i += 1;
                }
                i += 1;
            }
            's' => {
                // size change like `\s-1` or `\s10`
                if matches!(chars.get(i), Some('+' | '-')) {
                    i += 1;
                }
                while chars.get(i).is_some_and(char::is_ascii_digit) {
                    i += 1;
                }
            }
            '-' => out.push('-'),
            'e' | '\\' => out.push('\\'),
            ' ' | '~' | '0' => out.push(' '),
            '&' | '|' | '^' | '/' | ',' | '%' | 'c' | ':' => {}
            '\'' => out.push('\''),
            '`' => out.push('`'),
            '"' => break,
            other => out.push(other),
        }
    }
    out
}

fn special(name: &str) -> &'static str {
    match name {
        "bu" => "*",
        "em" | "en" | "hy" | "mi" => "-",
        "aq" | "oq" | "cq" | "'" => "'",
        "lq" | "rq" | "Lq" | "Rq" | "dq" => "\"",
        "co" => "(c)",
        "ti" => "~",
        "ga" => "`",
        "rs" => "\\",
        _ => "",
    }
}

fn split_macro_args(rest: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut had = false;
    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if quoted && chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    quoted = !quoted;
                    had = true;
                }
            }
            '\\' if chars.peek().is_some() && !quoted => {
                cur.push('\\');
                cur.push(chars.next().unwrap_or(' '));
            }
            ' ' | '\t' if !quoted => {
                if had || !cur.is_empty() {
                    args.push(std::mem::take(&mut cur));
                    had = false;
                }
            }
            _ => cur.push(c),
        }
    }
    if had || !cur.is_empty() {
        args.push(cur);
    }
    args
}

/// Text carried by a man(7) font macro line (`.B`, `.BR`, `.I`, ...), or None for other macros.
fn font_macro_text(name: &str, rest: &str) -> Option<String> {
    let args = split_macro_args(rest);
    match name {
        "B" | "I" | "SM" | "SB" => Some(plain(&args.join(" "))),
        "BR" | "RB" | "BI" | "IB" | "IR" | "RI" => Some(plain(&args.concat())),
        _ => None,
    }
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Splits `.XX args` into (`XX`, `args`). Returns None for text lines.
fn macro_line(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix('.').or_else(|| line.strip_prefix('\''))?;
    let rest = rest.trim_start();
    let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
    Some((&rest[..end], rest[end..].trim_start()))
}

// ---------------------------------------------------------------------------
// Option headers and descriptions
// ---------------------------------------------------------------------------

/// Reads names and the argument out of an option header like `-C, --directory=DIR`.
fn parse_header(header: &str) -> Option<(Vec<String>, Option<String>)> {
    let header = collapse(header);
    let header = header.trim().trim_end_matches(':').trim();
    if !header.starts_with('-') {
        return None;
    }
    let mut names = Vec::new();
    let mut arg: Option<String> = None;
    let normalised = header.replace(" / ", ", ").replace(" | ", ", ").replace(" or ", ", ");
    for piece in normalised.split(", ") {
        let piece = piece.trim();
        if !piece.starts_with('-') {
            continue;
        }
        let end = piece.find(|c: char| c.is_whitespace() || c == '=' || c == '[').unwrap_or(piece.len());
        let name = &piece[..end];
        let valid = name.trim_start_matches('-').chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
            && !name.ends_with('-') | (name == "--");
        if !valid || name == "-" || name == "--" {
            continue;
        }
        let rest = piece[end..].trim().trim_start_matches('=').trim();
        if arg.is_none() && !rest.is_empty() && rest != "..." {
            arg = Some(rest.trim_matches(['[', ']', '=']).to_string());
        }
        if !names.contains(&name.to_string()) {
            names.push(name.to_string());
        }
    }
    if names.is_empty() { None } else { Some((names, arg)) }
}

/// A short description: the first sentence or two of the body text.
fn summarize(body: &str) -> String {
    let text = collapse(body);
    let mut end = text.len();
    let mut sentences = 0;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'.'
            && bytes[i + 1] == b' '
            && i > 0
            && !text[..i].ends_with("e.g")
            && !text[..i].ends_with("i.e")
        {
            sentences += 1;
            if sentences == 1 && i < 45 {
                i += 1;
                continue;
            }
            end = i + 1;
            break;
        }
        i += 1;
    }
    let mut out = text[..end].trim().to_string();
    if out.chars().count() > MAX_DESC_CHARS {
        out = out.chars().take(MAX_DESC_CHARS).collect();
        if let Some(cut) = out.rfind(' ') {
            out.truncate(cut);
        }
        out.push_str("...");
    }
    out
}

#[derive(Default)]
struct Builder {
    header: Option<String>,
    body: String,
    options: Vec<OptEntry>,
}

impl Builder {
    fn finish(&mut self) {
        if let Some(h) = self.header.take()
            && let Some((names, arg)) = parse_header(&h)
        {
            let desc = summarize(&self.body);
            if !desc.is_empty() {
                self.options.push(OptEntry { names, arg, desc });
            }
        }
        self.body.clear();
    }

    fn start(&mut self, header: String) {
        self.finish();
        self.header = Some(header);
    }

    fn add_text(&mut self, text: &str) {
        if self.header.is_some() && self.body.len() < 1200 {
            self.body.push_str(text);
            self.body.push(' ');
        }
    }
}

/// Parses roff source (man(7) or mdoc) for the NAME summary and the option entries.
pub fn parse_roff(src: &str) -> ManPage {
    let mdoc = src.lines().any(|l| l.starts_with(".Dd") || l.starts_with(".Sh "));
    if mdoc { parse_mdoc(src) } else { parse_man(src) }
}

fn parse_man(src: &str) -> ManPage {
    let mut page = ManPage::default();
    let mut b = Builder::default();
    let mut pending_tp = false;
    let mut pp_candidate = false;
    let mut in_nf = false;
    let mut in_name = false;
    let mut name_text = String::new();
    for line in src.lines() {
        if line.starts_with(".\\\"") || line.starts_with("'\\\"") || line.starts_with("\\\"") {
            continue;
        }
        if let Some((m, rest)) = macro_line(line) {
            match m {
                "nf" => in_nf = true,
                "fi" => in_nf = false,
                "SH" | "SS" => {
                    b.finish();
                    pending_tp = false;
                    pp_candidate = false;
                    in_name = m == "SH" && plain(&split_macro_args(rest).join(" ")).trim().eq_ignore_ascii_case("name");
                }
                "TP" | "TQ" | "HP" => {
                    if m != "TQ" {
                        b.finish();
                    }
                    pending_tp = true;
                    pp_candidate = false;
                }
                "IP" => {
                    let args = split_macro_args(rest);
                    match args.first().map(|t| plain(t)) {
                        Some(tag) if tag.trim().starts_with('-') => {
                            b.start(tag);
                            pending_tp = false;
                        }
                        Some(tag)
                            if !tag.trim().is_empty()
                                && !matches!(tag.trim(), "*" | "o")
                                && !tag.trim().chars().all(|c| c.is_ascii_digit() || c == '.') =>
                        {
                            b.finish();
                        }
                        _ => {}
                    }
                    pp_candidate = false;
                }
                "PP" | "P" | "LP" => {
                    if b.header.is_some() && !b.body.trim().is_empty() {
                        b.finish();
                    }
                    pending_tp = false;
                    pp_candidate = true;
                }
                "RS" | "RE" | "sp" | "br" | "ad" | "na" | "ne" | "in" | "ti" | "nh" | "hy" | "fc" | "ft" | "ta"
                | "bp" | "ll" | "ps" | "UR" | "UE" | "MT" | "ME" | "EX" | "EE" | "." => {}
                other => {
                    if let Some(text) = font_macro_text(other, rest) {
                        if pending_tp {
                            b.start(text);
                            pending_tp = false;
                        } else if pp_candidate && text.trim_start().starts_with('-') {
                            b.start(text);
                            pp_candidate = false;
                        } else {
                            b.add_text(&text);
                            if in_name {
                                name_text.push_str(&text);
                                name_text.push(' ');
                            }
                        }
                    }
                }
            }
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        let text = plain(line);
        if in_name {
            name_text.push_str(&text);
            name_text.push(' ');
        }
        if pending_tp {
            b.start(text);
            pending_tp = false;
        } else if pp_candidate && !in_nf && text.trim_start().starts_with('-') && text.len() < 160 {
            b.start(text);
            pp_candidate = false;
        } else {
            pp_candidate = false;
            b.add_text(&text);
        }
    }
    b.finish();
    page.options = b.options;
    page.summary = name_summary(&name_text);
    page
}

fn name_summary(name_text: &str) -> Option<String> {
    let t = collapse(name_text);
    let (_, desc) = t.split_once(" - ")?;
    let desc = desc.trim();
    if desc.is_empty() { None } else { Some(desc.to_string()) }
}

const MDOC_WORDS: [&str; 24] = [
    "Ar", "Cm", "Pa", "Em", "Sy", "Va", "Ev", "Ic", "Li", "Op", "Oo", "Oc", "Ql", "Dq", "Sq", "Pq", "Ns", "No", "Xr",
    "Nm", "Fa", "Fn", "Xo", "Xc",
];

/// Text of an mdoc macro line: `Fl i Ar file` becomes `-i file`.
fn mdoc_text(rest: &str, with_macro: Option<&str>) -> String {
    let mut tokens = split_macro_args(rest);
    if let Some(m) = with_macro {
        tokens.insert(0, m.to_string());
    }
    let mut out = String::new();
    let mut i = 0;
    while i < tokens.len() {
        let t = &tokens[i];
        match t.as_str() {
            "Fl" => {
                // `Fl` alone is a lone dash; `Fl x` is `-x`; `Fl -x` is `--x`.
                let next = tokens
                    .get(i + 1)
                    .filter(|n| !MDOC_WORDS.contains(&n.as_str()) && *n != "Fl" && !matches!(n.as_str(), "," | "|"));
                out.push_str(" -");
                if let Some(n) = next {
                    out.push_str(&plain(n));
                    i += 1;
                }
            }
            "Sm" | "Xo" | "Xc" | "Ns" | "Cm" | "Ic" | "Em" | "Sy" | "Pa" | "Va" | "Ev" | "Li" | "Oo" | "Oc" | "Op"
            | "No" | "Nm" | "Ql" | "Dq" | "Sq" | "Pq" | "Fa" | "Fn" | "Ar" => {
                if t == "Ar" || t == "Op" && false {
                    out.push(' ');
                }
            }
            "Xr" => {}
            "," | "|" => out.push_str(&format!("{t} ")),
            other => {
                out.push(' ');
                out.push_str(&plain(other));
            }
        }
        i += 1;
    }
    out.trim().to_string()
}

fn parse_mdoc(src: &str) -> ManPage {
    let mut page = ManPage::default();
    let mut b = Builder::default();
    let mut in_name = false;
    let mut name_desc = String::new();
    let mut header_lines: Option<String> = None;
    for line in src.lines() {
        if line.starts_with(".\\\"") || line.trim().is_empty() {
            continue;
        }
        if let Some((m, rest)) = macro_line(line) {
            // A multi-line `.It Fl x Xo ... .Xc` header: gather lines until `.Xc`.
            if let Some(h) = header_lines.as_mut() {
                if m == "Xc" {
                    let h = header_lines.take().unwrap_or_default();
                    b.start(h);
                } else {
                    h.push(' ');
                    h.push_str(&mdoc_text(rest, Some(m)));
                }
                continue;
            }
            match m {
                "Sh" => {
                    b.finish();
                    in_name = split_macro_args(rest).join(" ").eq_ignore_ascii_case("name");
                }
                "Nd" if in_name => name_desc = plain(rest),
                "It" => {
                    if rest.ends_with(" Xo") || rest == "Xo" {
                        header_lines = Some(mdoc_text(rest.trim_end_matches("Xo"), None));
                    } else {
                        b.start(mdoc_text(rest, None));
                    }
                }
                "El" | "Bl" | "Pp" => {
                    if m != "Pp" || !b.body.trim().is_empty() {
                        b.finish();
                    }
                }
                "Bd" | "Ed" | "Rs" | "Re" | "Sm" | "Dd" | "Dt" | "Os" | "Ss" | "Nm" | "Bk" | "Ek" | "An" | "At"
                | "Ex" => {}
                other => {
                    let text = mdoc_text(rest, Some(other));
                    b.add_text(&text);
                }
            }
            continue;
        }
        b.add_text(&plain(line));
    }
    b.finish();
    page.options = b.options;
    if !name_desc.trim().is_empty() {
        page.summary = Some(name_desc.trim().to_string());
    }
    page
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/sample.1");
    const MDOC_FIXTURE: &str = include_str!("../tests/fixtures/sample-mdoc.1");

    #[test]
    fn plain_handles_fonts_and_specials() {
        assert_eq!(plain(r"\fB\-\-since\fR=\fI<date>\fP"), "--since=<date>");
        assert_eq!(plain(r"don\(aqt \(lqquote\(rq\&"), "don't \"quote\"");
        assert_eq!(plain(r"a\ b \e c"), "a b \\ c");
    }

    #[test]
    fn parses_header_forms() {
        assert_eq!(parse_header("-i identity_file"), Some((vec!["-i".into()], Some("identity_file".into()))));
        assert_eq!(
            parse_header("-C, --directory=DIR"),
            Some((vec!["-C".into(), "--directory".into()], Some("DIR".into())))
        );
        assert_eq!(parse_header("--slurp / -s:"), Some((vec!["--slurp".into(), "-s".into()], None)));
        assert_eq!(
            parse_header("--color[=WHEN]"),
            Some((vec!["--color".into()], Some("=WHEN".trim_start_matches('=').into())))
        );
        assert_eq!(parse_header("not an option"), None);
    }

    #[test]
    fn tp_style_entries() {
        let page = parse_roff(FIXTURE);
        let x = page.lookup("--extract").expect("extract");
        assert!(x.names.contains(&"-x".to_string()) && x.names.contains(&"--get".to_string()));
        assert!(x.desc.starts_with("Extract files from an archive."), "{}", x.desc);
        let f = page.lookup("-f").expect("file");
        assert_eq!(f.arg.as_deref(), Some("ARCHIVE"));
    }

    #[test]
    fn ip_style_and_bold_macro_headers() {
        let page = parse_roff(FIXTURE);
        let m = page.lookup("-mtime").expect("mtime");
        assert_eq!(m.arg.as_deref(), Some("n"));
        assert!(m.desc.contains("last modified"), "{}", m.desc);
        let v = page.lookup("-v").expect("-v from .B header");
        assert!(v.desc.contains("verbose"), "{}", v.desc);
    }

    #[test]
    fn pp_style_entries_like_git() {
        let page = parse_roff(FIXTURE);
        let s = page.lookup("--since").expect("since");
        assert_eq!(s.arg.as_deref(), Some("<date>"));
        assert!(s.desc.starts_with("Show commits more recent than a specific date."), "{}", s.desc);
    }

    #[test]
    fn name_summary_is_read() {
        assert_eq!(parse_roff(FIXTURE).summary.as_deref(), Some("sample program for testing"));
    }

    #[test]
    fn mdoc_entries() {
        let page = parse_roff(MDOC_FIXTURE);
        assert_eq!(page.summary.as_deref(), Some("remote login client"));
        let i = page.lookup("-i").expect("-i");
        assert_eq!(i.arg.as_deref(), Some("identity_file"));
        assert!(i.desc.contains("identity (private key)"), "{}", i.desc);
        let p = page.lookup("-p").expect("-p");
        assert!(p.desc.starts_with("Port to connect to"), "{}", p.desc);
    }

    #[test]
    fn long_descriptions_are_trimmed() {
        let long = "word ".repeat(100);
        assert!(summarize(&long).chars().count() <= MAX_DESC_CHARS + 3);
    }

    #[test]
    #[ignore]
    fn dump_real_pages() {
        for name in ["tar", "git-commit", "find", "rsync", "jq", "ssh", "grep", "sed", "curl", "gawk"] {
            match load(name) {
                Some(p) => {
                    println!("== {name}: {} options; {:?}", p.options.len(), p.summary);
                    for o in p.options.iter().take(4) {
                        println!("   {:?} {:?} => {}", o.names, o.arg, o.desc);
                    }
                }
                None => println!("== {name}: no page"),
            }
        }
    }

    #[test]
    fn real_pages_if_present() {
        // These depend on the machine, so they only check what is there.
        for (prog, flag) in [
            ("tar", "-x"),
            ("git-commit", "--amend"),
            ("find", "-mtime"),
            ("rsync", "--verbose"),
            ("jq", "--slurp"),
            ("ssh", "-i"),
        ] {
            if let Some(page) = load(prog) {
                assert!(page.lookup(flag).is_some(), "{prog} should describe {flag}");
            }
        }
    }
}
