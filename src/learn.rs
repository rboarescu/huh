//! Lab mode (`/mode lab`): interactive, kid-friendly Linux terminal tutorial with XP, streaks,
//! 5-heart lessons (no lockout timers), 6-box spaced repetition review, and `#!` pet tie-in.
//!
//! Nothing here ever runs a command: user-typed commands are graded structurally via
//! `crate::shell::parse` and explained via `crate::explain`, `crate::outputs`, and `crate::risk_kinds`.

use crate::breakdown::{Block, Explanation, Row};
use crate::catalog::Command;
use crate::date::{self, Date};
use crate::lessons::{self, Lesson, Question, Unit};
use crate::pet::{self, Mood, Pet, Species};
use crate::practice::{self, Check, Practice, RecordedCommand, Report};
use crate::progress::{
    self, BADGES, MAX_FREEZES, MAX_HEARTS, Progress, StreakEvent, XP_COMBO_BONUS, XP_CORRECT, XP_FLAWLESS_BONUS,
    XP_LESSON_CLEAR, XP_PRACTICE_CLEAR, XP_REVIEW_CORRECT,
};
use crate::shell::{self, AndOr, Cmd, Conn, Item, Pipeline, Redir, Script, Sep, Simple, Word};
use crate::srs;
use crate::style::{Role, paint};
use anyhow::{Result, bail};
use std::collections::{HashSet, VecDeque};
use std::fmt::Write as _;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

const MAX_REVIEW_CARDS: usize = 8;

/// Outcome of handling a slash command typed while inside Lab mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlashEffect {
    NotSlash,
    Handled,
    SwitchToWatch,
}

/// Event returned by `LabHost::wait_practice_turn` while watching a hands-on practice sandbox.
#[derive(Clone, Debug)]
pub enum PracticePoll {
    UserLine(Option<String>),
    AutoUpdate { report: Report, recorded: Option<RecordedCommand>, nudged: bool },
    Quit,
}

/// How an interactive Lab session ended inside the companion pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneExit {
    SwitchToWatch,
    Quit,
}

/// One selectable item passed to [`LabHost::choose_option`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChoiceOption {
    pub key: String,
    pub label: String,
    pub detail: String,
    pub locked: bool,
}

/// Result returned by [`LabHost::choose_option`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChoiceResult {
    Picked(usize),
    Cancelled,
    Slash(String),
    Eof,
    Fallback,
}

/// Host abstraction separating Lab curriculum logic from terminal streaming, slash commands,
/// Pebble updates, and live practice watching.
pub trait LabHost {
    /// Streams narrative text (typewritten in the companion pane, instant in `PlainHost`).
    fn write_narrative(&mut self, text: &str) -> std::io::Result<()>;
    /// Prints UI chrome immediately (menus, check status, prompts, and error notices).
    fn write_instant(&mut self, text: &str) -> std::io::Result<()>;
    /// Notifies the host that pet state or progress changed so the header can update.
    fn on_pet_event(&mut self, mood: Option<Mood>, celebrate: bool);
    /// Handles a slash command (`/mode`, `/watch`, `/lab`, `/safety`, `/theme`, `/pet`, `/speed`,
    /// `/quiet`, `/tips`, `/find`, `/help`) from any interactive prompt inside Lab mode.
    fn handle_slash(&mut self, line: &str) -> std::io::Result<SlashEffect>;
    /// Polls for user input or watched-shell progress during a hands-on practice lab.
    /// Returns `None` in non-polling hosts (`PlainHost`), falling back to `read_line`.
    fn wait_practice_turn(
        &mut self,
        sandbox_dir: &Path,
        practice: &Practice,
        reported_cmds: &mut u64,
    ) -> Option<PracticePoll>;

    /// Presents an interactive inline selector when supported; returns `ChoiceResult::Fallback` in
    /// non-TTY / plain hosts so callers print their numbered text prompt instead.
    fn choose_option(
        &mut self,
        _title: &str,
        _items: &[ChoiceOption],
        _default_idx: usize,
        _hint: &str,
        _reader: &mut dyn BufRead,
    ) -> std::io::Result<ChoiceResult> {
        Ok(ChoiceResult::Fallback)
    }

    /// Reads and executes a slash command triggered via `/` inside a `select_menu`.
    fn read_slash_from_menu(&mut self, _prefill: &str, _reader: &mut dyn BufRead) -> std::io::Result<SlashEffect> {
        Ok(SlashEffect::NotSlash)
    }

    /// Writes `prompt` (when non-empty) and reads a trimmed line; raw-mode hosts override this so
    /// the line editor owns and redraws the prompt alongside `/` dropdowns.
    fn read_prompt(&mut self, prompt: &str, reader: &mut dyn BufRead) -> std::io::Result<Option<String>> {
        if !prompt.is_empty() {
            self.write_instant(&format!("{} ", paint(Role::Dim, prompt)))?;
        }
        self.read_line(reader)
    }

    /// True when `wait_practice_turn` manages drawing the practice prompt itself (raw `LineEditor`).
    fn handles_practice_prompt(&self) -> bool {
        false
    }

    /// Reads the next trimmed line from input; `PaneHost` overrides this to poll stdin alongside
    /// Pebble animation and terminal resize handling.
    fn read_line(&mut self, reader: &mut dyn BufRead) -> std::io::Result<Option<String>> {
        let mut buf = String::new();
        let n = reader.read_line(&mut buf)?;
        if n == 0 { Ok(None) } else { Ok(Some(buf.trim().to_string())) }
    }
}

/// Default synchronous host writing directly to `W` (used by `run_with_io` and unit tests).
#[cfg_attr(not(test), allow(dead_code))]
pub struct PlainHost<'a, W: Write> {
    writer: &'a mut W,
}

impl<'a, W: Write> PlainHost<'a, W> {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(writer: &'a mut W) -> Self {
        Self { writer }
    }
}

impl<W: Write> LabHost for PlainHost<'_, W> {
    fn write_narrative(&mut self, text: &str) -> std::io::Result<()> {
        self.writer.write_all(text.as_bytes())?;
        self.writer.flush()
    }

    fn write_instant(&mut self, text: &str) -> std::io::Result<()> {
        self.writer.write_all(text.as_bytes())?;
        self.writer.flush()
    }

    fn on_pet_event(&mut self, _mood: Option<Mood>, _celebrate: bool) {}

    fn handle_slash(&mut self, _line: &str) -> std::io::Result<SlashEffect> {
        Ok(SlashEffect::NotSlash)
    }

    fn wait_practice_turn(
        &mut self,
        _sandbox_dir: &Path,
        _practice: &Practice,
        _reported_cmds: &mut u64,
    ) -> Option<PracticePoll> {
        None
    }
}

/// Summary of side effects from a Lab session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionOutcome {
    pub pet_fed: bool,
    pub species_unlocked: bool,
    pub lessons_completed: usize,
    pub practices_completed: usize,
}

/// Configuration for `run_with_io`, keeping all clock, TTY, and state-path inputs explicit.
pub struct RunConfig<'a> {
    pub target: &'a [String],
    pub lesson: Option<usize>,
    pub interactive: bool,
    pub today: Date,
    pub now_epoch: u64,
    pub daily_goal: u32,
    pub progress_path: Option<&'a Path>,
    pub pet_path: Option<&'a Path>,
    pub practice_root: Option<PathBuf>,
    pub recording_dir: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Structural shell-command grader
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
struct NormScript {
    items: Vec<NormItem>,
}

#[derive(Debug, PartialEq)]
struct NormItem {
    chain: NormAndOr,
    sep: Sep,
}

#[derive(Debug, PartialEq)]
struct NormAndOr {
    first: NormPipeline,
    rest: Vec<(Conn, NormPipeline)>,
}

#[derive(Debug, PartialEq)]
struct NormPipeline {
    negated: bool,
    pipe_err: Vec<bool>,
    cmds: Vec<NormCmd>,
}

#[derive(Debug, PartialEq)]
enum NormCmd {
    Simple(NormSimple),
    Subshell(NormScript, Vec<NormRedir>),
    Group(NormScript, Vec<NormRedir>),
    Raw(String),
}

#[derive(Debug, PartialEq, Eq)]
struct NormSimple {
    assigns: Vec<(String, String)>,
    program: Option<String>,
    bool_flags: Vec<String>,
    valued_flags: Vec<(String, String)>,
    positionals: Vec<String>,
    redirs: Vec<NormRedir>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct NormRedir {
    fd: Option<String>,
    op: String,
    target: String,
    heredoc: Option<String>,
}

fn is_find_value_flag(flag: &str) -> bool {
    matches!(
        flag,
        "-name"
            | "-iname"
            | "-type"
            | "-size"
            | "-mtime"
            | "-atime"
            | "-ctime"
            | "-maxdepth"
            | "-mindepth"
            | "-path"
            | "-ipath"
            | "-user"
            | "-group"
            | "-perm"
            | "-newer"
    )
}

fn takes_short_value(prog: &str, ch: char) -> bool {
    match prog {
        "head" | "tail" => matches!(ch, 'n' | 'c'),
        "grep" | "egrep" | "fgrep" => matches!(ch, 'e' | 'f' | 'm' | 'A' | 'B' | 'C'),
        "sort" => matches!(ch, 'k' | 't' | 'o'),
        "cut" => matches!(ch, 'd' | 'f' | 'c' | 'b'),
        "tar" => matches!(ch, 'f' | 'C'),
        "mkdir" => ch == 'm',
        "ping" => ch == 'c',
        "curl" => ch == 'o',
        "ssh" => matches!(ch, 'l' | 'p' | 'i'),
        "kill" => ch == 's',
        _ => false,
    }
}

fn takes_long_value(prog: &str, flag: &str) -> bool {
    match prog {
        "head" | "tail" => matches!(flag, "--lines" | "--bytes"),
        "grep" | "egrep" | "fgrep" => {
            matches!(flag, "--regexp" | "--file" | "--max-count" | "--after-context" | "--before-context" | "--context")
        }
        "sort" => matches!(flag, "--key" | "--field-separator" | "--output"),
        "cut" => matches!(flag, "--delimiter" | "--fields" | "--characters" | "--bytes"),
        "tar" => matches!(flag, "--file" | "--directory"),
        "mkdir" => flag == "--mode",
        "curl" => flag == "--output",
        _ => false,
    }
}

fn normalize_redirs(redirs: &[Redir]) -> Vec<NormRedir> {
    let mut out: Vec<NormRedir> = redirs
        .iter()
        .map(|r| {
            let fd = match (r.fd.as_deref(), r.op.as_str()) {
                (Some("1"), ">" | ">>" | ">|") | (Some("0"), "<") => None,
                (other, _) => other.map(str::to_string),
            };
            NormRedir {
                fd,
                op: r.op.clone(),
                target: r.target.text.clone(),
                heredoc: r.heredoc.as_ref().map(|h| h.body.clone()),
            }
        })
        .collect();
    // Redirect order is semantically significant when any fd-duplicating operator (`>&`, `<&`)
    // is present (e.g. `> out.log 2>&1` differs from `2>&1 > out.log`).
    if !out.iter().any(|r| matches!(r.op.as_str(), ">&" | "<&")) {
        out.sort();
    }
    out
}

fn normalize_simple(simple: &Simple) -> NormSimple {
    let mut assigns: Vec<(String, String)> = simple.assigns.iter().map(|(k, v)| (k.clone(), v.text.clone())).collect();
    assigns.sort();

    let redirs = normalize_redirs(&simple.redirs);
    let Some(first) = simple.words.first() else {
        return NormSimple {
            assigns,
            program: None,
            bool_flags: Vec::new(),
            valued_flags: Vec::new(),
            positionals: Vec::new(),
            redirs,
        };
    };

    let prog = first.text.as_str();
    let args: Vec<&str> = simple.words[1..].iter().map(|w| w.text.as_str()).collect();
    let mut bool_flags = Vec::new();
    let mut valued_flags: Vec<(String, String)> = Vec::new();
    let mut positionals = Vec::new();

    let mut i = 0;
    let mut end_of_opts = false;
    while i < args.len() {
        let arg = args[i];
        if end_of_opts {
            positionals.push(arg.to_string());
            i += 1;
            continue;
        }
        if arg == "--" {
            end_of_opts = true;
            i += 1;
            continue;
        }
        if arg == "-" || !arg.starts_with('-') {
            positionals.push(arg.to_string());
            i += 1;
            continue;
        }
        if arg.starts_with("--") {
            if let Some((flag, val)) = arg.split_once('=') {
                valued_flags.push((flag.to_string(), val.to_string()));
                i += 1;
            } else if takes_long_value(prog, arg) && i + 1 < args.len() {
                valued_flags.push((arg.to_string(), args[i + 1].to_string()));
                i += 2;
            } else {
                bool_flags.push(arg.to_string());
                i += 1;
            }
            continue;
        }
        // `find` uses single-dash multi-letter predicates (`-name`, `-delete`, `-type`); never split them.
        if prog == "find" {
            if is_find_value_flag(arg) && i + 1 < args.len() {
                valued_flags.push((arg.to_string(), args[i + 1].to_string()));
                i += 2;
            } else {
                bool_flags.push(arg.to_string());
                i += 1;
            }
            continue;
        }
        // Legacy `head -5` / `tail -10` numeric shorthand equals `-n 5` / `-n 10`.
        if matches!(prog, "head" | "tail") && arg.len() > 1 && arg[1..].bytes().all(|b| b.is_ascii_digit()) {
            valued_flags.push(("-n".to_string(), arg[1..].to_string()));
            i += 1;
            continue;
        }
        // Short flag or combined short flags (`-la`, `-n10`, `-iA2`, `-d:`).
        let chars: Vec<(usize, char)> = arg[1..].char_indices().collect();
        let mut consumed_next = false;
        for (pos, (byte_idx, ch)) in chars.iter().enumerate() {
            if takes_short_value(prog, *ch) {
                let rest = &arg[1 + byte_idx + ch.len_utf8()..];
                if !rest.is_empty() {
                    valued_flags.push((format!("-{ch}"), rest.to_string()));
                } else if pos + 1 == chars.len() && i + 1 < args.len() {
                    valued_flags.push((format!("-{ch}"), args[i + 1].to_string()));
                    consumed_next = true;
                } else {
                    bool_flags.push(format!("-{ch}"));
                }
                break;
            }
            bool_flags.push(format!("-{ch}"));
        }
        i += if consumed_next { 2 } else { 1 };
    }

    bool_flags.sort();
    // Stable sort by flag name: reorders distinct valued flags while preserving repeated-flag order.
    valued_flags.sort_by(|a, b| a.0.cmp(&b.0));

    NormSimple { assigns, program: Some(prog.to_string()), bool_flags, valued_flags, positionals, redirs }
}

fn normalize_cmd(cmd: &Cmd) -> NormCmd {
    match cmd {
        Cmd::Simple(s) => NormCmd::Simple(normalize_simple(s)),
        Cmd::Subshell(script, redirs) => NormCmd::Subshell(normalize_script(script), normalize_redirs(redirs)),
        Cmd::Group(script, redirs) => NormCmd::Group(normalize_script(script), normalize_redirs(redirs)),
        Cmd::Test(raw) => NormCmd::Raw(format!("test:{}", raw.trim())),
        Cmd::For { raw_head, body, redirs, .. } => {
            NormCmd::Raw(format!("for:{}:{:?}:{:?}", raw_head.trim(), normalize_script(body), normalize_redirs(redirs)))
        }
        Cmd::While { until, cond, body, redirs, .. } => NormCmd::Raw(format!(
            "while:{until}:{:?}:{:?}:{:?}",
            normalize_script(cond),
            normalize_script(body),
            normalize_redirs(redirs)
        )),
        Cmd::If { branches, otherwise, redirs } => {
            let b: Vec<(NormScript, NormScript)> =
                branches.iter().map(|(c, body)| (normalize_script(c), normalize_script(body))).collect();
            let o = otherwise.as_ref().map(normalize_script);
            NormCmd::Raw(format!("if:{b:?}:{o:?}:{:?}", normalize_redirs(redirs)))
        }
        Cmd::Case { subject, arms, redirs } => {
            let a: Vec<(&[String], NormScript)> =
                arms.iter().map(|arm| (arm.patterns.as_slice(), normalize_script(&arm.body))).collect();
            NormCmd::Raw(format!("case:{}:{a:?}:{:?}", subject.text, normalize_redirs(redirs)))
        }
        Cmd::Func { name, body } => NormCmd::Raw(format!("func:{name}:{:?}", normalize_script(body))),
    }
}

fn normalize_pipeline(p: &Pipeline) -> NormPipeline {
    NormPipeline { negated: p.negated, pipe_err: p.pipe_err.clone(), cmds: p.cmds.iter().map(normalize_cmd).collect() }
}

fn normalize_and_or(a: &AndOr) -> NormAndOr {
    NormAndOr {
        first: normalize_pipeline(&a.first),
        rest: a.rest.iter().map(|(conn, pipe)| (*conn, normalize_pipeline(pipe))).collect(),
    }
}

fn normalize_item(item: &Item) -> NormItem {
    NormItem { chain: normalize_and_or(&item.chain), sep: item.sep }
}

fn normalize_script(script: &Script) -> NormScript {
    NormScript { items: script.items.iter().map(normalize_item).collect() }
}

fn parse_normalized(cmd: &str) -> Option<NormScript> {
    if cmd.trim().is_empty() {
        return None;
    }
    let parsed = shell::parse(cmd);
    if !parsed.notes.is_empty() || parsed.script.items.is_empty() {
        return None;
    }
    Some(normalize_script(&parsed.script))
}

/// Grades a user-typed shell command structurally against `accepted` answers without executing
/// anything. Rejects blank input and syntax problems (such as unbalanced quotes or dangling pipes).
pub fn grade_command(input: &str, accepted: &[String]) -> bool {
    let Some(actual) = parse_normalized(input) else {
        return false;
    };
    accepted.iter().any(|candidate| parse_normalized(candidate).as_ref() == Some(&actual))
}

/// First program name in a command string, for feeding the `#!` pet on right answers.
fn first_program(command: &str) -> Option<String> {
    let parsed = shell::parse(command);
    let item = parsed.script.items.first()?;
    let Cmd::Simple(simple) = item.chain.first.cmds.first()? else {
        return None;
    };
    simple.words.first().map(|w: &Word| w.text.clone()).filter(|s| !s.is_empty())
}

fn question_program(q: &Question) -> Option<String> {
    match q {
        Question::ExplainChoice { command, .. } | Question::ReadOutput { command, .. } => first_program(command),
        Question::TypeCommand { accepted, .. } => accepted.first().and_then(|c| first_program(c)),
        Question::SpotDanger { command: Some(cmd), .. } => first_program(cmd),
        Question::SpotDanger { command: None, choices, answer, .. } => {
            choices.get(*answer).and_then(|c| first_program(c))
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering & feedback helpers
// ---------------------------------------------------------------------------

fn hearts_meter(hearts: u8) -> String {
    let alive = hearts.min(MAX_HEARTS) as usize;
    let lost = (MAX_HEARTS as usize).saturating_sub(alive);
    format!("[{}{}]", "♥".repeat(alive), "·".repeat(lost))
}

fn xp_bar(current: u32, goal: u32) -> String {
    let denom = goal.max(1) as usize;
    let filled = ((current as usize).saturating_mul(10) / denom).min(10);
    format!("[{}{}]", "█".repeat(filled), "·".repeat(10 - filled))
}

/// Condensed command explanation reusing `huh`'s breakdown renderer.
fn condensed_explanation(e: &Explanation) -> String {
    let mut out = String::new();
    for step in e.steps.iter().take(3) {
        crate::render::hanging(&mut out, 2, "", &step.text);
    }
    let mut rows: Vec<Row> = Vec::new();
    if let Some(Block::Cmd(c)) = e.blocks.iter().find(|b| matches!(b, Block::Cmd(_))) {
        rows.extend(c.rows.iter().skip(1).take(4).cloned());
    }
    crate::render::rows(&mut out, &rows, 4);
    if let Some(w) = e.warnings.first() {
        let tag = format!("[{}]", w.risk.name());
        crate::render::hanging_tagged(&mut out, 2, ("! ", Role::Warning), &tag, &format!("{tag} {}", w.text));
    }
    out
}

fn render_pet_celebration(pet_name: &str, species: Species, lines: &[String]) -> String {
    let grid = pet::art(species, Mood::Celebrating);
    let mut out = String::new();
    for (row, art_row) in grid.iter().enumerate() {
        let padded = format!("{art_row:<width$}", width = pet::ART_MAX_WIDTH);
        let text = lines.get(row).map(String::as_str).unwrap_or("");
        if text.is_empty() {
            let _ = writeln!(out, "  {}", paint(Mood::Celebrating.role(), &padded));
        } else {
            let _ = writeln!(out, "  {}  {text}", paint(Mood::Celebrating.role(), &padded));
        }
    }
    if lines.is_empty() {
        let _ = writeln!(out, "  {}", paint(Role::Success, &format!("{pet_name} is cheering for you!")));
    }
    out
}

fn unit_lesson_ids(unit: &Unit) -> Vec<&str> {
    unit.lessons.iter().map(|l| l.id.as_str()).collect()
}

fn prereq_unit(units: &[Unit], unit_idx: usize) -> Option<&Unit> {
    if let Some(dep) = units[unit_idx].unlock_after.as_deref() {
        units.iter().find(|u| u.id == dep)
    } else {
        unit_idx.checked_sub(1).map(|prev| &units[prev])
    }
}

fn prev_unit_ids(units: &[Unit], unit_idx: usize) -> Option<Vec<&str>> {
    prereq_unit(units, unit_idx).map(unit_lesson_ids)
}

fn locked_unit_message(units: &[Unit], unit_idx: usize) -> String {
    let unit = &units[unit_idx];
    match prereq_unit(units, unit_idx) {
        Some(prev_unit) => format!(
            "Unit {} ({}) is locked — finish all lessons in Unit {} ({}) first!",
            unit.number, unit.title, prev_unit.number, prev_unit.title
        ),
        None => format!("Unit {} ({}) is locked!", unit.number, unit.title),
    }
}

/// Pure progress summary for `/mode lab` (`progress`) and non-interactive fallback.
pub fn render_progress(units: &[Unit], progress: &Progress, pet: &Pet, today: Date, daily_goal: u32) -> String {
    let mut out = String::new();
    let daily = progress.daily_xp_on(today);
    let due = srs::due_count(&progress.srs, today);

    let _ = writeln!(out, "{}", paint(Role::Header, "Lab — Progress"));
    let _ = writeln!(
        out,
        "  XP: {} total · Today: {} {}/{} XP{}",
        paint(Role::Value, &progress.xp.to_string()),
        paint(Role::Accent, &xp_bar(daily, daily_goal)),
        paint(Role::Value, &daily.to_string()),
        daily_goal,
        if daily >= daily_goal { format!(" {}", paint(Role::Success, "(goal met!)")) } else { String::new() }
    );
    let _ = writeln!(
        out,
        "  Streak: {} day(s) (best {}) · Freezes: {}/{} · Review due: {}",
        paint(Role::Value, &progress.streak.to_string()),
        progress.best_streak,
        progress.freezes,
        MAX_FREEZES,
        paint(if due > 0 { Role::Note } else { Role::Value }, &due.to_string())
    );
    let unlocked_names: Vec<&str> = pet.unlocked_species.iter().map(|s| s.name()).collect();
    let _ = writeln!(
        out,
        "  Pet: {} ({}) · unlocked species: {}",
        paint(Role::Accent, &pet.name),
        pet.species.label(),
        unlocked_names.join(", ")
    );
    if due > 0 {
        let _ = writeln!(out, "  {}", paint(Role::Note, "Tip: type `review` in `/mode lab` to practice due cards."));
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "{}", paint(Role::Header, "Skill Path:"));
    for (u_idx, unit) in units.iter().enumerate() {
        let prev = prev_unit_ids(units, u_idx);
        let u_unlocked = progress.is_unit_unlocked(u_idx, prev.as_deref());
        let ids = unit_lesson_ids(unit);
        let u_done = progress.is_unit_completed(&ids);
        let done_count = unit.lessons.iter().filter(|l| progress.is_lesson_completed(&l.id)).count();

        let status = if u_done {
            paint(Role::Success, "[done]")
        } else if u_unlocked {
            paint(Role::Accent, &format!("[{done_count}/{}]", unit.lessons.len()))
        } else {
            paint(Role::Dim, "[locked]")
        };

        let side_quest_tag =
            if unit.is_side_quest() { format!(" {}", paint(Role::Note, "[real-world]")) } else { String::new() };

        let species_hint = unit.unlocks_species.as_deref().and_then(|s| Species::parse(s).ok()).map(|sp| {
            if pet.is_unlocked(sp) {
                paint(Role::Success, &format!(" · unlocked {}", sp.label()))
            } else {
                paint(Role::Dim, &format!(" · unlocks {}", sp.label()))
            }
        });

        let _ = writeln!(
            out,
            "  Unit {}: {} {status}{side_quest_tag}{}",
            unit.number,
            paint(Role::Header, &unit.title),
            species_hint.unwrap_or_default()
        );
        let _ = writeln!(out, "    {}", paint(Role::Dim, &unit.summary));
        for (l_idx, lesson) in unit.lessons.iter().enumerate() {
            let prev_l = l_idx.checked_sub(1).map(|i| unit.lessons[i].id.as_str());
            let l_unlocked = progress.is_lesson_unlocked(u_unlocked, l_idx, prev_l);
            let l_done = progress.is_lesson_completed(&lesson.id);
            let mark = if l_done {
                paint(Role::Success, "[x]")
            } else if l_unlocked {
                paint(Role::Accent, "[ ]")
            } else {
                paint(Role::Dim, "[lock]")
            };
            let _ = writeln!(out, "    {mark} {}. {}", l_idx + 1, lesson.title);
        }
        if let Some(practice) = &unit.practice {
            let p_done = progress.is_practice_completed(&practice.id);
            let p_mark = if p_done {
                paint(Role::Success, "[x]")
            } else if u_unlocked {
                paint(Role::Accent, "[ ]")
            } else {
                paint(Role::Dim, "[lock]")
            };
            let p_status = if p_done {
                paint(Role::Success, "(done)")
            } else if u_unlocked {
                paint(Role::Dim, &format!("(available — `practice {}` in `/mode lab`)", unit.number))
            } else {
                paint(Role::Dim, "(locked)")
            };
            let _ = writeln!(out, "    {p_mark} Practice: {} {p_status}", practice.title);
        }
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "{} ({}/{}):", paint(Role::Header, "Badges"), progress.badges.len(), BADGES.len());
    if progress.badges.is_empty() {
        let _ = writeln!(out, "  {}", paint(Role::Dim, "None yet — finish your first lesson to earn First Step!"));
    } else {
        for earned in &progress.badges {
            if let Some(def) = progress::badge_def(&earned.id) {
                let _ = writeln!(
                    out,
                    "  * {} ({}) — {} [{}]",
                    paint(Role::Success, def.title),
                    def.id,
                    def.description,
                    earned.earned_day
                );
            } else {
                let _ = writeln!(out, "  * {} [{}]", earned.id, earned.earned_day);
            }
        }
    }
    let _ = writeln!(out);
    let _ =
        writeln!(out, "  {}", paint(Role::Dim, "Commands: `/mode lab`, `review`, `practice [unit]`, `/mode watch`"));
    out
}

// ---------------------------------------------------------------------------
// State helpers (atomic, metadata-preserving)
// ---------------------------------------------------------------------------

fn load_progress(path: Option<&Path>) -> Progress {
    path.map_or_else(Progress::default, progress::load_from)
}

fn save_progress(path: Option<&Path>, progress: &Progress) -> Result<()> {
    if let Some(p) = path {
        progress::save_to(p, progress)?;
    }
    Ok(())
}

fn load_pet(path: Option<&Path>) -> Pet {
    path.map_or_else(Pet::default, pet::load_from)
}

/// Feeds the pet via load-modify-save so any concurrent rename/species metadata on disk is preserved.
fn feed_pet(pet_path: Option<&Path>, program: Option<&str>, now_epoch: u64) -> Option<Mood> {
    let path = pet_path?;
    let mut p = pet::load_from(path);
    let mood = p.feed(program, now_epoch);
    let _ = pet::save_to(path, &p);
    Some(mood)
}

/// Unlocks `species` in `pet.toml` via load-modify-save; returns `true` only when newly unlocked.
fn unlock_pet_species(pet_path: Option<&Path>, species: Species) -> bool {
    let Some(path) = pet_path else {
        return false;
    };
    let mut p = pet::load_from(path);
    let newly_unlocked = p.unlock_species(species);
    if newly_unlocked {
        let _ = pet::save_to(path, &p);
    }
    newly_unlocked
}

// ---------------------------------------------------------------------------
// Question presentation & interaction
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AnswerOutcome {
    Correct,
    Wrong,
    Quit,
    Eof,
    SwitchToWatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    Continue,
    SwitchToWatch,
    QuitAll,
}

fn is_quit_cmd(s: &str) -> bool {
    matches!(s.to_ascii_lowercase().as_str(), "q" | "quit" | "/quit")
}

fn is_choice_hint_cmd(s: &str) -> bool {
    matches!(s.to_ascii_lowercase().as_str(), "h" | "hint" | "/hint" | "?")
}

fn is_type_hint_cmd(s: &str) -> bool {
    matches!(s.to_ascii_lowercase().as_str(), "hint" | "/hint" | "?")
}

fn run_menu_slash(s: &str, reader: &mut dyn BufRead, host: &mut dyn LabHost) -> Result<SlashEffect> {
    if s == "/" { Ok(host.read_slash_from_menu(s, reader)?) } else { Ok(host.handle_slash(s)?) }
}

fn ask_question<R: BufRead>(
    q: &Question,
    header_line: &str,
    catalog: &[Command],
    reader: &mut R,
    host: &mut dyn LabHost,
) -> Result<AnswerOutcome> {
    host.write_instant(&format!("\n{header_line}  {}\n", paint(Role::Accent, &format!("[{}]", q.kind_label()))))?;

    match q {
        Question::ExplainChoice { command, prompt, choices, answer, note, .. } => {
            host.write_instant(&format!("  {}\n", paint(Role::Command, &format!("$ {command}"))))?;
            host.write_narrative(&format!("{prompt}\n"))?;
            let outcome = prompt_choice(q, choices, *answer, reader, host)?;
            if matches!(outcome, AnswerOutcome::Quit | AnswerOutcome::Eof | AnswerOutcome::SwitchToWatch) {
                return Ok(outcome);
            }
            if outcome == AnswerOutcome::Correct {
                host.write_instant(&format!("  {}\n", paint(Role::Success, "Correct!")))?;
            } else {
                host.write_instant(&format!(
                    "  {} {}\n",
                    paint(Role::Warning, "Not quite."),
                    paint(Role::Warning, &format!("Answer: {}) {}", answer + 1, choices[*answer]))
                ))?;
            }
            let exp = crate::explain::explain_command(catalog, command);
            let mut narrative = condensed_explanation(&exp);
            let _ = writeln!(narrative, "  {}", paint(Role::Note, note));
            host.write_narrative(&narrative)?;
            Ok(outcome)
        }
        Question::TypeCommand { prompt, accepted, note, .. } => {
            host.write_narrative(&format!("{prompt}\n"))?;
            host.write_instant(&format!("  {}\n", paint(Role::Dim, "(type 'hint' for a clue, or 'q' to quit)")))?;
            let mut hint_step = 0usize;
            loop {
                let Some(line) = host.read_prompt("$", reader)? else {
                    return Ok(AnswerOutcome::Eof);
                };
                if is_quit_cmd(&line) {
                    return Ok(AnswerOutcome::Quit);
                }
                if is_type_hint_cmd(&line) {
                    host.write_instant(&format!("  {} {}\n", paint(Role::Note, "Hint:"), q.hint_at(hint_step)))?;
                    hint_step = hint_step.saturating_add(1);
                    continue;
                }
                match host.handle_slash(&line)? {
                    SlashEffect::NotSlash => {}
                    SlashEffect::Handled => continue,
                    SlashEffect::SwitchToWatch => return Ok(AnswerOutcome::SwitchToWatch),
                }
                if line.is_empty() {
                    host.write_instant(&format!(
                        "  {}\n",
                        paint(Role::Dim, "Type a command, 'hint' for a clue, or 'q' to quit.")
                    ))?;
                    continue;
                }
                if grade_command(&line, accepted) {
                    host.write_instant(&format!("  {}\n", paint(Role::Success, "Correct!")))?;
                    host.write_narrative(&format!("  {}\n", paint(Role::Note, note)))?;
                    return Ok(AnswerOutcome::Correct);
                }
                let mut instant = format!("  {}\n", paint(Role::Warning, "Not quite."));
                let mut narrative = String::new();
                let parsed = shell::parse(&line);
                if !parsed.notes.is_empty() {
                    for pnote in &parsed.notes {
                        let _ = writeln!(instant, "  {} {pnote}", paint(Role::Warning, "Syntax:"));
                    }
                } else {
                    let exp = crate::explain::explain_command(catalog, &line);
                    let detail = condensed_explanation(&exp);
                    if !detail.is_empty() {
                        let _ = writeln!(narrative, "  {}", paint(Role::Dim, "What you typed means:"));
                        narrative.push_str(&detail);
                    }
                }
                host.write_instant(&instant)?;
                if let Some(canonical) = accepted.first() {
                    let _ = writeln!(
                        narrative,
                        "  {} {}",
                        paint(Role::Warning, "Expected:"),
                        paint(Role::Command, canonical)
                    );
                }
                let _ = writeln!(narrative, "  {}", paint(Role::Note, note));
                host.write_narrative(&narrative)?;
                return Ok(AnswerOutcome::Wrong);
            }
        }
        Question::ReadOutput { command, output, prompt, choices, answer, note, .. } => {
            let mut instant = format!("  {}\n", paint(Role::Command, &format!("$ {command}")));
            for line in output.trim_end_matches('\n').lines() {
                let _ = writeln!(instant, "    {line}");
            }
            host.write_instant(&instant)?;
            host.write_narrative(&format!("{prompt}\n"))?;
            let outcome = prompt_choice(q, choices, *answer, reader, host)?;
            if matches!(outcome, AnswerOutcome::Quit | AnswerOutcome::Eof | AnswerOutcome::SwitchToWatch) {
                return Ok(outcome);
            }
            if outcome == AnswerOutcome::Correct {
                host.write_instant(&format!("  {}\n", paint(Role::Success, "Correct!")))?;
            } else {
                host.write_instant(&format!(
                    "  {} {}\n",
                    paint(Role::Warning, "Not quite."),
                    paint(Role::Warning, &format!("Answer: {}) {}", answer + 1, choices[*answer]))
                ))?;
            }
            let mut narrative = String::new();
            if let Some(described) = crate::outputs::describe(output) {
                for line in described.text.trim_end().lines() {
                    let _ = writeln!(narrative, "  {line}");
                }
            }
            let _ = writeln!(narrative, "  {}", paint(Role::Note, note));
            host.write_narrative(&narrative)?;
            Ok(outcome)
        }
        Question::SpotDanger { prompt, command, choices, answer, risk, note, .. } => {
            if let Some(cmd) = command {
                host.write_instant(&format!("  {}\n", paint(Role::Command, &format!("$ {cmd}"))))?;
            }
            host.write_narrative(&format!("{prompt}\n"))?;
            let outcome = prompt_choice(q, choices, *answer, reader, host)?;
            if matches!(outcome, AnswerOutcome::Quit | AnswerOutcome::Eof | AnswerOutcome::SwitchToWatch) {
                return Ok(outcome);
            }
            if outcome == AnswerOutcome::Correct {
                host.write_instant(&format!("  {}\n", paint(Role::Success, "Correct!")))?;
            } else {
                host.write_instant(&format!(
                    "  {} {}\n",
                    paint(Role::Warning, "Not quite."),
                    paint(Role::Warning, &format!("Answer: {}) {}", answer + 1, choices[*answer]))
                ))?;
            }
            let narrative = format!(
                "  {}{} {}\n  {}\n",
                paint(Role::Warning, "! "),
                paint(Role::Risk, &format!("[{}]", risk.name())),
                paint(Role::Warning, risk.explanation()),
                paint(Role::Note, note)
            );
            host.write_narrative(&narrative)?;
            Ok(outcome)
        }
    }
}

fn print_choices(choices: &[String], host: &mut dyn LabHost) -> Result<()> {
    let mut out = String::new();
    for (idx, choice) in choices.iter().enumerate() {
        let _ = writeln!(out, "  {} {choice}", paint(Role::Dim, &format!("{})", idx + 1)));
    }
    host.write_instant(&out)?;
    Ok(())
}

fn prompt_choice<R: BufRead>(
    q: &Question,
    choices: &[String],
    answer: usize,
    reader: &mut R,
    host: &mut dyn LabHost,
) -> Result<AnswerOutcome> {
    let mut hint_step = 0usize;
    let max = choices.len();
    let mut options: Vec<ChoiceOption> = choices
        .iter()
        .enumerate()
        .map(|(idx, choice)| ChoiceOption {
            key: (idx + 1).to_string(),
            label: choice.clone(),
            detail: String::new(),
            locked: false,
        })
        .collect();
    options.push(ChoiceOption { key: "h".into(), label: "Show a hint".into(), detail: "clue".into(), locked: false });

    loop {
        match host.choose_option(
            "Choose an answer:",
            &options,
            0,
            "↑/↓ move · Enter pick · 1-4 hotkey · h hint · / commands · q quit",
            reader,
        )? {
            ChoiceResult::Picked(idx) => {
                if idx == max {
                    host.write_instant(&format!("  {} {}\n", paint(Role::Note, "Hint:"), q.hint_at(hint_step)))?;
                    hint_step = hint_step.saturating_add(1);
                    continue;
                }
                host.write_instant(&format!("  {} {}) {}\n", paint(Role::Dim, "Choice:"), idx + 1, choices[idx]))?;
                return Ok(if idx == answer { AnswerOutcome::Correct } else { AnswerOutcome::Wrong });
            }
            ChoiceResult::Cancelled => return Ok(AnswerOutcome::Quit),
            ChoiceResult::Eof => return Ok(AnswerOutcome::Eof),
            ChoiceResult::Slash(s) => {
                if run_menu_slash(&s, reader, host)? == SlashEffect::SwitchToWatch {
                    return Ok(AnswerOutcome::SwitchToWatch);
                }
            }
            ChoiceResult::Fallback => {
                print_choices(choices, host)?;
                loop {
                    let Some(line) = host.read_prompt(&format!("Choose 1-{max} ('hint', 'q'):"), reader)? else {
                        return Ok(AnswerOutcome::Eof);
                    };
                    if is_quit_cmd(&line) {
                        return Ok(AnswerOutcome::Quit);
                    }
                    if is_choice_hint_cmd(&line) {
                        host.write_instant(&format!("  {} {}\n", paint(Role::Note, "Hint:"), q.hint_at(hint_step)))?;
                        hint_step = hint_step.saturating_add(1);
                        continue;
                    }
                    match host.handle_slash(&line)? {
                        SlashEffect::NotSlash => {}
                        SlashEffect::Handled => continue,
                        SlashEffect::SwitchToWatch => return Ok(AnswerOutcome::SwitchToWatch),
                    }
                    if let Ok(pick) = line.parse::<usize>()
                        && (1..=max).contains(&pick)
                    {
                        return Ok(if pick - 1 == answer { AnswerOutcome::Correct } else { AnswerOutcome::Wrong });
                    }
                    host.write_instant(&format!(
                        "  {}\n",
                        paint(Role::Dim, &format!("Enter a number from 1 to {max}, 'hint', or 'q' to quit."))
                    ))?;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Streak & badge notices
// ---------------------------------------------------------------------------

fn format_streak_event(out: &mut String, event: &StreakEvent, progress: &Progress) {
    match event {
        StreakEvent::Started { streak } => {
            let _ = writeln!(out, "  {}", paint(Role::Accent, &format!("Streak started: {streak} day!")));
        }
        StreakEvent::Advanced { streak, freeze_earned } => {
            let bonus = if *freeze_earned {
                format!(" +1 streak freeze earned ({}/{MAX_FREEZES})!", progress.freezes)
            } else {
                String::new()
            };
            let _ = writeln!(out, "  {}", paint(Role::Success, &format!("Streak: {streak} days!{bonus}")));
        }
        StreakEvent::FreezeUsed { streak, freezes_left } => {
            let _ = writeln!(
                out,
                "  {}",
                paint(
                    Role::Note,
                    &format!(
                        "Streak freeze used! Your streak is safe at {streak} days ({freezes_left} freeze(s) left)."
                    )
                )
            );
        }
        StreakEvent::SameDay => {
            if progress.streak > 0 {
                let _ = writeln!(
                    out,
                    "  {}",
                    paint(Role::Dim, &format!("Streak: {} day(s) (active today)", progress.streak))
                );
            }
        }
    }
}

fn format_new_badges(out: &mut String, badge_ids: &[String]) {
    for id in badge_ids {
        if let Some(def) = progress::badge_def(id) {
            let _ = writeln!(
                out,
                "  {} {} ({}) — {}",
                paint(Role::Success, "Badge unlocked:"),
                paint(Role::Header, def.title),
                def.id,
                def.description
            );
        } else {
            let _ = writeln!(out, "  {} {id}", paint(Role::Success, "Badge unlocked:"));
        }
    }
}

fn award_streak_badges(progress: &mut Progress, today: Date) -> Vec<String> {
    let mut earned = Vec::new();
    for (threshold, badge_id) in [(3, "streak_3"), (7, "streak_7"), (30, "streak_30")] {
        if progress.streak >= threshold && progress.award_badge(badge_id, today) {
            earned.push(badge_id.to_string());
        }
    }
    earned
}

// ---------------------------------------------------------------------------
// Lesson loop (5 hearts, zero lockout timers)
// ---------------------------------------------------------------------------

enum ZeroHeartsAction {
    RetryFresh,
    BackToMenu,
    SwitchToWatch,
    QuitAll,
}

fn review_missed_items<R: BufRead>(
    missed: &[&Question],
    ctx: &mut LearnState<'_>,
    catalog: &[Command],
    reader: &mut R,
    host: &mut dyn LabHost,
) -> Result<Flow> {
    host.write_instant(&format!(
        "\n{}\n",
        paint(Role::Header, &format!("Quick review of {} missed item(s) — no hearts lost!", missed.len()))
    ))?;
    let mut review_xp = 0u32;
    let mut right = 0usize;
    let mut badges = Vec::new();
    let mut flow = Flow::Continue;

    for (idx, q) in missed.iter().enumerate() {
        let header = format!(
            "{} {}",
            paint(Role::Header, &format!("Missed {}/{}", idx + 1, missed.len())),
            paint(Role::Dim, "(no hearts)")
        );
        match ask_question(q, &header, catalog, reader, host)? {
            AnswerOutcome::Correct => {
                right += 1;
                review_xp = review_xp.saturating_add(XP_REVIEW_CORRECT);
                srs::record_answer(&mut ctx.progress.srs, q.id(), true, ctx.today);
                if let Some(mood) = feed_pet(ctx.pet_path, question_program(q).as_deref(), ctx.now_epoch) {
                    ctx.outcome.pet_fed = true;
                    host.on_pet_event(Some(mood), mood == Mood::Celebrating);
                }
                if q.is_spot_danger()
                    && let Some(b) = ctx.progress.record_danger_spotted(ctx.today)
                {
                    badges.push(b);
                }
                host.write_instant(&format!(
                    "  {}\n",
                    paint(Role::Success, &format!("Nice recovery! +{XP_REVIEW_CORRECT} XP"))
                ))?;
            }
            AnswerOutcome::Wrong => {
                srs::record_answer(&mut ctx.progress.srs, q.id(), false, ctx.today);
                host.write_instant(&format!("  {}\n", paint(Role::Note, "No heart lost — keep practicing!")))?;
            }
            AnswerOutcome::Quit => break,
            AnswerOutcome::Eof => {
                flow = Flow::QuitAll;
                break;
            }
            AnswerOutcome::SwitchToWatch => {
                flow = Flow::SwitchToWatch;
                break;
            }
        }
    }

    if review_xp > 0 {
        let ev = ctx.progress.award_xp(review_xp, ctx.today);
        badges.extend(award_streak_badges(&mut ctx.progress, ctx.today));
        let mut msg = format!(
            "\n{}\n",
            paint(Role::Success, &format!("Missed-item review done: {right}/{} right (+{review_xp} XP)", missed.len()))
        );
        format_streak_event(&mut msg, &ev, &ctx.progress);
        format_new_badges(&mut msg, &badges);
        host.write_instant(&msg)?;
    }
    save_progress(ctx.progress_path, &ctx.progress)?;
    if review_xp > 0 {
        host.on_pet_event(None, false);
    }
    Ok(flow)
}

fn post_review_choice<R: BufRead>(reader: &mut R, host: &mut dyn LabHost) -> Result<ZeroHeartsAction> {
    let options = [
        ChoiceOption {
            key: "t".into(),
            label: "Retry this lesson fresh".into(),
            detail: format!("{MAX_HEARTS} hearts"),
            locked: false,
        },
        ChoiceOption { key: "q".into(), label: "Back to menu".into(), detail: String::new(), locked: false },
    ];
    loop {
        match host.choose_option(
            "Ready to try again?",
            &options,
            0,
            "↑/↓ move · Enter select · / commands · q back",
            reader,
        )? {
            ChoiceResult::Picked(0) => return Ok(ZeroHeartsAction::RetryFresh),
            ChoiceResult::Picked(1) | ChoiceResult::Cancelled => return Ok(ZeroHeartsAction::BackToMenu),
            ChoiceResult::Picked(_) => return Ok(ZeroHeartsAction::BackToMenu),
            ChoiceResult::Eof => return Ok(ZeroHeartsAction::QuitAll),
            ChoiceResult::Slash(s) => {
                if run_menu_slash(&s, reader, host)? == SlashEffect::SwitchToWatch {
                    return Ok(ZeroHeartsAction::SwitchToWatch);
                }
            }
            ChoiceResult::Fallback => {
                let post = format!("\n  [t] Retry this lesson fresh ({MAX_HEARTS} hearts)\n  [q] Back to menu\n");
                host.write_instant(&post)?;
                loop {
                    let Some(next) = host.read_prompt("Choose [t/q]:", reader)? else {
                        return Ok(ZeroHeartsAction::QuitAll);
                    };
                    match next.to_ascii_lowercase().as_str() {
                        "t" | "retry" => return Ok(ZeroHeartsAction::RetryFresh),
                        "q" | "quit" | "/quit" | "b" | "back" => return Ok(ZeroHeartsAction::BackToMenu),
                        _ => {}
                    }
                    match host.handle_slash(&next)? {
                        SlashEffect::NotSlash => {}
                        SlashEffect::Handled => continue,
                        SlashEffect::SwitchToWatch => return Ok(ZeroHeartsAction::SwitchToWatch),
                    }
                    host.write_instant(&format!(
                        "  {}\n",
                        paint(Role::Dim, "Press 't' to retry fresh or 'q' for menu.")
                    ))?;
                }
            }
        }
    }
}

fn handle_zero_hearts<R: BufRead>(
    missed: &[&Question],
    ctx: &mut LearnState<'_>,
    catalog: &[Command],
    reader: &mut R,
    host: &mut dyn LabHost,
) -> Result<ZeroHeartsAction> {
    save_progress(ctx.progress_path, &ctx.progress)?;
    let options = [
        ChoiceOption {
            key: "r".into(),
            label: "Review missed items right now".into(),
            detail: "no hearts".into(),
            locked: false,
        },
        ChoiceOption {
            key: "t".into(),
            label: "Retry this lesson fresh".into(),
            detail: format!("{MAX_HEARTS} hearts"),
            locked: false,
        },
        ChoiceOption { key: "q".into(), label: "Back to menu".into(), detail: String::new(), locked: false },
    ];

    loop {
        match host.choose_option(
            "Out of hearts — no timer and no waiting!",
            &options,
            0,
            "↑/↓ move · Enter select · / commands · q back",
            reader,
        )? {
            ChoiceResult::Picked(0) => {
                match review_missed_items(missed, ctx, catalog, reader, host)? {
                    Flow::SwitchToWatch => return Ok(ZeroHeartsAction::SwitchToWatch),
                    Flow::QuitAll => return Ok(ZeroHeartsAction::QuitAll),
                    Flow::Continue => {}
                }
                return post_review_choice(reader, host);
            }
            ChoiceResult::Picked(1) => return Ok(ZeroHeartsAction::RetryFresh),
            ChoiceResult::Picked(2) | ChoiceResult::Cancelled => return Ok(ZeroHeartsAction::BackToMenu),
            ChoiceResult::Picked(_) => return Ok(ZeroHeartsAction::BackToMenu),
            ChoiceResult::Eof => return Ok(ZeroHeartsAction::QuitAll),
            ChoiceResult::Slash(s) => {
                if run_menu_slash(&s, reader, host)? == SlashEffect::SwitchToWatch {
                    return Ok(ZeroHeartsAction::SwitchToWatch);
                }
            }
            ChoiceResult::Fallback => {
                let menu = format!(
                    "\n{}\n  [r] Review missed items right now (no hearts)\n  [t] Retry this lesson fresh ({MAX_HEARTS} hearts)\n  [q] Back to menu\n",
                    paint(Role::Warning, "Out of hearts — no timer and no waiting!")
                );
                host.write_instant(&menu)?;
                loop {
                    let Some(line) = host.read_prompt("Choose [r/t/q]:", reader)? else {
                        return Ok(ZeroHeartsAction::QuitAll);
                    };
                    match line.to_ascii_lowercase().as_str() {
                        "r" | "review" => {
                            match review_missed_items(missed, ctx, catalog, reader, host)? {
                                Flow::SwitchToWatch => return Ok(ZeroHeartsAction::SwitchToWatch),
                                Flow::QuitAll => return Ok(ZeroHeartsAction::QuitAll),
                                Flow::Continue => {}
                            }
                            return post_review_choice(reader, host);
                        }
                        "t" | "retry" => return Ok(ZeroHeartsAction::RetryFresh),
                        "q" | "quit" | "/quit" | "b" | "back" => return Ok(ZeroHeartsAction::BackToMenu),
                        _ => {}
                    }
                    match host.handle_slash(&line)? {
                        SlashEffect::NotSlash => {}
                        SlashEffect::Handled => continue,
                        SlashEffect::SwitchToWatch => return Ok(ZeroHeartsAction::SwitchToWatch),
                    }
                    host.write_instant(&format!(
                        "  {}\n",
                        paint(Role::Dim, "Choose 'r' to review, 't' to retry fresh, or 'q' for menu.")
                    ))?;
                }
            }
        }
    }
}

struct LearnState<'a> {
    progress: Progress,
    today: Date,
    now_epoch: u64,
    daily_goal: u32,
    progress_path: Option<&'a Path>,
    pet_path: Option<&'a Path>,
    practice_root: Option<&'a Path>,
    recording_dir: Option<&'a Path>,
    outcome: SessionOutcome,
}

fn run_lesson<R: BufRead>(
    units: &[Unit],
    unit: &Unit,
    lesson: &Lesson,
    ctx: &mut LearnState<'_>,
    catalog: &[Command],
    reader: &mut R,
    host: &mut dyn LabHost,
) -> Result<Flow> {
    'attempt: loop {
        let mut hearts = MAX_HEARTS;
        let mut combo = 0u32;
        let mut lesson_xp = 0u32;
        let mut misses = 0usize;
        let mut queue: VecDeque<&Question> = lesson.questions.iter().collect();
        let mut requeued: HashSet<String> = HashSet::new();
        let mut missed_questions: Vec<&Question> = Vec::new();
        let mut missed_seen: HashSet<String> = HashSet::new();
        let mut mid_badges: Vec<String> = Vec::new();
        let total_initial = lesson.questions.len();
        let mut seen_initial = 0usize;

        host.write_instant(&format!(
            "\n{} — {}\n",
            paint(Role::Header, &format!("Unit {}: {}", unit.number, unit.title)),
            paint(Role::Accent, &lesson.title)
        ))?;
        host.write_narrative(&format!("  {}\n", paint(Role::Dim, &lesson.intro)))?;

        while let Some(q) = queue.pop_front() {
            let is_retry = seen_initial >= total_initial;
            if !is_retry {
                seen_initial += 1;
            }
            let counter = if is_retry {
                format!("Retry ({} left)", queue.len() + 1)
            } else {
                format!("Question {seen_initial}/{total_initial}")
            };
            let combo_part = if combo >= 2 {
                format!("  {}", paint(Role::Accent, &format!("{combo}x combo")))
            } else {
                String::new()
            };
            let header = format!(
                "{}  {}{combo_part}",
                paint(Role::Header, &counter),
                paint(Role::Warning, &hearts_meter(hearts))
            );

            match ask_question(q, &header, catalog, reader, host)? {
                AnswerOutcome::Correct => {
                    combo = combo.saturating_add(1);
                    let bonus = if combo >= 3 { XP_COMBO_BONUS } else { 0 };
                    let gained = XP_CORRECT + bonus;
                    lesson_xp = lesson_xp.saturating_add(gained);
                    srs::record_answer(&mut ctx.progress.srs, q.id(), true, ctx.today);
                    if let Some(mood) = feed_pet(ctx.pet_path, question_program(q).as_deref(), ctx.now_epoch) {
                        ctx.outcome.pet_fed = true;
                        host.on_pet_event(Some(mood), mood == Mood::Celebrating);
                    }
                    if q.is_spot_danger()
                        && let Some(badge) = ctx.progress.record_danger_spotted(ctx.today)
                    {
                        mid_badges.push(badge);
                    }
                    let msg =
                        if bonus > 0 { format!("+{gained} XP ({combo}x combo!)") } else { format!("+{gained} XP") };
                    host.write_instant(&format!("  {}\n", paint(Role::Success, &msg)))?;
                }
                AnswerOutcome::Wrong => {
                    hearts = hearts.saturating_sub(1);
                    combo = 0;
                    misses += 1;
                    srs::record_answer(&mut ctx.progress.srs, q.id(), false, ctx.today);
                    host.on_pet_event(Some(Mood::Alert), false);
                    if missed_seen.insert(q.id().to_string()) {
                        missed_questions.push(q);
                    }
                    let will_retry = requeued.insert(q.id().to_string());
                    if will_retry {
                        queue.push_back(q);
                    }
                    let retry_hint = if will_retry && hearts > 0 {
                        format!(" {}", paint(Role::Dim, "(you'll get another try at the end)"))
                    } else {
                        String::new()
                    };
                    host.write_instant(&format!(
                        "  {}{retry_hint}\n",
                        paint(Role::Warning, &format!("Hearts: {}", hearts_meter(hearts)))
                    ))?;
                    if hearts == 0 {
                        match handle_zero_hearts(&missed_questions, ctx, catalog, reader, host)? {
                            ZeroHeartsAction::RetryFresh => {
                                host.write_instant(&format!(
                                    "\n{}\n",
                                    paint(Role::Accent, "Fresh start! 5 hearts restored.")
                                ))?;
                                continue 'attempt;
                            }
                            ZeroHeartsAction::BackToMenu => return Ok(Flow::Continue),
                            ZeroHeartsAction::SwitchToWatch => return Ok(Flow::SwitchToWatch),
                            ZeroHeartsAction::QuitAll => return Ok(Flow::QuitAll),
                        }
                    }
                }
                AnswerOutcome::Quit => {
                    save_progress(ctx.progress_path, &ctx.progress)?;
                    host.write_instant(&format!(
                        "{}\n",
                        paint(Role::Dim, "Left lesson early — progress so far is saved.")
                    ))?;
                    return Ok(Flow::Continue);
                }
                AnswerOutcome::Eof => {
                    save_progress(ctx.progress_path, &ctx.progress)?;
                    return Ok(Flow::QuitAll);
                }
                AnswerOutcome::SwitchToWatch => {
                    save_progress(ctx.progress_path, &ctx.progress)?;
                    return Ok(Flow::SwitchToWatch);
                }
            }
        }

        // Lesson cleared!
        let flawless = misses == 0 && hearts == MAX_HEARTS;
        let flawless_xp = if flawless { XP_FLAWLESS_BONUS } else { 0 };
        let total_xp = lesson_xp.saturating_add(XP_LESSON_CLEAR).saturating_add(flawless_xp);
        let streak_event = ctx.progress.award_xp(total_xp, ctx.today);
        let mut earned_badges = mid_badges;
        earned_badges.extend(ctx.progress.complete_lesson(&lesson.id, flawless, ctx.today));

        let ids = unit_lesson_ids(unit);
        let mut unlocked_species: Option<Species> = None;
        if ctx.progress.is_unit_completed(&ids) {
            if ctx.progress.award_badge(&unit.badge, ctx.today) {
                earned_badges.push(unit.badge.clone());
            }
            if let Some(sp_name) = &unit.unlocks_species
                && let Ok(sp) = Species::parse(sp_name)
                && unlock_pet_species(ctx.pet_path, sp)
            {
                unlocked_species = Some(sp);
                ctx.outcome.species_unlocked = true;
            }
            if units.iter().filter(|u| !u.is_side_quest()).all(|u| ctx.progress.is_unit_completed(&unit_lesson_ids(u)))
                && ctx.progress.award_badge("shell_grad", ctx.today)
            {
                earned_badges.push("shell_grad".to_string());
            }
        }

        save_progress(ctx.progress_path, &ctx.progress)?;
        ctx.outcome.lessons_completed += 1;
        host.on_pet_event(Some(Mood::Celebrating), true);

        let mut summary = String::new();
        let _ = writeln!(summary);
        let _ = writeln!(
            summary,
            "{}",
            paint(
                Role::Success,
                &format!("Lesson complete: {}!{}", lesson.title, if flawless { " (Flawless!)" } else { "" })
            )
        );
        let flawless_note = if flawless { format!(", flawless +{XP_FLAWLESS_BONUS}") } else { String::new() };
        let _ = writeln!(
            summary,
            "  XP: +{} (answers +{lesson_xp}, clear +{XP_LESSON_CLEAR}{flawless_note}) · Total: {}",
            paint(Role::Value, &total_xp.to_string()),
            paint(Role::Value, &ctx.progress.xp.to_string())
        );
        let daily = ctx.progress.daily_xp_on(ctx.today);
        let _ = writeln!(
            summary,
            "  Daily goal: {} {}/{} XP{}",
            paint(Role::Accent, &xp_bar(daily, ctx.daily_goal)),
            daily,
            ctx.daily_goal,
            if daily >= ctx.daily_goal { format!(" {}", paint(Role::Success, "Goal reached!")) } else { String::new() }
        );
        format_streak_event(&mut summary, &streak_event, &ctx.progress);
        format_new_badges(&mut summary, &earned_badges);
        host.write_instant(&summary)?;

        let pet_now = load_pet(ctx.pet_path);
        if let Some(sp) = unlocked_species {
            let lines = vec![
                paint(Role::Success, &format!("NEW PET SPECIES UNLOCKED: {}!", sp.label())),
                format!("You finished Unit {} ({})!", unit.number, unit.title),
                format!("Switch anytime with: /pet species {} (or huh pet --species {})", sp.name(), sp.name()),
                paint(Role::Dim, "Your #! companion is celebrating with you!"),
            ];
            host.write_narrative(&format!("\n{}", render_pet_celebration(&pet_now.name, sp, &lines)))?;
        } else {
            let lines = vec![
                paint(Role::Success, &format!("{} cheers you on!", pet_now.name)),
                format!("Lessons cleared: {} · Total XP: {}", ctx.progress.completed_lessons.len(), ctx.progress.xp),
                paint(Role::Dim, "Keep going or type `progress` anytime."),
                String::new(),
            ];
            host.write_narrative(&format!("\n{}", render_pet_celebration(&pet_now.name, pet_now.species, &lines)))?;
        }
        return Ok(Flow::Continue);
    }
}

// ---------------------------------------------------------------------------
// Spaced repetition review mode (`review`)
// ---------------------------------------------------------------------------

fn run_review<R: BufRead>(
    units: &[Unit],
    ctx: &mut LearnState<'_>,
    catalog: &[Command],
    reader: &mut R,
    host: &mut dyn LabHost,
) -> Result<Flow> {
    let due_ids: Vec<(String, u8)> = srs::due_cards(&ctx.progress.srs, ctx.today, ctx.progress.srs.len())
        .into_iter()
        .filter(|c| lessons::find_question(units, &c.question_id).is_some())
        .take(MAX_REVIEW_CARDS)
        .map(|c| (c.question_id.clone(), c.box_level))
        .collect();

    if due_ids.is_empty() {
        if ctx.progress.srs.is_empty() {
            host.write_instant(&format!(
                "{}\n",
                paint(
                    Role::Note,
                    "No review cards yet! Any questions you miss during Lab lessons will appear here for spaced practice."
                )
            ))?;
        } else {
            host.write_instant(&format!(
                "{}\n",
                paint(
                    Role::Success,
                    &format!(
                        "All caught up! 0 review cards due today ({} card(s) scheduled for later).",
                        ctx.progress.srs.len()
                    )
                )
            ))?;
        }
        return Ok(Flow::Continue);
    }

    host.write_instant(&format!(
        "{}\n",
        paint(Role::Header, &format!("Spaced Review — {} card(s) due today (no hearts lost!)", due_ids.len()))
    ))?;

    let mut review_xp = 0u32;
    let mut right = 0usize;
    let mut answered = 0usize;
    let mut badges = Vec::new();
    let mut flow = Flow::Continue;

    for (idx, (qid, old_box)) in due_ids.iter().enumerate() {
        let Some((unit, lesson, q)) = lessons::find_question(units, qid) else {
            continue;
        };
        let header = format!(
            "{}  {}",
            paint(Role::Header, &format!("Review {}/{}", idx + 1, due_ids.len())),
            paint(Role::Dim, &format!("Box {old_box} · U{}: {}", unit.number, lesson.title))
        );
        match ask_question(q, &header, catalog, reader, host)? {
            AnswerOutcome::Correct => {
                answered += 1;
                right += 1;
                review_xp = review_xp.saturating_add(XP_REVIEW_CORRECT);
                srs::record_answer(&mut ctx.progress.srs, qid, true, ctx.today);
                if let Some(mood) = feed_pet(ctx.pet_path, question_program(q).as_deref(), ctx.now_epoch) {
                    ctx.outcome.pet_fed = true;
                    host.on_pet_event(Some(mood), mood == Mood::Celebrating);
                }
                if q.is_spot_danger()
                    && let Some(b) = ctx.progress.record_danger_spotted(ctx.today)
                {
                    badges.push(b);
                }
                let card_info = ctx
                    .progress
                    .srs
                    .iter()
                    .find(|c| c.question_id == *qid)
                    .map(|c| format!(" (Box {} -> Box {}, next due {})", old_box, c.box_level, c.due_day))
                    .unwrap_or_default();
                host.write_instant(&format!(
                    "  {}\n",
                    paint(Role::Success, &format!("+{XP_REVIEW_CORRECT} XP{card_info}"))
                ))?;
            }
            AnswerOutcome::Wrong => {
                answered += 1;
                srs::record_answer(&mut ctx.progress.srs, qid, false, ctx.today);
                host.on_pet_event(Some(Mood::Alert), false);
                host.write_instant(&format!(
                    "  {}\n",
                    paint(Role::Note, "Reset to Box 0 — no hearts lost, you'll see it again soon!")
                ))?;
            }
            AnswerOutcome::Quit => break,
            AnswerOutcome::Eof => {
                flow = Flow::QuitAll;
                break;
            }
            AnswerOutcome::SwitchToWatch => {
                flow = Flow::SwitchToWatch;
                break;
            }
        }
    }

    let streak_event = if review_xp > 0 {
        let ev = ctx.progress.award_xp(review_xp, ctx.today);
        badges.extend(award_streak_badges(&mut ctx.progress, ctx.today));
        Some(ev)
    } else {
        None
    };

    save_progress(ctx.progress_path, &ctx.progress)?;
    if review_xp > 0 {
        host.on_pet_event(Some(Mood::Celebrating), true);
    }

    let mut summary =
        format!("\n{}\n", paint(Role::Header, &format!("Review complete: {right}/{answered} right (+{review_xp} XP)")));
    if let Some(ev) = &streak_event {
        format_streak_event(&mut summary, ev, &ctx.progress);
    }
    format_new_badges(&mut summary, &badges);
    host.write_instant(&summary)?;
    Ok(flow)
}

// ---------------------------------------------------------------------------
// Hands-on practice sandbox runner (`practice [unit]`)
// ---------------------------------------------------------------------------

fn practice_program(practice: &Practice) -> Option<&str> {
    for check in &practice.checks {
        if let Check::RecordedOutput { program, .. } = check {
            return Some(program.as_str());
        }
    }
    for check in &practice.checks {
        match check {
            Check::FileModeHas { .. } | Check::FileModeEq { .. } => return Some("chmod"),
            Check::IsSymlink { .. } => return Some("ln"),
            _ => {}
        }
    }
    Some("practice")
}

fn shell_cd_arg(path: &Path) -> String {
    let s = path.display().to_string();
    if s.chars().any(|c| c.is_whitespace() || matches!(c, '\'' | '"' | '$' | '`' | '\\' | ';' | '&' | '|')) {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else {
        s
    }
}

fn format_practice_report(report: &Report, practice: &Practice) -> String {
    let mut out = String::new();
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", paint(Role::Header, "Checklist:"));
    for (check, outcome) in practice.checks.iter().zip(&report.outcomes) {
        if outcome.passed {
            let _ = writeln!(
                out,
                "  {} {} — {}",
                paint(Role::Success, "[ok]"),
                outcome.label,
                paint(Role::Dim, &outcome.detail)
            );
        } else {
            let _ = writeln!(
                out,
                "  {} {} — {}",
                paint(Role::Dim, "[..]"),
                outcome.label,
                paint(Role::Note, &outcome.detail)
            );
            if outcome.needs_fallback
                && let Check::RecordedOutput { fallback_prompt, .. } = check
            {
                let _ = writeln!(
                    out,
                    "       {} {fallback_prompt}",
                    paint(Role::Accent, "No terminal recording? Answer here:")
                );
            }
        }
    }
    out
}

fn run_practice_lab<R: BufRead>(
    unit: &Unit,
    practice: &Practice,
    ctx: &mut LearnState<'_>,
    reader: &mut R,
    host: &mut dyn LabHost,
) -> Result<Flow> {
    let Some(root) = ctx.practice_root else {
        bail!("cannot find practice directory ($XDG_DATA_HOME or $HOME)");
    };

    let mut sandbox = practice::Sandbox::prepare(root, practice)?;
    let mut hint_step = 0usize;
    let mut fallback_answers: Vec<String> = Vec::new();
    let mut reported_cmds = 0u64;

    let mut intro = String::new();
    let _ = writeln!(intro);
    let _ = writeln!(
        intro,
        "{} — {}",
        paint(Role::Header, &format!("Hands-On Practice (Unit {})", unit.number)),
        paint(Role::Accent, &practice.title)
    );
    let _ = writeln!(intro, "  Goal: {}", practice.goal);
    let _ = writeln!(intro, "  Sandbox: {}", paint(Role::Command, &format!("cd {}", shell_cd_arg(sandbox.dir()))));
    let _ = writeln!(
        intro,
        "  {}",
        paint(Role::Dim, "Work in another terminal inside that folder; huh never runs your commands.")
    );
    host.write_instant(&intro)?;

    let mut latest_recorded = practice::latest_recorded_from_env_or_dir(ctx.recording_dir);
    let mut report = practice::evaluate(sandbox.dir(), practice, latest_recorded.as_ref(), &fallback_answers)?;
    host.write_instant(&format_practice_report(&report, practice))?;

    if !report.all_passed() {
        host.write_instant(&format!(
            "\n  {}\n",
            paint(Role::Dim, "Controls: Enter to check · 'hint' for a clue · 'reset' to recreate files · 'q' to quit")
        ))?;

        let mut need_prompt = true;
        loop {
            let has_fallback = report.outcomes.iter().any(|o| !o.passed && o.needs_fallback);
            let prompt = if has_fallback {
                "Press Enter to check, type answer, 'hint', 'reset', or 'q':"
            } else {
                "Press Enter to check ('hint', 'reset', 'q'):"
            };
            if need_prompt {
                if !host.handles_practice_prompt() {
                    host.write_instant(&format!("{} ", paint(Role::Dim, prompt)))?;
                }
                need_prompt = false;
            }

            let line_opt = match host.wait_practice_turn(sandbox.dir(), practice, &mut reported_cmds) {
                Some(PracticePoll::Quit) => return Ok(Flow::QuitAll),
                Some(PracticePoll::AutoUpdate { report: auto_report, recorded, nudged }) => {
                    if recorded.is_some() {
                        latest_recorded = recorded;
                    }
                    let next_report = if fallback_answers.is_empty() {
                        auto_report
                    } else {
                        practice::evaluate(sandbox.dir(), practice, latest_recorded.as_ref(), &fallback_answers)?
                    };
                    if next_report != report {
                        report = next_report;
                        host.write_instant(&format_practice_report(&report, practice))?;
                        need_prompt = true;
                    } else if nudged {
                        need_prompt = true;
                    }
                    if report.all_passed() {
                        break;
                    }
                    continue;
                }
                Some(PracticePoll::UserLine(line)) => {
                    need_prompt = true;
                    line
                }
                None => {
                    need_prompt = true;
                    host.read_line(reader)?
                }
            };

            let Some(line) = line_opt else {
                return Ok(Flow::QuitAll);
            };
            let lower = line.to_ascii_lowercase();
            if is_quit_cmd(&lower) {
                host.write_instant(&format!(
                    "{}\n",
                    paint(Role::Dim, "Left practice lab early — type `practice` in Lab anytime to resume.")
                ))?;
                return Ok(Flow::Continue);
            }
            if lower == "hint" || lower == "/hint" || lower == "?" {
                if !practice.hints.is_empty() {
                    let hint = &practice.hints[hint_step.min(practice.hints.len() - 1)];
                    host.write_instant(&format!("  {} {hint}\n", paint(Role::Note, "Hint:")))?;
                    hint_step = hint_step.saturating_add(1);
                }
                continue;
            }
            if lower == "reset" || lower == "/reset" {
                sandbox = practice::Sandbox::prepare(root, practice)?;
                fallback_answers.clear();
                hint_step = 0;
                host.write_instant(&format!("  {}\n", paint(Role::Note, "Sandbox reset to fresh starting files.")))?;
                latest_recorded = practice::latest_recorded_from_env_or_dir(ctx.recording_dir);
                report = practice::evaluate(sandbox.dir(), practice, latest_recorded.as_ref(), &fallback_answers)?;
                host.write_instant(&format_practice_report(&report, practice))?;
                continue;
            }
            if lower != "/check" {
                match host.handle_slash(&line)? {
                    SlashEffect::NotSlash => {}
                    SlashEffect::Handled => continue,
                    SlashEffect::SwitchToWatch => return Ok(Flow::SwitchToWatch),
                }
            }
            if !line.is_empty() && lower != "check" && lower != "/check" {
                if has_fallback {
                    fallback_answers.push(line);
                } else {
                    host.write_instant(&format!(
                        "  {}\n",
                        paint(Role::Dim, "Press Enter to check your sandbox, 'hint', 'reset', or 'q' to quit.")
                    ))?;
                    continue;
                }
            }

            if let Some(rec) = practice::latest_recorded_from_env_or_dir(ctx.recording_dir) {
                latest_recorded = Some(rec);
            }
            report = practice::evaluate(sandbox.dir(), practice, latest_recorded.as_ref(), &fallback_answers)?;
            host.write_instant(&format_practice_report(&report, practice))?;
            if report.all_passed() {
                break;
            }
        }
    }

    let streak_event = ctx.progress.award_xp(XP_PRACTICE_CLEAR, ctx.today);
    let earned_badges = ctx.progress.complete_practice(&practice.id, ctx.today);
    save_progress(ctx.progress_path, &ctx.progress)?;
    if feed_pet(ctx.pet_path, practice_program(practice), ctx.now_epoch).is_some() {
        ctx.outcome.pet_fed = true;
    }
    ctx.outcome.practices_completed += 1;
    let _ = sandbox.cleanup();
    host.on_pet_event(Some(Mood::Celebrating), true);

    let mut summary = String::new();
    let _ = writeln!(summary);
    let _ = writeln!(summary, "{}", paint(Role::Success, &format!("Practice complete: {}!", practice.title)));
    let _ = writeln!(
        summary,
        "  XP: +{} · Total: {}",
        paint(Role::Value, &XP_PRACTICE_CLEAR.to_string()),
        paint(Role::Value, &ctx.progress.xp.to_string())
    );
    let daily = ctx.progress.daily_xp_on(ctx.today);
    let _ = writeln!(
        summary,
        "  Daily goal: {} {}/{} XP{}",
        paint(Role::Accent, &xp_bar(daily, ctx.daily_goal)),
        daily,
        ctx.daily_goal,
        if daily >= ctx.daily_goal { format!(" {}", paint(Role::Success, "Goal reached!")) } else { String::new() }
    );
    format_streak_event(&mut summary, &streak_event, &ctx.progress);
    format_new_badges(&mut summary, &earned_badges);
    host.write_instant(&summary)?;

    let pet_now = load_pet(ctx.pet_path);
    let lines = vec![
        paint(Role::Success, &format!("{} loves hands-on practice!", pet_now.name)),
        format!("Labs cleared: {} · Total XP: {}", ctx.progress.completed_practices.len(), ctx.progress.xp),
        paint(Role::Dim, "Sandbox folder cleaned up automatically."),
        String::new(),
    ];
    host.write_narrative(&format!("\n{}", render_pet_celebration(&pet_now.name, pet_now.species, &lines)))?;
    Ok(Flow::Continue)
}

fn practice_units_summary(units: &[Unit]) -> String {
    units
        .iter()
        .filter(|u| u.practice.is_some())
        .map(|u| format!("{}: {}", u.number, u.id))
        .collect::<Vec<_>>()
        .join(", ")
}

fn run_practice_target<R: BufRead>(
    query: &str,
    units: &[Unit],
    ctx: &mut LearnState<'_>,
    reader: &mut R,
    host: &mut dyn LabHost,
) -> Result<Flow> {
    let Some(unit) = lessons::find_unit(units, query) else {
        bail!("unknown practice unit \"{query}\"; units with practice labs: {}", practice_units_summary(units));
    };
    let u_idx = units.iter().position(|u| u.id == unit.id).unwrap_or(0);
    let prev = prev_unit_ids(units, u_idx);
    if !ctx.progress.is_unit_unlocked(u_idx, prev.as_deref()) {
        host.write_instant(&format!("{}\n", paint(Role::Note, &locked_unit_message(units, u_idx))))?;
        return Ok(Flow::Continue);
    }
    let Some(practice) = &unit.practice else {
        host.write_instant(&format!(
            "{}\n",
            paint(
                Role::Note,
                &format!(
                    "Unit {} ({}) does not have a hands-on practice lab. Try one of: {}",
                    unit.number,
                    unit.title,
                    practice_units_summary(units)
                )
            )
        ))?;
        return Ok(Flow::Continue);
    };
    run_practice_lab(unit, practice, ctx, reader, host)
}

fn render_practice_list(units: &[Unit], progress: &Progress) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{}", paint(Role::Header, "Lab — Hands-On Practice Labs"));
    let _ = writeln!(
        out,
        "  {}",
        paint(Role::Dim, "Safe throwaway folders under ~/.local/share/huh/practice; nothing is ever run by huh.")
    );
    let _ = writeln!(out);
    for (u_idx, unit) in units.iter().enumerate() {
        let Some(practice) = &unit.practice else {
            continue;
        };
        let prev = prev_unit_ids(units, u_idx);
        let unlocked = progress.is_unit_unlocked(u_idx, prev.as_deref());
        let done = progress.is_practice_completed(&practice.id);
        let mark = if done {
            paint(Role::Success, "[x]")
        } else if unlocked {
            paint(Role::Accent, "[ ]")
        } else {
            paint(Role::Dim, "[lock]")
        };
        let tag = if done {
            paint(Role::Success, "(done)")
        } else if unlocked {
            paint(Role::Accent, "(open)")
        } else {
            paint(Role::Dim, "(locked)")
        };
        let _ = writeln!(out, "  Unit {}: {mark} {} {tag}", unit.number, practice.title);
    }
    out
}

fn run_practice_menu<R: BufRead>(
    units: &[Unit],
    ctx: &mut LearnState<'_>,
    interactive: bool,
    reader: &mut R,
    host: &mut dyn LabHost,
) -> Result<Flow> {
    if !interactive {
        host.write_instant(&format!(
            "{}\n{}\n",
            render_practice_list(units, &ctx.progress),
            paint(Role::Dim, "Open `/mode lab` in an interactive terminal to launch a hands-on lab.")
        ))?;
        return Ok(Flow::Continue);
    }

    loop {
        let mut practice_units: Vec<(usize, &Unit, &Practice)> = Vec::new();
        let mut options: Vec<ChoiceOption> = Vec::new();
        let mut first_open_idx: Option<usize> = None;
        let mut first_unlocked_idx: Option<usize> = None;
        for (u_idx, unit) in units.iter().enumerate() {
            let Some(practice) = &unit.practice else {
                continue;
            };
            let prev = prev_unit_ids(units, u_idx);
            let unlocked = ctx.progress.is_unit_unlocked(u_idx, prev.as_deref());
            let done = ctx.progress.is_practice_completed(&practice.id);
            let item_idx = options.len();
            if unlocked && first_unlocked_idx.is_none() {
                first_unlocked_idx = Some(item_idx);
            }
            if unlocked && !done && first_open_idx.is_none() {
                first_open_idx = Some(item_idx);
            }
            options.push(ChoiceOption {
                key: unit.number.to_string(),
                label: format!("Unit {}: {}", unit.number, practice.title),
                detail: if done {
                    "done".into()
                } else if unlocked {
                    "open".into()
                } else {
                    "locked".into()
                },
                locked: !unlocked,
            });
            practice_units.push((u_idx, unit, practice));
        }
        options.push(ChoiceOption {
            key: "b".into(),
            label: "Back to menu".into(),
            detail: String::new(),
            locked: false,
        });
        let default_idx = first_open_idx.or(first_unlocked_idx).unwrap_or(0);

        match host.choose_option(
            "Lab — Hands-On Practice Labs",
            &options,
            default_idx,
            "↑/↓ move · Enter launch · / commands · b/Esc back",
            reader,
        )? {
            ChoiceResult::Picked(idx) => {
                if let Some((u_idx, unit, practice)) = practice_units.get(idx).copied() {
                    let prev = prev_unit_ids(units, u_idx);
                    if !ctx.progress.is_unit_unlocked(u_idx, prev.as_deref()) {
                        host.write_instant(&format!("  {}\n", paint(Role::Note, &locked_unit_message(units, u_idx))))?;
                        continue;
                    }
                    match run_practice_lab(unit, practice, ctx, reader, host)? {
                        Flow::Continue => {}
                        other => return Ok(other),
                    }
                    continue;
                }
                return Ok(Flow::Continue);
            }
            ChoiceResult::Cancelled => return Ok(Flow::Continue),
            ChoiceResult::Eof => return Ok(Flow::QuitAll),
            ChoiceResult::Slash(s) => {
                if run_menu_slash(&s, reader, host)? == SlashEffect::SwitchToWatch {
                    return Ok(Flow::SwitchToWatch);
                }
                continue;
            }
            ChoiceResult::Fallback => {}
        }

        host.write_instant(&format!(
            "\n{}\n{} ",
            render_practice_list(units, &ctx.progress),
            paint(Role::Dim, "Pick a unit number/name (Enter for next open lab, 'q' back):")
        ))?;

        let Some(line) = host.read_line(reader)? else {
            return Ok(Flow::QuitAll);
        };
        let lower = line.to_ascii_lowercase();
        if matches!(lower.as_str(), "b" | "back" | "q" | "quit" | "/quit") {
            return Ok(Flow::Continue);
        }
        match host.handle_slash(&line)? {
            SlashEffect::NotSlash => {}
            SlashEffect::Handled => continue,
            SlashEffect::SwitchToWatch => return Ok(Flow::SwitchToWatch),
        }
        if line.is_empty() {
            let next_unit = units
                .iter()
                .enumerate()
                .find(|(u_idx, u)| {
                    let prev = prev_unit_ids(units, *u_idx);
                    ctx.progress.is_unit_unlocked(*u_idx, prev.as_deref())
                        && u.practice.as_ref().is_some_and(|p| !ctx.progress.is_practice_completed(&p.id))
                })
                .or_else(|| {
                    units.iter().enumerate().find(|(u_idx, u)| {
                        let prev = prev_unit_ids(units, *u_idx);
                        ctx.progress.is_unit_unlocked(*u_idx, prev.as_deref()) && u.practice.is_some()
                    })
                });
            if let Some((_, unit)) = next_unit
                && let Some(practice) = &unit.practice
            {
                match run_practice_lab(unit, practice, ctx, reader, host)? {
                    Flow::Continue => {}
                    other => return Ok(other),
                }
            } else {
                host.write_instant(&format!("  {}\n", paint(Role::Note, "No unlocked practice labs available yet.")))?;
            }
            continue;
        }
        match run_practice_target(&line, units, ctx, reader, host) {
            Ok(Flow::Continue) => {}
            Ok(other) => return Ok(other),
            Err(err) => host.write_instant(&format!("  {}\n", paint(Role::Note, &format!("{err}"))))?,
        }
    }
}

// ---------------------------------------------------------------------------
// Interactive menus & unit/lesson selection
// ---------------------------------------------------------------------------

fn first_open_lesson_for_track(units: &[Unit], progress: &Progress, side_quests: bool) -> Option<(usize, usize)> {
    for (u_idx, unit) in units.iter().enumerate() {
        if unit.is_side_quest() != side_quests {
            continue;
        }
        let prev = prev_unit_ids(units, u_idx);
        if !progress.is_unit_unlocked(u_idx, prev.as_deref()) {
            continue;
        }
        for (l_idx, lesson) in unit.lessons.iter().enumerate() {
            let prev_l = l_idx.checked_sub(1).map(|i| unit.lessons[i].id.as_str());
            if progress.is_lesson_unlocked(true, l_idx, prev_l) && !progress.is_lesson_completed(&lesson.id) {
                return Some((u_idx, l_idx));
            }
        }
    }
    None
}

fn first_open_lesson(units: &[Unit], progress: &Progress) -> Option<(usize, usize)> {
    first_open_lesson_for_track(units, progress, false).or_else(|| first_open_lesson_for_track(units, progress, true))
}

fn run_unit_menu<R: BufRead>(
    unit_idx: usize,
    units: &[Unit],
    ctx: &mut LearnState<'_>,
    catalog: &[Command],
    reader: &mut R,
    host: &mut dyn LabHost,
) -> Result<Flow> {
    let unit = &units[unit_idx];
    loop {
        let mut options: Vec<ChoiceOption> = Vec::new();
        let mut default_idx = None;
        for (l_idx, lesson) in unit.lessons.iter().enumerate() {
            let prev_l = l_idx.checked_sub(1).map(|i| unit.lessons[i].id.as_str());
            let unlocked = ctx.progress.is_lesson_unlocked(true, l_idx, prev_l);
            let done = ctx.progress.is_lesson_completed(&lesson.id);
            if unlocked && !done && default_idx.is_none() {
                default_idx = Some(l_idx);
            }
            options.push(ChoiceOption {
                key: (l_idx + 1).to_string(),
                label: lesson.title.clone(),
                detail: if done {
                    "done".into()
                } else if unlocked {
                    "open".into()
                } else {
                    "locked".into()
                },
                locked: !unlocked,
            });
        }
        if let Some(practice) = &unit.practice {
            let p_done = ctx.progress.is_practice_completed(&practice.id);
            options.push(ChoiceOption {
                key: "p".into(),
                label: format!("Practice lab: {}", practice.title),
                detail: if p_done { "done".into() } else { "open".into() },
                locked: false,
            });
        }
        options.push(ChoiceOption {
            key: "b".into(),
            label: "Back to units".into(),
            detail: String::new(),
            locked: false,
        });

        let chosen_idx = match host.choose_option(
            &format!("Unit {}: {} — {}", unit.number, unit.title, unit.summary),
            &options,
            default_idx.unwrap_or(0),
            "↑/↓ move · Enter start · / commands · b/Esc back",
            reader,
        )? {
            ChoiceResult::Picked(idx) => {
                if idx < unit.lessons.len() {
                    idx
                } else if unit.practice.is_some() && idx == unit.lessons.len() {
                    if let Some(practice) = &unit.practice {
                        match run_practice_lab(unit, practice, ctx, reader, host)? {
                            Flow::Continue => {}
                            other => return Ok(other),
                        }
                    }
                    continue;
                } else {
                    return Ok(Flow::Continue);
                }
            }
            ChoiceResult::Cancelled => return Ok(Flow::Continue),
            ChoiceResult::Eof => return Ok(Flow::QuitAll),
            ChoiceResult::Slash(s) => {
                if run_menu_slash(&s, reader, host)? == SlashEffect::SwitchToWatch {
                    return Ok(Flow::SwitchToWatch);
                }
                continue;
            }
            ChoiceResult::Fallback => {
                let mut out = String::new();
                let _ = writeln!(out);
                let _ = writeln!(out, "{}", paint(Role::Header, &format!("Unit {}: {}", unit.number, unit.title)));
                let _ = writeln!(out, "  {}", paint(Role::Dim, &unit.summary));
                for (l_idx, lesson) in unit.lessons.iter().enumerate() {
                    let prev_l = l_idx.checked_sub(1).map(|i| unit.lessons[i].id.as_str());
                    let unlocked = ctx.progress.is_lesson_unlocked(true, l_idx, prev_l);
                    let done = ctx.progress.is_lesson_completed(&lesson.id);
                    let mark = if done {
                        paint(Role::Success, "[x]")
                    } else if unlocked {
                        paint(Role::Accent, "[ ]")
                    } else {
                        paint(Role::Dim, "[lock]")
                    };
                    let _ = writeln!(out, "  {}. {mark} {}", l_idx + 1, lesson.title);
                }
                if let Some(practice) = &unit.practice {
                    let p_mark = if ctx.progress.is_practice_completed(&practice.id) {
                        paint(Role::Success, "[x]")
                    } else {
                        paint(Role::Accent, "[ ]")
                    };
                    let _ = writeln!(out, "  p. {p_mark} Practice lab: {}", practice.title);
                }
                let practice_hint = if unit.practice.is_some() { ", 'p' practice" } else { "" };
                let _ = write!(
                    out,
                    "{} ",
                    paint(
                        Role::Dim,
                        &format!("Pick lesson 1-{} (Enter for next{practice_hint}, 'b' back):", unit.lessons.len())
                    )
                );
                host.write_instant(&out)?;
                let Some(line) = host.read_line(reader)? else {
                    return Ok(Flow::QuitAll);
                };
                let lower = line.to_ascii_lowercase();
                if matches!(lower.as_str(), "b" | "back" | "q" | "quit" | "/quit") {
                    return Ok(Flow::Continue);
                }
                match host.handle_slash(&line)? {
                    SlashEffect::NotSlash => {}
                    SlashEffect::Handled => continue,
                    SlashEffect::SwitchToWatch => return Ok(Flow::SwitchToWatch),
                }
                if matches!(lower.as_str(), "p" | "practice" | "x") {
                    if let Some(practice) = &unit.practice {
                        match run_practice_lab(unit, practice, ctx, reader, host)? {
                            Flow::Continue => {}
                            other => return Ok(other),
                        }
                    } else {
                        host.write_instant(&format!(
                            "  {}\n",
                            paint(Role::Note, "This unit does not have a practice lab.")
                        ))?;
                    }
                    continue;
                }
                if line.is_empty() {
                    unit.lessons
                        .iter()
                        .enumerate()
                        .find(|(idx, l)| {
                            let prev_l = idx.checked_sub(1).map(|i| unit.lessons[i].id.as_str());
                            ctx.progress.is_lesson_unlocked(true, *idx, prev_l)
                                && !ctx.progress.is_lesson_completed(&l.id)
                        })
                        .map(|(idx, _)| idx)
                        .unwrap_or(0)
                } else if let Ok(n) = line.parse::<usize>()
                    && (1..=unit.lessons.len()).contains(&n)
                {
                    n - 1
                } else {
                    host.write_instant(&format!(
                        "  {}\n",
                        paint(
                            Role::Dim,
                            &format!(
                                "Enter 1-{}, Enter for next{practice_hint}, or 'b' to go back.",
                                unit.lessons.len()
                            )
                        )
                    ))?;
                    continue;
                }
            }
        };

        let prev_l = chosen_idx.checked_sub(1).map(|i| unit.lessons[i].id.as_str());
        if !ctx.progress.is_lesson_unlocked(true, chosen_idx, prev_l) {
            let prev_title = &unit.lessons[chosen_idx - 1].title;
            host.write_instant(&format!(
                "  {}\n",
                paint(
                    Role::Note,
                    &format!(
                        "Lesson {} is locked — finish Lesson {} ({prev_title}) first!",
                        chosen_idx + 1,
                        chosen_idx
                    )
                )
            ))?;
            continue;
        }
        match run_lesson(units, unit, &unit.lessons[chosen_idx], ctx, catalog, reader, host)? {
            Flow::Continue => {}
            other => return Ok(other),
        }
    }
}

fn run_interactive_menu<R: BufRead>(
    units: &[Unit],
    ctx: &mut LearnState<'_>,
    catalog: &[Command],
    reader: &mut R,
    host: &mut dyn LabHost,
) -> Result<PaneExit> {
    let mut default_idx = 0usize;
    loop {
        let daily = ctx.progress.daily_xp_on(ctx.today);
        let due = srs::due_count(&ctx.progress.srs, ctx.today);
        let pet_now = load_pet(ctx.pet_path);

        let mut options: Vec<ChoiceOption> = Vec::with_capacity(units.len() + 5);
        let next_detail = if let Some((u_idx, l_idx)) = first_open_lesson(units, &ctx.progress) {
            format!("Unit {}: {}", units[u_idx].number, units[u_idx].lessons[l_idx].title)
        } else {
            "All lessons complete".into()
        };
        options.push(ChoiceOption {
            key: "n".into(),
            label: "Continue next lesson".into(),
            detail: next_detail,
            locked: false,
        });

        let mut total_labs = 0usize;
        let mut done_labs = 0usize;
        for (u_idx, unit) in units.iter().enumerate() {
            let prev = prev_unit_ids(units, u_idx);
            let unlocked = ctx.progress.is_unit_unlocked(u_idx, prev.as_deref());
            let ids = unit_lesson_ids(unit);
            let done = ctx.progress.is_unit_completed(&ids);
            let done_count = unit.lessons.iter().filter(|l| ctx.progress.is_lesson_completed(&l.id)).count();
            let mut parts: Vec<String> = Vec::new();
            if done {
                parts.push("[done]".into());
            } else if unlocked {
                parts.push(format!("[{done_count}/{}]", unit.lessons.len()));
            } else {
                parts.push("[locked]".into());
            }
            if unit.is_side_quest() {
                parts.push("real-world".into());
            }
            if let Some(practice) = &unit.practice {
                total_labs += 1;
                if ctx.progress.is_practice_completed(&practice.id) {
                    done_labs += 1;
                    parts.push("[lab done]".into());
                } else if unlocked {
                    parts.push("[lab]".into());
                }
            }
            if let Some(sp) = unit.unlocks_species.as_deref().and_then(|s| Species::parse(s).ok()) {
                if pet_now.is_unlocked(sp) {
                    parts.push(format!("{} unlocked", sp.label()));
                } else {
                    parts.push(format!("unlocks {}", sp.label()));
                }
            }
            options.push(ChoiceOption {
                key: unit.number.to_string(),
                label: unit.title.clone(),
                detail: parts.join(" · "),
                locked: !unlocked,
            });
        }

        options.push(ChoiceOption {
            key: "x".into(),
            label: "Practice labs".into(),
            detail: format!("{done_labs}/{total_labs} done"),
            locked: false,
        });
        options.push(ChoiceOption {
            key: "r".into(),
            label: "Spaced review".into(),
            detail: if due > 0 { format!("{due} due today") } else { "0 due".into() },
            locked: false,
        });
        options.push(ChoiceOption {
            key: "p".into(),
            label: "Progress".into(),
            detail: "XP, streaks & badges".into(),
            locked: false,
        });
        options.push(ChoiceOption {
            key: "w".into(),
            label: "Back to Watch".into(),
            detail: "Switch to live terminal watch".into(),
            locked: false,
        });

        let menu_title = format!(
            "Lab — XP {} (today {}/{}) · Streak {}d · Review due {}",
            ctx.progress.xp, daily, ctx.daily_goal, ctx.progress.streak, due
        );
        match host.choose_option(
            &menu_title,
            &options,
            default_idx.min(options.len() - 1),
            &format!("↑/↓ move · Enter select · 1-{} unit · / commands · q quit", units.len()),
            reader,
        )? {
            ChoiceResult::Picked(idx) => {
                default_idx = idx;
                if idx == 0 {
                    if let Some((u_idx, l_idx)) = first_open_lesson(units, &ctx.progress) {
                        match run_lesson(
                            units,
                            &units[u_idx],
                            &units[u_idx].lessons[l_idx],
                            ctx,
                            catalog,
                            reader,
                            host,
                        )? {
                            Flow::Continue => {}
                            Flow::SwitchToWatch => return Ok(PaneExit::SwitchToWatch),
                            Flow::QuitAll => return Ok(PaneExit::Quit),
                        }
                    } else {
                        host.write_instant(&format!(
                            "  {}\n",
                            paint(
                                Role::Success,
                                "You finished every available lesson! Pick any unit to replay, try `practice`, or type `review`."
                            )
                        ))?;
                    }
                    continue;
                }
                if idx <= units.len() {
                    let u_idx = idx - 1;
                    let prev = prev_unit_ids(units, u_idx);
                    if !ctx.progress.is_unit_unlocked(u_idx, prev.as_deref()) {
                        host.write_instant(&format!("  {}\n", paint(Role::Note, &locked_unit_message(units, u_idx))))?;
                        continue;
                    }
                    match run_unit_menu(u_idx, units, ctx, catalog, reader, host)? {
                        Flow::Continue => continue,
                        Flow::SwitchToWatch => return Ok(PaneExit::SwitchToWatch),
                        Flow::QuitAll => return Ok(PaneExit::Quit),
                    }
                }
                let action_idx = idx - units.len() - 1;
                match action_idx {
                    0 => match run_practice_menu(units, ctx, true, reader, host)? {
                        Flow::Continue => continue,
                        Flow::SwitchToWatch => return Ok(PaneExit::SwitchToWatch),
                        Flow::QuitAll => return Ok(PaneExit::Quit),
                    },
                    1 => {
                        host.write_instant("\n")?;
                        match run_review(units, ctx, catalog, reader, host)? {
                            Flow::Continue => continue,
                            Flow::SwitchToWatch => return Ok(PaneExit::SwitchToWatch),
                            Flow::QuitAll => return Ok(PaneExit::Quit),
                        }
                    }
                    2 => {
                        let pet_cur = load_pet(ctx.pet_path);
                        host.write_instant(&format!(
                            "\n{}",
                            render_progress(units, &ctx.progress, &pet_cur, ctx.today, ctx.daily_goal)
                        ))?;
                        continue;
                    }
                    _ => {
                        let _ = host.handle_slash("/watch")?;
                        return Ok(PaneExit::SwitchToWatch);
                    }
                }
            }
            ChoiceResult::Cancelled | ChoiceResult::Eof => return Ok(PaneExit::Quit),
            ChoiceResult::Slash(s) => {
                if run_menu_slash(&s, reader, host)? == SlashEffect::SwitchToWatch {
                    return Ok(PaneExit::SwitchToWatch);
                }
                continue;
            }
            ChoiceResult::Fallback => {}
        }

        let mut out = String::new();
        let _ = writeln!(out);
        let _ = writeln!(out, "{}", paint(Role::Header, "Lab — Linux Terminal Tutorial"));
        let _ = writeln!(
            out,
            "  XP: {} (today {}/{}) · Streak: {}d · Freezes: {}/{MAX_FREEZES} · Review due: {}",
            paint(Role::Value, &ctx.progress.xp.to_string()),
            daily,
            ctx.daily_goal,
            ctx.progress.streak,
            ctx.progress.freezes,
            paint(if due > 0 { Role::Note } else { Role::Value }, &due.to_string())
        );
        let _ = writeln!(out);

        for (u_idx, unit) in units.iter().enumerate() {
            let prev = prev_unit_ids(units, u_idx);
            let unlocked = ctx.progress.is_unit_unlocked(u_idx, prev.as_deref());
            let ids = unit_lesson_ids(unit);
            let done = ctx.progress.is_unit_completed(&ids);
            let done_count = unit.lessons.iter().filter(|l| ctx.progress.is_lesson_completed(&l.id)).count();
            let status = if done {
                paint(Role::Success, "[done]")
            } else if unlocked {
                paint(Role::Accent, &format!("[{done_count}/{}]", unit.lessons.len()))
            } else {
                paint(Role::Dim, "[locked]")
            };
            let side_quest_hint =
                if unit.is_side_quest() { format!(" {}", paint(Role::Note, "[real-world]")) } else { String::new() };
            let lab_hint = unit
                .practice
                .as_ref()
                .map(|p| {
                    if ctx.progress.is_practice_completed(&p.id) {
                        format!(" {}", paint(Role::Success, "[lab done]"))
                    } else if unlocked {
                        format!(" {}", paint(Role::Accent, "[lab]"))
                    } else {
                        String::new()
                    }
                })
                .unwrap_or_default();
            let species_hint = unit
                .unlocks_species
                .as_deref()
                .and_then(|s| Species::parse(s).ok())
                .map(|sp| {
                    if pet_now.is_unlocked(sp) {
                        format!(" ({} unlocked)", sp.label())
                    } else {
                        format!(" (unlocks {})", sp.label())
                    }
                })
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "  {}. {} {status}{side_quest_hint}{lab_hint}{}",
                unit.number,
                unit.title,
                paint(Role::Dim, &species_hint)
            );
        }

        let _ = writeln!(out);
        let _ = write!(
            out,
            "{} ",
            paint(
                Role::Dim,
                &format!(
                    "Pick unit 1-{}, 'next' (Enter), 'practice [unit]' ('x'), 'review', 'progress', '/mode watch', or 'q':",
                    units.len()
                )
            )
        );
        host.write_instant(&out)?;

        let Some(line) = host.read_line(reader)? else {
            return Ok(PaneExit::Quit);
        };
        let lower = line.to_ascii_lowercase();
        if is_quit_cmd(&lower) {
            return Ok(PaneExit::Quit);
        }
        match host.handle_slash(&line)? {
            SlashEffect::NotSlash => {}
            SlashEffect::Handled => continue,
            SlashEffect::SwitchToWatch => return Ok(PaneExit::SwitchToWatch),
        }
        if lower == "p" || lower == "progress" || lower == "status" {
            let pet_cur = load_pet(ctx.pet_path);
            host.write_instant(&format!(
                "\n{}",
                render_progress(units, &ctx.progress, &pet_cur, ctx.today, ctx.daily_goal)
            ))?;
            continue;
        }
        if lower == "r" || lower == "review" {
            host.write_instant("\n")?;
            match run_review(units, ctx, catalog, reader, host)? {
                Flow::Continue => continue,
                Flow::SwitchToWatch => return Ok(PaneExit::SwitchToWatch),
                Flow::QuitAll => return Ok(PaneExit::Quit),
            }
        }
        if lower == "x" || lower == "practice" {
            match run_practice_menu(units, ctx, true, reader, host)? {
                Flow::Continue => continue,
                Flow::SwitchToWatch => return Ok(PaneExit::SwitchToWatch),
                Flow::QuitAll => return Ok(PaneExit::Quit),
            }
        }
        if let Some(rest) = lower.strip_prefix("practice ").or_else(|| lower.strip_prefix("x ")) {
            let query = rest.trim();
            if query.is_empty() {
                match run_practice_menu(units, ctx, true, reader, host)? {
                    Flow::Continue => continue,
                    Flow::SwitchToWatch => return Ok(PaneExit::SwitchToWatch),
                    Flow::QuitAll => return Ok(PaneExit::Quit),
                }
            } else {
                match run_practice_target(query, units, ctx, reader, host) {
                    Ok(Flow::Continue) => continue,
                    Ok(Flow::SwitchToWatch) => return Ok(PaneExit::SwitchToWatch),
                    Ok(Flow::QuitAll) => return Ok(PaneExit::Quit),
                    Err(err) => {
                        host.write_instant(&format!("  {}\n", paint(Role::Note, &format!("{err}"))))?;
                        continue;
                    }
                }
            }
        }
        if lower.is_empty() || lower == "next" || lower == "n" {
            if let Some((u_idx, l_idx)) = first_open_lesson(units, &ctx.progress) {
                match run_lesson(units, &units[u_idx], &units[u_idx].lessons[l_idx], ctx, catalog, reader, host)? {
                    Flow::Continue => {}
                    Flow::SwitchToWatch => return Ok(PaneExit::SwitchToWatch),
                    Flow::QuitAll => return Ok(PaneExit::Quit),
                }
            } else {
                host.write_instant(&format!(
                    "  {}\n",
                    paint(
                        Role::Success,
                        "You finished every available lesson! Pick any unit to replay, try `practice`, or type `review`."
                    )
                ))?;
            }
            continue;
        }
        if let Some(unit) = lessons::find_unit(units, &line) {
            let u_idx = units.iter().position(|u| u.id == unit.id).unwrap_or(0);
            let prev = prev_unit_ids(units, u_idx);
            if !ctx.progress.is_unit_unlocked(u_idx, prev.as_deref()) {
                host.write_instant(&format!("  {}\n", paint(Role::Note, &locked_unit_message(units, u_idx))))?;
                continue;
            }
            match run_unit_menu(u_idx, units, ctx, catalog, reader, host)? {
                Flow::Continue => continue,
                Flow::SwitchToWatch => return Ok(PaneExit::SwitchToWatch),
                Flow::QuitAll => return Ok(PaneExit::Quit),
            }
        }
        host.write_instant(&format!(
            "  {}\n",
            paint(
                Role::Dim,
                &format!(
                    "Unknown choice \"{line}\". Enter 1-{}, 'next', 'practice [unit]', 'review', 'progress', '/mode watch', or 'q'.",
                    units.len()
                )
            )
        ))?;
    }
}

fn run_target_unit<R: BufRead>(
    query: &str,
    lesson_num: Option<usize>,
    units: &[Unit],
    ctx: &mut LearnState<'_>,
    catalog: &[Command],
    reader: &mut R,
    host: &mut dyn LabHost,
) -> Result<Flow> {
    let Some(unit) = lessons::find_unit(units, query) else {
        let names: Vec<String> = units.iter().map(|u| format!("{}: {}", u.number, u.id)).collect();
        bail!(
            "unknown lab target \"{query}\"; use `progress`, `review`, `practice [unit]`, or a unit ({})",
            names.join(", ")
        );
    };
    let u_idx = units.iter().position(|u| u.id == unit.id).unwrap_or(0);
    let prev = prev_unit_ids(units, u_idx);
    if !ctx.progress.is_unit_unlocked(u_idx, prev.as_deref()) {
        host.write_instant(&format!("{}\n", paint(Role::Note, &locked_unit_message(units, u_idx))))?;
        return Ok(Flow::Continue);
    }

    let l_idx = match lesson_num {
        Some(n) => {
            if n == 0 || n > unit.lessons.len() {
                host.write_instant(&format!(
                    "{}\n",
                    paint(
                        Role::Note,
                        &format!(
                            "Unit {} has {} lessons (choose `--lesson 1` through `{}`).",
                            unit.number,
                            unit.lessons.len(),
                            unit.lessons.len()
                        )
                    )
                ))?;
                return Ok(Flow::Continue);
            }
            let idx = n - 1;
            let prev_l = idx.checked_sub(1).map(|i| unit.lessons[i].id.as_str());
            if !ctx.progress.is_lesson_unlocked(true, idx, prev_l) {
                let prev_title = &unit.lessons[idx - 1].title;
                host.write_instant(&format!(
                    "{}\n",
                    paint(
                        Role::Note,
                        &format!(
                            "Lesson {n} ({}) is locked — finish Lesson {} ({prev_title}) first!",
                            unit.lessons[idx].title,
                            n - 1
                        )
                    )
                ))?;
                return Ok(Flow::Continue);
            }
            idx
        }
        None => unit
            .lessons
            .iter()
            .enumerate()
            .find(|(idx, l)| {
                let prev_l = idx.checked_sub(1).map(|i| unit.lessons[i].id.as_str());
                ctx.progress.is_lesson_unlocked(true, *idx, prev_l) && !ctx.progress.is_lesson_completed(&l.id)
            })
            .map(|(idx, _)| idx)
            .unwrap_or(0),
    };

    run_lesson(units, unit, &unit.lessons[l_idx], ctx, catalog, reader, host)
}

// ---------------------------------------------------------------------------
// Target resolution & public entry points
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
enum LearnTarget {
    Menu,
    Progress,
    Review,
    Practice(Option<String>),
    Unit { query: String, lesson: Option<usize> },
}

fn resolve_target(target: &[String], lesson: Option<usize>) -> Result<LearnTarget> {
    let args: Vec<&str> = target.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    let Some(&first) = args.first() else {
        if let Some(n) = lesson {
            bail!("specify a unit with `--lesson {n}`, e.g. `1 --lesson {n}`");
        }
        return Ok(LearnTarget::Menu);
    };

    let lower = first.to_ascii_lowercase();
    match lower.as_str() {
        "progress" | "status" => {
            if lesson.is_some() {
                bail!("`--lesson` cannot be combined with `progress`");
            }
            if args.len() > 1 {
                bail!("`progress` does not accept extra arguments");
            }
            Ok(LearnTarget::Progress)
        }
        "review" => {
            if lesson.is_some() {
                bail!("`--lesson` cannot be combined with `review`");
            }
            if args.len() > 1 {
                bail!("`review` does not accept extra arguments");
            }
            Ok(LearnTarget::Review)
        }
        "practice" => {
            if lesson.is_some() {
                bail!("`--lesson` cannot be combined with `practice` (use `practice [unit]` instead)");
            }
            match args.len() {
                1 => Ok(LearnTarget::Practice(None)),
                2 => Ok(LearnTarget::Practice(Some(args[1].to_string()))),
                _ => bail!("too many arguments for `practice`; use `practice [unit]`"),
            }
        }
        _ => match args.len() {
            1 => Ok(LearnTarget::Unit { query: first.to_string(), lesson }),
            2 if lesson.is_none() && args[1].parse::<usize>().is_ok() => {
                Ok(LearnTarget::Unit { query: first.to_string(), lesson: args[1].parse().ok() })
            }
            _ => bail!(
                "unexpected arguments {:?}; use `progress`, `review`, `practice [unit]`, or `<unit> [--lesson N]`",
                &args[1..]
            ),
        },
    }
}

/// Core Lab runner parameterized over `LabHost` and explicit I/O, date, and state paths.
pub fn run_with_host<R: BufRead>(
    config: &RunConfig<'_>,
    catalog: &[Command],
    units: &[Unit],
    host: &mut dyn LabHost,
    reader: &mut R,
    enter_menu_after_target: bool,
) -> Result<(SessionOutcome, PaneExit)> {
    let target = resolve_target(config.target, config.lesson)?;
    let mut ctx = LearnState {
        progress: load_progress(config.progress_path),
        today: config.today,
        now_epoch: config.now_epoch,
        daily_goal: config.daily_goal,
        progress_path: config.progress_path,
        pet_path: config.pet_path,
        practice_root: config.practice_root.as_deref(),
        recording_dir: config.recording_dir.as_deref(),
        outcome: SessionOutcome::default(),
    };

    let flow = match target {
        LearnTarget::Progress => {
            let pet_now = load_pet(config.pet_path);
            host.write_instant(&render_progress(units, &ctx.progress, &pet_now, ctx.today, ctx.daily_goal))?;
            Flow::Continue
        }
        LearnTarget::Review => run_review(units, &mut ctx, catalog, reader, host)?,
        LearnTarget::Practice(Some(unit_query)) => run_practice_target(&unit_query, units, &mut ctx, reader, host)?,
        LearnTarget::Practice(None) => run_practice_menu(units, &mut ctx, config.interactive, reader, host)?,
        LearnTarget::Unit { query, lesson } => run_target_unit(&query, lesson, units, &mut ctx, catalog, reader, host)?,
        LearnTarget::Menu => {
            if !config.interactive {
                let pet_now = load_pet(config.pet_path);
                host.write_instant(&format!(
                    "{}\n{}\n",
                    render_progress(units, &ctx.progress, &pet_now, ctx.today, ctx.daily_goal),
                    paint(
                        Role::Dim,
                        "Open `/mode lab` in an interactive terminal to start lessons (or `review` / `practice`)."
                    )
                ))?;
                return Ok((ctx.outcome, PaneExit::Quit));
            }
            let exit = run_interactive_menu(units, &mut ctx, catalog, reader, host)?;
            return Ok((ctx.outcome, exit));
        }
    };

    let exit = match flow {
        Flow::SwitchToWatch => PaneExit::SwitchToWatch,
        Flow::QuitAll => PaneExit::Quit,
        Flow::Continue if enter_menu_after_target && config.interactive => {
            run_interactive_menu(units, &mut ctx, catalog, reader, host)?
        }
        Flow::Continue => PaneExit::Quit,
    };
    Ok((ctx.outcome, exit))
}

/// Testable entry point driving Lab modes over explicit I/O, date, and state paths (`PlainHost`).
#[cfg_attr(not(test), allow(dead_code))]
pub fn run_with_io<R: BufRead, W: Write>(
    config: &RunConfig<'_>,
    catalog: &[Command],
    units: &[Unit],
    reader: &mut R,
    writer: &mut W,
) -> Result<SessionOutcome> {
    let mut host = PlainHost::new(writer);
    let (outcome, _) = run_with_host(config, catalog, units, &mut host, reader, false)?;
    Ok(outcome)
}

/// Parses optional arguments passed after `/lab [...]` inside the companion pane.
pub fn parse_lab_args(args: &[String]) -> Result<(Vec<String>, Option<usize>)> {
    let mut target: Vec<String> = Vec::new();
    let mut lesson: Option<usize> = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if let Some(rest) = arg.strip_prefix("--lesson=") {
            if lesson.is_some() {
                bail!("`--lesson` specified more than once");
            }
            lesson = Some(rest.parse::<usize>().map_err(|_| anyhow::anyhow!("invalid lesson number \"{rest}\""))?);
            i += 1;
        } else if arg == "--lesson" || arg == "-l" {
            if lesson.is_some() {
                bail!("`--lesson` specified more than once");
            }
            let val = args.get(i + 1).ok_or_else(|| anyhow::anyhow!("missing lesson number after `{arg}`"))?;
            lesson = Some(val.parse::<usize>().map_err(|_| anyhow::anyhow!("invalid lesson number \"{val}\""))?);
            i += 2;
        } else {
            target.push(arg.to_string());
            i += 1;
        }
    }
    // Validate early so bad flag combinations fail cleanly.
    let _ = resolve_target(&target, lesson)?;
    Ok((target, lesson))
}

/// Interactive Lab entry point invoked from the unified `huh` companion pane (`/mode lab` or `/lab`).
pub fn run_in_pane(
    args: &[String],
    catalog: &[Command],
    recording_dir: Option<PathBuf>,
    interactive: bool,
    host: &mut dyn LabHost,
) -> Result<PaneExit> {
    let (target, lesson) = parse_lab_args(args)?;
    let units = lessons::load_embedded()?;
    let now_epoch = date::now_epoch();
    let today = date::from_epoch(now_epoch);
    let progress_path = progress::default_state_path();
    let pet_path = pet::default_state_path();
    let config = RunConfig {
        target: &target,
        lesson,
        interactive,
        today,
        now_epoch,
        daily_goal: progress::daily_goal(),
        progress_path: progress_path.as_deref(),
        pet_path: pet_path.as_deref(),
        practice_root: practice::default_practice_root(),
        recording_dir,
    };
    let mut empty = std::io::empty();
    let (_, exit) = run_with_host(&config, catalog, &units, host, &mut empty, true)?;
    Ok(exit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor, Read};

    const BASE_EPOCH: u64 = 1_795_046_400; // 2026-11-19 00:00:00 UTC

    fn catalog() -> Vec<Command> {
        crate::catalog::load_embedded().unwrap()
    }

    fn units() -> Vec<Unit> {
        lessons::load_embedded().unwrap()
    }

    fn day(s: &str) -> Date {
        date::parse(s).unwrap()
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/learn-tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn ss(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// Test reader that runs a hook on the first read (right after `Sandbox::prepare`) before
    /// feeding scripted lines, enabling single-threaded end-to-end practice sandbox tests.
    struct HookReader<F: FnOnce()> {
        hook: Option<F>,
        inner: Cursor<Vec<u8>>,
    }

    impl<F: FnOnce()> HookReader<F> {
        fn new(input: &str, hook: F) -> Self {
            HookReader { hook: Some(hook), inner: Cursor::new(input.as_bytes().to_vec()) }
        }
    }

    impl<F: FnOnce()> Read for HookReader<F> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if let Some(f) = self.hook.take() {
                f();
            }
            self.inner.read(buf)
        }
    }

    impl<F: FnOnce()> BufRead for HookReader<F> {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            if let Some(f) = self.hook.take() {
                f();
            }
            self.inner.fill_buf()
        }

        fn consume(&mut self, amt: usize) {
            self.inner.consume(amt);
        }
    }

    #[test]
    fn grade_command_handles_flags_quoting_pipelines_redirects_and_order() {
        // Combined short flags and reordering.
        assert!(grade_command("ls -la", &ss(&["ls -l -a"])));
        assert!(grade_command("ls -al", &ss(&["ls -la"])));
        assert!(grade_command("ls -a -l", &ss(&["ls -la"])));
        assert!(grade_command("ls photos -l", &ss(&["ls -l photos"])));
        assert!(!grade_command("ls -l", &ss(&["ls -la"])));
        assert!(!grade_command("ls -laa", &ss(&["ls -la"])));

        // Positional argument order is preserved.
        assert!(grade_command("cp notes.txt notes-backup.txt", &ss(&["cp notes.txt notes-backup.txt"])));
        assert!(!grade_command("cp notes-backup.txt notes.txt", &ss(&["cp notes.txt notes-backup.txt"])));
        assert!(!grade_command("mv art poster.png", &ss(&["mv poster.png art"])));

        // Value-taking flags stay paired with their values and normalize attached forms.
        assert!(grade_command("find . -name '*.txt' -type f", &ss(&["find . -type f -name \"*.txt\""])));
        assert!(!grade_command("find . -iname '*.txt'", &ss(&["find . -name '*.txt'"])));
        assert!(!grade_command("find . -name '*.txt' -delete", &ss(&["find . -name '*.txt'"])));
        assert!(grade_command("head -n 5 story.txt", &ss(&["head -n5 story.txt"])));
        assert!(grade_command("head -5 story.txt", &ss(&["head -n 5 story.txt"])));
        assert!(grade_command("tail --lines 10 game.log", &ss(&["tail --lines=10 game.log"])));
        assert!(!grade_command("head -n 10 story.txt", &ss(&["head -n 5 story.txt"])));
        assert!(grade_command("grep -i -A 2 error app.log", &ss(&["grep -A2 -i 'error' app.log"])));
        assert!(grade_command("grep -ie error app.log", &ss(&["grep -i -e error app.log"])));
        assert!(grade_command("sort -k 2 -t , data.csv", &ss(&["sort -t, -k2 data.csv"])));
        assert!(grade_command("cut -f1 -d: /etc/passwd", &ss(&["cut -d : -f 1 /etc/passwd"])));
        assert!(grade_command("ping -c4 1.1.1.1", &ss(&["ping -c 4 1.1.1.1"])));
        assert!(grade_command(
            "curl --output=guide.txt https://example.com/guide.txt",
            &ss(&["curl --output guide.txt https://example.com/guide.txt"])
        ));
        assert!(!grade_command("head -n", &ss(&["head -n 5"])));

        // Equivalent quoting via Word.text.
        assert!(grade_command("grep \"error\" 'app.log'", &ss(&["grep error app.log"])));
        assert!(grade_command("echo 'hello world'", &ss(&["echo \"hello world\""])));
        assert!(!grade_command("echo hello world", &ss(&["echo \"hello world\""])));

        // Pipelines, connectors, env assigns, and redirections (including 2>&1 order sensitivity!).
        assert!(grade_command("grep 'error' app.log|wc -l", &ss(&["grep error app.log | wc -l"])));
        assert!(!grade_command("wc -l | grep error app.log", &ss(&["grep error app.log | wc -l"])));
        assert!(grade_command("mkdir a && cd a", &ss(&["mkdir a && cd a"])));
        assert!(!grade_command("mkdir a || cd a", &ss(&["mkdir a && cd a"])));
        assert!(!grade_command("mkdir a; cd a", &ss(&["mkdir a && cd a"])));
        assert!(grade_command("ls >> \"files.txt\"", &ss(&["ls >> files.txt"])));
        assert!(!grade_command("ls > files.txt", &ss(&["ls >> files.txt"])));
        assert!(grade_command("cmd 2> err.log > out.log", &ss(&["cmd 1> out.log 2> err.log"])));
        assert!(grade_command("cmd 1> out.log 2>&1", &ss(&["cmd > out.log 2>&1"])));
        assert!(!grade_command("cmd 2>&1 > out.log", &ss(&["cmd > out.log 2>&1"])));
        assert!(grade_command("B=2 A=1 env", &ss(&["A=1 B=2 env"])));
        assert!(!grade_command("A=1 env", &ss(&["env"])));

        // Rejects blank, comment-only, and invalid syntax (unbalanced quotes, dangling pipe).
        assert!(!grade_command("", &ss(&["ls"])));
        assert!(!grade_command("   ", &ss(&["ls"])));
        assert!(!grade_command("# ls", &ss(&["ls"])));
        assert!(!grade_command("grep 'error app.log", &ss(&["grep error app.log"])));
        assert!(!grade_command("ls |", &ss(&["ls"])));
        assert!(!grade_command("ls &&", &ss(&["ls"])));
    }

    #[test]
    fn flawless_lesson_awards_xp_badges_and_feeds_pet() {
        let dir = temp_dir("flawless");
        let prog_path = dir.join("progress.toml");
        let pet_path = dir.join("pet.toml");
        let today = day("2026-11-19");

        // Pre-rename pet to ensure user metadata is preserved when learn feeds it.
        let mut initial_pet = Pet::default();
        initial_pet.rename("Biscuit").unwrap();
        pet::save_to(&pet_path, &initial_pet).unwrap();

        let script = "2\nhint\nls\n1\n3\npwd\n";
        let target = ss(&["1"]);
        let mut reader = Cursor::new(script.as_bytes());
        let mut output = Vec::new();
        let cfg = RunConfig {
            target: &target,
            lesson: Some(1),
            interactive: true,
            today,
            now_epoch: BASE_EPOCH,
            daily_goal: 50,
            progress_path: Some(&prog_path),
            pet_path: Some(&pet_path),
            practice_root: None,
            recording_dir: None,
        };

        let outcome = run_with_io(&cfg, &catalog(), &units(), &mut reader, &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(!text.contains('\x1b'), "plain text in tests: {text}");
        assert!(text.contains("Hint:"), "{text}");
        assert!(text.contains("Flawless!"), "{text}");
        assert!(text.contains("First Step"), "{text}");
        assert!(text.contains("Flawless Run"), "{text}");
        assert!(text.contains("Biscuit cheers you on!"), "{text}");
        assert!(outcome.pet_fed);
        assert!(!outcome.species_unlocked);
        assert_eq!(outcome.lessons_completed, 1);

        // 5 answers: 10 + 10 + 15 + 15 + 15 = 65; clear 25 + flawless 15 = 40; total = 105 XP.
        let saved = progress::load_from(&prog_path);
        assert_eq!(saved.xp, 105);
        assert_eq!(saved.streak, 1);
        assert!(saved.is_lesson_completed("u1-where-am-i"));
        assert!(saved.has_badge("first_step"));
        assert!(saved.has_badge("flawless"));
        assert!(saved.srs.is_empty(), "no missed items -> empty SRS deck");

        let saved_pet = pet::load_from(&pet_path);
        assert_eq!(saved_pet.name, "Biscuit", "custom name preserved");
        assert_eq!(saved_pet.commands_fed, 5);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zero_hearts_allows_instant_missed_review_and_fresh_retry_with_no_lockout() {
        let dir = temp_dir("zero-hearts");
        let prog_path = dir.join("progress.toml");
        let pet_path = dir.join("pet.toml");
        let today = day("2026-11-19");

        let script = concat!(
            "1\ncd\n2\n1\nls\n",  // 5 misses -> 0 hearts
            "r\n",                // instant missed-item review
            "2\nls\n1\n3\npwd\n", // answer all 5 missed items right in review
            "t\n",                // retry fresh with 5 hearts
            "2\nls\n1\n3\npwd\n", // clear lesson flawlessly
        );
        let target = ss(&["1"]);
        let mut reader = Cursor::new(script.as_bytes());
        let mut output = Vec::new();
        let cfg = RunConfig {
            target: &target,
            lesson: Some(1),
            interactive: true,
            today,
            now_epoch: BASE_EPOCH,
            daily_goal: 50,
            progress_path: Some(&prog_path),
            pet_path: Some(&pet_path),
            practice_root: None,
            recording_dir: None,
        };

        let outcome = run_with_io(&cfg, &catalog(), &units(), &mut reader, &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("Out of hearts — no timer and no waiting!"), "{text}");
        assert!(text.contains("What you typed means:"), "wrong TypeCommand explains user input: {text}");
        assert!(text.contains("Missed-item review done: 5/5 right"), "{text}");
        assert!(text.contains("Fresh start! 5 hearts restored."), "{text}");
        assert!(text.contains("Lesson complete: Where Am I?!"), "{text}");
        assert_eq!(outcome.lessons_completed, 1);

        let saved = progress::load_from(&prog_path);
        assert!(saved.is_lesson_completed("u1-where-am-i"));
        assert_eq!(saved.srs.len(), 5, "all 5 missed questions entered SRS deck");
        assert!(saved.srs.iter().all(|c| c.box_level >= 2), "reviewed + cleared promoted cards: {:?}", saved.srs);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn second_miss_on_requeued_question_does_not_promise_another_retry() {
        let dir = temp_dir("second-miss");
        let prog_path = dir.join("progress.toml");
        let pet_path = dir.join("pet.toml");
        let today = day("2026-11-19");

        // Miss q1 once ("1"), get q2..q5 right ("ls", "1", "3", "pwd"), then miss the requeued q1
        // a second time ("1") — lesson still clears with 3 hearts left, and the retry hint appears
        // ONLY once (on the first miss, not the second).
        let script = "1\nls\n1\n3\npwd\n1\n";
        let target = ss(&["1"]);
        let mut output = Vec::new();
        let cfg = RunConfig {
            target: &target,
            lesson: Some(1),
            interactive: true,
            today,
            now_epoch: BASE_EPOCH,
            daily_goal: 50,
            progress_path: Some(&prog_path),
            pet_path: Some(&pet_path),
            practice_root: None,
            recording_dir: None,
        };
        run_with_io(&cfg, &catalog(), &units(), &mut Cursor::new(script.as_bytes()), &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert_eq!(
            text.matches("(you'll get another try at the end)").count(),
            1,
            "second miss on already-requeued question must not promise another try: {text}"
        );
        assert!(text.contains("Lesson complete: Where Am I?!"), "{text}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn completing_unit_1_unlocks_pathfinder_badge_and_cat_species() {
        let dir = temp_dir("unit1-unlock");
        let prog_path = dir.join("progress.toml");
        let pet_path = dir.join("pet.toml");
        let today = day("2026-11-19");
        let cat = catalog();
        let all_units = units();

        let mut pet_init = Pet::default();
        pet_init.rename("Nori").unwrap();
        pet::save_to(&pet_path, &pet_init).unwrap();

        // Check Unit 2 is locked before Unit 1 is complete.
        let mut out_locked = Vec::new();
        let target_u2 = ss(&["2"]);
        let cfg_u2 = RunConfig {
            target: &target_u2,
            lesson: None,
            interactive: true,
            today,
            now_epoch: BASE_EPOCH,
            daily_goal: 50,
            progress_path: Some(&prog_path),
            pet_path: Some(&pet_path),
            practice_root: None,
            recording_dir: None,
        };
        run_with_io(&cfg_u2, &cat, &all_units, &mut Cursor::new(&b""[..]), &mut out_locked).unwrap();
        assert!(String::from_utf8(out_locked).unwrap().contains("locked"));

        let lesson_inputs =
            [(1, "2\nls\n1\n3\npwd\n"), (2, "1\n3\n2\nls -al\nls -l photos\n"), (3, "1\ncd ~\n2\n3\ncd docs\n")];
        let target_u1 = ss(&["1"]);
        let mut last_text = String::new();
        let mut last_outcome = SessionOutcome::default();
        for (lesson_num, input) in lesson_inputs {
            let mut out = Vec::new();
            let cfg = RunConfig {
                target: &target_u1,
                lesson: Some(lesson_num),
                interactive: true,
                today,
                now_epoch: BASE_EPOCH + (lesson_num as u64) * 60,
                daily_goal: 50,
                progress_path: Some(&prog_path),
                pet_path: Some(&pet_path),
                practice_root: None,
                recording_dir: None,
            };
            last_outcome = run_with_io(&cfg, &cat, &all_units, &mut Cursor::new(input.as_bytes()), &mut out).unwrap();
            last_text = String::from_utf8(out).unwrap();
            if lesson_num == 2 {
                assert!(
                    last_text.contains("long folder listing"),
                    "ReadOutput should include outputs::describe summary: {last_text}"
                );
            }
        }

        assert!(last_outcome.species_unlocked, "finishing Unit 1 Lesson 3 should unlock Cat");
        assert!(last_text.contains("NEW PET SPECIES UNLOCKED: Terminal Cat!"), "{last_text}");
        assert!(last_text.contains("huh pet --species cat"), "{last_text}");
        assert!(last_text.contains("/\\_/\\"), "celebrating Cat ASCII art: {last_text}");

        let saved_prog = progress::load_from(&prog_path);
        assert!(saved_prog.has_badge("pathfinder"));
        assert!(saved_prog.is_unit_unlocked(1, Some(&unit_lesson_ids(&all_units[0]))));

        let saved_pet = pet::load_from(&pet_path);
        assert_eq!(saved_pet.name, "Nori", "user rename preserved");
        assert!(saved_pet.is_unlocked(Species::Cat), "Cat unlocked in pet.toml");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn completing_unit_8_unlocks_night_watch_owl_and_shell_grad() {
        let dir = temp_dir("unit8-grad");
        let prog_path = dir.join("progress.toml");
        let pet_path = dir.join("pet.toml");
        let today = day("2026-11-19");
        let all_units = units();

        // Pre-complete Units 1..=7 and Unit 8 Lessons 1..=2.
        let mut p = Progress::default();
        for unit in &all_units {
            for (l_idx, lesson) in unit.lessons.iter().enumerate() {
                if unit.number < 8 || l_idx < 2 {
                    p.complete_lesson(&lesson.id, false, today);
                }
            }
        }
        progress::save_to(&prog_path, &p).unwrap();

        // Unit 8 Lesson 3 ("Boss Level: Spot the Danger") answers: 2, 3, 1, 3, 3.
        let script = "2\n3\n1\n3\n3\n";
        let target = ss(&["8"]);
        let mut output = Vec::new();
        let cfg = RunConfig {
            target: &target,
            lesson: Some(3),
            interactive: true,
            today,
            now_epoch: BASE_EPOCH,
            daily_goal: 50,
            progress_path: Some(&prog_path),
            pet_path: Some(&pet_path),
            practice_root: None,
            recording_dir: None,
        };
        let outcome =
            run_with_io(&cfg, &catalog(), &all_units, &mut Cursor::new(script.as_bytes()), &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(outcome.species_unlocked, "Unit 8 should unlock Snowy Owl");
        assert!(text.contains("NEW PET SPECIES UNLOCKED: Snowy Owl!"), "{text}");
        assert!(text.contains("huh pet --species owl"), "{text}");
        assert!(text.contains(",___,"), "celebrating Owl ASCII art: {text}");
        assert!(text.contains("Night Watch"), "{text}");
        assert!(text.contains("Shell Graduate"), "{text}");

        let saved_prog = progress::load_from(&prog_path);
        assert!(saved_prog.has_badge("night_watch"));
        assert!(saved_prog.has_badge("shell_grad"));
        assert!(saved_prog.has_badge("danger_spotter"), "5 boss SpotDanger questions unlock danger_spotter");

        let saved_pet = pet::load_from(&pet_path);
        assert!(saved_pet.is_unlocked(Species::Owl), "Owl unlocked in pet.toml");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn practice_runner_filesystem_checks_award_xp_badge_and_clean_up() {
        let dir = temp_dir("practice-fs");
        let prog_path = dir.join("progress.toml");
        let pet_path = dir.join("pet.toml");
        let practice_root = dir.join("practice");
        let today = day("2026-11-19");
        let all_units = units();

        // Unlock Unit 2 by completing Unit 1 lessons.
        let mut p = Progress::default();
        for lesson in &all_units[0].lessons {
            p.complete_lesson(&lesson.id, false, today);
        }
        progress::save_to(&prog_path, &p).unwrap();

        let sandbox_dir = practice_root.join("u2-tidy-desk");
        let sandbox_for_hook = sandbox_dir.clone();
        let mut reader = HookReader::new("hint\n\n", move || {
            // Mutate sandbox right after `Sandbox::prepare` to satisfy all 5 Unit 2 checks.
            std::fs::create_dir(sandbox_for_hook.join("notes")).unwrap();
            std::fs::rename(sandbox_for_hook.join("draft.txt"), sandbox_for_hook.join("notes/final.txt")).unwrap();
            std::fs::write(sandbox_for_hook.join("notes/todo.txt"), "learn linux\n").unwrap();
        });

        let target = ss(&["practice", "2"]);
        let mut output = Vec::new();
        let cfg = RunConfig {
            target: &target,
            lesson: None,
            interactive: true,
            today,
            now_epoch: BASE_EPOCH,
            daily_goal: 50,
            progress_path: Some(&prog_path),
            pet_path: Some(&pet_path),
            practice_root: Some(practice_root.clone()),
            recording_dir: Some(dir.join("empty-rec")),
        };

        let outcome = run_with_io(&cfg, &catalog(), &all_units, &mut reader, &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(!text.contains('\x1b'), "plain text in tests: {text}");
        assert!(text.contains("Hands-On Practice (Unit 2)"), "{text}");
        assert!(text.contains("Hint:"), "{text}");
        assert!(text.contains("Practice complete: Tidy Your Desk!"), "{text}");
        assert!(text.contains("Hands-On"), "{text}");
        assert!(outcome.pet_fed);
        assert_eq!(outcome.practices_completed, 1);
        assert!(!sandbox_dir.exists(), "sandbox directory should be cleaned up after passing");

        let saved = progress::load_from(&prog_path);
        assert_eq!(saved.xp, u64::from(XP_PRACTICE_CLEAR));
        assert!(saved.is_practice_completed("u2-tidy-desk"));
        assert!(saved.has_badge("hands_on"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn practice_runner_recorded_output_passes_via_fallback_answer() {
        let dir = temp_dir("practice-fallback");
        let prog_path = dir.join("progress.toml");
        let pet_path = dir.join("pet.toml");
        let practice_root = dir.join("practice");
        let today = day("2026-11-19");
        let all_units = units();

        // Unit 1 practice (`u1-hidden-clue`) uses `RecordedOutput` with fallback answer `penguin-tracks`.
        // Try a wrong fallback answer first, then `reset`, then the right fallback answer.
        let script = "wrong-guess\nreset\npenguin-tracks\n";
        let target = ss(&["practice", "1"]);
        let mut output = Vec::new();
        let cfg = RunConfig {
            target: &target,
            lesson: None,
            interactive: true,
            today,
            now_epoch: BASE_EPOCH,
            daily_goal: 50,
            progress_path: Some(&prog_path),
            pet_path: Some(&pet_path),
            practice_root: Some(practice_root.clone()),
            recording_dir: Some(dir.join("no-recordings")),
        };

        let outcome =
            run_with_io(&cfg, &catalog(), &all_units, &mut Cursor::new(script.as_bytes()), &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("What secret phrase is inside .secret_clue?"), "{text}");
        assert!(text.contains("Sandbox reset to fresh starting files."), "{text}");
        assert!(text.contains("verified via check question"), "{text}");
        assert!(text.contains("Practice complete: Find the Hidden Clue!"), "{text}");
        assert_eq!(outcome.practices_completed, 1);
        assert!(!practice_root.join("u1-hidden-clue").exists(), "sandbox cleaned up");

        let saved = progress::load_from(&prog_path);
        assert!(saved.is_practice_completed("u1-hidden-clue"));
        assert!(saved.has_badge("hands_on"));

        // Non-TTY `huh learn practice` lists practice labs and prints a hint cleanly.
        let target_list = ss(&["practice"]);
        let mut list_out = Vec::new();
        let cfg_list = RunConfig {
            target: &target_list,
            lesson: None,
            interactive: false,
            today,
            now_epoch: BASE_EPOCH,
            daily_goal: 50,
            progress_path: Some(&prog_path),
            pet_path: Some(&pet_path),
            practice_root: Some(practice_root),
            recording_dir: Some(dir.join("no-recordings")),
        };
        run_with_io(&cfg_list, &catalog(), &all_units, &mut Cursor::new(&b""[..]), &mut list_out).unwrap();
        let list_text = String::from_utf8(list_out).unwrap();
        assert!(list_text.contains("[x] Find the Hidden Clue (done)"), "{list_text}");
        assert!(list_text.contains("interactive terminal"), "{list_text}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn review_mode_advances_leitner_box_and_filters_stale_ids_before_truncating() {
        let dir = temp_dir("review");
        let prog_path = dir.join("progress.toml");
        let pet_path = dir.join("pet.toml");
        let today = day("2026-11-19");
        let earlier = day("2026-11-10");

        // Seed 10 stale/invalid cards due earlier (so they sort BEFORE real cards!) plus 1 valid
        // SpotDanger card (`u2-l2-q5`, answer index 1 -> "2") due today.
        let mut p = Progress::default();
        for idx in 0..10 {
            srs::record_answer(&mut p.srs, &format!("retired-card-{idx:02}"), false, earlier);
        }
        srs::record_answer(&mut p.srs, "u2-l2-q5", false, today);
        progress::save_to(&prog_path, &p).unwrap();

        let target = ss(&["review"]);
        let mut out = Vec::new();
        let cfg = RunConfig {
            target: &target,
            lesson: None,
            interactive: true,
            today,
            now_epoch: BASE_EPOCH,
            daily_goal: 50,
            progress_path: Some(&prog_path),
            pet_path: Some(&pet_path),
            practice_root: None,
            recording_dir: None,
        };
        let outcome = run_with_io(&cfg, &catalog(), &units(), &mut Cursor::new(&b"2\n"[..]), &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Spaced Review — 1 card(s) due today"), "stale IDs filtered before cap: {text}");
        assert!(text.contains("! [destroys data]"), "SpotDanger feedback: {text}");
        assert!(text.contains("Box 0 -> Box 1"), "{text}");
        assert!(outcome.pet_fed);

        let after = progress::load_from(&prog_path);
        assert_eq!(after.xp, u64::from(XP_REVIEW_CORRECT));
        assert_eq!(after.danger_spotted, 1);
        let card = after.srs.iter().find(|c| c.question_id == "u2-l2-q5").unwrap();
        assert_eq!(card.box_level, 1);
        assert_eq!(card.due_day, "2026-11-20");

        // Running review again today reports 0 valid due cards cleanly.
        let mut out2 = Vec::new();
        run_with_io(&cfg, &catalog(), &units(), &mut Cursor::new(&b""[..]), &mut out2).unwrap();
        assert!(String::from_utf8(out2).unwrap().contains("All caught up!"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_progress_and_non_tty_fallback_are_plain_and_informative() {
        let today = day("2026-11-19");
        let all_units = units();
        let mut p = Progress::default();
        p.award_xp(30, today);
        p.complete_lesson("u1-where-am-i", true, today);
        srs::record_answer(&mut p.srs, "u1-l1-q1", false, today);
        let pet = Pet::default();

        let rendered = render_progress(&all_units, &p, &pet, today, 50);
        assert!(!rendered.contains('\x1b'), "plain text in tests: {rendered}");
        assert!(rendered.contains("XP: 30 total"), "{rendered}");
        assert!(rendered.contains("30/50 XP"), "{rendered}");
        assert!(rendered.contains("Streak: 1 day(s)"), "{rendered}");
        assert!(rendered.contains("Freezes: 1/3"), "{rendered}");
        assert!(rendered.contains("Review due: 1"), "{rendered}");
        assert!(rendered.contains("Finding Your Way"), "{rendered}");
        assert!(rendered.contains("unlocks Terminal Cat"), "{rendered}");
        assert!(rendered.contains("unlocks Snowy Owl"), "{rendered}");
        assert!(rendered.contains("[x] 1. Where Am I?"), "{rendered}");
        assert!(rendered.contains("[ ] 2. Hidden Files & Details"), "{rendered}");
        assert!(rendered.contains("[lock] 3. Moving Around"), "{rendered}");
        assert!(rendered.contains("[ ] Practice: Find the Hidden Clue"), "{rendered}");
        assert!(rendered.contains("`practice 1` in `/mode lab`"), "{rendered}");
        assert!(rendered.contains("Commands: `/mode lab`, `review`, `practice [unit]`, `/mode watch`"), "{rendered}");
        assert!(rendered.contains("First Step"), "{rendered}");

        // Non-interactive bare Lab prints progress + terminal hint.
        let dir = temp_dir("non-tty");
        let prog_path = dir.join("progress.toml");
        let pet_path = dir.join("pet.toml");
        progress::save_to(&prog_path, &p).unwrap();
        let mut out = Vec::new();
        let cfg = RunConfig {
            target: &[],
            lesson: None,
            interactive: false,
            today,
            now_epoch: BASE_EPOCH,
            daily_goal: 50,
            progress_path: Some(&prog_path),
            pet_path: Some(&pet_path),
            practice_root: None,
            recording_dir: None,
        };
        run_with_io(&cfg, &catalog(), &all_units, &mut Cursor::new(&b""[..]), &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Lab — Progress"), "{text}");
        assert!(text.contains("interactive terminal"), "{text}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_lab_args_and_target_resolution_cover_practice_and_reject_bad_combos() {
        assert_eq!(parse_lab_args(&[]).unwrap(), (vec![], None));
        assert_eq!(parse_lab_args(&ss(&["progress"])).unwrap(), (ss(&["progress"]), None));
        assert_eq!(parse_lab_args(&ss(&["review"])).unwrap(), (ss(&["review"]), None));
        assert_eq!(parse_lab_args(&ss(&["practice"])).unwrap(), (ss(&["practice"]), None));
        assert_eq!(parse_lab_args(&ss(&["practice", "2"])).unwrap(), (ss(&["practice", "2"]), None));
        assert_eq!(parse_lab_args(&ss(&["1", "2"])).unwrap(), (ss(&["1", "2"]), None));
        assert_eq!(parse_lab_args(&ss(&["1", "--lesson", "2"])).unwrap(), (ss(&["1"]), Some(2)));
        assert_eq!(parse_lab_args(&ss(&["finding", "--lesson=3"])).unwrap(), (ss(&["finding"]), Some(3)));

        assert_eq!(resolve_target(&ss(&["practice"]), None).unwrap(), LearnTarget::Practice(None));
        assert_eq!(resolve_target(&ss(&["practice", "2"]), None).unwrap(), LearnTarget::Practice(Some("2".into())));
        assert_eq!(
            resolve_target(&ss(&["1", "2"]), None).unwrap(),
            LearnTarget::Unit { query: "1".into(), lesson: Some(2) }
        );

        // Reject `--lesson` with `progress`, `review`, `practice`, or missing unit.
        assert!(resolve_target(&ss(&["progress"]), Some(1)).is_err());
        assert!(resolve_target(&ss(&["review"]), Some(1)).is_err());
        assert!(resolve_target(&ss(&["practice"]), Some(1)).is_err());
        assert!(resolve_target(&ss(&["practice", "2"]), Some(1)).is_err());
        assert!(resolve_target(&[], Some(1)).is_err());
        assert!(parse_lab_args(&ss(&["practice", "--lesson", "1"])).is_err());
        assert!(parse_lab_args(&ss(&["1", "--lesson", "nope"])).is_err());
        assert!(parse_lab_args(&ss(&["1", "--lesson"])).is_err());
    }

    #[test]
    fn lab_host_intercepts_slash_commands_and_handles_practice_auto_update() {
        struct MockHost {
            narrative: String,
            instant: String,
            slash_log: Vec<String>,
            pet_events: Vec<(Option<Mood>, bool)>,
            auto_report: Option<Report>,
        }
        impl LabHost for MockHost {
            fn write_narrative(&mut self, text: &str) -> std::io::Result<()> {
                self.narrative.push_str(text);
                Ok(())
            }
            fn write_instant(&mut self, text: &str) -> std::io::Result<()> {
                self.instant.push_str(text);
                Ok(())
            }
            fn on_pet_event(&mut self, mood: Option<Mood>, celebrate: bool) {
                self.pet_events.push((mood, celebrate));
            }
            fn handle_slash(&mut self, line: &str) -> std::io::Result<SlashEffect> {
                let trimmed = line.trim();
                if trimmed == "/theme nord" {
                    self.slash_log.push(trimmed.to_string());
                    self.instant.push_str("Theme set to nord.\n");
                    return Ok(SlashEffect::Handled);
                }
                if trimmed == "/mode watch" || trimmed == "/watch" {
                    self.slash_log.push(trimmed.to_string());
                    return Ok(SlashEffect::SwitchToWatch);
                }
                Ok(SlashEffect::NotSlash)
            }
            fn wait_practice_turn(
                &mut self,
                sandbox_dir: &Path,
                practice: &Practice,
                reported_cmds: &mut u64,
            ) -> Option<PracticePoll> {
                let _ = (sandbox_dir, practice);
                if let Some(report) = self.auto_report.take() {
                    *reported_cmds += 1;
                    return Some(PracticePoll::AutoUpdate { report, recorded: None, nudged: false });
                }
                None
            }
        }

        let dir = temp_dir("lab-host");
        let prog_path = dir.join("progress.toml");
        let pet_path = dir.join("pet.toml");
        let practice_root = dir.join("practice");
        let today = day("2026-11-19");
        let all_units = units();

        // Inside Unit 1 Lesson 1 TypeCommand/PromptChoice: `/theme nord` is Handled, `/mode watch` switches cleanly.
        let mut host = MockHost {
            narrative: String::new(),
            instant: String::new(),
            slash_log: Vec::new(),
            pet_events: Vec::new(),
            auto_report: None,
        };
        let target = ss(&["1"]);
        let cfg = RunConfig {
            target: &target,
            lesson: Some(1),
            interactive: true,
            today,
            now_epoch: BASE_EPOCH,
            daily_goal: 50,
            progress_path: Some(&prog_path),
            pet_path: Some(&pet_path),
            practice_root: None,
            recording_dir: None,
        };
        let script = "/theme nord\n2\n/watch\n";
        let (outcome, exit) =
            run_with_host(&cfg, &catalog(), &all_units, &mut host, &mut Cursor::new(script.as_bytes()), true).unwrap();
        assert_eq!(exit, PaneExit::SwitchToWatch);
        assert_eq!(host.slash_log, vec!["/theme nord", "/watch"]);
        assert!(!host.narrative.is_empty(), "lesson intro and prompt streamed via write_narrative");
        assert!(host.instant.contains("Theme set to nord."), "{}", host.instant);
        assert!(outcome.lessons_completed == 0);
        assert!(host.pet_events.iter().any(|(m, _)| *m == Some(Mood::Happy)));

        // AutoUpdate in wait_practice_turn auto-completes practice lab when all checks pass.
        let passed_report = Report {
            outcomes: vec![practice::CheckOutcome {
                label: "Found secret".into(),
                passed: true,
                detail: "verified".into(),
                needs_fallback: false,
            }],
        };
        let mut practice_host = MockHost {
            narrative: String::new(),
            instant: String::new(),
            slash_log: Vec::new(),
            pet_events: Vec::new(),
            auto_report: Some(passed_report),
        };
        let p_target = ss(&["practice", "1"]);
        let p_cfg = RunConfig {
            target: &p_target,
            lesson: None,
            interactive: true,
            today,
            now_epoch: BASE_EPOCH,
            daily_goal: 50,
            progress_path: Some(&prog_path),
            pet_path: Some(&pet_path),
            practice_root: Some(practice_root.clone()),
            recording_dir: Some(dir.join("no-rec")),
        };
        let (p_outcome, p_exit) =
            run_with_host(&p_cfg, &catalog(), &all_units, &mut practice_host, &mut Cursor::new(&b""[..]), false)
                .unwrap();
        assert_eq!(p_exit, PaneExit::Quit);
        assert_eq!(p_outcome.practices_completed, 1);
        assert!(practice_host.instant.contains("Practice complete: Find the Hidden Clue!"));
        assert!(practice_host.pet_events.iter().any(|(m, c)| *m == Some(Mood::Celebrating) && *c));
        assert!(!practice_root.join("u1-hidden-clue").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn choose_option_drives_all_five_lab_flows_and_menu_slash_shortcut() {
        struct MenuHost {
            script: VecDeque<ChoiceResult>,
            type_lines: VecDeque<String>,
            recorded_menus: Vec<(String, Vec<ChoiceOption>)>,
            slash_prefills: Vec<String>,
            instant: String,
            narrative: String,
        }
        impl LabHost for MenuHost {
            fn write_narrative(&mut self, text: &str) -> std::io::Result<()> {
                self.narrative.push_str(text);
                Ok(())
            }
            fn write_instant(&mut self, text: &str) -> std::io::Result<()> {
                self.instant.push_str(text);
                Ok(())
            }
            fn on_pet_event(&mut self, _mood: Option<Mood>, _celebrate: bool) {}
            fn handle_slash(&mut self, line: &str) -> std::io::Result<SlashEffect> {
                if !line.starts_with('/') {
                    return Ok(SlashEffect::NotSlash);
                }
                if line == "/watch" {
                    return Ok(SlashEffect::SwitchToWatch);
                }
                Ok(SlashEffect::Handled)
            }
            fn wait_practice_turn(
                &mut self,
                _sandbox_dir: &Path,
                _practice: &Practice,
                _reported_cmds: &mut u64,
            ) -> Option<PracticePoll> {
                None
            }
            fn choose_option(
                &mut self,
                title: &str,
                items: &[ChoiceOption],
                _default_idx: usize,
                _hint: &str,
                _reader: &mut dyn BufRead,
            ) -> std::io::Result<ChoiceResult> {
                self.recorded_menus.push((title.to_string(), items.to_vec()));
                Ok(self.script.pop_front().unwrap_or(ChoiceResult::Eof))
            }
            fn read_slash_from_menu(
                &mut self,
                prefill: &str,
                _reader: &mut dyn BufRead,
            ) -> std::io::Result<SlashEffect> {
                self.slash_prefills.push(prefill.to_string());
                Ok(SlashEffect::Handled)
            }
            fn read_line(&mut self, _reader: &mut dyn BufRead) -> std::io::Result<Option<String>> {
                Ok(self.type_lines.pop_front())
            }
        }

        let dir = temp_dir("choose-flows");
        let prog_path = dir.join("progress.toml");
        let pet_path = dir.join("pet.toml");
        let all_units = units();
        let cat = catalog();

        // Main menu flow:
        // 1) Slash('/') -> read_slash_from_menu("/")
        // 2) Picked(2) (Unit 2, locked) -> locked message
        // 3) Picked(9) (Practice labs) -> Practice menu -> Picked(practice_count) (Back)
        // 4) Picked(1) (Unit 1) -> Unit menu -> Picked(0) (Lesson 1)
        //    -> Q1 ExplainChoice: Picked(choices) (hint), Picked(0) (wrong -> 4 hearts)
        //    -> Q2 TypeCommand: "cd" (wrong -> 3 hearts)
        //    -> Q3 ExplainChoice: Picked(1) (wrong -> 2 hearts)
        //    -> Q4 ReadOutput: Picked(0) (wrong -> 1 heart)
        //    -> Q5 TypeCommand: "ls" (wrong -> 0 hearts!)
        //    -> Zero hearts menu: Picked(0) (Review missed) -> exit review on Q1 Cancelled -> Post review Picked(1) (Back)
        //    -> Unit menu: Cancelled (Back to main menu)
        // 5) Main menu: Picked(12) (Back to Watch) -> SwitchToWatch!
        let practice_count = all_units.iter().filter(|u| u.practice.is_some()).count();
        let mut host = MenuHost {
            script: VecDeque::from([
                ChoiceResult::Slash("/".into()),
                ChoiceResult::Picked(2),
                ChoiceResult::Picked(all_units.len() + 1),
                ChoiceResult::Picked(practice_count),
                ChoiceResult::Picked(1),
                ChoiceResult::Picked(0),
                ChoiceResult::Picked(4),
                ChoiceResult::Picked(0),
                ChoiceResult::Picked(1),
                ChoiceResult::Picked(0),
                ChoiceResult::Picked(0),
                ChoiceResult::Cancelled,
                ChoiceResult::Picked(1),
                ChoiceResult::Cancelled,
                ChoiceResult::Picked(all_units.len() + 4),
            ]),
            type_lines: VecDeque::from(["cd".to_string(), "ls".to_string()]),
            recorded_menus: Vec::new(),
            slash_prefills: Vec::new(),
            instant: String::new(),
            narrative: String::new(),
        };

        let cfg = RunConfig {
            target: &[],
            lesson: None,
            interactive: true,
            today: day("2026-11-19"),
            now_epoch: BASE_EPOCH,
            daily_goal: 50,
            progress_path: Some(&prog_path),
            pet_path: Some(&pet_path),
            practice_root: None,
            recording_dir: None,
        };
        let (_, exit) = run_with_host(&cfg, &cat, &all_units, &mut host, &mut Cursor::new(&b""[..]), true).unwrap();
        assert_eq!(exit, PaneExit::SwitchToWatch);
        assert_eq!(host.slash_prefills, vec!["/"]);

        let (main_title, main_items) = &host.recorded_menus[0];
        assert!(main_title.starts_with("Lab — XP"), "{main_title}");
        assert_eq!(main_items.len(), all_units.len() + 5);
        assert!(!main_items[1].locked, "Unit 1 unlocked");
        assert!(main_items[2].locked, "Unit 2 locked");
        assert_eq!(main_items.last().unwrap().label, "Back to Watch");

        assert!(host.instant.contains("Unit 2 (Files & Folders) is locked"), "{}", host.instant);
        assert!(host.instant.contains("Hint:"), "hint option worked: {}", host.instant);
        assert!(host.recorded_menus.iter().any(|(t, _)| t.contains("Practice Labs")));
        assert!(host.recorded_menus.iter().any(|(t, _)| t.starts_with("Unit 1:")));
        assert!(host.recorded_menus.iter().any(|(t, _)| t.contains("Out of hearts")));

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn with_side_quest_unit(mut all_units: Vec<Unit>) -> Vec<Unit> {
        all_units.retain(|u| !u.is_side_quest());
        let mut lesson = all_units[0].lessons[0].clone();
        lesson.id = "u9-sq-l1".to_string();
        for (idx, q) in lesson.questions.iter_mut().enumerate() {
            let new_id = format!("u9-sq-q{}", idx + 1);
            match q {
                Question::ExplainChoice { id, .. }
                | Question::TypeCommand { id, .. }
                | Question::ReadOutput { id, .. }
                | Question::SpotDanger { id, .. } => *id = new_id,
            }
        }
        all_units.push(Unit {
            id: "apps-and-games-test".to_string(),
            number: 9,
            title: "Apps & Games Test".to_string(),
            summary: "Install apps safely.".to_string(),
            badge: "game_runner".to_string(),
            unlocks_species: None,
            unlock_after: Some("finding-your-way".to_string()),
            practice: None,
            lessons: vec![lesson],
        });
        lessons::validate(&all_units).expect("synthetic side-quest curriculum must validate");
        all_units
    }

    #[test]
    fn side_quest_unlocks_from_unlock_after_and_preserves_core_continue_order() {
        let dir = temp_dir("side-quest-unlock");
        let prog_path = dir.join("progress.toml");
        let pet_path = dir.join("pet.toml");
        let today = day("2026-11-19");
        let all_units = with_side_quest_unit(units());
        let cat = catalog();

        // Initially Unit 9 (`unlock_after = "finding-your-way"`) is locked and names Unit 1, not Unit 8.
        let mut out_locked = Vec::new();
        let target_u9 = ss(&["9"]);
        let cfg_u9 = RunConfig {
            target: &target_u9,
            lesson: None,
            interactive: true,
            today,
            now_epoch: BASE_EPOCH,
            daily_goal: 50,
            progress_path: Some(&prog_path),
            pet_path: Some(&pet_path),
            practice_root: None,
            recording_dir: None,
        };
        run_with_io(&cfg_u9, &cat, &all_units, &mut Cursor::new(&b""[..]), &mut out_locked).unwrap();
        let locked_text = String::from_utf8(out_locked).unwrap();
        assert!(
            locked_text.contains(
                "Unit 9 (Apps & Games Test) is locked — finish all lessons in Unit 1 (Finding Your Way) first!"
            ),
            "{locked_text}"
        );

        let initial_progress = render_progress(&all_units, &Progress::default(), &Pet::default(), today, 50);
        assert!(initial_progress.contains("Apps & Games Test [locked] [real-world]"), "{initial_progress}");

        // Complete Unit 1 only; Unit 2 remains incomplete.
        let mut p = Progress::default();
        for lesson in &all_units[0].lessons {
            p.complete_lesson(&lesson.id, false, today);
        }
        progress::save_to(&prog_path, &p).unwrap();

        // `Continue next lesson` prefers core Unit 2 over unlocked side-quest Unit 9.
        assert_eq!(first_open_lesson(&all_units, &p), Some((1, 0)));
        let unlocked_progress = render_progress(&all_units, &p, &Pet::default(), today, 50);
        assert!(unlocked_progress.contains("Apps & Games Test [0/1] [real-world]"), "{unlocked_progress}");

        // Main menu renders `real-world` detail, unlocked state, and dynamic `1-9 unit` footer.
        struct InspectHost {
            items: Vec<ChoiceOption>,
            hints: Vec<String>,
            instant: String,
        }
        impl LabHost for InspectHost {
            fn write_narrative(&mut self, _text: &str) -> std::io::Result<()> {
                Ok(())
            }
            fn write_instant(&mut self, text: &str) -> std::io::Result<()> {
                self.instant.push_str(text);
                Ok(())
            }
            fn on_pet_event(&mut self, _mood: Option<Mood>, _celebrate: bool) {}
            fn handle_slash(&mut self, _line: &str) -> std::io::Result<SlashEffect> {
                Ok(SlashEffect::NotSlash)
            }
            fn wait_practice_turn(
                &mut self,
                _sandbox_dir: &Path,
                _practice: &Practice,
                _reported_cmds: &mut u64,
            ) -> Option<PracticePoll> {
                None
            }
            fn choose_option(
                &mut self,
                _title: &str,
                items: &[ChoiceOption],
                _default_idx: usize,
                hint: &str,
                _reader: &mut dyn BufRead,
            ) -> std::io::Result<ChoiceResult> {
                self.items = items.to_vec();
                self.hints.push(hint.to_string());
                Ok(ChoiceResult::Cancelled)
            }
        }
        let mut inspect_host = InspectHost { items: Vec::new(), hints: Vec::new(), instant: String::new() };
        let cfg_menu = RunConfig {
            target: &[],
            lesson: None,
            interactive: true,
            today,
            now_epoch: BASE_EPOCH,
            daily_goal: 50,
            progress_path: Some(&prog_path),
            pet_path: Some(&pet_path),
            practice_root: None,
            recording_dir: None,
        };
        run_with_host(&cfg_menu, &cat, &all_units, &mut inspect_host, &mut Cursor::new(&b""[..]), true).unwrap();
        assert!(inspect_host.hints.iter().any(|h| h.contains("1-9 unit")), "{:?}", inspect_host.hints);
        let u9_item = inspect_host.items.iter().find(|opt| opt.key == "9").expect("Unit 9 item");
        assert!(!u9_item.locked, "Unit 9 should unlock immediately after Unit 1");
        assert!(u9_item.detail.contains("real-world"), "{}", u9_item.detail);

        // Unit 9 can now be played and completed while Unit 2 is still incomplete.
        let mut out_u9 = Vec::new();
        run_with_io(&cfg_u9, &cat, &all_units, &mut Cursor::new(b"2\nls\n1\n3\npwd\n".as_slice()), &mut out_u9)
            .unwrap();
        let u9_text = String::from_utf8(out_u9).unwrap();
        assert!(u9_text.contains("Game Runner"), "{u9_text}");
        let saved = progress::load_from(&prog_path);
        assert!(saved.has_badge("game_runner"));
        assert!(!saved.has_badge("shell_grad"));

        // When all core units 1..=8 are done and Unit 9 is still open, `first_open_lesson` falls back to Unit 9.
        let mut core_done = Progress::default();
        for unit in all_units.iter().filter(|u| !u.is_side_quest()) {
            for lesson in &unit.lessons {
                core_done.complete_lesson(&lesson.id, false, today);
            }
        }
        assert_eq!(first_open_lesson(&all_units, &core_done), Some((8, 0)));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn completing_core_units_awards_shell_grad_with_incomplete_side_quest() {
        let dir = temp_dir("core-shell-grad");
        let prog_path = dir.join("progress.toml");
        let pet_path = dir.join("pet.toml");
        let today = day("2026-11-19");
        let all_units = with_side_quest_unit(units());

        let mut p = Progress::default();
        for unit in all_units.iter().filter(|u| !u.is_side_quest()) {
            for (l_idx, lesson) in unit.lessons.iter().enumerate() {
                if unit.number < 8 || l_idx < 2 {
                    p.complete_lesson(&lesson.id, false, today);
                }
            }
        }
        progress::save_to(&prog_path, &p).unwrap();

        let script = "2\n3\n1\n3\n3\n";
        let target = ss(&["8"]);
        let mut output = Vec::new();
        let cfg = RunConfig {
            target: &target,
            lesson: Some(3),
            interactive: true,
            today,
            now_epoch: BASE_EPOCH,
            daily_goal: 50,
            progress_path: Some(&prog_path),
            pet_path: Some(&pet_path),
            practice_root: None,
            recording_dir: None,
        };
        run_with_io(&cfg, &catalog(), &all_units, &mut Cursor::new(script.as_bytes()), &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("Shell Graduate"), "{text}");

        let saved_prog = progress::load_from(&prog_path);
        assert!(saved_prog.has_badge("night_watch"));
        assert!(saved_prog.has_badge("shell_grad"));
        assert!(!saved_prog.has_badge("game_runner"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
