//! The engine behind `huh explain`: walks the parsed command line and builds a part-by-part
//! explanation plus a list of warnings. It only reads: its own tables, man pages and the PATH
//! directory listing. It never runs anything, and never opens files named in the command.

use crate::catalog::Command;
use crate::decode;
use crate::knowledge::{ArgKind, Knowledge, RoleKind, Section};
use crate::manpage::{self, ManPage};
use crate::risk::{self, INTERPRETERS, Inv, PosArg, Risk, SHELLS, Warning};
use crate::risk_extra;
use crate::shell::{self, Cmd, Conn, Pipeline, Redir, Script, Sep, Simple, SubKind, Word};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

#[derive(Debug, Clone)]
pub struct Row {
    pub piece: String,
    pub meaning: String,
}

#[derive(Debug, Clone)]
pub struct Inner {
    pub label: String,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, Default)]
pub struct CmdBlock {
    pub raw: String,
    pub headline: String,
    pub rows: Vec<Row>,
    pub inner: Vec<Inner>,
    /// Programs this runs, outermost first (`sudo`, `bash`).
    pub chain: Vec<String>,
    /// A shell or interpreter that reads its program from standard input.
    pub stdin_script: bool,
    /// Downloads something from the network (curl, wget).
    pub fetches: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnKind {
    Pipe,
    PipeErr,
    And,
    Or,
    Seq,
    Background,
}

#[derive(Debug, Clone)]
pub enum Block {
    Cmd(CmdBlock),
    Connector(ConnKind),
}

#[derive(Debug, Clone)]
pub struct Step {
    pub conn: Option<ConnKind>,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct Explanation {
    pub steps: Vec<Step>,
    pub blocks: Vec<Block>,
    pub warnings: Vec<Warning>,
    pub notes: Vec<String>,
    /// The input had more than one line, so it is a script rather than a single line.
    pub multiline: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Read man pages from disk for options nothing else describes.
    pub man: bool,
    /// Look at the PATH directories to say whether a program is installed.
    pub path: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { man: true, path: true }
    }
}

#[derive(Clone, Default)]
struct Env {
    root: bool,
    remote: Option<String>,
    container: bool,
    piped_in: bool,
}

/// Explains a whole command line (possibly several lines). Never runs anything.
pub fn explain_line(catalog: &[Command], text: &str, opts: Options) -> Explanation {
    let parsed = shell::parse(text);
    let mut engine = Engine::new(catalog, opts);
    scan_assignments(&parsed.script, &mut engine.assigned);
    let mut blocks = engine.explain_script(&parsed.script, &Env::default());
    polish(&mut blocks);
    let steps = make_steps(&blocks);
    let mut notes = parsed.notes;
    notes.append(&mut engine.notes);
    let multiline = text.trim().lines().count() > 1;
    Explanation { steps, blocks, warnings: std::mem::take(&mut engine.warnings), notes, multiline }
}

/// Starts every sentence with a capital letter.
fn polish(blocks: &mut [Block]) {
    for b in blocks {
        if let Block::Cmd(c) = b {
            for r in &mut c.rows {
                r.meaning = capitalize(&r.meaning);
            }
            for i in &mut c.inner {
                polish(&mut i.blocks);
            }
        }
    }
}

fn make_steps(blocks: &[Block]) -> Vec<Step> {
    let mut steps = Vec::new();
    let mut conn = None;
    for b in blocks {
        match b {
            Block::Connector(k) => conn = Some(*k),
            Block::Cmd(c) => {
                steps.push(Step { conn: conn.take(), text: c.headline.clone() });
            }
        }
    }
    steps
}

fn scan_assignments(script: &Script, set: &mut HashSet<String>) {
    for item in &script.items {
        let chain = &item.chain;
        for p in std::iter::once(&chain.first).chain(chain.rest.iter().map(|(_, p)| p)) {
            for cmd in &p.cmds {
                scan_cmd(cmd, set);
            }
        }
    }
}

fn scan_cmd(cmd: &Cmd, set: &mut HashSet<String>) {
    match cmd {
        Cmd::Simple(s) => {
            for (n, v) in &s.assigns {
                set.insert(n.clone());
                for sub in &v.subs {
                    scan_assignments(&sub.script, set);
                }
            }
            let first = s.words.first().map(|w| w.text.as_str()).unwrap_or("");
            if matches!(first, "export" | "declare" | "local" | "readonly" | "typeset" | "read") {
                for w in s.words.iter().skip(1) {
                    let name = w.text.split('=').next().unwrap_or("");
                    if !name.starts_with('-') && !name.is_empty() {
                        set.insert(name.to_string());
                    }
                }
            }
            for w in &s.words {
                for sub in &w.subs {
                    scan_assignments(&sub.script, set);
                }
            }
        }
        Cmd::Func { name, body } => {
            set.insert(format!("fn:{name}"));
            scan_assignments(body, set);
        }
        Cmd::Subshell(s, _) | Cmd::Group(s, _) => scan_assignments(s, set),
        Cmd::For { var, body, .. } => {
            set.insert(var.clone());
            scan_assignments(body, set);
        }
        Cmd::While { cond, body, .. } => {
            scan_assignments(cond, set);
            scan_assignments(body, set);
        }
        Cmd::If { branches, otherwise, .. } => {
            for (c, b) in branches {
                scan_assignments(c, set);
                scan_assignments(b, set);
            }
            if let Some(o) = otherwise {
                scan_assignments(o, set);
            }
        }
        Cmd::Case { arms, .. } => {
            for a in arms {
                scan_assignments(&a.body, set);
            }
        }
        Cmd::Test(_) => {}
    }
}

const BUILTINS: [&str; 35] = [
    ":", "cd", "echo", "export", "source", ".", "alias", "unalias", "set", "unset", "exit", "return", "read", "test",
    "[", "[[", "true", "false", "type", "command", "builtin", "exec", "eval", "wait", "trap", "umask", "ulimit",
    "history", "jobs", "fg", "bg", "pwd", "printf", "shift", "declare",
];

fn shorten(text: &str, max: usize) -> String {
    let flat: String = text.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    if flat.chars().count() <= max {
        flat
    } else {
        let cut: String = flat.chars().take(max.saturating_sub(3)).collect();
        format!("{cut}...")
    }
}

fn capitalize(text: &str) -> String {
    let mut c = text.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn known_var(name: &str) -> Option<&'static str> {
    Some(match name {
        "HOME" => "your home folder",
        "PWD" => "the current folder",
        "OLDPWD" => "the previous folder",
        "USER" | "LOGNAME" => "your user name",
        "PATH" => "the list of folders searched for programs",
        "SHELL" => "your login shell",
        "TMPDIR" => "the temporary folder",
        "HOSTNAME" => "this computer's name",
        "UID" => "your numeric user ID",
        "XDG_CONFIG_HOME" => "your config folder (usually ~/.config)",
        _ => return None,
    })
}

fn word_to_pos(w: &Word) -> PosArg {
    PosArg {
        text: w.text.clone(),
        vars: w.vars.iter().map(|v| (v.name.clone(), v.guarded)).collect(),
        has_sub: w.subs.iter().any(|s| matches!(s.kind, SubKind::Command | SubKind::Backtick | SubKind::Process(_))),
        glob: w.glob,
    }
}

fn canonical_key(prog: &str) -> String {
    match prog {
        "gawk" | "mawk" | "nawk" => "awk",
        "egrep" | "fgrep" => "grep",
        "python3.11" | "python3.12" | "python3.13" => "python3",
        "ssh-copy-id" => "ssh",
        "[" => "test",
        "apt-get" => "apt",
        other => other,
    }
    .to_string()
}

struct Found {
    meaning: String,
    arg: ArgKind,
    names: Vec<String>,
    from_man: bool,
}

/// Working state while reading one command's arguments.
struct State {
    key: String,
    path: Vec<String>,
    desc: String,
    pos_idx: usize,
    only_pos: bool,
    catalog_cmd: Option<usize>,
    pos0: Option<String>,
    script_given: bool,
    first_positional_done: bool,
    /// Words that name what is done (like `restart`), shown in the headline.
    extra: Vec<String>,
}

struct Engine<'a> {
    catalog: &'a [Command],
    opts: Options,
    man: HashMap<String, Option<Rc<ManPage>>>,
    which: HashMap<String, Option<String>>,
    warnings: Vec<Warning>,
    notes: Vec<String>,
    assigned: HashSet<String>,
}

impl<'a> Engine<'a> {
    fn new(catalog: &'a [Command], opts: Options) -> Self {
        Engine {
            catalog,
            opts,
            man: HashMap::new(),
            which: HashMap::new(),
            warnings: Vec::new(),
            notes: Vec::new(),
            assigned: HashSet::new(),
        }
    }

    /// Adds a warning unless it is already there. Several `sudo` warnings that grant the same
    /// rights are merged into one that names every command.
    fn push_warning(&mut self, w: Warning) {
        if self.warnings.contains(&w) {
            return;
        }
        if matches!(w.part.as_str(), "sudo" | "doas")
            && w.text.starts_with("runs `")
            && let Some(cut) = w.text.rfind('`')
        {
            let (head, tail) = w.text.split_at(cut + 1);
            for e in &mut self.warnings {
                if e.part == w.part
                    && e.text.starts_with("runs `")
                    && let Some(ecut) = e.text.rfind('`')
                    && e.text[ecut + 1..] == *tail
                {
                    let added = head.strip_prefix("runs ").unwrap_or(head);
                    if !e.text[..=ecut].contains(added) {
                        e.text = format!("{} and {added}{tail}", &e.text[..=ecut]);
                        e.context = String::new();
                    }
                    return;
                }
            }
        }
        self.warnings.push(w);
    }

    fn note(&mut self, text: String) {
        if !self.notes.contains(&text) {
            self.notes.push(text);
        }
    }

    fn man_page(&mut self, name: &str) -> Option<Rc<ManPage>> {
        if !self.opts.man || !manpage::safe_page_name(name) {
            return None;
        }
        if let Some(hit) = self.man.get(name) {
            return hit.clone();
        }
        let page = manpage::load(name).map(Rc::new);
        self.man.insert(name.to_string(), page.clone());
        page
    }

    /// The man page names to try for a command path: `git-commit`, then `git`.
    fn man_names(path: &[String]) -> Vec<String> {
        let mut names = Vec::new();
        for end in (1..=path.len()).rev() {
            let name = path[..end].join("-");
            names.push(name);
        }
        if path[0] == "awk" {
            names.push("gawk".to_string());
        }
        names
    }

    fn installed(&mut self, name: &str) -> bool {
        if !manpage::safe_page_name(name) {
            return false;
        }
        if let Some(hit) = self.which.get(name) {
            return hit.is_some();
        }
        let mut found = None;
        if let Some(path) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path) {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    found = Some(candidate.display().to_string());
                    break;
                }
            }
        }
        let hit = found.is_some();
        self.which.insert(name.to_string(), found);
        hit
    }

    // -----------------------------------------------------------------------
    // Scripts, pipelines, compound commands
    // -----------------------------------------------------------------------

    fn explain_script(&mut self, script: &Script, env: &Env) -> Vec<Block> {
        let mut blocks: Vec<Block> = Vec::new();
        let mut prev_sep = Sep::Seq;
        for (n, item) in script.items.iter().enumerate() {
            if n > 0 {
                blocks.push(Block::Connector(if prev_sep == Sep::Background {
                    ConnKind::Background
                } else {
                    ConnKind::Seq
                }));
            }
            self.explain_pipeline(&item.chain.first, env, &mut blocks);
            for (conn, p) in &item.chain.rest {
                blocks.push(Block::Connector(match conn {
                    Conn::And => ConnKind::And,
                    Conn::Or => ConnKind::Or,
                }));
                self.explain_pipeline(p, env, &mut blocks);
            }
            if item.sep == Sep::Background
                && n + 1 == script.items.len()
                && let Some(Block::Cmd(c)) = blocks.iter_mut().rev().find(|b| matches!(b, Block::Cmd(_)))
            {
                c.headline.push_str(" (started in the background with &; the shell does not wait for it)");
            }
            prev_sep = item.sep;
        }
        blocks
    }

    fn explain_pipeline(&mut self, p: &Pipeline, env: &Env, blocks: &mut Vec<Block>) {
        let mut built: Vec<CmdBlock> = Vec::new();
        for (i, cmd) in p.cmds.iter().enumerate() {
            if i > 0 {
                blocks.push(Block::Connector(if p.pipe_err.get(i - 1).copied().unwrap_or(false) {
                    ConnKind::PipeErr
                } else {
                    ConnKind::Pipe
                }));
            }
            let mut env_i = env.clone();
            env_i.piped_in = env.piped_in || i > 0;
            let mut block = self.explain_cmd(cmd, &env_i, &p.raw);
            if i == 0 && p.negated {
                block.rows.insert(
                    0,
                    Row {
                        piece: "!".into(),
                        meaning: "inverts the result: success counts as failure and the other way round".into(),
                    },
                );
            }
            if i > 0 {
                let prev = &built[i - 1];
                if prev.fetches && block.stdin_script {
                    let part = format!("{} | {}", shorten(&prev.raw, 60), shorten(&block.raw, 60));
                    let root = block.chain.iter().any(|c| c == "sudo" || c == "doas");
                    self.warnings.push(Warning {
                        part,
                        context: String::new(),
                        text: format!(
                            "downloads a script from the internet and runs it immediately{}, without you seeing what it does first. If the server is hacked or the address is wrong, whatever it serves runs on this machine. Download it to a file, read it, then run it.",
                            if root { " as root (sudo)" } else { "" }
                        ),
                        risk: Risk::RunsInternetCode,
                    });
                }
            }
            built.push(block.clone());
            blocks.push(Block::Cmd(block));
        }
    }

    fn explain_cmd(&mut self, cmd: &Cmd, env: &Env, _pipeline_raw: &str) -> CmdBlock {
        match cmd {
            Cmd::Simple(s) => self.explain_simple(s, env),
            Cmd::Subshell(script, redirs) => {
                let mut b = compound("( ... )", "a group of commands in parentheses");
                b.rows.push(Row { piece: "( ... )".into(), meaning: "runs the commands inside in a subshell: a separate copy of the shell, so a cd or a variable set inside does not affect the rest of the line".into() });
                let inner = self.explain_script(script, env);
                b.inner.push(Inner { label: "inside the parentheses:".into(), blocks: inner });
                self.redir_rows(redirs, env, &mut b, "( ... )");
                b
            }
            Cmd::Group(script, redirs) => {
                let mut b = compound("{ ...; }", "a group of commands in braces");
                b.rows.push(Row { piece: "{ ...; }".into(), meaning: "runs the commands inside as one group in this shell; a redirection after the } applies to the whole group".into() });
                let inner = self.explain_script(script, env);
                b.inner.push(Inner { label: "inside the braces:".into(), blocks: inner });
                self.redir_rows(redirs, env, &mut b, "{ ...; }");
                b
            }
            Cmd::For { var, words, body, raw_head, redirs } => {
                let mut b = compound(raw_head, "a loop");
                let what = match words {
                    None => "each argument given to the script".to_string(),
                    Some(ws) if ws.len() == 1 && ws[0].glob && !ws[0].text.contains('/') => {
                        format!("each name in this folder matching the wildcard {}", ws[0].raw)
                    }
                    Some(ws) => {
                        let listed: Vec<String> = ws.iter().map(|w| w.raw.clone()).collect();
                        let glob = ws.iter().any(|w| w.glob);
                        format!(
                            "each of: {}{}",
                            shorten(&listed.join(" "), 60),
                            if glob {
                                " (a wildcard expands to every matching name, in this folder unless a path is given)"
                            } else {
                                ""
                            }
                        )
                    }
                };
                b.headline = format!("loop: repeats the commands inside once for {what}");
                b.rows.push(Row {
                    piece: raw_head.clone(),
                    meaning: format!(
                        "repeats the commands below once for {what}, setting ${var} to the current item each time"
                    ),
                });
                if let Some(ws) = words {
                    for w in ws {
                        self.sub_inners(w, env, &mut b);
                    }
                }
                let inner = self.explain_script(body, env);
                b.inner.push(Inner { label: "each time round, it runs:".into(), blocks: inner });
                self.compound_redirs(redirs, env, &mut b, "loop");
                b
            }
            Cmd::While { until, cond, body, raw_head, redirs } => {
                let mut b = compound(raw_head, "a loop");
                b.headline = format!(
                    "loop: repeats the commands inside {} the condition {}",
                    if *until { "until" } else { "while" },
                    if *until { "succeeds" } else { "keeps succeeding" }
                );
                b.rows.push(Row {
                    piece: shorten(raw_head, 60),
                    meaning: if *until {
                        "repeats the commands below until the condition command succeeds".into()
                    } else {
                        "repeats the commands below as long as the condition command succeeds".into()
                    },
                });
                let c = self.explain_script(cond, env);
                b.inner.push(Inner { label: "the condition:".into(), blocks: c });
                let inner = self.explain_script(body, env);
                b.inner.push(Inner { label: "each time round, it runs:".into(), blocks: inner });
                self.compound_redirs(redirs, env, &mut b, "loop");
                b
            }
            Cmd::If { branches, otherwise, redirs } => {
                let mut b = compound("if ...; then ...; fi", "a condition");
                let first_test = branches.first().and_then(|(c, _)| {
                    self.explain_script(c, env)
                        .into_iter()
                        .find_map(|b| if let Block::Cmd(c) = b { Some(c.raw) } else { None })
                });
                b.headline = match first_test {
                    Some(t) => format!(
                        "condition: runs different commands depending on whether `{}` succeeds",
                        shorten(&t, 50)
                    ),
                    None => "condition: runs different commands depending on whether a test succeeds".into(),
                };
                b.rows.push(Row {
                    piece: "if ... then ... fi".into(),
                    meaning: "runs the commands in the `then` part only when the `if` command succeeds".into(),
                });
                for (n, (cond, body)) in branches.iter().enumerate() {
                    let c = self.explain_script(cond, env);
                    b.inner.push(Inner {
                        label: if n == 0 { "the test (if):".into() } else { "otherwise, the next test (elif):".into() },
                        blocks: c,
                    });
                    let t = self.explain_script(body, env);
                    b.inner.push(Inner { label: "when it succeeds (then):".into(), blocks: t });
                }
                if let Some(o) = otherwise {
                    let e = self.explain_script(o, env);
                    b.inner.push(Inner { label: "when nothing matched (else):".into(), blocks: e });
                }
                self.compound_redirs(redirs, env, &mut b, "if ... fi");
                b
            }
            Cmd::Case { subject, arms, redirs } => {
                let mut b = compound("case ... esac", "a choice");
                b.headline =
                    format!("choice: runs different commands depending on what {} matches", shorten(&subject.raw, 30));
                b.rows.push(Row {
                    piece: "case ... esac".into(),
                    meaning: format!(
                        "compares {} against each pattern in turn and runs the commands of the first one that matches",
                        shorten(&subject.raw, 40)
                    ),
                });
                for arm in arms {
                    let blocks = self.explain_script(&arm.body, env);
                    b.inner.push(Inner { label: format!("if it matches {}:", arm.patterns.join(" or ")), blocks });
                }
                self.compound_redirs(redirs, env, &mut b, "case ... esac");
                b
            }
            Cmd::Func { name, body } => {
                let mut b = compound(&format!("{name}() {{ ... }}"), "a function definition");
                b.headline = format!("defines a function named {name} (nothing runs yet)");
                b.rows.push(Row {
                    piece: format!("{name}()"),
                    meaning: format!(
                        "defines a function named {name}; the body below only runs when {name} is called later"
                    ),
                });
                if name == ":" {
                    self.warnings.push(Warning {
                        part: ":(){ ... }".into(),
                        context: String::new(),
                        text: "this shape (a function named `:` that calls itself) is the classic fork bomb: it multiplies processes until the machine freezes.".into(),
                        risk: Risk::KillsWork,
                    });
                }
                let inner = self.explain_script(body, env);
                b.inner.push(Inner { label: "the function body:".into(), blocks: inner });
                b
            }
            Cmd::Test(text) => {
                let mut b = compound(text, "a test");
                b.headline = format!("test: checks a condition ({})", shorten(text, 40));
                b.rows.push(Row { piece: shorten(text, 60), meaning: "checks a condition about files, text or numbers; whether it succeeds decides what the next `&&` or `||` does".into() });
                b
            }
        }
    }

    // -----------------------------------------------------------------------
    // Words, substitutions, redirections
    // -----------------------------------------------------------------------

    fn sub_inners(&mut self, w: &Word, env: &Env, out: &mut CmdBlock) {
        for sub in &w.subs {
            let label = match &sub.kind {
                SubKind::Command | SubKind::Backtick => {
                    format!(
                        "{} runs first, and what it prints is pasted into the line{}:",
                        shorten(&sub.raw, 50),
                        if sub.in_quotes { " (as one word, because it is in quotes)" } else { "" }
                    )
                }
                SubKind::Process(dir) => format!(
                    "{} {}",
                    shorten(&sub.raw, 50),
                    if *dir == '<' {
                        "runs, and the command reads its output as if it were a file:"
                    } else {
                        "runs, and what the command writes goes into it as if it were a file:"
                    }
                ),
                SubKind::Arith => continue,
            };
            let blocks = self.explain_script(&sub.script, env);
            if !blocks.is_empty() {
                out.inner.push(Inner { label, blocks });
            }
        }
    }

    /// Adds notes about variables, wildcards and substitutions found in a word.
    fn decorate(&self, meaning: String, w: &Word) -> String {
        let mut notes: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        for v in &w.vars {
            if !seen.insert(v.name.clone()) {
                continue;
            }
            if let Some(d) = known_var(&v.name) {
                notes.push(format!("${} is {d}", v.name));
            } else if !v.guarded && !self.assigned.contains(&v.name) && !v.name.chars().all(|c| c.is_ascii_digit()) {
                notes.push(format!(
                    "${} is not set on this line: it may be empty, or come from your environment",
                    v.name
                ));
            }
        }
        if w.glob {
            notes.push("the shell expands the wildcard to all matching names before the program starts".into());
        }
        if w.text.starts_with("~/") || w.text == "~" {
            notes.push("~ is your home folder".into());
        }
        if w.subs.iter().any(|s| matches!(s.kind, SubKind::Command | SubKind::Backtick))
            && !meaning.contains("shown below")
        {
            notes.push("the value is what the command shown below prints".into());
        }
        if w.text == "." && !meaning.contains("current folder") {
            notes.push(". means the current folder".into());
        }
        if w.subs.iter().any(|s| matches!(s.kind, SubKind::Arith)) {
            notes.push("includes arithmetic that the shell calculates".into());
        }
        if notes.is_empty() { meaning } else { format!("{meaning} ({})", notes.join("; ")) }
    }

    /// Redirections after `done`, `fi` or `esac` belong to the whole construct.
    fn compound_redirs(&mut self, redirs: &[Redir], env: &Env, out: &mut CmdBlock, context: &str) {
        let before = out.rows.len();
        self.redir_rows(redirs, env, out, context);
        for r in &mut out.rows[before..] {
            r.meaning.push_str(" (applies to the whole construct, not to one command inside it)");
        }
    }

    fn redir_rows(&mut self, redirs: &[Redir], env: &Env, out: &mut CmdBlock, context: &str) {
        for r in redirs {
            let target = r.target.text.as_str();
            let fd = r.fd.as_deref();
            let stream = match fd {
                Some("2") => "error messages (stderr)",
                Some("1") | None => "normal output",
                Some(_) => "that stream",
            };
            let meaning = match r.op.as_str() {
                ">" | ">|" if target.starts_with('&') || false => {
                    format!("sends {stream} to the same place as descriptor {}", target.trim_start_matches('&'))
                }
                ">&" => {
                    let dest = if target == "1" {
                        "normal output"
                    } else if target == "2" {
                        "error output"
                    } else {
                        target
                    };
                    format!(
                        "sends {stream} to the same place as {dest}{}",
                        if target == "1" || target == "2" { "" } else { " (a file)" }
                    )
                }
                "&>" => {
                    format!("sends both normal output and error messages into `{target}`, replacing what was there")
                }
                "&>>" => format!("adds both normal output and error messages to the end of `{target}`"),
                ">" | ">|" if target == "/dev/null" => format!("throws away {stream}"),
                ">" | ">|" => format!("writes {stream} into `{target}`, replacing what was there"),
                ">>" if target == "/dev/null" => format!("throws away {stream}"),
                ">>" => format!("adds {stream} to the end of `{target}` (keeps what was there)"),
                "<" => format!("reads input from the file `{target}` instead of the keyboard"),
                "<<<" => format!("feeds the text {} to the command as its input", shorten(&r.target.raw, 40)),
                "<>" => format!("opens `{target}` for both reading and writing"),
                "<&" => format!("reads input from descriptor {target}"),
                "<<" | "<<-" => {
                    let doc = r.heredoc.as_ref();
                    let lines = doc.map_or(0, |d| d.body.lines().count());
                    let quoted = doc.is_some_and(|d| d.quoted);
                    let delim = doc.map_or(target, |d| d.delimiter.as_str());
                    let tabs = if doc.is_some_and(|d| d.strip_tabs) { " (leading tabs are stripped)" } else { "" };
                    let what = if lines == 0 {
                        "the lines of text that follow (none were included in this text)".to_string()
                    } else {
                        format!("the {lines} line(s) of text that follow")
                    };
                    let mut m = format!(
                        "feeds {what}, up to a line saying `{delim}`, to the command as its input{tabs}{}",
                        if quoted {
                            " (the delimiter is quoted, so nothing inside is expanded)"
                        } else {
                            " (without quotes on the delimiter, $variables and $(commands) inside are expanded)"
                        }
                    );
                    if let Some(d) = doc
                        && let Some(first) = d.body.lines().find(|l| !l.trim().is_empty())
                    {
                        m.push_str(&format!("; it starts with: {}", shorten(first.trim(), 40)));
                    }
                    m
                }
                other => format!("redirection {other} {target}"),
            };
            let piece =
                if r.op == "<<" || r.op == "<<-" { format!("{}{}", r.op, r.target.raw) } else { shorten(&r.raw, 40) };
            out.rows.push(Row { piece, meaning: self.decorate(meaning, &r.target) });
            self.sub_inners(&r.target, env, out);
            let raw = shorten(&r.raw, 40);
            risk::check_redir(&r.op, fd, target, &raw, context, &mut self.warnings);
        }
    }

    // -----------------------------------------------------------------------
    // Simple commands
    // -----------------------------------------------------------------------

    fn explain_simple(&mut self, s: &Simple, env: &Env) -> CmdBlock {
        let mut out = CmdBlock { raw: s.raw.clone(), ..CmdBlock::default() };
        for (name, value) in &s.assigns {
            let meaning = if s.words.is_empty() {
                format!("sets the shell variable {name}; it stays set for the rest of this shell session")
            } else {
                format!("sets {name} to {} for this one command only", shorten(&value.raw, 40))
            };
            out.rows.push(Row {
                piece: shorten(&format!("{name}={}", value.raw), 50),
                meaning: self.decorate(meaning, value),
            });
            if let Some((risk, text)) = risk_extra::assignment_warning(name, &value.text) {
                self.push_warning(Warning {
                    part: shorten(&format!("{name}=..."), 50),
                    context: shorten(&s.raw, 90),
                    text,
                    risk,
                });
            }
            self.sub_inners(value, env, &mut out);
        }
        if s.words.is_empty() {
            out.headline = if s.assigns.is_empty() {
                "does nothing but redirect".to_string()
            } else {
                format!("sets {}", s.assigns.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", "))
            };
            self.redir_rows(&s.redirs, env, &mut out, &s.raw);
            return out;
        }
        let prog_word = &s.words[0];
        if !prog_word.is_plain() {
            out.rows.push(Row {
                piece: shorten(&prog_word.raw, 50),
                meaning: "the program to run is worked out when the line runs, so it cannot be explained in advance"
                    .into(),
            });
            out.headline = "runs a program chosen at run time".into();
            self.sub_inners(prog_word, env, &mut out);
            self.redir_rows(&s.redirs, env, &mut out, &s.raw);
            return out;
        }
        let full = prog_word.text.clone();
        let prog = full.rsplit('/').next().unwrap_or(&full).to_string();
        let key = canonical_key(&prog);
        let kn = Knowledge::get();
        let words_text: Vec<&str> = s.words.iter().map(|w| w.text.as_str()).collect();
        let catalog_cmd = self.catalog_for(&prog, &words_text);
        let section = kn.section(std::slice::from_ref(&key));
        let man_summary = if section.is_none() && catalog_cmd.is_none() {
            self.man_page(&key).and_then(|p| p.summary.clone())
        } else {
            None
        };
        let mut desc = section
            .map(|s| s.desc.clone())
            .filter(|d| !d.is_empty())
            .or_else(|| catalog_cmd.map(|i| self.catalog[i].description.clone()))
            .or_else(|| man_summary.clone().map(|m| format!("{} (from the man page)", capitalize(&m))))
            .unwrap_or_default();
        let remote_or_container = env.remote.is_some() || env.container;
        let mut meaning = if desc.is_empty() {
            format!("`{prog}`: a program huh has no notes on, and it found no man page for it")
        } else {
            desc.clone()
        };
        if self.assigned.contains(&format!("fn:{prog}")) {
            meaning = format!("calls the shell function `{prog}` that this line defines");
            desc = format!("calls the function `{prog}` defined on this line");
        } else if full.contains('/') && section.is_none() {
            meaning =
                format!("runs the program or script at `{full}`; huh does not open it, so it cannot say what it does");
            desc = "runs a local program or script".to_string();
        } else if self.opts.path
            && !remote_or_container
            && !full.contains('/')
            && !BUILTINS.contains(&prog.as_str())
            && section.is_none_or(|s| !s.wrapper)
            && !self.installed(&prog)
        {
            meaning.push_str(&format!(" (`{prog}` was not found on this machine's PATH)"));
        }
        let prog_row = out.rows.len();
        out.rows.push(Row { piece: prog_word.raw.clone(), meaning });
        out.chain.push(prog.clone());
        out.fetches = matches!(prog.as_str(), "curl" | "wget");

        let mut inv = Inv {
            prog: key.clone(),
            path: vec![key.clone()],
            raw: shorten(&s.raw, 90),
            root: env.root,
            remote: env.remote.clone(),
            piped_in: env.piped_in,
            stdin_from: s
                .redirs
                .iter()
                .find(|r| matches!(r.op.as_str(), "<" | "<<<") && r.fd.as_deref().is_none_or(|f| f == "0"))
                .map(|r| {
                    if r.op == "<" { format!("`{}`", r.target.text) } else { "the text given on the line".to_string() }
                }),
            ..Inv::default()
        };
        let mut st = State {
            key: key.clone(),
            path: vec![key.clone()],
            desc: desc.clone(),
            pos_idx: 0,
            only_pos: false,
            catalog_cmd,
            pos0: None,
            script_given: false,
            first_positional_done: false,
            extra: Vec::new(),
        };
        let mut inner_headline: Option<(String, Vec<String>)> = None;
        let mut shell_strings: HashSet<usize> = HashSet::new();
        self.explain_args(&mut st, &s.words, env, &mut out, &mut inv, &mut inner_headline, &mut shell_strings);

        // `bash -c '...'`: the string is a command line of its own.
        if SHELLS.contains(&key.as_str())
            && let Some(code) = inv.value(&["-c"]).map(str::to_string)
        {
            let parsed = shell::parse(&code);
            for n in parsed.notes {
                self.note(format!("inside the -c string: {n}"));
            }
            let blocks = self.explain_script(&parsed.script, env);
            if let Some(h) = summarize_blocks(&blocks) {
                inner_headline = Some((h, Vec::new()));
            }
            out.inner.push(Inner { label: format!("{key} runs this command line:"), blocks });
        }
        // `bash <(curl ...)`, `eval "$(curl ...)"`: remote code through a substitution.
        if SHELLS.contains(&key.as_str())
            || matches!(key.as_str(), "eval" | "source" | ".")
            || INTERPRETERS.contains(&key.as_str())
        {
            for w in s.words.iter().skip(1) {
                for sub in &w.subs {
                    if script_fetches(&sub.script)
                        && matches!(sub.kind, SubKind::Process(_) | SubKind::Command | SubKind::Backtick)
                    {
                        self.warnings.push(Warning {
                            part: shorten(&w.raw, 60),
                            context: shorten(&s.raw, 90),
                            text: "downloads a script from the internet and runs it immediately, without you seeing what it does first. Download it to a file, read it, then run it.".into(),
                            risk: Risk::RunsInternetCode,
                        });
                    }
                }
            }
        }
        // Does this shell or interpreter read its program from stdin?
        if SHELLS.contains(&key.as_str()) || INTERPRETERS.contains(&key.as_str()) {
            let has_c = inv.has(&["-c"]);
            let has_s = inv.has(&["-s"]);
            let script_file = inv.pos.first().is_some_and(|p| p.text != "-") && !has_s;
            out.stdin_script = !has_c && !script_file;
            if out.stdin_script
                && env.piped_in
                && let Some(row) = out.rows.get_mut(prog_row)
            {
                row.meaning.push_str(" (here it runs, as a script, whatever text is piped into it)");
            }
        }

        // Substitutions in words that were not consumed as nested command lines.
        for (idx, w) in s.words.iter().enumerate() {
            if !shell_strings.contains(&idx) {
                self.sub_inners(w, env, &mut out);
            }
        }
        self.redir_rows(&s.redirs, env, &mut out, &shorten(&s.raw, 90));

        // Risk analysis for this command.
        let assigned = self.assigned.clone();
        let mut found = Vec::new();
        risk::check(&inv, &assigned, &mut found);
        for w in found {
            self.push_warning(w);
        }

        // Headline.
        let mut display =
            std::iter::once(prog.clone()).chain(st.path.iter().skip(1).cloned()).collect::<Vec<_>>().join(" ");
        if st.path.len() == 1 && full.contains('/') {
            display = full.clone();
        }
        for e in &st.extra {
            display.push(' ');
            display.push_str(e);
        }
        out.headline = match (&inner_headline, key.as_str()) {
            (Some((h, _)), "sudo" | "doas") => match inv.value(&["-u", "--user"]) {
                Some(u) if u != "root" => format!("as the user {u}: {h}"),
                _ => format!("as administrator: {h}"),
            },
            (
                Some((h, _)),
                "env" | "nohup" | "time" | "nice" | "ionice" | "setsid" | "stdbuf" | "command" | "exec" | "builtin"
                | "xargs",
            ) => format!("{key} runs {h}"),
            (Some((h, _)), "timeout") => format!("with a time limit: {h}"),
            (Some((h, _)), "watch") => format!("repeatedly: {h}"),
            (Some((h, _)), "ssh") => format!("on {}: {h}", env_host(&st)),
            (Some((h, _)), _) => format!("{display}: {}; it runs: {h}", st.desc.to_lowercase_first()),
            (None, _) => {
                if st.desc.is_empty() {
                    display.to_string()
                } else {
                    format!("{display}: {}", st.desc)
                }
            }
        };
        if let Some((_, chain)) = inner_headline {
            out.chain.extend(chain);
        }
        out
    }

    fn catalog_for(&self, prog: &str, words: &[&str]) -> Option<usize> {
        let mut best: Option<(usize, usize)> = None;
        for (idx, cmd) in self.catalog.iter().enumerate() {
            let Some((prefix, _, primary)) = crate::explain::template_stage_prefix(&cmd.template, prog) else {
                continue;
            };
            if !primary
                || prefix.is_empty()
                || words.len() < prefix.len()
                || !prefix.iter().enumerate().all(|(i, p)| words[i] == p)
            {
                continue;
            }
            if best.is_none_or(|(_, len)| prefix.len() > len) {
                best = Some((idx, prefix.len()));
            }
        }
        best.map(|(i, _)| i)
    }

    #[allow(clippy::too_many_arguments)]
    fn explain_args(
        &mut self,
        st: &mut State,
        words: &[Word],
        env: &Env,
        out: &mut CmdBlock,
        inv: &mut Inv,
        inner_headline: &mut Option<(String, Vec<String>)>,
        shell_strings: &mut HashSet<usize>,
    ) {
        let kn = Knowledge::get();
        let prog_section_single = kn.section(std::slice::from_ref(&st.key)).is_some_and(|s| s.single);
        let prog_oldstyle = kn.section(std::slice::from_ref(&st.key)).is_some_and(|s| s.oldstyle);
        let mut i = 1;
        while i < words.len() {
            let w = &words[i];
            let t = w.text.as_str();
            if !st.only_pos && t == "--" && !w.quoted {
                out.rows.push(Row {
                    piece: "--".into(),
                    meaning: "end of options: everything after this is a plain argument, even if it starts with a dash"
                        .into(),
                });
                st.only_pos = true;
                i += 1;
                continue;
            }
            let section = kn.section(&st.path);
            let is_first_only =
                section.is_some_and(|s| s.first) || kn.section(std::slice::from_ref(&st.key)).is_some_and(|s| s.first);
            let _ = is_first_only;
            let dash_opt = t.starts_with('-') && t.len() > 1;
            let plus_opt = st.key == "dig" && t.starts_with('+');
            let chmod_mode_word = st.key == "chmod"
                && st.pos_idx == 0
                && dash_opt
                && Knowledge::get().find_opt(&st.path, t).is_none()
                && decode::chmod_mode(t).is_some();
            if !st.only_pos && (dash_opt || plus_opt) && !chmod_mode_word {
                // find's -exec family swallows a whole command.
                if st.key == "find" && matches!(t, "-exec" | "-execdir" | "-ok" | "-okdir") {
                    i = self.find_exec(st, words, i, env, out, inv);
                    continue;
                }
                i += self.explain_option(st, words, i, prog_section_single, env, out, inv);
                continue;
            }
            // Positional.
            if st.key == "find" && matches!(t, "(" | ")" | "!") && !w.quoted {
                let meaning = match t {
                    "(" => "start a group of tests",
                    ")" => "end the group of tests",
                    _ => "invert the next test",
                };
                out.rows.push(Row { piece: w.raw.clone(), meaning: meaning.into() });
                i += 1;
                continue;
            }
            if matches!(st.key.as_str(), "env" | "export") && is_assignment(&w.raw) {
                let (name, value) = w.text.split_once('=').unwrap_or((&w.text, ""));
                if let Some((risk, text)) = risk_extra::assignment_warning(name, value) {
                    self.push_warning(Warning {
                        part: shorten(&format!("{name}=..."), 50),
                        context: shorten(&out.raw, 90),
                        text,
                        risk,
                    });
                }
            }
            if st.key == "env" && is_assignment(&w.raw) {
                out.rows.push(Row {
                    piece: shorten(&w.raw, 50),
                    meaning: self.decorate(
                        format!("sets the variable {} for the command", w.raw.split('=').next().unwrap_or("")),
                        w,
                    ),
                });
                i += 1;
                continue;
            }
            // Old-style bundles: `tar xzf x.tgz`, `ps aux`.
            if prog_oldstyle
                && !st.first_positional_done
                && st.pos_idx == 0
                && t.chars().all(|c| c.is_ascii_alphabetic())
                && !t.is_empty()
            {
                let all_known = t.chars().all(|c| self.lookup_old(st, c).is_some());
                if all_known {
                    st.first_positional_done = true;
                    let consumed = self.explain_bundle(st, words, i, t, true, env, out, inv);
                    i += consumed;
                    continue;
                }
            }
            st.first_positional_done = true;
            // Subcommands.
            if has_children(kn, &st.path) {
                let mut next = st.path.clone();
                next.push(sub_alias(&st.key, t).to_string());
                if let Some(sec) = kn.section(&next) {
                    out.rows.push(Row { piece: w.raw.clone(), meaning: capitalize(&sec.desc) });
                    st.path = next;
                    st.desc = sec.desc.clone();
                    inv.path = st.path.clone();
                    st.pos_idx = 0;
                    i += 1;
                    continue;
                } else if st.path.len() == 1 && !t.contains(['/', '=']) && w.is_plain() {
                    let name = format!("{}-{}", st.key, t);
                    let summary = self.man_page(&name).and_then(|p| p.summary.clone());
                    let meaning = match summary {
                        Some(s) => format!("the `{} {t}` subcommand: {} (from the man page)", st.key, s),
                        None => format!("a `{}` subcommand that is not in huh's notes", st.key),
                    };
                    out.rows.push(Row { piece: w.raw.clone(), meaning });
                    st.path.push(t.to_string());
                    inv.path = st.path.clone();
                    st.pos_idx = 0;
                    i += 1;
                    continue;
                }
            }
            i += self.explain_positional(st, words, i, env, out, inv, inner_headline, shell_strings);
            if inner_headline.is_some() && st.only_pos && false {
                break;
            }
            if st.pos_idx == usize::MAX {
                break;
            }
        }
    }

    fn lookup_old(&mut self, st: &State, c: char) -> Option<Found> {
        let kn = Knowledge::get();
        let bare = kn.find_opt(&st.path, &c.to_string());
        let dashed = kn.find_opt(&st.path, &format!("-{c}"));
        bare.or(dashed).map(|o| Found {
            meaning: o.meaning.clone(),
            arg: o.arg.clone(),
            names: o.names.clone(),
            from_man: false,
        })
    }

    /// Looks up an option: curated table, then catalog, then the man page.
    fn lookup(&mut self, st: &State, flag: &str) -> Option<Found> {
        let kn = Knowledge::get();
        if let Some(o) = kn.find_opt(&st.path, flag) {
            return Some(Found {
                meaning: o.meaning.clone(),
                arg: o.arg.clone(),
                names: o.names.clone(),
                from_man: false,
            });
        }
        if let Some(idx) = st.catalog_cmd
            && let Some(f) = self.catalog[idx].flags.iter().find(|f| f.value == flag)
        {
            return Some(Found {
                meaning: crate::explain::question_to_statement(&f.question),
                arg: ArgKind::None,
                names: vec![flag.to_string()],
                from_man: false,
            });
        }
        for name in Self::man_names(&st.path) {
            if let Some(page) = self.man_page(&name)
                && let Some(entry) = page.lookup(flag)
            {
                let arg = match &entry.arg {
                    Some(a) if a.starts_with('[') => ArgKind::Optional(a.trim_matches(['[', ']', '=']).to_string()),
                    Some(a) => ArgKind::Required(a.clone()),
                    None => ArgKind::None,
                };
                return Some(Found { meaning: entry.desc.clone(), arg, names: entry.names.clone(), from_man: true });
            }
        }
        None
    }

    fn render_meaning(&self, found: &Found, value: Option<&str>) -> String {
        let mut meaning = found.meaning.trim_end_matches('.').to_string();
        let label = match &found.arg {
            ArgKind::Required(l) | ArgKind::Optional(l) => Some(l.as_str()),
            ArgKind::None => None,
        };
        if let (Some(label), Some(value)) = (label, value) {
            let shown = if value.chars().count() > 40 { "the text given".to_string() } else { value.to_string() };
            if !found.from_man && replace_word(&mut meaning, label, &shown) {
            } else if value.chars().count() <= 60 && !value.contains('$') {
                meaning.push_str(&format!(" (given: {shown})"));
            }
        }
        if found.from_man {
            meaning.push_str(" (from the man page)");
        }
        capitalize(&meaning)
    }

    #[allow(clippy::too_many_arguments)]
    fn explain_option(
        &mut self,
        st: &mut State,
        words: &[Word],
        i: usize,
        single: bool,
        env: &Env,
        out: &mut CmdBlock,
        inv: &mut Inv,
    ) -> usize {
        let w = &words[i];
        let t = w.text.clone();
        // head -20, tail -5
        if matches!(st.key.as_str(), "head" | "tail") && t.len() > 1 && t[1..].chars().all(|c| c.is_ascii_digit()) {
            let which = if st.key == "head" { "first" } else { "last" };
            out.rows.push(Row {
                piece: w.raw.clone(),
                meaning: format!("show the {which} {} lines (same as -n {})", &t[1..], &t[1..]),
            });
            inv.flags.push("-n".into());
            return 1;
        }
        // kill -9, kill -KILL
        if matches!(st.key.as_str(), "kill" | "killall" | "pkill")
            && Knowledge::get().find_opt(&st.path, &t).is_none()
            && decode::signal(&t[1..]).is_some()
        {
            let sig = decode::signal(&t[1..]).expect("checked");
            out.rows.push(Row {
                piece: w.raw.clone(),
                meaning: format!("send signal {} ({}): {}", sig.number, sig.name, sig.meaning),
            });
            inv.raw_flags.push((if sig.number == 9 { "-9".into() } else { t.clone() }, w.raw.clone()));
            inv.flags.push(t.clone());
            if sig.number == 9 {
                inv.raw_flags.push(("-KILL".into(), w.raw.clone()));
            }
            return 1;
        }
        let long = t.starts_with("--");
        if long {
            let (flag, attached) = match t.split_once('=') {
                Some((f, v)) => (f.to_string(), Some(v.to_string())),
                None => (t.clone(), None),
            };
            return self.apply_flag(st, words, i, &flag, attached, None, env, out, inv);
        }
        if t.starts_with('+') {
            return self.apply_flag(st, words, i, &t, None, None, env, out, inv);
        }
        // Single dash: an exact known option first (`-name`, `-it`, `-ef`).
        if self.lookup(st, &t).is_some() {
            return self.apply_flag(st, words, i, &t, None, None, env, out, inv);
        }
        let body = t[1..].to_string();
        if single || !self.can_split(st, &body) {
            // Single-dash long options (`-auto-approve`, `-race`, `-jar`): keep the token whole.
            let (flag, attached) = match t.split_once('=') {
                Some((f, v)) => (f.to_string(), Some(v.to_string())),
                None => (t.clone(), None),
            };
            return self.apply_flag(st, words, i, &flag, attached, None, env, out, inv);
        }
        self.explain_bundle(st, words, i, &body, false, env, out, inv)
    }

    /// May `-abc` be read as the separate short options -a -b -c? Only when the program is in
    /// the curated table as a getopt-style program, or when every letter is a known short
    /// option of it (curated, catalog or man page). Otherwise the token is kept whole.
    fn can_split(&mut self, st: &State, body: &str) -> bool {
        if body.chars().count() <= 1 {
            return true;
        }
        let kn = Knowledge::get();
        if kn.section(std::slice::from_ref(&st.key)).is_some() {
            return true;
        }
        let letters: Vec<char> = body.chars().collect();
        // An attached value (`-p"secret"`, `-j4`) is fine once its first letter is known and takes one.
        let first = format!("-{}", letters[0]);
        if let Some(f) = self.lookup(st, &first)
            && !matches!(f.arg, ArgKind::None)
        {
            return true;
        }
        letters.iter().all(|c| c.is_ascii_alphanumeric() && self.lookup(st, &format!("-{c}")).is_some())
    }

    /// Splits `-xzvf` into single options (or `xzvf` for old-style tar).
    #[allow(clippy::too_many_arguments)]
    fn explain_bundle(
        &mut self,
        st: &mut State,
        words: &[Word],
        i: usize,
        body: &str,
        old: bool,
        env: &Env,
        out: &mut CmdBlock,
        inv: &mut Inv,
    ) -> usize {
        let chars: Vec<char> = body.chars().collect();
        let mut consumed = 1;
        let mut idx = 0;
        while idx < chars.len() {
            let c = chars[idx];
            let flag = format!("-{c}");
            let found = if old { self.lookup_old(st, c) } else { self.lookup(st, &flag) };
            let rest: String = chars[idx + 1..].iter().collect();
            let takes = found.as_ref().map(|f| f.arg.clone()).unwrap_or(ArgKind::None);
            let attached = match &takes {
                ArgKind::Required(_) | ArgKind::Optional(_) if !rest.is_empty() => {
                    Some(rest.trim_start_matches('=').to_string())
                }
                _ => None,
            };
            let used_rest = attached.is_some();
            let lookup_flag = if old && found.as_ref().is_some_and(|f| f.names.contains(&c.to_string())) {
                c.to_string()
            } else {
                flag.clone()
            };
            let extra = self.apply_flag_found(
                st,
                words,
                i,
                &lookup_flag,
                &(if old { c.to_string() } else { flag.clone() }),
                attached,
                found,
                env,
                out,
                inv,
                true,
            );
            consumed = consumed.max(extra);
            if used_rest || matches!(takes, ArgKind::Required(_)) {
                break;
            }
            idx += 1;
        }
        consumed
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_flag(
        &mut self,
        st: &mut State,
        words: &[Word],
        i: usize,
        flag: &str,
        attached: Option<String>,
        _unused: Option<()>,
        env: &Env,
        out: &mut CmdBlock,
        inv: &mut Inv,
    ) -> usize {
        let found = self.lookup(st, flag);
        self.apply_flag_found(st, words, i, flag, flag, attached, found, env, out, inv, false)
    }

    /// Builds the row for one option (with its value) and records it. Returns words consumed.
    #[allow(clippy::too_many_arguments)]
    fn apply_flag_found(
        &mut self,
        st: &mut State,
        words: &[Word],
        i: usize,
        flag: &str,
        display_flag: &str,
        attached: Option<String>,
        found: Option<Found>,
        _env: &Env,
        out: &mut CmdBlock,
        inv: &mut Inv,
        bundled: bool,
    ) -> usize {
        let w = &words[i];
        let mut consumed = 1;
        let mut value: Option<String> = attached.clone();
        let mut value_word: Option<&Word> = None;
        let requires = matches!(found.as_ref().map(|f| &f.arg), Some(ArgKind::Required(_)))
            || (found.is_none()
                && risk::SECRET_OPTIONS.contains(&flag)
                && !flag.contains('=')
                && words.get(i + 1).is_some_and(|n| !n.text.starts_with('-')));
        if requires && value.is_none() && i + 1 < words.len() && !st.only_pos {
            let next = &words[i + 1];
            value = Some(next.text.clone());
            value_word = Some(next);
            consumed = 2;
        }
        let piece = match (&attached, value_word) {
            (_, Some(vw)) => format!("{display_flag} {}", shorten(&vw.raw, 50)),
            (Some(_), None)
                if bundled
                    && w.raw.starts_with(display_flag)
                    && (w.raw.contains(['"', '\''])
                        || matches!(found.as_ref().map(|f| &f.arg), Some(ArgKind::Optional(_)))) =>
            {
                shorten(&w.raw, 60)
            }
            (Some(a), None) if bundled => format!("{display_flag} {}", shorten(a, 50)),
            _ if bundled => display_flag.to_string(),
            _ => shorten(&w.raw, 60),
        };
        let mut meaning;
        match &found {
            Some(f) => {
                inv.flags.extend(f.names.iter().cloned());
                if let Some((replace, note)) = self.value_note(st, flag, value.as_deref()) {
                    meaning =
                        if replace { capitalize(&note) } else { format!("{}: {note}", self.render_meaning(f, None)) };
                } else {
                    meaning = self.render_meaning(f, value.as_deref());
                }
            }
            None => {
                let man_exists =
                    Self::man_names(&st.path).iter().any(|n| self.opts.man && manpage::find_page(n).is_some());
                meaning = if man_exists {
                    format!("an option of `{}` that is neither in huh's notes nor in its man page", st.path.join(" "))
                } else {
                    format!("an option of `{}` that huh does not know", st.path.join(" "))
                };
                if words[i].raw.contains(['"', '\'']) && flag.starts_with('-') && !flag.starts_with("--") {
                    meaning.push_str(" (kept whole, quotes included, so its value is not torn into separate letters)");
                } else if flag.starts_with('-') && !flag.starts_with("--") && flag.chars().count() > 2 {
                    meaning.push_str(
                        " (written with one dash but longer than one letter, so huh kept it whole instead of reading it as several short options)",
                    );
                }
            }
        }
        inv.flags.push(flag.to_string());
        for n in found.as_ref().map(|f| f.names.clone()).unwrap_or_default() {
            inv.raw_flags.push((n, shorten(&piece, 60)));
        }
        inv.raw_flags.push((flag.to_string(), shorten(&piece, 60)));
        if let Some(v) = &value {
            inv.values.push((flag.to_string(), v.clone()));
            for n in found.as_ref().map(|f| f.names.clone()).unwrap_or_default() {
                inv.values.push((n, v.clone()));
            }
        }
        if matches!(flag, "-e" | "-f" | "--expression" | "--file") && matches!(st.key.as_str(), "sed" | "awk")
            || (st.key == "awk" && flag == "-f")
        {
            st.script_given = true;
        }
        let vw = value_word.unwrap_or(w);
        meaning = self.decorate(meaning, vw);
        out.rows.push(Row { piece, meaning });
        consumed
    }

    /// Value-specific wording: returns (replace_whole_meaning, text).
    fn value_note(&self, st: &State, flag: &str, value: Option<&str>) -> Option<(bool, String)> {
        let value = value?;
        match (st.key.as_str(), flag) {
            ("find", "-mtime" | "-atime" | "-ctime" | "-mmin" | "-amin" | "-cmin") => {
                decode::find_time(flag, value).map(|t| (true, t))
            }
            ("find", "-size") => decode::find_size(value).map(|t| (true, t)),
            ("find", "-type") => decode::find_type(value).map(|t| (true, t)),
            ("docker", "-p" | "--publish") => decode::port_mapping(value).map(|t| (false, t)),
            ("docker", "-v" | "--volume") => decode::volume_spec(value).map(|t| (false, t)),
            ("kill" | "killall" | "pkill", "-s" | "--signal") => {
                decode::signal(value).map(|s| (true, format!("send signal {} ({}): {}", s.number, s.name, s.meaning)))
            }
            ("go", "-count") => Some((
                true,
                match value {
                    "1" => "run each test once and turn off result caching".to_string(),
                    n => format!("run each test {n} times (a number other than 0 also turns off result caching)"),
                },
            )),
            ("git", "--depth") => Some((
                true,
                match value {
                    "1" => "only download the latest commit, with no earlier history".to_string(),
                    n => format!("only download the latest {n} commits of history"),
                },
            )),
            ("mysql" | "mariadb", "-p" | "--password") => {
                Some((true, format!("the password is `{value}`, written right after -p with no space")))
            }
            ("mysql" | "mariadb", "-e" | "--execute") | ("psql", "-c" | "--command") => {
                Some((true, format!("run this SQL, then exit: {}", shorten(value, 60))))
            }
            ("timeout", "-s" | "--signal") => decode::signal(value)
                .map(|s| (true, format!("send signal {} ({}) when the time is up", s.number, s.name))),
            _ => None,
        }
    }

    fn find_exec(
        &mut self,
        st: &mut State,
        words: &[Word],
        i: usize,
        env: &Env,
        out: &mut CmdBlock,
        inv: &mut Inv,
    ) -> usize {
        let name = words[i].text.clone();
        let mut j = i + 1;
        while j < words.len()
            && !(words[j].text == ";" || words[j].text == "+" && j > i + 1 && words[j - 1].text == "{}")
        {
            j += 1;
        }
        let cmd_words = &words[i + 1..j.min(words.len())];
        let end = words.get(j);
        let terminator = end.map(|w| w.raw.clone()).unwrap_or_default();
        let each = match terminator.as_str() {
            "+" => "as few runs as possible, with many matches in each",
            _ => "once for each match",
        };
        let verb = if name.contains("ok") { "asks first, then runs" } else { "runs" };
        let piece = shorten(
            &format!("{name} {} {terminator}", cmd_words.iter().map(|w| w.raw.clone()).collect::<Vec<_>>().join(" ")),
            60,
        );
        out.rows.push(Row {
            piece,
            meaning: format!(
                "{verb} the command shown below {each}; {{}} stands for the matched path{}",
                if name.ends_with("dir") { ", and it runs from the match's own folder" } else { "" }
            ),
        });
        inv.flags.push(name.clone());
        if !cmd_words.is_empty() {
            let simple = Simple {
                assigns: vec![],
                words: cmd_words.to_vec(),
                redirs: vec![],
                raw: cmd_words.iter().map(|w| w.raw.clone()).collect::<Vec<_>>().join(" "),
            };
            let block = self.explain_simple(&simple, env);
            out.inner.push(Inner {
                label: format!("{}:", if each.starts_with("once") { "for each match, find runs" } else { "find runs" }),
                blocks: vec![Block::Cmd(block)],
            });
        }
        st.pos_idx = st.pos_idx.max(1);
        (j + 1).min(words.len().max(j + 1))
    }

    #[allow(clippy::too_many_arguments)]
    fn explain_positional(
        &mut self,
        st: &mut State,
        words: &[Word],
        i: usize,
        env: &Env,
        out: &mut CmdBlock,
        inv: &mut Inv,
        inner_headline: &mut Option<(String, Vec<String>)>,
        shell_strings: &mut HashSet<usize>,
    ) -> usize {
        let kn = Knowledge::get();
        let w = &words[i];
        let t = w.text.clone();
        let section: Option<&Section> = kn.section(&st.path);
        let idx = st.pos_idx;
        st.pos_idx += 1;
        if st.pos_idx == 1 {
            st.pos0 = Some(t.clone());
        }
        let role = section.and_then(|s| {
            let r = s.roles.get(idx).or_else(|| {
                s.roles.last().filter(|r| r.repeat || matches!(r.kind, RoleKind::Argv(_) | RoleKind::Shell(_)))
            });
            r.cloned()
        });
        inv.pos.push(word_to_pos(w));

        // `=word` meanings (systemctl restart, ...).
        if idx == 0
            && let Some(s) = section
            && let Some((_, m)) = s.words.iter().find(|(word, _)| *word == t)
        {
            out.rows.push(Row { piece: w.raw.clone(), meaning: capitalize(m) });
            st.extra.push(t.clone());
            st.desc = capitalize(m);
            return 1;
        }
        if st.key == "trap" {
            let events: Vec<String> = words[1..]
                .iter()
                .filter(|w| !w.text.starts_with('-') || w.text.len() > 1)
                .skip(1)
                .map(|w| trap_event(&w.text))
                .collect();
            if idx == 0 {
                if t.is_empty() {
                    out.rows.push(Row {
                        piece: shorten(&w.raw, 60),
                        meaning: "an empty handler: the signals that follow are ignored".into(),
                    });
                    return 1;
                }
                if t == "-" {
                    out.rows.push(Row {
                        piece: "-".into(),
                        meaning: "removes the handler: the signals that follow go back to their default action".into(),
                    });
                    return 1;
                }
                let parsed = shell::parse(&t);
                for n in parsed.notes {
                    self.note(format!("in the trap command: {n}"));
                }
                out.rows.push(Row {
                    piece: shorten(&w.raw, 60),
                    meaning: "a command line that is not run now: the shell stores it and runs it later, when the event named after it happens; explained below".into(),
                });
                let blocks = self.explain_script(&parsed.script, env);
                if let Some(h) = summarize_blocks(&blocks) {
                    *inner_headline = Some((h, Vec::new()));
                }
                st.desc = "Stores a command that runs later, when the event named happens (not now)".to_string();
                let when = if events.is_empty() { "the event named".to_string() } else { events.join(" or ") };
                out.inner.push(Inner { label: format!("later, when {when}, the shell runs:"), blocks });
                return 1;
            }
            out.rows.push(Row { piece: w.raw.clone(), meaning: capitalize(&trap_event(&t)) });
            return 1;
        }
        if st.key == "su" && t == "-" && idx == 0 {
            out.rows.push(Row {
                piece: "-".into(),
                meaning: "not a user name: `-` means start a login shell, with the environment that user would get when logging in (a user name, if any, comes after it)".into(),
            });
            return 1;
        }
        if st.key == "dig" {
            let meaning = if let Some(server) = t.strip_prefix('@') {
                Some(format!("ask the DNS server {server} instead of the default one"))
            } else if matches!(
                t.as_str(),
                "A" | "AAAA" | "MX" | "NS" | "TXT" | "CNAME" | "SOA" | "ANY" | "PTR" | "SRV" | "CAA"
            ) {
                Some(format!("the kind of record to ask for ({t})"))
            } else {
                None
            };
            if let Some(m) = meaning {
                out.rows.push(Row { piece: w.raw.clone(), meaning: capitalize(&m) });
                return 1;
            }
        }
        if st.key == "test" {
            let m = match t.as_str() {
                "]" => Some("the closing bracket of the test"),
                "!" => Some("inverts the test that follows"),
                "=" | "==" => Some("the two texts are equal"),
                "!=" => Some("the two texts differ"),
                _ => None,
            };
            if let Some(m) = m {
                out.rows.push(Row { piece: w.raw.clone(), meaning: capitalize(m) });
                return 1;
            }
        }
        if matches!(st.key.as_str(), "cp" | "mv") && idx >= 1 && i + 1 == words.len() {
            out.rows.push(Row {
                piece: shorten(&w.raw, 60),
                meaning: self.decorate("where it goes: the destination (a folder, or the new name)".to_string(), w),
            });
            return 1;
        }
        if matches!(st.key.as_str(), "export" | "declare" | "local" | "readonly")
            && let Some((name, _)) = t.split_once('=')
        {
            let m = format!(
                "sets the variable {name}{}",
                if st.key == "export" { " so that programs started from here can see it" } else { "" }
            );
            out.rows.push(Row { piece: shorten(&w.raw, 60), meaning: self.decorate(m, w) });
            return 1;
        }
        // key=value operands (dd).
        if section.is_some_and(|s| s.kv)
            && let Some((k, v)) = t.split_once('=')
        {
            let key = format!("{k}=");
            if let Some(o) = kn.find_kv(&st.path, &key) {
                let mut meaning = capitalize(&o.meaning);
                if st.key == "dd" && key == "of=" {
                    if v.starts_with("/dev/") && !v.starts_with("/dev/null") {
                        meaning.push_str(&format!(": {v} is a device, so this writes straight onto the hardware"));
                    } else {
                        meaning.push_str(&format!(": {v}"));
                    }
                } else if st.key == "dd" && key == "if=" {
                    match v {
                        "/dev/zero" => meaning.push_str(": /dev/zero is an endless stream of zero bytes"),
                        "/dev/urandom" | "/dev/random" => {
                            meaning.push_str(&format!(": {v} is an endless stream of random bytes"))
                        }
                        _ => meaning.push_str(&format!(": {v}")),
                    }
                } else if st.key == "dd" && key == "bs=" {
                    meaning.push_str(&format!(": {v} per block"));
                } else if matches!(key.as_str(), "count=" | "skip=" | "seek=") {
                    meaning.push_str(&format!(": {v}"));
                }
                out.rows.push(Row { piece: shorten(&w.raw, 50), meaning: self.decorate(meaning, w) });
                return 1;
            }
        }
        let Some(role) = role else {
            let m = generic_arg(&t, &st.key);
            out.rows.push(Row { piece: shorten(&w.raw, 60), meaning: self.decorate(m, w) });
            return 1;
        };
        match &role.kind {
            RoleKind::Argv(ctx) => {
                let rest = &words[i..];
                let raw = rest.iter().map(|w| w.raw.clone()).collect::<Vec<_>>().join(" ");
                let simple = Simple { assigns: vec![], words: rest.to_vec(), redirs: vec![], raw: raw.clone() };
                let mut env2 = env.clone();
                let label: String = match (st.key.as_str(), ctx.as_str()) {
                    ("sudo" | "doas", _) => {
                        let as_user = inv.value(&["-u", "--user"]).map(str::to_string);
                        env2.root = as_user.as_deref().is_none_or(|u| u == "root");
                        match as_user {
                            Some(u) if u != "root" => format!("as the user {u}, it runs:"),
                            _ => "with administrator (root) rights, it runs:".to_string(),
                        }
                    }
                    ("xargs", _) => {
                        env2.piped_in = true;
                        let src = match &inv.stdin_from {
                            Some(f) => format!("the items read from {f}"),
                            None => "the items read from standard input".to_string(),
                        };
                        match inv.value(&["-I", "-i", "--replace"]) {
                            Some(r) => format!("for each of {src}, which replaces {r} in the command, xargs runs:"),
                            None => format!("for {src} (added to the end of its arguments), xargs runs:"),
                        }
                    }
                    ("env", _) => "with that environment, it runs:".into(),
                    ("nohup", _) => "immune to the terminal closing, it runs:".into(),
                    ("time", _) => "and reports how long this takes:".into(),
                    ("timeout", _) => "and stops it if it runs too long:".into(),
                    (_, "container") => {
                        env2.container = true;
                        "inside the container, it runs:".into()
                    }
                    _ => "it runs:".into(),
                };
                let block = self.explain_simple(&simple, &env2);
                inv.inner_raw = Some(shorten(&raw, 90));
                inv.inner_prog = rest.first().map(|w| w.text.rsplit('/').next().unwrap_or("").to_string());
                *inner_headline = Some((block.headline.clone(), block.chain.clone()));
                out.stdin_script = block.stdin_script;
                out.fetches = out.fetches || block.fetches;
                out.inner.push(Inner { label, blocks: vec![Block::Cmd(block)] });
                st.pos_idx = usize::MAX;
                return rest.len();
            }
            RoleKind::Shell(ctx) => {
                let rest = &words[i..];
                for k in i..words.len() {
                    shell_strings.insert(k);
                }
                let text: String = rest.iter().map(|w| w.text.clone()).collect::<Vec<_>>().join(" ");
                let raw: String = rest.iter().map(|w| w.raw.clone()).collect::<Vec<_>>().join(" ");
                let mut env2 = env.clone();
                let label;
                let where_text;
                match ctx.as_str() {
                    "remote" => {
                        let host = st.pos0.clone().unwrap_or_default();
                        let host = host.rsplit('@').next().unwrap_or("").to_string();
                        where_text = format!("on the remote computer {host}");
                        label = format!("{where_text}, ssh runs this command line (not on this machine):");
                        env2.remote = Some(host);
                        env2.root = false;
                    }
                    "container" => {
                        where_text = "inside the container".into();
                        label = "inside the container, it runs:".into();
                        env2.container = true;
                    }
                    _ => {
                        where_text = "here".into();
                        label = if st.key == "watch" {
                            "every couple of seconds, watch runs:".into()
                        } else {
                            "it runs this command line:".into()
                        };
                    }
                }
                let parsed = shell::parse(&text);
                for n in parsed.notes {
                    self.note(format!("in `{}`: {n}", shorten(&raw, 40)));
                }
                out.rows.push(Row {
                    piece: shorten(&raw, 60),
                    meaning: format!("a command line that runs {where_text}; explained below"),
                });
                let blocks = self.explain_script(&parsed.script, &env2);
                if let Some(h) = summarize_blocks(&blocks) {
                    *inner_headline = Some((h, Vec::new()));
                }
                inv.inner_raw = Some(shorten(&text, 90));
                out.inner.push(Inner { label, blocks });
                st.pos_idx = usize::MAX;
                return rest.len();
            }
            RoleKind::Plain => {}
        }
        let text_role = role.text.as_str();
        let shell_arg = SHELLS.contains(&st.key.as_str()) && (inv.has(&["-s"]) || inv.has(&["-c"]));
        let mut meaning = match text_role {
            _ if shell_arg && inv.has(&["-c"]) && idx == 0 => {
                "the name the command string uses for itself ($0)".to_string()
            }
            _ if shell_arg => "an argument passed to the script".to_string(),
            "@mode" => match decode::chmod_mode(&t) {
                Some(n) => format!("the new permissions: {n}"),
                None => "the new permissions (a number like 755, or letters like u+x)".to_string(),
            },
            "@owner" => match decode::owner_spec(&t) {
                Some(n) => format!("the new owner and group: {n}"),
                None => "the new owner (and group), written user:group".to_string(),
            },
            "@sedscript" if !st.script_given => match decode::sed_script(&t) {
                Some(n) => format!("the sed script: {n}"),
                None => "the sed editing script (what to change on each line)".to_string(),
            },
            "@sedscript" => "the file to edit (or standard input)".to_string(),
            "@awkprogram" if !st.script_given => match decode::awk_program(&t) {
                Some(n) => format!("the awk program: {n}"),
                None => "the awk program: what to do with each line".to_string(),
            },
            "@awkprogram" => "the file to read (or standard input)".to_string(),
            "@tarmember" => {
                if inv.has(&["-c", "--create"]) {
                    "a file or folder to put into the archive".to_string()
                } else if inv.has(&["-x", "--extract", "--get"]) {
                    "a file or folder inside the archive to unpack (everything is unpacked if none is named)"
                        .to_string()
                } else if inv.has(&["-t", "--list"]) {
                    "a file or folder inside the archive to list".to_string()
                } else {
                    "a file or folder to include".to_string()
                }
            }
            "@pid" => {
                if w.subs.iter().any(|s| matches!(s.kind, SubKind::Command | SubKind::Backtick)) {
                    "the process ID(s) printed by the command shown below".to_string()
                } else if t.chars().all(|c| c.is_ascii_digit()) {
                    format!("the process ID {t}")
                } else if t.starts_with('%') {
                    "a job of this shell".to_string()
                } else {
                    "a process ID".to_string()
                }
            }
            "" => generic_arg(&t, &st.key),
            other => capitalize(other),
        };
        if st.key == "make" {
            meaning = if t.contains('=') {
                "sets the Makefile variable".to_string()
            } else {
                format!("the target to build: runs the `{t}` recipe from the Makefile")
            };
        }
        if st.key == "git"
            && let Some(note) = decode::git_ref(&t)
        {
            meaning.push_str(&format!(" ({note})"));
        }
        if st.key == "ssh" && idx == 0 {
            meaning = format!(
                "the computer to log in to{}",
                if t.contains('@') { " (user@host: the part before @ is the account)" } else { "" }
            );
        }
        if st.key == "docker" && st.path.get(1).is_some_and(|p| p == "run") && idx == 0 {
            meaning = format!(
                "the image to start the container from ({t}{})",
                if t.contains(':') { "" } else { ", latest version unless a :tag is given" }
            );
        }
        if matches!(st.key.as_str(), "scp" | "rsync")
            && t.contains(':')
            && !t.starts_with(['/', '.', '~', '-'])
            && !t.contains("://")
        {
            meaning
                .push_str(" (the part before the colon names another computer; the path after it is on that computer)");
        }
        out.rows.push(Row { piece: shorten(&w.raw, 60), meaning: self.decorate(meaning, w) });
        1
    }
}

trait LowerFirst {
    fn to_lowercase_first(&self) -> String;
}

impl LowerFirst for String {
    fn to_lowercase_first(&self) -> String {
        let mut c = self.chars();
        match c.next() {
            Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
            None => String::new(),
        }
    }
}

fn summarize_blocks(blocks: &[Block]) -> Option<String> {
    let heads: Vec<&str> =
        blocks.iter().filter_map(|b| if let Block::Cmd(c) = b { Some(c.headline.as_str()) } else { None }).collect();
    match heads.len() {
        0 => None,
        1 => Some(heads[0].to_string()),
        2 => Some(format!("{}; then {}", heads[0], heads[1])),
        n => Some(format!("{}; then {}; and {} more", heads[0], heads[1], n - 2)),
    }
}

fn trap_event(name: &str) -> String {
    let upper = name.trim_start_matches("SIG").to_ascii_uppercase();
    match upper.as_str() {
        "EXIT" | "0" => "the script or shell exits, normally or because of an error (EXIT)".into(),
        "ERR" => "a command fails (ERR)".into(),
        "INT" | "2" => "you press Ctrl+C (INT)".into(),
        "TERM" | "15" => "the process is asked to stop (TERM)".into(),
        "HUP" | "1" => "the terminal closes (HUP)".into(),
        "DEBUG" => "before every command (DEBUG)".into(),
        "RETURN" => "a function returns (RETURN)".into(),
        _ => format!("the signal or event {name}"),
    }
}

fn env_host(st: &State) -> String {
    st.pos0.clone().map(|h| h.rsplit('@').next().unwrap_or("").to_string()).unwrap_or_default()
}

fn compound(raw: &str, headline: &str) -> CmdBlock {
    CmdBlock { raw: raw.to_string(), headline: headline.to_string(), ..CmdBlock::default() }
}

/// Short spellings of subcommands (`npm i` is `npm install`).
fn sub_alias<'a>(prog: &str, sub: &'a str) -> &'a str {
    match (prog, sub) {
        ("npm" | "pnpm" | "yarn", "i" | "isntall" | "in") => "install",
        ("npm", "rm" | "r" | "un" | "remove") => "uninstall",
        ("npm", "t" | "tst") => "test",
        ("npm", "run-script") => "run",
        ("pnpm", "rm" | "uninstall") => "remove",
        ("pip", "i") => "install",
        ("docker", "rm") => "rm",
        _ => sub,
    }
}

fn has_children(kn: &Knowledge, path: &[String]) -> bool {
    kn.has_children(path)
}

fn is_assignment(raw: &str) -> bool {
    match raw.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !name.starts_with(|c: char| c.is_ascii_digit())
        }
        None => false,
    }
}

fn generic_arg(text: &str, prog: &str) -> String {
    let _ = prog;
    if text.contains("://") {
        "a web address (URL)".to_string()
    } else if text == "." {
        "the current folder".to_string()
    } else if text == ".." {
        "the parent folder".to_string()
    } else if text == "-" {
        "standard input (or output)".to_string()
    } else if text.starts_with('/') || text.starts_with("./") || text.starts_with("../") || text.starts_with('~') {
        "a file or folder path".to_string()
    } else if !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()) {
        format!("the number {text}")
    } else if text.contains('/') || text.contains('.') {
        "a file or folder name".to_string()
    } else {
        "an argument".to_string()
    }
}

fn replace_word(text: &mut String, label: &str, value: &str) -> bool {
    let bytes: Vec<char> = text.chars().collect();
    let lab: Vec<char> = label.chars().collect();
    if lab.is_empty() {
        return false;
    }
    let mut out = String::new();
    let mut i = 0;
    let mut replaced = false;
    while i < bytes.len() {
        let is_match = bytes[i..].starts_with(&lab)
            && (i == 0 || !bytes[i - 1].is_alphanumeric() && bytes[i - 1] != '_')
            && bytes.get(i + lab.len()).is_none_or(|c| !c.is_alphanumeric() && *c != '_');
        if is_match {
            out.push_str(value);
            i += lab.len();
            replaced = true;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    if replaced {
        *text = out;
    }
    replaced
}

fn script_fetches(script: &Script) -> bool {
    script.items.iter().any(|item| {
        std::iter::once(&item.chain.first).chain(item.chain.rest.iter().map(|(_, p)| p)).any(|p| {
            p.cmds.iter().any(|c| match c {
                Cmd::Simple(s) => s.words.first().is_some_and(|w| matches!(w.text.as_str(), "curl" | "wget")),
                _ => false,
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn explain(text: &str) -> Explanation {
        explain_line(&[], text, Options { man: false, path: false })
    }

    fn all_rows(blocks: &[Block], out: &mut Vec<Row>) {
        for b in blocks {
            if let Block::Cmd(c) = b {
                out.extend(c.rows.iter().cloned());
                for i in &c.inner {
                    all_rows(&i.blocks, out);
                }
            }
        }
    }

    fn rows(text: &str) -> Vec<Row> {
        let e = explain(text);
        let mut r = Vec::new();
        all_rows(&e.blocks, &mut r);
        r
    }

    fn meaning_of<'a>(rows: &'a [Row], piece: &str) -> &'a str {
        rows.iter()
            .find(|r| r.piece == piece)
            .map(|r| r.meaning.as_str())
            .unwrap_or_else(|| panic!("no row {piece:?} in {rows:#?}"))
    }

    #[test]
    fn find_mtime_is_not_backwards() {
        let r = rows("find . -name '*.log' -mtime +7 -delete");
        assert!(meaning_of(&r, "-mtime +7").contains("more than 7 days ago"));
        assert!(meaning_of(&r, "-delete").to_lowercase().contains("delete"));
        assert!(explain("find . -mtime +7 -delete").warnings.iter().any(|w| w.part == "-delete"));
    }

    #[test]
    fn flags_depend_on_the_program() {
        let ssh = rows("ssh -i ~/.ssh/key -p 2222 deploy@web1");
        assert!(meaning_of(&ssh, "-i ~/.ssh/key").contains("private key"));
        assert!(meaning_of(&ssh, "-p 2222").contains("port 2222"));
        assert!(meaning_of(&ssh, "deploy@web1").contains("computer"));
        let sed = rows("sed -i 's/a/b/' f.txt");
        assert!(meaning_of(&sed, "-i").contains("Edit the files themselves"));
    }

    #[test]
    fn tar_roles() {
        let r = rows("tar -xzvf backup.tar.gz -C /opt");
        assert!(meaning_of(&r, "-f backup.tar.gz").contains("archive file is backup.tar.gz"));
        assert!(meaning_of(&r, "-C /opt").contains("/opt"));
        assert!(!r.iter().any(|x| x.meaning.contains("put into a new archive")));
    }

    #[test]
    fn docker_roles() {
        let r = rows("docker run -d -p 8080:80 --name web nginx:alpine");
        assert!(meaning_of(&r, "-p 8080:80").contains("host port 8080 is forwarded to port 80"));
        assert!(meaning_of(&r, "--name web").contains("web"));
        assert!(meaning_of(&r, "nginx:alpine").contains("image"));
        let r = rows("docker exec -it web bash");
        assert!(meaning_of(&r, "web").contains("container"));
        assert!(!meaning_of(&r, "bash").contains("container to run"));
    }

    #[test]
    fn chmod_modes_are_decoded() {
        let r = rows("chmod -R 755 /var/www");
        assert!(meaning_of(&r, "755").contains("the owner can read, write and run"));
        let w = explain("chmod 777 notes.txt");
        assert!(w.warnings.iter().any(|x| x.part == "777"));
    }

    #[test]
    fn substitution_is_explained_recursively() {
        let e = explain("kill -9 $(lsof -t -i:3000)");
        let r = rows("kill -9 $(lsof -t -i:3000)");
        assert!(meaning_of(&r, "-9").contains("KILL"));
        assert!(meaning_of(&r, "-t").contains("process IDs"));
        assert!(r.iter().any(|x| x.piece == "-i :3000" && x.meaning.contains("3000")));
        assert!(e.warnings.iter().any(|w| w.part == "-9"));
    }

    #[test]
    fn git_reset_hard_warns() {
        let e = explain("git reset --hard HEAD~1");
        assert!(e.warnings.iter().any(|w| w.part == "--hard"));
        let r = rows("git reset --hard HEAD~1");
        assert!(meaning_of(&r, "HEAD~1").contains("just before"));
    }

    #[test]
    fn curl_pipe_bash_warns() {
        let e = explain("curl -fsSL https://example.com/install.sh | sudo bash -s -- --yes");
        assert!(e.warnings.iter().any(|w| w.text.contains("downloads a script")));
        assert!(e.warnings.iter().any(|w| w.part == "sudo"));
    }

    #[test]
    fn nested_command_strings() {
        let e = explain("bash -c 'echo hi && rm -rf /tmp/x'");
        assert!(e.warnings.iter().any(|w| w.part == "rm"));
        let e = explain("ssh host 'sudo systemctl restart nginx'");
        let r = rows("ssh host 'sudo systemctl restart nginx'");
        assert!(r.iter().any(|x| x.meaning.contains("remote computer host")));
        assert!(e.warnings.iter().any(|w| w.text.contains("on host")));
    }

    #[test]
    fn unset_variable_in_rm() {
        let e = explain("rm -rf \"$DIR\"/*");
        assert!(e.warnings.iter().any(|w| w.text.contains("empty or unset")));
        let e = explain("DIR=/tmp/x; rm -rf \"$DIR\"/*");
        assert!(!e.warnings.iter().any(|w| w.text.contains("empty or unset")));
    }

    #[test]
    fn steps_summarise_in_order() {
        let e = explain("git fetch origin && git reset --hard origin/main && git clean -fdx");
        assert_eq!(e.steps.len(), 3);
        assert!(e.steps[1].text.contains("git reset"));
    }

    /// The explain path must never run, spawn or evaluate anything from the command line.
    #[test]
    fn explain_modules_never_start_processes() {
        let sources = [
            ("breakdown.rs", include_str!("breakdown.rs")),
            ("shell.rs", include_str!("shell.rs")),
            ("manpage.rs", include_str!("manpage.rs")),
            ("knowledge.rs", include_str!("knowledge.rs")),
            ("decode.rs", include_str!("decode.rs")),
            ("risk.rs", include_str!("risk.rs")),
            ("render.rs", include_str!("render.rs")),
            ("explain.rs", include_str!("explain.rs")),
        ];
        // Built from pieces so that this test does not match itself.
        let needles = [
            concat!("std::pro", "cess"),
            concat!("Command", "::new"),
            concat!("process::", "Command"),
            concat!(".spa", "wn("),
            concat!(".exec", "v("),
            concat!("libc", "::system"),
        ];
        for (name, text) in sources {
            for needle in needles {
                assert!(!text.contains(needle), "{name} must not contain `{needle}`");
            }
        }
    }

    /// Everything the explanation says, without line wrapping, so that eval substrings match.
    fn flat_text(e: &Explanation) -> String {
        fn walk(blocks: &[Block], out: &mut String) {
            for b in blocks {
                if let Block::Cmd(c) = b {
                    out.push_str(&format!("{}\n", c.raw));
                    for r in &c.rows {
                        out.push_str(&format!("{}  {}\n", r.piece, r.meaning));
                    }
                    for i in &c.inner {
                        out.push_str(&format!("{}\n", i.label));
                        walk(&i.blocks, out);
                    }
                }
            }
        }
        let mut out = String::new();
        out.push_str(&format!("This line does {} things\n", e.steps.len()));
        for s in &e.steps {
            out.push_str(&format!("{}\n", s.text));
        }
        walk(&e.blocks, &mut out);
        for n in &e.notes {
            out.push_str(&format!("{n}\n"));
        }
        out
    }

    fn catalog_entry(template: &str) -> Command {
        toml::from_str(&format!(
            "name = \"package owns file\"\ndescription = \"Find which installed package a file belongs to\"\ntemplate = \"{template}\"\n"
        ))
        .expect("catalog entry")
    }

    #[test]
    fn a_catalog_entry_needs_its_flag_discriminator() {
        for (template, line, owns) in [
            ("dpkg -S {file}", ["dpkg", "-S", "/usr/bin/ls"], true),
            ("dpkg -S {file}", ["dpkg", "-i", "foo.deb"], false),
            ("rpm -qf {file}", ["rpm", "-qf", "/usr/bin/ls"], true),
            ("rpm -qf {file}", ["rpm", "-i", "foo.rpm"], false),
            ("pacman -Qo {file}", ["pacman", "-S", "foo"], false),
            ("pacman -Qo {file}", ["pacman", "-Qo", "/bin/ls"], true),
            ("apk info -W {file}", ["apk", "info", "curl"], false),
            ("apk info -W {file}", ["apk", "info", "-W"], true),
        ] {
            let catalog = [catalog_entry(template)];
            let engine = Engine::new(&catalog, Options { man: false, path: false });
            assert_eq!(engine.catalog_for(line[0], &line).is_some(), owns, "{template} vs {line:?}");
        }
    }

    #[test]
    fn dpkg_install_is_not_described_as_owning_a_file() {
        let catalog = [catalog_entry("dpkg -S {file}")];
        let e = explain_line(&catalog, "sudo dpkg -i foo.deb", Options { man: false, path: false });
        let text = flat_text(&e);
        assert!(!text.contains("belongs to"), "{text}");
        assert!(!text.contains("no description"), "{text}");
        assert!(e.warnings.iter().any(|w| w.text.contains("maintainer scripts")), "{:?}", e.warnings);
    }

    #[test]
    fn sudo_with_a_user_does_not_claim_root() {
        let e = explain("sudo -u postgres psql -c \"DROP DATABASE prod;\"");
        let sudo = e.warnings.iter().find(|w| w.part == "sudo").expect("sudo warning");
        assert!(sudo.text.contains("as the user postgres") && !sudo.text.contains("administrator (root)"), "{sudo:?}");
        assert!(e.warnings.iter().any(|w| w.text.contains("DROP DATABASE prod")));
    }

    #[test]
    fn unknown_programs_keep_single_dash_options_whole() {
        let r = rows("terraform2 apply -auto-approve");
        assert!(r.iter().any(|x| x.piece == "-auto-approve"), "{r:#?}");
        assert!(!r.iter().any(|x| x.piece == "-a"), "{r:#?}");
        let r = rows("mysql -u root -p\"hunter2\" -e \"select 1\"");
        assert!(meaning_of(&r, "-p\"hunter2\"").contains("password"), "{r:#?}");
    }

    #[test]
    fn man_page_names_cannot_climb_out_of_the_man_folders() {
        for bad in ["../../etc/x", "a/b", "", ".hidden", "..", "a\0b"] {
            assert!(!crate::manpage::safe_page_name(bad), "{bad:?}");
            assert!(crate::manpage::find_page(bad).is_none(), "{bad:?}");
        }
        assert!(crate::manpage::safe_page_name("git-commit"));
        // Running the engine with the man page lookup on must not probe such a path either.
        let _ = explain_line(&[], "../../etc/x --help; ./.hidden -x", Options { man: true, path: true });
    }

    #[test]
    fn the_whole_loop_owns_its_redirect() {
        let e = explain("while read l; do echo $l; done < /etc/passwd");
        assert_eq!(e.steps.len(), 1, "{:?}", e.steps);
        let e = explain("set -e\nls");
        assert!(e.multiline && crate::render::render(&e).contains("This script does 2 things"));
    }

    #[derive(serde::Deserialize)]
    struct EvalFile {
        case: Vec<EvalCase>,
    }

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct EvalCase {
        line: String,
        #[serde(default)]
        expect: Vec<String>,
        #[serde(default)]
        absent: Vec<String>,
        #[serde(default)]
        warn: Vec<String>,
        #[serde(default)]
        no_warn: Vec<String>,
        needs_man: Option<String>,
        /// Substrings that must appear in the rendered text (headline included).
        #[serde(default)]
        render: Vec<String>,
        #[serde(default)]
        render_absent: Vec<String>,
    }

    #[test]
    fn explain_evals() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/evals/explain.toml");
        let file: EvalFile =
            toml::from_str(&std::fs::read_to_string(path).expect("read evals/explain.toml")).expect("parse");
        assert!(file.case.len() >= 40, "the eval file should hold at least 40 cases");
        let mut failures = Vec::new();
        let mut skipped = 0;
        for case in &file.case {
            if let Some(page) = &case.needs_man
                && crate::manpage::find_page(page).is_none()
            {
                skipped += 1;
                continue;
            }
            let opts = Options { man: case.needs_man.is_some(), path: false };
            let e = explain_line(&[], &case.line, opts);
            let rendered = crate::render::render(&e);
            let text = flat_text(&e);
            let warnings: String = e.warnings.iter().map(|w| format!("{} {}\n", w.part, w.text)).collect();
            for s in &case.expect {
                if !text.to_lowercase().contains(&s.to_lowercase()) {
                    failures.push(format!("{:?}: missing {s:?}\n{text}", case.line));
                }
            }
            for s in &case.render {
                if !rendered.contains(s.as_str()) {
                    failures.push(format!("{:?}: rendered text is missing {s:?}\n{rendered}", case.line));
                }
            }
            for s in &case.render_absent {
                if rendered.contains(s.as_str()) {
                    failures.push(format!("{:?}: rendered text should not contain {s:?}\n{rendered}", case.line));
                }
            }
            for s in &case.absent {
                if text.to_lowercase().contains(&s.to_lowercase()) {
                    failures.push(format!("{:?}: should not contain {s:?}\n{text}", case.line));
                }
            }
            for s in &case.warn {
                if !warnings.contains(s.as_str()) {
                    failures.push(format!("{:?}: no warning containing {s:?}; warnings were:\n{warnings}", case.line));
                }
            }
            for s in &case.no_warn {
                if warnings.contains(s.as_str()) {
                    failures.push(format!("{:?}: unexpected warning containing {s:?}:\n{warnings}", case.line));
                }
            }
            for line in rendered.lines() {
                assert!(line.chars().count() <= 100, "line too wide ({}): {line}", line.chars().count());
            }
        }
        assert!(
            failures.is_empty(),
            "{} eval failure(s) ({skipped} skipped):\n{}",
            failures.len(),
            failures.join("\n---\n")
        );
    }

    #[test]
    fn never_panics_on_junk() {
        for text in [
            "",
            "   ",
            "|",
            "&&",
            "((",
            "$(",
            "'",
            "for x in; do",
            "case",
            "a ||| b",
            "\u{0}",
            "<<<",
            "cat <<",
            "echo ${",
            "git -",
            "tar -",
            "x=",
            "=",
            "--",
            "-",
            "find -exec",
        ] {
            let _ = explain(text);
        }
    }
}
