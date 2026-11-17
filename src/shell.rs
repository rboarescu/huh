//! A forgiving parser for a useful subset of bash. It only builds a tree out of the text:
//! nothing here runs, expands or evaluates anything, and it never panics, whatever the input.
//! Problems (unbalanced quotes, stray brackets) become notes and the parser carries on.

/// How deep `$(...)`, `( )`, `{ }`, loops and so on may nest before the parser gives up.
const MAX_DEPTH: usize = 48;

#[derive(Debug, Clone, PartialEq)]
pub enum SubKind {
    /// `$(...)`
    Command,
    /// `` `...` ``
    Backtick,
    /// `<(...)` or `>(...)`
    Process(char),
    /// `$((...))`
    Arith,
}

/// A command substitution (or similar) found inside a word.
#[derive(Debug, Clone)]
pub struct Sub {
    pub kind: SubKind,
    pub raw: String,
    pub script: Script,
    /// True when it sits inside double quotes.
    pub in_quotes: bool,
}

/// A variable mentioned in a word, like `$DIR` or `${DIR:-/tmp}`.
#[derive(Debug, Clone, PartialEq)]
pub struct VarRef {
    pub name: String,
    /// `${X:-default}`, `${X:=default}`, `${X:?msg}`: the line copes with X being unset.
    pub guarded: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Word {
    /// Exactly as written.
    pub raw: String,
    /// With quotes and escapes removed. Substitutions and variables stay as written.
    pub text: String,
    /// Any quoting was used.
    pub quoted: bool,
    /// Contains an unquoted `*`, `?` or `[..]`.
    pub glob: bool,
    pub subs: Vec<Sub>,
    pub vars: Vec<VarRef>,
    /// Character offsets of each variable reference inside `text`, parallel to `vars`.
    pub var_offsets: Vec<usize>,
}

impl Word {
    pub fn is_plain(&self) -> bool {
        self.subs.is_empty() && self.vars.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct Heredoc {
    pub delimiter: String,
    pub quoted: bool,
    pub body: String,
    pub strip_tabs: bool,
}

#[derive(Debug, Clone)]
pub struct Redir {
    /// A file descriptor number or `&` (as in `&>`), when written.
    pub fd: Option<String>,
    /// `>`, `>>`, `<`, `<<`, `<<<`, `>|`, `>&`, `<&`, `&>`, `&>>`, `<>`.
    pub op: String,
    pub target: Word,
    pub heredoc: Option<Heredoc>,
    pub raw: String,
}

#[derive(Debug, Clone, Default)]
pub struct Simple {
    pub assigns: Vec<(String, Word)>,
    pub words: Vec<Word>,
    pub redirs: Vec<Redir>,
    pub raw: String,
}

#[derive(Debug, Clone)]
pub struct Arm {
    pub patterns: Vec<String>,
    pub body: Script,
}

#[derive(Debug, Clone)]
pub enum Cmd {
    Simple(Simple),
    Subshell(Script, Vec<Redir>),
    Group(Script, Vec<Redir>),
    For {
        var: String,
        words: Option<Vec<Word>>,
        body: Script,
        raw_head: String,
        redirs: Vec<Redir>,
    },
    While {
        until: bool,
        cond: Script,
        body: Script,
        raw_head: String,
        redirs: Vec<Redir>,
    },
    If {
        branches: Vec<(Script, Script)>,
        otherwise: Option<Script>,
        redirs: Vec<Redir>,
    },
    Case {
        subject: Word,
        arms: Vec<Arm>,
        redirs: Vec<Redir>,
    },
    Func {
        name: String,
        body: Script,
    },
    /// `[[ ... ]]`, kept as text.
    Test(String),
}

#[derive(Debug, Clone, Default)]
pub struct Pipeline {
    pub negated: bool,
    pub cmds: Vec<Cmd>,
    /// For each `|` between commands: was it `|&`?
    pub pipe_err: Vec<bool>,
    pub raw: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Conn {
    And,
    Or,
}

#[derive(Debug, Clone, Default)]
pub struct AndOr {
    pub first: Pipeline,
    pub rest: Vec<(Conn, Pipeline)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sep {
    /// `;` or a newline, or the end of the input.
    Seq,
    /// `&`: run in the background.
    Background,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub chain: AndOr,
    pub sep: Sep,
}

#[derive(Debug, Clone, Default)]
pub struct Script {
    pub items: Vec<Item>,
}

pub struct Parsed {
    pub script: Script,
    pub notes: Vec<String>,
}

/// Parses `input`. Never fails and never panics; `notes` lists anything that looked wrong.
pub fn parse(input: &str) -> Parsed {
    let mut p = Parser::new(input);
    let script = p.parse_top();
    Parsed { script, notes: p.notes }
}

struct Parser {
    s: Vec<char>,
    pos: usize,
    depth: usize,
    notes: Vec<String>,
    /// Heredoc bodies already consumed: when the parser reaches the newline at `.0`, it jumps to `.1`.
    skip: Vec<(usize, usize)>,
    /// Where the next heredoc body on the current line starts searching.
    heredoc_from: Option<usize>,
    heredoc_nl: Option<usize>,
}

const RESERVED_STOPS: [&str; 8] = ["then", "else", "elif", "fi", "do", "done", "esac", "}"];

impl Parser {
    fn new(input: &str) -> Parser {
        Parser {
            s: input.chars().collect(),
            pos: 0,
            depth: 0,
            notes: Vec::new(),
            skip: Vec::new(),
            heredoc_from: None,
            heredoc_nl: None,
        }
    }

    fn peek(&self) -> Option<char> {
        self.s.get(self.pos).copied()
    }

    fn at(&self, offset: usize) -> Option<char> {
        self.s.get(self.pos + offset).copied()
    }

    fn eof(&self) -> bool {
        self.pos >= self.s.len()
    }

    fn slice(&self, from: usize, to: usize) -> String {
        let to = to.min(self.s.len());
        if from >= to { String::new() } else { self.s[from..to].iter().collect() }
    }

    fn note(&mut self, text: impl Into<String>) {
        let text = text.into();
        if !self.notes.contains(&text) {
            self.notes.push(text);
        }
    }

    fn starts_with(&self, text: &str) -> bool {
        text.chars().enumerate().all(|(i, c)| self.at(i) == Some(c))
    }

    /// Skips blanks, `\`-newline continuations and comments. Newlines only when `newlines` is set.
    fn skip_blanks(&mut self, newlines: bool) {
        loop {
            match self.peek() {
                Some(' ' | '\t' | '\r') => self.pos += 1,
                Some('\\') if self.at(1) == Some('\n') => self.pos += 2,
                Some('#') => {
                    while self.peek().is_some_and(|c| c != '\n') {
                        self.pos += 1;
                    }
                }
                Some('\n') if newlines => self.newline(),
                _ => break,
            }
        }
    }

    /// Consumes a newline, jumping over any heredoc body that starts after it.
    fn newline(&mut self) {
        let at = self.pos;
        self.pos += 1;
        if let Some(&(_, end)) = self.skip.iter().find(|(nl, _)| *nl == at) {
            self.pos = end;
        }
        self.heredoc_from = None;
        self.heredoc_nl = None;
    }

    fn parse_top(&mut self) -> Script {
        let mut script = Script::default();
        loop {
            let part = self.parse_script(&[]);
            script.items.extend(part.items);
            if self.eof() {
                break;
            }
            // Something stopped the script that has no business at the top level.
            let stray: String = self.slice(self.pos, self.pos + 2);
            let shown = if stray.starts_with(";;") {
                ";;".to_string()
            } else if let Some(word) = self.peek_word().filter(|w| RESERVED_STOPS.contains(&w.as_str())) {
                word
            } else {
                stray.chars().take(1).collect()
            };
            self.note(format!("`{shown}` appears without anything to close or match it"));
            let before = self.pos;
            if self.peek_word().is_some() && !self.starts_with(")") {
                let w = self.peek_word().unwrap_or_default();
                self.pos += w.chars().count().max(1);
            } else {
                self.pos += if self.starts_with(";;") { 2 } else { 1 };
            }
            if self.pos == before {
                self.pos += 1;
            }
        }
        script
    }

    /// The next plain word at the current position (not consumed), if it is just letters.
    fn peek_word(&self) -> Option<String> {
        let mut i = self.pos;
        let mut w = String::new();
        while let Some(&c) = self.s.get(i) {
            if c.is_ascii_alphanumeric() || c == '_' || c == '}' && w.is_empty() {
                w.push(c);
                i += 1;
                if c == '}' {
                    break;
                }
            } else {
                break;
            }
        }
        let next = self.s.get(i).copied();
        let ends = next.is_none_or(|c| c.is_whitespace() || matches!(c, ';' | '&' | '|' | ')' | '(' | '<' | '>'));
        if w.is_empty() || !ends { None } else { Some(w) }
    }

    fn at_stop(&mut self, stops: &[&str]) -> bool {
        self.skip_blanks(true);
        if self.eof() || self.peek() == Some(')') || self.starts_with(";;") {
            return true;
        }
        self.peek_word().is_some_and(|w| stops.contains(&w.as_str()))
    }

    fn parse_script(&mut self, stops: &[&str]) -> Script {
        let mut script = Script::default();
        if self.depth >= MAX_DEPTH {
            self.note("the line is nested too deeply to explain fully");
            self.pos = self.s.len();
            return script;
        }
        self.depth += 1;
        loop {
            if self.at_stop(stops) {
                break;
            }
            let before = self.pos;
            let chain = self.parse_and_or();
            self.skip_blanks(false);
            let sep = match self.peek() {
                Some(';') if !self.starts_with(";;") => {
                    self.pos += 1;
                    Sep::Seq
                }
                Some('&') => {
                    self.pos += 1;
                    Sep::Background
                }
                _ => Sep::Seq,
            };
            if !chain.first.cmds.is_empty() {
                script.items.push(Item { chain, sep });
            }
            if self.pos == before {
                // No progress: skip one character rather than loop forever.
                self.pos += 1;
            }
        }
        self.depth -= 1;
        script
    }

    fn parse_and_or(&mut self) -> AndOr {
        let first = self.parse_pipeline();
        let mut rest = Vec::new();
        loop {
            self.skip_blanks(false);
            let conn = if self.starts_with("&&") {
                Conn::And
            } else if self.starts_with("||") {
                Conn::Or
            } else {
                break;
            };
            self.pos += 2;
            self.skip_blanks(true);
            let next = self.parse_pipeline();
            if next.cmds.is_empty() {
                self.note("the line ends right after `&&` or `||`, with nothing to run next");
                break;
            }
            rest.push((conn, next));
        }
        AndOr { first, rest }
    }

    fn parse_pipeline(&mut self) -> Pipeline {
        let start = self.skip_and_mark();
        let mut pipeline = Pipeline::default();
        while self.peek() == Some('!') && self.at(1).is_none_or(char::is_whitespace) {
            pipeline.negated = !pipeline.negated;
            self.pos += 1;
            self.skip_blanks(false);
        }
        loop {
            self.skip_blanks(false);
            let Some(cmd) = self.parse_command() else { break };
            pipeline.cmds.push(cmd);
            self.skip_blanks(false);
            if self.starts_with("|&") {
                self.pos += 2;
                pipeline.pipe_err.push(true);
            } else if self.peek() == Some('|') && self.at(1) != Some('|') {
                self.pos += 1;
                pipeline.pipe_err.push(false);
            } else {
                break;
            }
            self.skip_blanks(true);
            if self.eof() {
                self.note("the line ends right after a `|`, with nothing to pipe into");
            }
        }
        pipeline.raw = self.slice(start, self.pos).trim().to_string();
        pipeline
    }

    fn skip_and_mark(&mut self) -> usize {
        self.skip_blanks(false);
        self.pos
    }

    fn parse_command(&mut self) -> Option<Cmd> {
        self.skip_blanks(false);
        let c = self.peek()?;
        if matches!(c, '\n' | ';' | ')') || self.starts_with("&&") || self.starts_with("||") || c == '&' || c == '|' {
            return None;
        }
        if c == '(' && self.at(1) != Some('(') {
            self.pos += 1;
            let script = self.parse_script(&[]);
            if self.peek() == Some(')') {
                self.pos += 1;
            } else {
                self.note("a `(` is never closed");
            }
            let redirs = self.parse_trailing_redirs();
            return Some(Cmd::Subshell(script, redirs));
        }
        if c == '(' {
            // `((arithmetic))` as a command.
            let start = self.pos;
            self.skip_balanced_parens();
            return Some(Cmd::Test(self.slice(start, self.pos)));
        }
        if let Some(word) = self.peek_word() {
            match word.as_str() {
                "{" => {}
                "if" => return Some(self.parse_if()),
                "for" => return Some(self.parse_for()),
                "while" | "until" => return Some(self.parse_while(word == "until")),
                "case" => return Some(self.parse_case()),
                "function" => return Some(self.parse_function_keyword()),
                _ => {}
            }
        }
        if c == '{' && self.at(1).is_none_or(char::is_whitespace) {
            self.pos += 1;
            let script = self.parse_script(&["}"]);
            if self.peek_word().as_deref() == Some("}") {
                self.pos += 1;
            } else {
                self.note("a `{` is never closed with `}`");
            }
            let redirs = self.parse_trailing_redirs();
            return Some(Cmd::Group(script, redirs));
        }
        if let Some(name) = self.function_def_name() {
            self.skip_blanks(true);
            let body = match self.parse_command() {
                Some(Cmd::Group(script, _)) => script,
                Some(other) => Script {
                    items: vec![Item {
                        chain: AndOr { first: Pipeline { cmds: vec![other], ..Pipeline::default() }, rest: Vec::new() },
                        sep: Sep::Seq,
                    }],
                },
                None => Script::default(),
            };
            return Some(Cmd::Func { name, body });
        }
        Some(self.parse_simple())
    }

    /// If the input continues with `name()` or `name ()`, consumes up to and including `()`.
    fn function_def_name(&mut self) -> Option<String> {
        let mut i = self.pos;
        let mut name = String::new();
        while let Some(&c) = self.s.get(i) {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '-' | '.') {
                name.push(c);
                i += 1;
            } else {
                break;
            }
        }
        if name.is_empty() {
            return None;
        }
        while matches!(self.s.get(i), Some(' ' | '\t')) {
            i += 1;
        }
        if self.s.get(i) == Some(&'(') {
            let mut j = i + 1;
            while matches!(self.s.get(j), Some(' ' | '\t')) {
                j += 1;
            }
            if self.s.get(j) == Some(&')') {
                self.pos = j + 1;
                return Some(name);
            }
        }
        None
    }

    fn parse_function_keyword(&mut self) -> Cmd {
        self.pos += "function".len();
        self.skip_blanks(false);
        let mut name = String::new();
        while let Some(c) = self.peek() {
            if c.is_whitespace() || c == '(' || c == '{' {
                break;
            }
            name.push(c);
            self.pos += 1;
        }
        self.skip_blanks(false);
        if self.peek() == Some('(') && self.at(1) == Some(')') {
            self.pos += 2;
        }
        self.skip_blanks(true);
        let body = match self.parse_command() {
            Some(Cmd::Group(script, _)) => script,
            _ => Script::default(),
        };
        Cmd::Func { name, body }
    }

    fn skip_balanced_parens(&mut self) {
        let mut depth = 0usize;
        while let Some(c) = self.peek() {
            self.pos += 1;
            match c {
                '(' => depth += 1,
                ')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
    }

    fn expect_word(&mut self, word: &str) -> bool {
        self.skip_blanks(true);
        if self.peek_word().as_deref() == Some(word) {
            self.pos += word.len();
            true
        } else {
            self.note(format!("expected `{word}` but did not find it"));
            false
        }
    }

    fn skip_separators(&mut self) {
        loop {
            self.skip_blanks(true);
            if self.peek() == Some(';') && !self.starts_with(";;") {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn parse_if(&mut self) -> Cmd {
        self.pos += 2;
        let mut branches = Vec::new();
        let mut otherwise = None;
        let cond = self.parse_script(&["then"]);
        self.expect_word("then");
        let body = self.parse_script(&["elif", "else", "fi"]);
        branches.push((cond, body));
        loop {
            self.skip_blanks(true);
            match self.peek_word().as_deref() {
                Some("elif") => {
                    self.pos += 4;
                    let cond = self.parse_script(&["then"]);
                    self.expect_word("then");
                    let body = self.parse_script(&["elif", "else", "fi"]);
                    branches.push((cond, body));
                }
                Some("else") => {
                    self.pos += 4;
                    otherwise = Some(self.parse_script(&["fi"]));
                }
                _ => break,
            }
        }
        self.expect_word("fi");
        let redirs = self.parse_trailing_redirs();
        Cmd::If { branches, otherwise, redirs }
    }

    fn parse_for(&mut self) -> Cmd {
        let start = self.pos;
        self.pos += 3;
        self.skip_blanks(false);
        if self.starts_with("((") {
            self.skip_balanced_parens();
            self.skip_separators();
            let head = self.slice(start, self.pos);
            self.expect_word("do");
            let body = self.parse_script(&["done"]);
            self.expect_word("done");
            let redirs = self.parse_trailing_redirs();
            return Cmd::For { var: String::new(), words: None, body, raw_head: head, redirs };
        }
        let var = self.read_plain_name();
        self.skip_blanks(false);
        let mut words = None;
        if self.peek_word().as_deref() == Some("in") {
            self.pos += 2;
            let mut list = Vec::new();
            loop {
                self.skip_blanks(false);
                match self.peek() {
                    None | Some('\n' | ';') => break,
                    _ => {}
                }
                let before = self.pos;
                list.push(self.parse_word());
                if self.pos == before {
                    break;
                }
            }
            words = Some(list);
        }
        self.skip_separators();
        let head = self.slice(start, self.pos).trim().trim_end_matches(';').trim().to_string();
        self.expect_word("do");
        let body = self.parse_script(&["done"]);
        self.expect_word("done");
        let redirs = self.parse_trailing_redirs();
        Cmd::For { var, words, body, raw_head: head, redirs }
    }

    fn read_plain_name(&mut self) -> String {
        let mut name = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                name.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        name
    }

    fn parse_while(&mut self, until: bool) -> Cmd {
        let start = self.pos;
        self.pos += 5;
        let cond = self.parse_script(&["do"]);
        let head = self.slice(start, self.pos).trim().to_string();
        self.expect_word("do");
        let body = self.parse_script(&["done"]);
        self.expect_word("done");
        let redirs = self.parse_trailing_redirs();
        Cmd::While { until, cond, body, raw_head: head, redirs }
    }

    fn parse_case(&mut self) -> Cmd {
        self.pos += 4;
        self.skip_blanks(false);
        let subject = self.parse_word();
        self.skip_blanks(true);
        if self.peek_word().as_deref() == Some("in") {
            self.pos += 2;
        } else {
            self.note("expected `in` after `case`");
        }
        let mut arms = Vec::new();
        loop {
            self.skip_blanks(true);
            if self.eof() || self.peek_word().as_deref() == Some("esac") {
                break;
            }
            if self.peek() == Some('(') {
                self.pos += 1;
            }
            let mut patterns = Vec::new();
            let mut current = String::new();
            while let Some(c) = self.peek() {
                match c {
                    ')' => {
                        self.pos += 1;
                        break;
                    }
                    '|' => {
                        patterns.push(std::mem::take(&mut current));
                        self.pos += 1;
                    }
                    '\n' => break,
                    _ => {
                        current.push(c);
                        self.pos += 1;
                    }
                }
            }
            patterns.push(current.trim().to_string());
            let body = self.parse_script(&["esac"]);
            self.skip_blanks(true);
            if self.starts_with(";;&") {
                self.pos += 3;
            } else if self.starts_with(";;") || self.starts_with(";&") {
                self.pos += 2;
            }
            arms.push(Arm { patterns: patterns.into_iter().map(|p| p.trim().to_string()).collect(), body });
            if self.eof() {
                break;
            }
        }
        self.expect_word("esac");
        let redirs = self.parse_trailing_redirs();
        Cmd::Case { subject, arms, redirs }
    }

    fn parse_trailing_redirs(&mut self) -> Vec<Redir> {
        let mut redirs = Vec::new();
        loop {
            self.skip_blanks(false);
            match self.try_redir() {
                Some(r) => redirs.push(r),
                None => break,
            }
        }
        redirs
    }

    fn parse_simple(&mut self) -> Cmd {
        let start = self.pos;
        let mut end = start;
        let mut simple = Simple::default();
        loop {
            self.skip_blanks(false);
            let Some(c) = self.peek() else { break };
            let in_test = simple.words.first().is_some_and(|w| w.text == "[[")
                && simple.words.last().is_some_and(|w| w.text != "]]");
            if in_test && c != '\n' {
                // Inside `[[ ... ]]` the operators are just words.
                let word = self.parse_test_word();
                let done = word.text == "]]";
                simple.words.push(word);
                end = self.pos;
                if done {
                    break;
                }
                continue;
            }
            if matches!(c, '\n' | ';' | ')' | '|') {
                break;
            }
            if c == '&' && !self.starts_with("&>") {
                break;
            }
            if let Some(redir) = self.try_redir() {
                simple.redirs.push(redir);
                end = self.pos;
                continue;
            }
            let before = self.pos;
            let word = self.parse_word();
            if self.pos == before {
                self.pos += 1;
                continue;
            }
            if simple.words.is_empty()
                && let Some(eq) = assignment_split(&word)
            {
                simple.assigns.push(eq);
                end = self.pos;
                continue;
            }
            simple.words.push(word);
            end = self.pos;
        }
        simple.raw = clean_continuations(&self.slice(start, end.max(start)));
        if simple.words.first().is_some_and(|w| w.text == "[[") {
            return Cmd::Test(simple.raw);
        }
        Cmd::Simple(simple)
    }

    fn parse_test_word(&mut self) -> Word {
        // Operators like `&&`, `<`, `(` are words inside [[ ]].
        let start = self.pos;
        if matches!(self.peek(), Some('&' | '<' | '>' | '(' | ')' | '|')) {
            while matches!(self.peek(), Some('&' | '<' | '>' | '(' | ')' | '|')) {
                self.pos += 1;
            }
            let raw = self.slice(start, self.pos);
            return Word { text: raw.clone(), raw, ..Word::default() };
        }
        self.parse_word()
    }

    /// Reads a redirection at the current position, if there is one.
    fn try_redir(&mut self) -> Option<Redir> {
        let start = self.pos;
        let mut i = self.pos;
        let mut fd = String::new();
        while self.s.get(i).is_some_and(char::is_ascii_digit) {
            fd.push(self.s[i]);
            i += 1;
        }
        let c = self.s.get(i).copied()?;
        let ops: [&str; 11] = ["&>>", "<<<", "<<-", "&>", ">>", "<<", ">|", ">&", "<&", "<>", ""];
        let mut op = String::new();
        let here: String = self.s[i..(i + 3).min(self.s.len())].iter().collect();
        if c == '&' && fd.is_empty() {
            for cand in ["&>>", "&>"] {
                if here.starts_with(cand) {
                    op = cand.to_string();
                    break;
                }
            }
        } else if c == '<' || c == '>' {
            for cand in ops {
                if !cand.is_empty() && !cand.starts_with('&') && here.starts_with(cand) {
                    op = cand.to_string();
                    break;
                }
            }
            if op.is_empty() {
                op = c.to_string();
            }
            // `<(` and `>(` are process substitutions, which are words.
            if fd.is_empty() && self.s.get(i + 1) == Some(&'(') {
                return None;
            }
        }
        if op.is_empty() {
            return None;
        }
        self.pos = i + op.chars().count();
        self.skip_blanks(false);
        let (target, heredoc) = if op == "<<" || op == "<<-" {
            let word = self.parse_word();
            let delimiter = word.text.clone();
            let body = self.read_heredoc_body(&delimiter, op == "<<-");
            (word.clone(), Some(Heredoc { delimiter, quoted: word.quoted, body, strip_tabs: op == "<<-" }))
        } else {
            (self.parse_word(), None)
        };
        let raw = self.slice(start, self.pos).trim().to_string();
        Some(Redir { fd: if fd.is_empty() { None } else { Some(fd) }, op, target, heredoc, raw })
    }

    fn read_heredoc_body(&mut self, delimiter: &str, strip_tabs: bool) -> String {
        // The body starts on the line after the current one (or after a previous heredoc's body).
        let nl = match self.heredoc_nl {
            Some(n) => n,
            None => {
                let mut j = self.pos;
                while j < self.s.len() && self.s[j] != '\n' {
                    j += 1;
                }
                j
            }
        };
        if nl >= self.s.len() {
            self.note(format!("the heredoc `<<{delimiter}` has no body in this text"));
            return String::new();
        }
        let mut line_start = self.heredoc_from.unwrap_or(nl + 1);
        let mut body = String::new();
        let mut found = false;
        while line_start < self.s.len() {
            let mut line_end = line_start;
            while line_end < self.s.len() && self.s[line_end] != '\n' {
                line_end += 1;
            }
            let line = self.slice(line_start, line_end);
            let compare = if strip_tabs { line.trim_start_matches('\t') } else { line.as_str() };
            line_start = (line_end + 1).min(self.s.len());
            if compare == delimiter {
                found = true;
                break;
            }
            body.push_str(&line);
            if line_end < self.s.len() {
                body.push('\n');
            }
        }
        if !found {
            self.note(format!("the heredoc `<<{delimiter}` is never closed by a line saying `{delimiter}`"));
        }
        // Reaching the newline at `nl` continues after the body.
        self.skip.retain(|(n, _)| *n != nl);
        self.skip.push((nl, line_start));
        self.heredoc_nl = Some(nl);
        self.heredoc_from = Some(line_start);
        body
    }

    /// Reads one shell word (with quotes, escapes, substitutions) starting at the current position.
    fn parse_word(&mut self) -> Word {
        let start = self.pos;
        let mut w = Word::default();
        let mut text = String::new();
        let mut in_brackets = false;
        while let Some(c) = self.peek() {
            match c {
                ' ' | '\t' | '\n' | '\r' | '|' | '&' | ';' | ')' => break,
                '<' | '>' if self.at(1) != Some('(') => break,
                '(' if !(self.pos > start && matches!(self.s.get(self.pos - 1), Some('<' | '>' | '$' | '='))) => {
                    // `(` in the middle of a word, e.g. `a(b`: treat it as the end of the word.
                    if self.pos == start {
                        self.pos += 1;
                        text.push('(');
                        w.raw.push('(');
                        continue;
                    }
                    break;
                }
                '\\' => {
                    if self.at(1) == Some('\n') {
                        self.pos += 2;
                        continue;
                    }
                    w.quoted = true;
                    self.pos += 1;
                    if let Some(next) = self.peek() {
                        text.push(next);
                        self.pos += 1;
                    }
                }
                '\'' => {
                    w.quoted = true;
                    self.pos += 1;
                    let from = self.pos;
                    while self.peek().is_some_and(|c| c != '\'') {
                        self.pos += 1;
                    }
                    text.push_str(&self.slice(from, self.pos));
                    if self.peek() == Some('\'') {
                        self.pos += 1;
                    } else {
                        self.note("a single quote `'` is never closed");
                    }
                }
                '"' => {
                    w.quoted = true;
                    self.pos += 1;
                    self.parse_dquote(&mut w, &mut text);
                }
                '`' => self.parse_backtick(&mut w, &mut text, false),
                '$' => self.parse_dollar(&mut w, &mut text, false),
                '<' | '>' => {
                    // process substitution `<(...)` / `>(...)`
                    let dir = c;
                    let from = self.pos;
                    self.pos += 2;
                    let script = self.parse_script(&[]);
                    if self.peek() == Some(')') {
                        self.pos += 1;
                    } else {
                        self.note("a `(` is never closed");
                    }
                    let raw = self.slice(from, self.pos);
                    text.push_str(&raw);
                    w.subs.push(Sub { kind: SubKind::Process(dir), raw, script, in_quotes: false });
                }
                '*' | '?' => {
                    w.glob = true;
                    text.push(c);
                    self.pos += 1;
                }
                '[' => {
                    in_brackets = true;
                    w.glob |= self.s[self.pos..].iter().take_while(|c| !c.is_whitespace()).any(|&c| c == ']');
                    text.push(c);
                    self.pos += 1;
                }
                '{' if self.s[self.pos..].iter().take_while(|c| !c.is_whitespace()).any(|&c| c == ',' || c == '.') => {
                    // `{a,b}` or `{1..3}`: brace expansion happens only if the word holds a matching `}`.
                    let rest: String = self.s[self.pos..].iter().take_while(|c| !c.is_whitespace()).collect();
                    if rest.contains('}') {
                        w.glob = true;
                    }
                    text.push(c);
                    self.pos += 1;
                }
                _ => {
                    text.push(c);
                    self.pos += 1;
                }
            }
        }
        let _ = in_brackets;
        w.raw = self.slice(start, self.pos);
        w.text = text;
        if w.raw.starts_with('~') {
            w.glob |= false;
        }
        w
    }

    fn parse_dquote(&mut self, w: &mut Word, text: &mut String) {
        loop {
            match self.peek() {
                None => {
                    self.note("a double quote `\"` is never closed");
                    break;
                }
                Some('"') => {
                    self.pos += 1;
                    break;
                }
                Some('\\') => {
                    self.pos += 1;
                    match self.peek() {
                        Some('\n') => self.pos += 1,
                        Some(n @ ('$' | '`' | '"' | '\\')) => {
                            text.push(n);
                            self.pos += 1;
                        }
                        Some(n) => {
                            text.push('\\');
                            text.push(n);
                            self.pos += 1;
                        }
                        None => text.push('\\'),
                    }
                }
                Some('`') => self.parse_backtick(w, text, true),
                Some('$') => self.parse_dollar(w, text, true),
                Some(c) => {
                    text.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    fn parse_backtick(&mut self, w: &mut Word, text: &mut String, in_quotes: bool) {
        let from = self.pos;
        self.pos += 1;
        let mut inner = String::new();
        let mut closed = false;
        while let Some(c) = self.peek() {
            self.pos += 1;
            match c {
                '`' => {
                    closed = true;
                    break;
                }
                '\\' => match self.peek() {
                    Some(n @ ('`' | '\\' | '$')) => {
                        inner.push(n);
                        self.pos += 1;
                    }
                    _ => inner.push('\\'),
                },
                _ => inner.push(c),
            }
        }
        if !closed {
            self.note("a backtick is never closed");
        }
        let raw = self.slice(from, self.pos);
        let script = self.sub_parse(&inner);
        text.push_str(&raw);
        w.subs.push(Sub { kind: SubKind::Backtick, raw, script, in_quotes });
    }

    /// Parses text from inside a substitution with a fresh parser, carrying over notes.
    fn sub_parse(&mut self, inner: &str) -> Script {
        if self.depth >= MAX_DEPTH {
            self.note("the line is nested too deeply to explain fully");
            return Script::default();
        }
        let mut p = Parser::new(inner);
        p.depth = self.depth + 1;
        let script = p.parse_top();
        for n in p.notes {
            self.note(n);
        }
        script
    }

    fn parse_dollar(&mut self, w: &mut Word, text: &mut String, in_quotes: bool) {
        let from = self.pos;
        match self.at(1) {
            Some('(') if self.at(2) == Some('(') => {
                self.pos += 1;
                self.skip_balanced_parens();
                let raw = self.slice(from, self.pos);
                text.push_str(&raw);
                w.subs.push(Sub { kind: SubKind::Arith, raw, script: Script::default(), in_quotes });
            }
            Some('(') => {
                self.pos += 2;
                let script = self.parse_script(&[]);
                if self.peek() == Some(')') {
                    self.pos += 1;
                } else {
                    self.note("a `$(` is never closed");
                }
                let raw = self.slice(from, self.pos);
                text.push_str(&raw);
                w.subs.push(Sub { kind: SubKind::Command, raw, script, in_quotes });
            }
            Some('{') => {
                self.pos += 2;
                let mut inner = String::new();
                let mut depth = 1;
                while let Some(c) = self.peek() {
                    self.pos += 1;
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    inner.push(c);
                }
                if depth != 0 {
                    self.note("a `${` is never closed");
                }
                let raw = self.slice(from, self.pos);
                let name: String = inner
                    .trim_start_matches(['!', '#'])
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                let rest = &inner[inner
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '!' || c == '#'))
                    .unwrap_or(inner.len())..];
                let guarded = rest.starts_with(":-")
                    || rest.starts_with(":=")
                    || rest.starts_with(":?")
                    || rest.starts_with('-')
                    || rest.starts_with('=')
                    || rest.starts_with(":+")
                    || rest.starts_with('+');
                w.var_offsets.push(text.chars().count());
                text.push_str(&raw);
                if !name.is_empty() {
                    w.vars.push(VarRef { name, guarded });
                } else {
                    w.var_offsets.pop();
                }
            }
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                self.pos += 1;
                let name = self.read_plain_name();
                let raw = self.slice(from, self.pos);
                w.var_offsets.push(text.chars().count());
                text.push_str(&raw);
                w.vars.push(VarRef { name, guarded: false });
            }
            Some(c) if c.is_ascii_digit() || matches!(c, '?' | '$' | '!' | '#' | '@' | '*' | '-') => {
                self.pos += 2;
                let raw = self.slice(from, self.pos);
                text.push_str(&raw);
                if c.is_ascii_digit() {
                    w.var_offsets.push(text.chars().count() - raw.chars().count());
                    w.vars.push(VarRef { name: c.to_string(), guarded: false });
                }
            }
            Some('\'') if !in_quotes => {
                // $'...' ANSI-C quoting
                self.pos += 2;
                let from_text = self.pos;
                while let Some(c) = self.peek() {
                    if c == '\\' {
                        self.pos += 2;
                        continue;
                    }
                    if c == '\'' {
                        break;
                    }
                    self.pos += 1;
                }
                text.push_str(&self.slice(from_text, self.pos));
                if self.peek() == Some('\'') {
                    self.pos += 1;
                }
                w.quoted = true;
            }
            _ => {
                self.pos += 1;
                text.push('$');
            }
        }
    }
}

/// Joins `\`-newline continuations into single spaces, so the text reads as one line.
fn clean_continuations(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'\n') {
            chars.next();
            while matches!(chars.peek(), Some(' ' | '\t')) {
                chars.next();
            }
            if !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    out.trim().to_string()
}

/// `NAME=value` or `NAME+=value` (with an unquoted name) as a leading assignment.
fn assignment_split(word: &Word) -> Option<(String, Word)> {
    let raw = &word.raw;
    let eq = raw.find('=')?;
    let name = raw[..eq].trim_end_matches('+');
    if name.is_empty()
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        || name.chars().next()?.is_ascii_digit()
    {
        return None;
    }
    let value_raw = raw[eq + 1..].to_string();
    // The value's own text: reparse it as a word so quotes inside it are handled.
    let mut p = Parser::new(&value_raw);
    let mut value = p.parse_word();
    value.raw = value_raw;
    Some((name.to_string(), value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_simple(input: &str) -> Simple {
        let parsed = parse(input);
        match &parsed.script.items[0].chain.first.cmds[0] {
            Cmd::Simple(s) => s.clone(),
            other => panic!("expected a simple command, got {other:?}"),
        }
    }

    fn texts(simple: &Simple) -> Vec<String> {
        simple.words.iter().map(|w| w.text.clone()).collect()
    }

    #[test]
    fn splits_words_and_removes_quotes() {
        let s = first_simple(r#"echo 'a b' "c d" e\ f g"h i"j"#);
        assert_eq!(texts(&s), ["echo", "a b", "c d", "e f", "gh ij"]);
        assert!(s.words[1].quoted && !s.words[0].quoted);
    }

    #[test]
    fn pipes_and_lists() {
        let p = parse("a | b |& c && d || e; f &");
        assert_eq!(p.script.items.len(), 2);
        let first = &p.script.items[0].chain;
        assert_eq!(first.first.cmds.len(), 3);
        assert_eq!(first.first.pipe_err, [false, true]);
        assert_eq!(first.rest.len(), 2);
        assert_eq!(first.rest[0].0, Conn::And);
        assert_eq!(first.rest[1].0, Conn::Or);
        assert_eq!(p.script.items[1].sep, Sep::Background);
        assert!(p.notes.is_empty());
    }

    #[test]
    fn operators_inside_quotes_are_not_operators() {
        let p = parse(r#"git commit -m "fix | pipe && more; stuff""#);
        assert_eq!(p.script.items.len(), 1);
        assert_eq!(p.script.items[0].chain.first.cmds.len(), 1);
    }

    #[test]
    fn command_substitution_nests() {
        let s = first_simple("kill -9 $(lsof -t -i:3000)");
        assert_eq!(texts(&s), ["kill", "-9", "$(lsof -t -i:3000)"]);
        let sub = &s.words[2].subs[0];
        assert_eq!(sub.kind, SubKind::Command);
        let Cmd::Simple(inner) = &sub.script.items[0].chain.first.cmds[0] else { panic!() };
        assert_eq!(texts(inner), ["lsof", "-t", "-i:3000"]);
        let s = first_simple("echo $(echo $(echo deep))");
        let Cmd::Simple(inner) = &s.words[1].subs[0].script.items[0].chain.first.cmds[0] else { panic!() };
        assert_eq!(inner.words[1].subs.len(), 1);
    }

    #[test]
    fn backticks_arithmetic_and_variables() {
        let s = first_simple("echo `date` $((1+2)) ${HOME}/x ${A:-b} $B \"$C\"");
        assert_eq!(s.words[1].subs[0].kind, SubKind::Backtick);
        assert_eq!(s.words[2].subs[0].kind, SubKind::Arith);
        assert_eq!(s.words[3].vars[0], VarRef { name: "HOME".into(), guarded: false });
        assert!(s.words[4].vars[0].guarded);
        assert_eq!(s.words[5].vars[0].name, "B");
        assert_eq!(s.words[6].vars[0].name, "C");
        let s = first_simple("make -j$(nproc)");
        assert_eq!(s.words[1].text, "-j$(nproc)");
        assert_eq!(s.words[1].subs.len(), 1);
    }

    #[test]
    fn redirections() {
        let s = first_simple("cmd >a >>b 2>&1 &>c <d <<<str 2>/dev/null >|f");
        let ops: Vec<&str> = s.redirs.iter().map(|r| r.op.as_str()).collect();
        assert_eq!(ops, [">", ">>", ">&", "&>", "<", "<<<", ">", ">|"]);
        assert_eq!(s.redirs[2].fd.as_deref(), Some("2"));
        assert_eq!(s.redirs[2].target.text, "1");
        assert_eq!(texts(&s), ["cmd"]);
    }

    #[test]
    fn heredoc_body_is_captured_and_skipped() {
        let p = parse("cat <<'EOF' > ~/.config/app.conf\nline one\nline | two\nEOF\necho after");
        assert_eq!(p.script.items.len(), 2);
        let Cmd::Simple(s) = &p.script.items[0].chain.first.cmds[0] else { panic!() };
        let doc = s.redirs[0].heredoc.as_ref().unwrap();
        assert_eq!(doc.body, "line one\nline | two\n");
        assert!(doc.quoted);
        assert_eq!(s.redirs[1].op, ">");
        assert_eq!(s.redirs[1].target.text, "~/.config/app.conf");
        assert!(p.notes.is_empty(), "{:?}", p.notes);
    }

    #[test]
    fn two_heredocs_on_one_line() {
        let p = parse("cmd <<A <<B\none\nA\ntwo\nB\nnext");
        assert_eq!(p.script.items.len(), 2);
        let Cmd::Simple(s) = &p.script.items[0].chain.first.cmds[0] else { panic!() };
        assert_eq!(s.redirs[0].heredoc.as_ref().unwrap().body, "one\n");
        assert_eq!(s.redirs[1].heredoc.as_ref().unwrap().body, "two\n");
    }

    #[test]
    fn assignments_before_a_command() {
        let s = first_simple("A=1 B='x y' env C=2 ./run.sh");
        assert_eq!(s.assigns.len(), 2);
        assert_eq!(s.assigns[1].1.text, "x y");
        assert_eq!(texts(&s), ["env", "C=2", "./run.sh"]);
    }

    #[test]
    fn compound_commands() {
        let p = parse("for f in *.png; do convert \"$f\" -resize 50% \"small_$f\"; done");
        let Cmd::For { var, words, body, .. } = &p.script.items[0].chain.first.cmds[0] else { panic!() };
        assert_eq!(var, "f");
        assert!(words.as_ref().unwrap()[0].glob);
        assert_eq!(body.items.len(), 1);
        assert!(p.notes.is_empty(), "{:?}", p.notes);

        let p = parse("if [ -f x ]; then echo yes; elif true; then echo mid; else echo no; fi");
        let Cmd::If { branches, otherwise, .. } = &p.script.items[0].chain.first.cmds[0] else { panic!() };
        assert_eq!(branches.len(), 2);
        assert!(otherwise.is_some());

        let p = parse("while read l; do echo $l; done < file");
        assert!(matches!(&p.script.items[0].chain.first.cmds[0], Cmd::While { .. }));

        let p = parse("case $x in a|b) echo ab ;; *) echo other ;; esac");
        let Cmd::Case { arms, .. } = &p.script.items[0].chain.first.cmds[0] else { panic!() };
        assert_eq!(arms.len(), 2);
        assert_eq!(arms[0].patterns, ["a", "b"]);
        assert!(p.notes.is_empty(), "{:?}", p.notes);
    }

    #[test]
    fn subshells_groups_functions() {
        let p = parse("(cd /tmp && ls) ; { echo a; echo b; } > out; f() { echo hi; }");
        assert!(matches!(&p.script.items[0].chain.first.cmds[0], Cmd::Subshell(..)));
        let Cmd::Group(g, redirs) = &p.script.items[1].chain.first.cmds[0] else { panic!() };
        assert_eq!(g.items.len(), 2);
        assert_eq!(redirs.len(), 1);
        assert!(matches!(&p.script.items[2].chain.first.cmds[0], Cmd::Func { .. }));
        assert!(p.notes.is_empty(), "{:?}", p.notes);
    }

    #[test]
    fn process_substitution_and_test_brackets() {
        let s = first_simple("bash <(curl -s https://x.sh)");
        assert_eq!(s.words[1].subs[0].kind, SubKind::Process('<'));
        let p = parse("[[ -f a && -d b ]] && echo ok");
        assert!(matches!(&p.script.items[0].chain.first.cmds[0], Cmd::Test(_)));
        assert_eq!(p.script.items[0].chain.rest.len(), 1);
    }

    #[test]
    fn line_continuations() {
        let s = first_simple("docker run \\\n  -d \\\n  nginx");
        assert_eq!(texts(&s), ["docker", "run", "-d", "nginx"]);
    }

    #[test]
    fn broken_input_gives_notes_not_panics() {
        for input in [
            "echo 'oops",
            "echo \"oops",
            "echo $(oops",
            "echo `oops",
            "(a",
            "a)",
            "a &&",
            "a |",
            "cat <<EOF\nno end",
            "${x",
            "case x in",
            "for",
            "if",
            "{ a",
            "$(",
            ";;",
        ] {
            let p = parse(input);
            assert!(
                !p.notes.is_empty() || p.script.items.is_empty() || input == "for" || input == "if",
                "{input}: {:?}",
                p.notes
            );
        }
    }

    #[test]
    fn survives_random_junk() {
        let alphabet: Vec<char> = "ab \t\n|&;()<>$`'\"\\{}[]*?=!#~-0123456789".chars().collect();
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        for _ in 0..4000 {
            let mut text = String::new();
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let len = (seed % 40) as usize;
            for _ in 0..len {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                text.push(alphabet[(seed % alphabet.len() as u64) as usize]);
            }
            let _ = parse(&text);
        }
        let deep = "$(".repeat(500) + &")".repeat(500);
        let _ = parse(&deep);
        let deep = "(".repeat(500);
        let _ = parse(&deep);
        let _ = parse("é日本語 'ü \"ß");
    }
}
