//! Unified `huh` companion pane (`watch` + `lab`):
//! - In `watch` mode, follows the recorded terminal next to it (see `session.rs`) and streams
//!   command explanations, `huh explain`, `huh "..."`, and `why` with a bounded typewriter.
//! - In `lab` mode, drives the interactive 8-unit Linux course, live auto-checked sandbox labs,
//!   and spaced review inside the same pane.
//! - Runs on a ~220ms `poll(fd=0)` event loop driving live Pebble 32x32/ASCII animation, rotating
//!   header tooltips, live `/` fuzzy command dropdowns, arrow-key selectors, and instant slash
//!   commands saved to `~/.local/state/huh/prefs.toml`.

use crate::breakdown::{Block, Explanation, Row};
use crate::catalog::Command;
use crate::learn::{self, ChoiceOption, ChoiceResult, LabHost, PaneExit, PracticePoll, SlashEffect};
use crate::matcher::LocalMatcher;
use crate::mode::Mode;
use crate::pet::{Mood, Pet, Species};
use crate::pin::{self, HeaderInfo};
use crate::practice::{self, Practice, RecordedCommand, Report};
use crate::prefs::{self, PaneMode, Prefs};
use crate::progress;
use crate::session;
use crate::slash::{self, FuzzyResolution, SlashDef};
use crate::sprite::{self, EffectiveMode, RenderStyle};
use crate::style::{self, Role, Style, TermEnv, Theme, paint};
use crate::tips;
use crate::tools::Tool;
use crate::tui::{self, Completion, EditorEvent, LineEditor, SelectItem, SelectOutcome};
use crate::typewriter::{self, Speed};
use anyhow::Result;
use std::cell::RefCell;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

const TICK_MS: i32 = 220;
const RELOCATE_EVERY_TICKS: u64 = 5;
/// Polls to wait for the end marker to reach the log after the status file changed (~2.2 s).
const PATIENCE: u32 = 10;
/// Most flag or argument rows shown for a command.
const MAX_ROWS: usize = 5;
/// Most lines of a `?words` answer.
const MAX_TOOL_LINES: usize = 20;
const PROMPT: &str = "huh> ";
const ALT_SCREEN: [&str; 3] = ["\x1b[?1049h", "\x1b[?1047h", "\x1b[?47h"];

static STOP: AtomicBool = AtomicBool::new(false);
static RECORDING_DIR: OnceLock<PathBuf> = OnceLock::new();
static RECORDING: OnceLock<PathBuf> = OnceLock::new();

pub fn set_recording(log: PathBuf) {
    RECORDING.set(log).ok();
}

pub fn set_recording_dir(dir: PathBuf) {
    RECORDING_DIR.set(dir).ok();
}

/// Startup options for the unified `huh` companion pane.
pub struct RunOpts {
    pub initial_mode: PaneMode,
    pub quiet: bool,
    pub tty: Option<String>,
    pub safety: Mode,
    pub theme: Theme,
    pub color_on: bool,
    pub dir: Option<PathBuf>,
    pub log: Option<PathBuf>,
}

/// One finished command, as recorded.
#[derive(Debug, PartialEq, Clone)]
pub struct Finished {
    pub command: String,
    pub exit: Option<i32>,
    /// The raw terminal text of its output, escape sequences included.
    pub raw: String,
}

type Stamp = (Option<SystemTime>, String);

/// Notices commands finishing in one recording.
pub struct Follower {
    log: PathBuf,
    status: PathBuf,
    seen: Option<Stamp>,
    waited: u32,
}

fn stamp(status: &Path) -> Option<Stamp> {
    let text = std::fs::read_to_string(status).ok()?;
    Some((std::fs::metadata(status).and_then(|m| m.modified()).ok(), text))
}

impl Follower {
    /// Starts after the command already recorded: only what finishes from now on is reported.
    pub fn new(log: &Path) -> Follower {
        let status = log.with_extension("status");
        Follower { log: log.to_path_buf(), seen: stamp(&status), status, waited: 0 }
    }

    /// True when the status file changed with a non-empty command since the last polled command.
    pub fn has_new_activity(&self) -> bool {
        let Some(now) = stamp(&self.status) else { return false };
        Some(&now) != self.seen.as_ref() && !now.1.trim().is_empty()
    }

    /// The command that finished since the last call, if any.
    pub fn poll(&mut self) -> Option<Finished> {
        let now = stamp(&self.status)?;
        if Some(&now) == self.seen.as_ref() || now.1.trim().is_empty() {
            return None;
        }
        let log = String::from_utf8_lossy(&std::fs::read(&self.log).ok()?).into_owned();
        if !session::region_closed(&log) && self.waited < PATIENCE {
            self.waited += 1;
            return None;
        }
        self.waited = 0;
        let (exit, command) = session::parse_status(&now.1);
        self.seen = Some(now);
        Some(Finished { command, exit, raw: session::last_region(&log).to_string() })
    }
}

fn last_activity(log: &Path) -> Option<SystemTime> {
    let status = log.with_extension("status");
    std::fs::metadata(&status).or_else(|_| std::fs::metadata(log)).ok()?.modified().ok()
}

fn ttys_of(log: &Path) -> Vec<String> {
    let Some(pid) = log.file_stem() else { return Vec::new() };
    let proc_dir = Path::new("/proc").join(pid);
    let stdin_of = |dir: &Path| std::fs::read_link(dir.join("fd/0")).ok().map(|t| t.display().to_string());
    let mut out = Vec::new();
    let children = std::fs::read_dir(proc_dir.join("task"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|task| std::fs::read_to_string(task.path().join("children")).ok())
        .flat_map(|list| list.split_whitespace().map(str::to_string).collect::<Vec<_>>());
    for child in children {
        if let Some(t) = stdin_of(&Path::new("/proc").join(&child)).filter(|t| t.starts_with("/dev/")) {
            out.push(t);
        }
    }
    out.extend(stdin_of(&proc_dir));
    out
}

fn tty_of(log: &Path) -> Option<String> {
    ttys_of(log).into_iter().next()
}

fn recordings(dir: &Path, own: Option<&Path>, check_alive: bool) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<(SystemTime, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "log") && Some(p.as_path()) != own)
        .filter(|p| !check_alive || p.file_stem().is_some_and(|pid| Path::new("/proc").join(pid).is_dir()))
        .filter_map(|p| Some((last_activity(&p)?, p)))
        .collect();
    found.sort_by_key(|a| std::cmp::Reverse(a.0));
    found.into_iter().map(|(_, p)| p).collect()
}

fn recording_dir() -> Option<PathBuf> {
    if let Some(dir) = RECORDING_DIR.get() {
        return Some(dir.clone());
    }
    if let Some(own) = std::env::var_os("HUH_SESSION_LOG").filter(|v| !v.is_empty())
        && let Some(dir) = Path::new(&own).parent()
        && dir.is_dir()
    {
        return Some(dir.to_path_buf());
    }
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".cache")))?;
    Some(base.join("huh"))
}

fn recording_of_tty(dir: &Path, tty: &str) -> Option<PathBuf> {
    recordings(dir, None, true).into_iter().find(|log| ttys_of(log).iter().any(|t| t == tty))
}

fn locate(tty: Option<&str>) -> Option<PathBuf> {
    if let Some(log) = RECORDING.get().filter(|log| log.is_file()) {
        return Some(log.clone());
    }
    let dir = recording_dir()?;
    match tty {
        Some(tty) if tty.chars().all(|c| c.is_ascii_digit()) => recording_of_tty(&dir, &format!("/dev/pts/{tty}")),
        Some(tty) => recording_of_tty(&dir, tty),
        None => {
            let own = std::env::var_os("HUH_SESSION_LOG").map(PathBuf::from);
            recordings(&dir, own.as_deref(), true).into_iter().next()
        }
    }
}

// ---------------------------------------------------------------------------
// Block & text formatting
// ---------------------------------------------------------------------------

fn programs(command: &str) -> Vec<String> {
    command
        .split(['|', ';', '&'])
        .filter_map(|stage| {
            stage
                .split_whitespace()
                .find(|w| !matches!(*w, "sudo" | "env" | "time") && !w.contains('='))
                .map(str::to_string)
        })
        .collect()
}

fn condensed(e: &Explanation) -> String {
    let mut out = String::new();
    for step in e.steps.iter().take(3) {
        crate::render::hanging(&mut out, 2, "", &step.text);
    }
    let mut rows: Vec<Row> = Vec::new();
    if let Some(Block::Cmd(c)) = e.blocks.iter().find(|b| matches!(b, Block::Cmd(_))) {
        rows.extend(c.rows.iter().skip(1).take(MAX_ROWS).cloned());
    }
    crate::render::rows(&mut out, &rows, 4);
    if let Some(w) = e.warnings.first() {
        let tag = format!("[{}]", w.risk.name());
        crate::render::hanging_tagged(&mut out, 2, ("! ", Role::Warning), &tag, &format!("{tag} {}", w.text));
    }
    out
}

/// Everything printed for one finished command (ends with a blank line).
pub fn block(catalog: &[Command], done: &Finished, explained: Option<&mut HashSet<String>>) -> String {
    render_block(catalog, done, explained).0
}

fn render_block(catalog: &[Command], done: &Finished, explained: Option<&mut HashSet<String>>) -> (String, bool) {
    let exit = done.exit.map_or("?".to_string(), |c| c.to_string());
    let exit_role = if done.exit == Some(0) {
        Role::Success
    } else if done.exit.is_some() {
        Role::Warning
    } else {
        Role::Dim
    };
    let mut out = format!(
        "{}{}\n",
        paint(Role::Header, &format!("$ {}", done.command)),
        paint(exit_role, &format!("  (exit {exit})"))
    );
    let explanation =
        (!done.command.trim().is_empty()).then(|| crate::explain::explain_command(catalog, &done.command));
    let has_warning = explanation.as_ref().is_some_and(|e| !e.warnings.is_empty());
    let mut explain_command = explanation.is_some();
    if let Some(seen) = explained {
        let names = programs(&done.command);
        explain_command = explain_command && !names.iter().all(|n| seen.contains(n));
        seen.extend(names);
    }
    if explain_command && let Some(e) = &explanation {
        out.push_str(&condensed(e));
    }
    if ALT_SCREEN.iter().any(|s| done.raw.contains(s)) {
        out.push_str(&paint(Role::Dim, "  full-screen program, nothing to explain"));
        out.push_str("\n\n");
        return (out, has_warning);
    }
    let cleaned = session::clean(&done.raw);
    if done.exit.is_some_and(|c| c != 0) {
        let last = session::Last { command: done.command.clone(), exit: done.exit, output: session::tail(&cleaned) };
        let mut answer = String::new();
        let _ = crate::doctor::explain(&last, &mut |text| answer = text.to_string());
        out.push_str(&format!("\n{}\n", style::answer(answer.trim_end())));
    } else if let Some(described) = crate::outputs::describe(&cleaned) {
        out.push_str(&format!("\n{}", described.text));
    }
    out.push('\n');
    (out, has_warning)
}

fn brief_why_nudge(done: &Finished) -> Option<String> {
    let cleaned = session::clean(&done.raw);
    let last = session::Last { command: done.command.clone(), exit: done.exit, output: session::tail(&cleaned) };
    let mut answer = String::new();
    let _ = crate::doctor::explain(&last, &mut |text| answer = text.to_string());
    let lines: Vec<&str> = answer.lines().map(str::trim).filter(|l| !l.is_empty()).take(2).collect();
    if lines.is_empty() {
        return None;
    }
    let exit_str = done.exit.map_or("?".to_string(), |c| c.to_string());
    let body = style::answer(&lines.join("\n  "));
    Some(format!("{}\n  {body}\n", paint(Role::Warning, &format!("$ {} (exit {exit_str})", done.command))))
}

fn no_hook_notice() -> String {
    format!(
        "{}\n{}\n{}\n\n",
        paint(Role::Note, "Live shell watching needs 'huh setup' in your shell."),
        paint(Role::Dim, "Meanwhile type commands/questions below or /mode lab to start lessons."),
        paint(Role::Dim, "(Waiting for a recorded shell — attaches automatically once one starts.)")
    )
}

// ---------------------------------------------------------------------------
// Pet session & watch milestone unlocks
// ---------------------------------------------------------------------------

struct PetSession {
    pet: Pet,
    transient: Option<Mood>,
    enabled: bool,
    path: Option<PathBuf>,
}

impl PetSession {
    fn load(enabled: bool) -> PetSession {
        let path = crate::pet::default_state_path();
        let pet = path.as_deref().map_or_else(Pet::default, crate::pet::load_from);
        PetSession { pet, transient: None, enabled, path }
    }

    fn merge_disk_metadata(&mut self) {
        let Some(path) = &self.path else { return };
        let Ok(text) = std::fs::read_to_string(path) else { return };
        let Ok(disk) = toml::from_str::<Pet>(&text) else { return };
        self.pet.name = disk.name;
        self.pet.species = disk.species;
        for sp in disk.unlocked_species {
            self.pet.unlock_species(sp);
        }
        self.pet.normalize();
    }

    fn banner(&self, now_epoch: u64) -> Option<crate::pet::Banner> {
        self.enabled.then(|| self.pet.banner(now_epoch, self.transient))
    }

    fn render_card(&mut self, now_epoch: u64) -> String {
        self.merge_disk_metadata();
        crate::pet::render_pane_card(&self.pet, now_epoch, self.transient)
    }

    fn observe_with_unlocks(
        &mut self,
        done: &Finished,
        has_warning: bool,
        now_epoch: u64,
    ) -> (Option<crate::pet::Banner>, Vec<Species>) {
        if !self.enabled || done.command.trim().is_empty() {
            return (None, Vec::new());
        }
        let mut unlocked = Vec::new();
        if done.exit == Some(0) && !has_warning {
            let prog = programs(&done.command).into_iter().next();
            let mood = self.pet.feed(prog.as_deref(), now_epoch);
            unlocked = self.pet.check_watch_unlocks();
            self.transient =
                if !unlocked.is_empty() || mood == Mood::Celebrating { Some(Mood::Celebrating) } else { None };
        } else if done.exit.is_some_and(|c| c != 0) || has_warning {
            let mood = self.pet.react_alert();
            if has_warning {
                unlocked = self.pet.record_warning();
            }
            self.transient = Some(if !unlocked.is_empty() { Mood::Celebrating } else { mood });
        } else {
            return (None, Vec::new());
        }
        self.merge_disk_metadata();
        (Some(self.pet.banner(now_epoch, self.transient)), unlocked)
    }

    #[cfg(test)]
    fn observe(&mut self, done: &Finished, has_warning: bool, now_epoch: u64) -> Option<crate::pet::Banner> {
        self.observe_with_unlocks(done, has_warning, now_epoch).0
    }

    fn save(&mut self) {
        self.merge_disk_metadata();
        self.save_direct();
    }

    fn save_direct(&self) {
        if let Some(path) = &self.path {
            let _ = crate::pet::save_to(path, &self.pet);
        }
    }

    fn reload_from_disk(&mut self, celebrate: bool) {
        if let Some(path) = &self.path
            && path.is_file()
        {
            let disk = crate::pet::load_from(path);
            let prev_unlocks = self.pet.unlocked_species.clone();
            self.pet = disk;
            for sp in prev_unlocks {
                self.pet.unlock_species(sp);
            }
            self.pet.normalize();
        }
        if celebrate {
            self.transient = Some(Mood::Celebrating);
        }
    }
}

fn format_watch_unlocks(unlocked: &[Species]) -> String {
    let mut out = String::new();
    for &sp in unlocked {
        let _ = writeln!(
            out,
            "  {} Switch anytime with `/pet species {}`.",
            paint(Role::Success, &format!("NEW PET UNLOCKED ({})!", sp.label())),
            sp.name()
        );
    }
    out
}

// ---------------------------------------------------------------------------
// Signals & Linux stdin poll reader
// ---------------------------------------------------------------------------

extern "C" fn on_interrupt(_: i32) {
    STOP.store(true, Ordering::Relaxed);
}

#[cfg(target_os = "linux")]
fn catch_ctrl_c() {
    unsafe extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }
    const SIGINT: i32 = 2;
    // SAFETY: the handler only stores into an atomic.
    unsafe {
        signal(SIGINT, on_interrupt as *const () as usize);
    }
}

#[cfg(not(target_os = "linux"))]
fn catch_ctrl_c() {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PollResult {
    Line,
    Timeout,
    Eof,
}

#[cfg(target_os = "linux")]
struct StdinReader {
    buf: Vec<u8>,
    eof: bool,
    interactive: bool,
}

#[cfg(target_os = "linux")]
impl StdinReader {
    fn new(interactive: bool) -> Self {
        Self { buf: Vec::new(), eof: false, interactive }
    }

    fn has_line(&self) -> bool {
        self.buf.contains(&b'\n')
    }

    fn pop_line(&mut self) -> Option<String> {
        if let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let mut raw: Vec<u8> = self.buf.drain(..=pos).collect();
            while raw.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
                raw.pop();
            }
            return Some(String::from_utf8_lossy(&raw).into_owned());
        }
        if self.eof && !self.buf.is_empty() {
            let raw: Vec<u8> = std::mem::take(&mut self.buf);
            return Some(String::from_utf8_lossy(&raw).into_owned());
        }
        None
    }

    #[cfg(test)]
    fn push_test_line(&mut self, line: &str) {
        self.buf.extend_from_slice(line.as_bytes());
        self.buf.push(b'\n');
    }

    /// Consumes a single blank Enter line if one arrived on stdin (used to swallow typewriter skip-Enter).
    fn consume_empty_newline(&mut self) {
        if self.has_line() {
            if let Some(pos) = self.buf.iter().position(|&b| b == b'\n')
                && self.buf[..pos].iter().all(|b| b.is_ascii_whitespace())
            {
                self.buf.drain(..=pos);
            }
            return;
        }
        if self.poll_fd(0) && !self.eof {
            let _ = self.read_chunk();
            if let Some(pos) = self.buf.iter().position(|&b| b == b'\n')
                && self.buf[..pos].iter().all(|b| b.is_ascii_whitespace())
            {
                self.buf.drain(..=pos);
            }
        }
    }

    fn poll_fd(&self, timeout_ms: i32) -> bool {
        if !self.interactive {
            if timeout_ms > 0 {
                std::thread::sleep(Duration::from_millis(timeout_ms as u64));
            }
            return false;
        }
        if self.has_line() || self.eof {
            return true;
        }
        #[repr(C)]
        struct PollFd {
            fd: i32,
            events: i16,
            revents: i16,
        }
        unsafe extern "C" {
            fn poll(fds: *mut PollFd, count: usize, timeout_ms: i32) -> i32;
        }
        const POLLIN: i16 = 1;
        const POLLHUP: i16 = 16;
        let mut pfd = PollFd { fd: 0, events: POLLIN, revents: 0 };
        // SAFETY: `pfd` is a valid stack PollFd targeting stdin fd 0.
        let rc = unsafe { poll(&mut pfd, 1, timeout_ms) };
        rc > 0 && (pfd.revents & (POLLIN | POLLHUP)) != 0
    }

    fn read_chunk(&mut self) -> bool {
        unsafe extern "C" {
            fn read(fd: i32, buf: *mut std::ffi::c_void, count: usize) -> isize;
        }
        let mut tmp = [0u8; 1024];
        // SAFETY: valid buffer and length passed to POSIX read on fd 0.
        let n = unsafe { read(0, tmp.as_mut_ptr().cast(), tmp.len()) };
        if n <= 0 {
            self.eof = true;
            return false;
        }
        self.buf.extend_from_slice(&tmp[..n as usize]);
        true
    }

    fn wait_tick(&mut self, timeout_ms: i32) -> PollResult {
        if self.has_line() {
            return PollResult::Line;
        }
        if self.eof {
            return if self.buf.is_empty() { PollResult::Eof } else { PollResult::Line };
        }
        if !self.interactive {
            if timeout_ms > 0 {
                std::thread::sleep(Duration::from_millis(timeout_ms as u64));
            }
            return PollResult::Timeout;
        }
        if !self.poll_fd(timeout_ms) {
            return PollResult::Timeout;
        }
        let _ = self.read_chunk();
        if self.has_line() || (self.eof && !self.buf.is_empty()) {
            PollResult::Line
        } else if self.eof {
            PollResult::Eof
        } else {
            PollResult::Timeout
        }
    }
}

#[cfg(not(target_os = "linux"))]
struct StdinReader {
    interactive: bool,
    eof: bool,
    queued: Option<String>,
}

#[cfg(not(target_os = "linux"))]
impl StdinReader {
    fn new(interactive: bool) -> Self {
        Self { interactive, eof: false, queued: None }
    }

    fn has_line(&self) -> bool {
        self.queued.is_some()
    }

    fn pop_line(&mut self) -> Option<String> {
        self.queued.take()
    }

    #[cfg(test)]
    fn push_test_line(&mut self, line: &str) {
        self.queued = Some(line.to_string());
    }

    fn consume_empty_newline(&mut self) {}

    fn poll_fd(&self, timeout_ms: i32) -> bool {
        if timeout_ms > 0 {
            std::thread::sleep(Duration::from_millis(timeout_ms as u64));
        }
        false
    }

    fn wait_tick(&mut self, timeout_ms: i32) -> PollResult {
        if self.queued.is_some() {
            return PollResult::Line;
        }
        if !self.interactive || self.eof {
            if timeout_ms > 0 {
                std::thread::sleep(Duration::from_millis(timeout_ms as u64));
            }
            return if self.eof { PollResult::Eof } else { PollResult::Timeout };
        }
        let mut buf = String::new();
        match std::io::stdin().read_line(&mut buf) {
            Ok(0) => {
                self.eof = true;
                PollResult::Eof
            }
            Ok(_) => {
                self.queued = Some(buf.trim_end_matches(['\r', '\n']).to_string());
                PollResult::Line
            }
            Err(_) => {
                self.eof = true;
                PollResult::Eof
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Input handling & slash commands
// ---------------------------------------------------------------------------

/// What a typed line leads to in Watch mode.
#[derive(Debug, PartialEq)]
pub enum Reply {
    /// Narrative text streamed through the typewriter (ends with a blank line).
    Text(String),
    /// Instant output never delayed by the typewriter (tools, slash feedback, notices).
    Instant(String),
    /// `/find`: suspend header, open finder, resume header.
    Find(Vec<String>),
    /// `/mode` / `/watch` / `/lab` / `/tips` / ...: switch modes or execute slash toggle.
    Slash(SlashCmd),
    /// A unique fuzzy slash match: prints `note` in dim text before executing `inner`.
    SlashNote { note: String, inner: Box<Reply> },
    /// Bare `/` or an ambiguous slash input: opens an interactive selector (or prints a list).
    SlashPick { title: String, raw_line: String, defs: Vec<&'static SlashDef> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlashCmd {
    ModeInteractive,
    Mode(PaneMode, Vec<String>),
    Safety(Option<String>),
    Theme(Option<String>),
    Pet(Vec<String>),
    Speed(Option<String>),
    Quiet(Option<String>),
    Tips(Option<String>),
}

/// What answering a typed line needs. Nothing in it runs anything.
pub struct Input<'a> {
    everything: &'a [Command],
    matcher: LocalMatcher,
    mode: Mode,
    files: Vec<String>,
    tools: std::cell::OnceCell<Vec<Tool>>,
}

impl<'a> Input<'a> {
    pub fn new(
        everything: &'a [Command],
        usable: &[Command],
        mode: Mode,
        files: Vec<String>,
        tools: Option<Vec<Tool>>,
    ) -> Input<'a> {
        let cell = std::cell::OnceCell::new();
        if let Some(tools) = tools {
            let _ = cell.set(tools);
        }
        Input { everything, matcher: LocalMatcher::new(usable), mode, files, tools: cell }
    }

    fn tools(&self) -> &[Tool] {
        self.tools.get_or_init(|| crate::tools::load_all(self.everything))
    }
}

fn help_text() -> String {
    "Commands & toggles (instant, nothing you type is ever run; type / for fuzzy commands · Tab completes):\n\
     \x20 <command line>      explain what it does, as `huh explain`\n\
     \x20 <plain words>       suggest commands, as `huh \"...\"`\n\
     \x20 ?word [word...]     search installed programs, as `huh tools`\n\
     \x20 why / Enter         explain last failure / replay last command in full\n\
     \x20 /mode [watch|lab]   switch Watch / Lab mode (shortcuts: /watch, /lab)\n\
     \x20 /safety [norm|exp]  toggle safety guardrails;  /theme [name]: live theme\n\
     \x20 /pet [status|name|species|style|customize]     manage Pebble & sprites\n\
     \x20 /speed [off|fast|normal|slow|ms]  typewriter;  /quiet [on|off]\n\
     \x20 /tips [on|off|next] toggle or cycle header tips;  /find [words]\n\n"
        .to_string()
}

fn note(text: &str) -> String {
    format!("{}\n\n", paint(Role::Note, text))
}

fn why_text(done: &Finished) -> String {
    let exit = done.exit.map_or(String::new(), |c| format!(" (exit {c})"));
    let last = session::Last {
        command: done.command.clone(),
        exit: done.exit,
        output: session::tail(&session::clean(&done.raw)),
    };
    let mut answer = String::new();
    let _ = crate::doctor::explain(&last, &mut |text| answer = text.to_string());
    let header = paint(Role::Header, &format!("$ {}{exit}", done.command));
    format!("{header}\n{}\n\n", crate::style::answer(answer.trim_end()))
}

fn tools_text(words: &[String], tools: &[Tool]) -> String {
    let matches = crate::tools::filter(tools, words);
    if matches.is_empty() {
        return note("no program matches");
    }
    let shown = &matches[..matches.len().min(MAX_TOOL_LINES)];
    let text = crate::tools::render(shown, tools.len(), true);
    let mut lines: Vec<&str> = text.lines().collect();
    lines.pop();
    let mut out = lines.join("\n");
    out.push('\n');
    let more = matches.len() - shown.len();
    let tail = if more > 0 {
        format!("{more} more: huh tools {}", words.join(" "))
    } else {
        format!("{} of {} programs match", matches.len(), tools.len())
    };
    out.push_str(&paint(Role::Dim, &tail));
    out.push_str("\n\n");
    out
}

fn split_slash(line: &str, name: &str) -> Option<Vec<String>> {
    if line == name {
        return Some(Vec::new());
    }
    let rest = line.strip_prefix(name)?;
    if rest.starts_with(char::is_whitespace) {
        Some(rest.split_whitespace().map(str::to_string).collect())
    } else {
        None
    }
}

/// Parses a pane slash command (`/mode`, `/watch`, `/lab`, `/safety`, `/theme`, `/pet`,
/// `/speed`, `/quiet`, `/find`, `/help`). Returns `None` for non-slash inputs or unknown paths.
pub fn parse_slash(line: &str) -> Option<Reply> {
    let line = line.trim();
    if let Some(words) = split_slash(line, "/find") {
        return Some(Reply::Find(words));
    }
    if line == "help" || line == "/help" {
        return Some(Reply::Instant(help_text()));
    }
    if let Some(args) = split_slash(line, "/mode") {
        return Some(if args.is_empty() {
            Reply::Slash(SlashCmd::ModeInteractive)
        } else if args.len() == 1 {
            match PaneMode::parse(&args[0]) {
                Ok(mode) => Reply::Slash(SlashCmd::Mode(mode, Vec::new())),
                Err(err) => Reply::Instant(note(&format!("{err}"))),
            }
        } else if args[0].eq_ignore_ascii_case("lab") {
            Reply::Slash(SlashCmd::Mode(PaneMode::Lab, args[1..].to_vec()))
        } else {
            Reply::Instant(note(&format!("usage: /mode [{}]", PaneMode::VALID)))
        });
    }
    if let Some(args) = split_slash(line, "/watch") {
        return Some(if args.is_empty() {
            Reply::Slash(SlashCmd::Mode(PaneMode::Watch, Vec::new()))
        } else {
            Reply::Instant(note("usage: /watch (switches to Watch mode)"))
        });
    }
    if let Some(args) = split_slash(line, "/lab") {
        return Some(Reply::Slash(SlashCmd::Mode(PaneMode::Lab, args)));
    }
    if let Some(args) = split_slash(line, "/safety") {
        return Some(if args.len() <= 1 {
            Reply::Slash(SlashCmd::Safety(args.into_iter().next()))
        } else {
            Reply::Instant(note("usage: /safety [normal|expert]"))
        });
    }
    if let Some(args) = split_slash(line, "/theme") {
        return Some(if args.len() <= 1 {
            Reply::Slash(SlashCmd::Theme(args.into_iter().next()))
        } else {
            Reply::Instant(note("usage: /theme [name]"))
        });
    }
    if line == "pet" {
        return Some(Reply::Slash(SlashCmd::Pet(Vec::new())));
    }
    if let Some(args) = split_slash(line, "/pet") {
        return Some(Reply::Slash(SlashCmd::Pet(args)));
    }
    if let Some(args) = split_slash(line, "/speed") {
        return Some(if args.len() <= 1 {
            Reply::Slash(SlashCmd::Speed(args.into_iter().next()))
        } else {
            Reply::Instant(note(&format!("usage: /speed [{}]", Speed::VALID)))
        });
    }
    if let Some(args) = split_slash(line, "/quiet") {
        return Some(if args.len() <= 1 {
            Reply::Slash(SlashCmd::Quiet(args.into_iter().next()))
        } else {
            Reply::Instant(note("usage: /quiet [on|off]"))
        });
    }
    if let Some(args) = split_slash(line, "/tips") {
        return Some(if args.len() <= 1 {
            Reply::Slash(SlashCmd::Tips(args.into_iter().next()))
        } else {
            Reply::Instant(note("usage: /tips [on|off|next]"))
        });
    }
    None
}

fn answer_slash(line: &str) -> Option<Reply> {
    let trimmed = line.trim();
    match slash::resolve_submit(trimmed) {
        FuzzyResolution::NotSlash => parse_slash(trimmed),
        FuzzyResolution::Exact(resolved) => {
            Some(parse_slash(&resolved).unwrap_or_else(|| Reply::Instant(note(&resolved))))
        }
        FuzzyResolution::Unique { resolved, note: hint } => {
            let inner = parse_slash(&resolved).unwrap_or_else(|| Reply::Instant(note(&resolved)));
            Some(Reply::SlashNote { note: hint, inner: Box::new(inner) })
        }
        FuzzyResolution::Ambiguous(defs) => {
            Some(Reply::SlashPick { title: "Matching slash commands:".into(), raw_line: trimmed.to_string(), defs })
        }
        FuzzyResolution::Palette(defs) => {
            Some(Reply::SlashPick { title: "Slash commands:".into(), raw_line: trimmed.to_string(), defs })
        }
        FuzzyResolution::Unknown(tok) => {
            Some(Reply::Instant(note(&format!("Unknown slash command \"{tok}\" — type / to browse commands."))))
        }
    }
}

/// Answers one typed line in Watch mode.
pub fn answer_line(line: &str, ctx: &Input, last: Option<&Finished>) -> Reply {
    let line = line.trim();
    if line.is_empty() {
        return match last {
            Some(done) => Reply::Text(block(ctx.everything, done, None)),
            None => Reply::Instant(note("nothing has finished in the followed terminal yet")),
        };
    }
    if let Some(slash) = answer_slash(line) {
        return slash;
    }
    if let Some(words) = line.strip_prefix('?') {
        let words: Vec<String> = words.split_whitespace().map(str::to_string).collect();
        if words.is_empty() {
            return Reply::Instant(note("type ? and the words to look for, e.g. ?compress"));
        }
        return Reply::Instant(tools_text(&words, ctx.tools()));
    }
    if line == "why" {
        return match last {
            Some(done) => Reply::Text(why_text(done)),
            None => Reply::Instant(note(
                "nothing has finished in the followed terminal yet, so there is nothing to explain",
            )),
        };
    }
    let known = |word: &str| ctx.matcher.knows_program(word) || crate::COMMON_PROGRAMS.contains(&word);
    if let Some(command) = crate::guard::command_line_request(line, &known) {
        let explanation = crate::explain::explain_command(ctx.everything, command);
        return Reply::Text(format!("{}\n", crate::render::render(&explanation)));
    }
    let outcome = crate::suggest_in(ctx.mode, &ctx.matcher, line, ctx.files.clone(), &mut |_| None);
    match outcome {
        Ok(outcome) => Reply::Text(format!("{}\n", crate::render_outcome(&outcome, ctx.everything, false))),
        Err(error) => Reply::Instant(note(&format!("huh: {error:#}"))),
    }
}

// ---------------------------------------------------------------------------
// Live pane state, slash execution, and Watch + Lab coordination
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
enum SlashOutcome {
    Output(String),
    SwitchMode(PaneMode, Vec<String>),
}

fn slash_completions(line: &str, unlocked_species: &[Species]) -> Vec<Completion> {
    slash::completions(line, unlocked_species)
        .into_iter()
        .map(|c| Completion { insert: c.insert, display: c.display, summary: c.summary, auto_submit: c.auto_submit })
        .collect()
}

fn theme_summary(theme: Theme) -> &'static str {
    match theme {
        Theme::BabyBlue => "default baby-blue palette",
        Theme::Auto => "Omarchy or terminal 16-colour",
        Theme::Omarchy => "current Omarchy desktop theme",
        Theme::Dark => "One Dark palette",
        Theme::Light => "One Light palette",
        Theme::Nord => "Nord Arctic palette",
        Theme::Gruvbox => "Gruvbox warm palette",
        Theme::Catppuccin => "Catppuccin Mocha palette",
        Theme::Mono => "bold, dim, and underline only",
    }
}

struct PaneState {
    everything: Vec<Command>,
    usable: Vec<Command>,
    files: Vec<String>,
    mode: PaneMode,
    safety: Mode,
    theme: Theme,
    color_on: bool,
    apply_global_style: bool,
    speed: Speed,
    pet_style: RenderStyle,
    pet_enabled: bool,
    stdout_tty: bool,
    quiet: bool,
    tips_enabled: bool,
    tip_index: usize,
    tip_ticks: u64,
    last_tip: Option<String>,
    explained: Option<HashSet<String>>,
    tty_arg: Option<String>,
    tty_label: String,
    tick: u64,
    last_ascii_frame: Option<[&'static str; crate::pet::ART_HEIGHT]>,
    last_pixel_key: Option<u64>,
    last_banner: Option<crate::pet::Banner>,
    last_pet_mode: EffectiveMode,
    prefs: Prefs,
    prefs_path: Option<PathBuf>,
    sprite_override_dir: Option<PathBuf>,
    pet_session: PetSession,
    lab_xp: Option<(u64, u32)>,
    pinned: bool,
    last_finished: Option<Finished>,
    follower: Option<Follower>,
    stdin: StdinReader,
    editor: LineEditor,
}

impl PaneState {
    fn save_prefs(&self) {
        if let Some(path) = &self.prefs_path {
            let _ = prefs::save_to(path, &self.prefs);
        }
    }

    fn use_tui(&self) -> bool {
        cfg!(target_os = "linux") && self.stdin.interactive && tui::is_interactive_tty()
    }

    fn effective_pet_mode(&self) -> EffectiveMode {
        let env_lookup = |k: &str| std::env::var(k).ok();
        sprite::detect_mode(self.pet_style, &env_lookup, self.stdout_tty, self.pet_enabled)
    }

    fn header_info(&self) -> HeaderInfo {
        let now = crate::date::now_epoch();
        HeaderInfo {
            mode: self.mode,
            safety: self.safety,
            tty_label: self.tty_label.clone(),
            pet: self.pet_session.banner(now),
            pet_mode: self.effective_pet_mode(),
            tick: self.tick,
            lab_xp: if self.mode == PaneMode::Lab { self.lab_xp } else { None },
            tip: self.tips_enabled.then(|| tips::tip_at(self.tip_index).to_string()),
        }
    }

    fn refresh_lab_xp(&mut self) {
        if self.mode == PaneMode::Lab {
            let p = progress::load_default();
            self.lab_xp = Some((p.xp, p.streak));
        } else {
            self.lab_xp = None;
        }
    }

    fn record_rendered_snapshot(&mut self, info: &HeaderInfo) {
        self.last_banner = info.pet.clone();
        self.last_pet_mode = info.pet_mode;
        self.last_tip = info.tip.clone();
        match (&info.pet, info.pet_mode) {
            (Some(b), EffectiveMode::Ascii) => {
                self.last_ascii_frame = Some(crate::pet::art_frame(b.species, b.mood, info.tick));
                self.last_pixel_key = None;
            }
            (Some(b), EffectiveMode::Pixel) => {
                self.last_pixel_key = Some(sprite::frame_key(b.mood, info.tick));
                self.last_ascii_frame = None;
            }
            _ => {
                self.last_ascii_frame = None;
                self.last_pixel_key = None;
            }
        }
    }

    fn emit_header_refresh(&mut self) {
        let info = self.header_info();
        self.record_rendered_snapshot(&info);
        if self.pinned
            && let Some(seq) = pin::refresh_pet_frame(&info)
        {
            write_raw(&seq);
        }
    }

    fn tick_animation(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        if self.tips_enabled {
            self.tip_ticks = self.tip_ticks.wrapping_add(1);
            if self.tip_ticks >= tips::ROTATE_TICKS {
                self.tip_ticks = 0;
                self.tip_index = (self.tip_index + 1) % tips::TIPS.len();
            }
        }
        if !self.pinned {
            return;
        }
        let info = self.header_info();
        let changed = info.tip != self.last_tip
            || info.pet != self.last_banner
            || info.pet_mode != self.last_pet_mode
            || match (&info.pet, info.pet_mode) {
                (Some(b), EffectiveMode::Ascii) => {
                    Some(crate::pet::art_frame(b.species, b.mood, info.tick)) != self.last_ascii_frame
                }
                (Some(b), EffectiveMode::Pixel) => Some(sprite::frame_key(b.mood, info.tick)) != self.last_pixel_key,
                _ => false,
            };
        if changed {
            self.record_rendered_snapshot(&info);
            if let Some(seq) = pin::refresh_pet_frame(&info) {
                write_raw(&seq);
            }
        }
    }

    fn try_attach_follower(&mut self) -> Option<String> {
        if self.follower.is_some() {
            return None;
        }
        let log = locate(self.tty_arg.as_deref())?;
        let tty_name = tty_of(&log).unwrap_or_else(|| log.display().to_string());
        self.follower = Some(Follower::new(&log));
        self.tty_label = tty_name.clone();
        self.emit_header_refresh();
        Some(format!("  {}\n\n", paint(Role::Dim, &format!("Attached to recorded terminal {tty_name}."))))
    }

    fn poll_watch_activity(&mut self, relocate_ticks: &mut u64) -> Option<String> {
        let mut out = String::new();
        if self.follower.is_none() {
            *relocate_ticks += 1;
            if relocate_ticks.is_multiple_of(RELOCATE_EVERY_TICKS)
                && let Some(msg) = self.try_attach_follower()
            {
                out.push_str(&msg);
            }
        }
        if let Some(done) = self.follower.as_mut().and_then(Follower::poll) {
            let (text, has_warning) = render_block(&self.everything, &done, self.explained.as_mut());
            let now = crate::date::now_epoch();
            let (banner, unlocked) = self.pet_session.observe_with_unlocks(&done, has_warning, now);
            if banner.is_some() {
                self.pet_session.save();
                self.emit_header_refresh();
            }
            self.last_finished = Some(done);
            out.push_str(&text);
            if !unlocked.is_empty() {
                out.push_str(&format_watch_unlocks(&unlocked));
            }
        }
        (!out.is_empty()).then_some(out)
    }

    fn write_typewritten(&mut self, text: &str) {
        let budget = self.speed.budget_ms();
        if budget == 0 {
            write_raw(text);
            return;
        }
        let use_tty_editor = self.use_tui();
        let mut flushed_by_stdin = false;
        let interactive = self.stdin.interactive;
        let has_buffered = !use_tty_editor && self.stdin.has_line();
        let mut stdout = std::io::stdout().lock();
        let _ = typewriter::write_animated(&mut stdout, text, budget, &mut |delay| {
            if STOP.load(Ordering::Relaxed) || self.follower.as_ref().is_some_and(Follower::has_new_activity) {
                return true;
            }
            if use_tty_editor {
                return self.editor.wait_typewriter_skip(delay);
            }
            if has_buffered {
                return true;
            }
            if interactive && self.stdin.poll_fd(delay.as_millis() as i32) {
                flushed_by_stdin = true;
                return true;
            }
            if !interactive && !delay.is_zero() {
                std::thread::sleep(delay);
            }
            false
        });
        drop(stdout);
        if flushed_by_stdin {
            self.stdin.consume_empty_newline();
        }
    }

    fn read_blocking_line(&mut self) -> Option<String> {
        loop {
            if STOP.load(Ordering::Relaxed) {
                return None;
            }
            if self.pinned
                && let Some(redraw) = pin::on_resize()
            {
                write_raw(&redraw);
            }
            match self.stdin.wait_tick(TICK_MS) {
                PollResult::Line => return self.stdin.pop_line(),
                PollResult::Eof => return None,
                PollResult::Timeout => self.tick_animation(),
            }
        }
    }

    fn read_editor_line(&mut self, prompt: &str) -> std::io::Result<EditorEvent> {
        let mut editor = std::mem::take(&mut self.editor);
        let unlocked = Rc::new(RefCell::new(self.pet_session.pet.unlocked_species.clone()));
        let unlocked_for_comp = Rc::clone(&unlocked);
        let complete = move |line: &str| slash_completions(line, &unlocked_for_comp.borrow());
        let mut on_tick = || {
            if self.pinned
                && let Some(redraw) = pin::on_resize()
            {
                write_raw(&redraw);
            }
            self.tick_animation();
            *unlocked.borrow_mut() = self.pet_session.pet.unlocked_species.clone();
            None
        };
        let res = editor.read_line_with_cancel(prompt, &complete, &mut on_tick, &STOP);
        self.editor = editor;
        res
    }

    fn read_watch_line(&mut self, relocate_ticks: &mut u64) -> std::io::Result<EditorEvent> {
        let mut editor = std::mem::take(&mut self.editor);
        let unlocked = Rc::new(RefCell::new(self.pet_session.pet.unlocked_species.clone()));
        let unlocked_for_comp = Rc::clone(&unlocked);
        let complete = move |line: &str| slash_completions(line, &unlocked_for_comp.borrow());
        let mut on_tick = || {
            if self.pinned
                && let Some(redraw) = pin::on_resize()
            {
                write_raw(&redraw);
            }
            self.tick_animation();
            let async_out = self.poll_watch_activity(relocate_ticks);
            *unlocked.borrow_mut() = self.pet_session.pet.unlocked_species.clone();
            async_out
        };
        let res = editor.read_line_with_cancel(PROMPT, &complete, &mut on_tick, &STOP);
        self.editor = editor;
        res
    }

    fn run_select_menu(
        &mut self,
        title: &str,
        items: &[SelectItem],
        initial: usize,
        hint: &str,
    ) -> std::io::Result<SelectOutcome> {
        if STOP.load(Ordering::Relaxed) {
            return Ok(SelectOutcome::Eof);
        }
        let mut on_tick = || {
            if self.pinned
                && let Some(redraw) = pin::on_resize()
            {
                write_raw(&redraw);
            }
            self.tick_animation();
            None
        };
        tui::select_menu(title, items, initial, hint, &mut on_tick)
    }

    fn exec_slash_from_menu(&mut self, raw: &str) -> SlashOutcome {
        if STOP.load(Ordering::Relaxed) {
            return SlashOutcome::Output(String::new());
        }
        let line = if raw == "/" {
            if !self.use_tui() {
                return SlashOutcome::Output(String::new());
            }
            self.editor.push_prefill("/");
            match self.read_editor_line(PROMPT) {
                Ok(EditorEvent::Submit(line)) if !line.trim().is_empty() => line,
                Ok(EditorEvent::Interrupted) => {
                    STOP.store(true, Ordering::Relaxed);
                    return SlashOutcome::Output(String::new());
                }
                _ => return SlashOutcome::Output(String::new()),
            }
        } else {
            raw.to_string()
        };
        self.exec_submitted_slash(&line)
    }

    fn exec_submitted_slash(&mut self, line: &str) -> SlashOutcome {
        match answer_slash(line) {
            Some(Reply::Instant(text)) | Some(Reply::Text(text)) => SlashOutcome::Output(text),
            Some(Reply::Find(words)) => {
                self.run_find(&words);
                SlashOutcome::Output(String::new())
            }
            Some(Reply::Slash(cmd)) => self.exec_slash(cmd),
            Some(Reply::SlashNote { note, inner }) => {
                write_raw(&format!("{}\n", paint(Role::Dim, &note)));
                self.run_reply_to_outcome(*inner)
            }
            Some(Reply::SlashPick { title, raw_line, defs }) => self.exec_slash_pick(&title, &raw_line, &defs),
            None => SlashOutcome::Output(String::new()),
        }
    }

    fn run_reply_to_outcome(&mut self, reply: Reply) -> SlashOutcome {
        match reply {
            Reply::Instant(text) | Reply::Text(text) => SlashOutcome::Output(text),
            Reply::Find(words) => {
                self.run_find(&words);
                SlashOutcome::Output(String::new())
            }
            Reply::Slash(cmd) => self.exec_slash(cmd),
            Reply::SlashNote { note, inner } => {
                write_raw(&format!("{}\n", paint(Role::Dim, &note)));
                self.run_reply_to_outcome(*inner)
            }
            Reply::SlashPick { title, raw_line, defs } => self.exec_slash_pick(&title, &raw_line, &defs),
        }
    }

    fn run_watch_reply(&mut self, reply: Reply) -> Option<(PaneMode, Vec<String>)> {
        match reply {
            Reply::Text(text) => {
                self.write_typewritten(&text);
                None
            }
            Reply::Instant(text) => {
                write_raw(&text);
                None
            }
            Reply::Find(words) => {
                self.run_find(&words);
                None
            }
            Reply::Slash(cmd) => match self.exec_slash(cmd) {
                SlashOutcome::Output(text) => {
                    write_raw(&text);
                    None
                }
                SlashOutcome::SwitchMode(mode, args) => Some((mode, args)),
            },
            Reply::SlashNote { note, inner } => {
                write_raw(&format!("{}\n", paint(Role::Dim, &note)));
                self.run_watch_reply(*inner)
            }
            Reply::SlashPick { title, raw_line, defs } => match self.exec_slash_pick(&title, &raw_line, &defs) {
                SlashOutcome::Output(text) => {
                    write_raw(&text);
                    None
                }
                SlashOutcome::SwitchMode(mode, args) => Some((mode, args)),
            },
        }
    }

    fn exec_slash_pick(&mut self, title: &str, raw_line: &str, defs: &[&'static SlashDef]) -> SlashOutcome {
        if defs.len() == 1 {
            let comps = slash::completions(raw_line, &self.pet_session.pet.unlocked_species);
            if !comps.is_empty() {
                if self.use_tui() {
                    let items: Vec<SelectItem> = comps
                        .iter()
                        .enumerate()
                        .map(|(i, c)| SelectItem {
                            key: if i < 9 { (i + 1).to_string() } else { String::new() },
                            label: c.display.clone(),
                            detail: c.summary.clone(),
                            enabled: true,
                        })
                        .collect();
                    return match self.run_select_menu(title, &items, 0, "↑/↓ move · Enter pick · q/Esc cancel") {
                        Ok(SelectOutcome::Picked(idx)) => self.exec_submitted_slash(&comps[idx].insert),
                        Ok(SelectOutcome::Slash(s)) => self.exec_slash_from_menu(&s),
                        Ok(SelectOutcome::Eof) => {
                            STOP.store(true, Ordering::Relaxed);
                            SlashOutcome::Output(String::new())
                        }
                        Ok(SelectOutcome::Cancelled) | Err(_) => SlashOutcome::Output(String::new()),
                    };
                }
                let mut out = format!("{}\n", paint(Role::Header, title));
                for (i, c) in comps.iter().enumerate() {
                    let _ = writeln!(out, "  [{}] {:<12} {}", i + 1, c.display, paint(Role::Dim, &c.summary));
                }
                out.push('\n');
                return SlashOutcome::Output(out);
            }
        }

        if self.use_tui() {
            let items: Vec<SelectItem> = defs
                .iter()
                .enumerate()
                .map(|(i, def)| SelectItem {
                    key: if i < 9 { (i + 1).to_string() } else { String::new() },
                    label: def.name.to_string(),
                    detail: def.summary.to_string(),
                    enabled: true,
                })
                .collect();
            return match self.run_select_menu(title, &items, 0, "↑/↓ move · Enter pick · 1-9 hotkey · q/Esc cancel")
            {
                Ok(SelectOutcome::Picked(idx)) => self.exec_submitted_slash(defs[idx].name),
                Ok(SelectOutcome::Slash(s)) if s != "/" => self.exec_submitted_slash(&s),
                Ok(SelectOutcome::Slash(_)) => SlashOutcome::Output(String::new()),
                Ok(SelectOutcome::Eof) => {
                    STOP.store(true, Ordering::Relaxed);
                    SlashOutcome::Output(String::new())
                }
                Ok(SelectOutcome::Cancelled) | Err(_) => SlashOutcome::Output(String::new()),
            };
        }

        let mut out = format!("{}\n", paint(Role::Header, title));
        for (i, def) in defs.iter().enumerate() {
            let _ = writeln!(out, "  [{}] {:<8} {}", i + 1, def.name, paint(Role::Dim, def.summary));
        }
        out.push('\n');
        SlashOutcome::Output(out)
    }

    fn exec_slash(&mut self, cmd: SlashCmd) -> SlashOutcome {
        match cmd {
            SlashCmd::ModeInteractive => {
                if self.use_tui() {
                    let items = [
                        SelectItem {
                            key: "1".into(),
                            label: "watch".into(),
                            detail: "live command explanations as you work".into(),
                            enabled: true,
                        },
                        SelectItem {
                            key: "2".into(),
                            label: "lab".into(),
                            detail: "interactive lessons & hands-on practice".into(),
                            enabled: true,
                        },
                    ];
                    let initial = usize::from(self.mode == PaneMode::Lab);
                    return match self.run_select_menu(
                        "Switch pane mode:",
                        &items,
                        initial,
                        "↑/↓ move · Enter switch · q/Esc cancel",
                    ) {
                        Ok(SelectOutcome::Picked(0)) => self.apply_mode_switch(PaneMode::Watch, Vec::new()),
                        Ok(SelectOutcome::Picked(_)) => self.apply_mode_switch(PaneMode::Lab, Vec::new()),
                        Ok(SelectOutcome::Slash(s)) => self.exec_slash_from_menu(&s),
                        Ok(SelectOutcome::Eof) => {
                            STOP.store(true, Ordering::Relaxed);
                            SlashOutcome::Output(String::new())
                        }
                        Ok(SelectOutcome::Cancelled) | Err(_) => {
                            SlashOutcome::Output(format!("{}\n\n", paint(Role::Dim, "Kept current mode.")))
                        }
                    };
                }
                let prompt = format!(
                    "{}\n  [1] watch — live command explanations\n  [2] lab   — interactive lessons & hands-on practice\n{} ",
                    paint(Role::Header, "Switch pane mode:"),
                    paint(Role::Dim, "Choose 1-2 (Enter cancels):")
                );
                write_raw(&prompt);
                let Some(choice) = self.read_blocking_line() else {
                    return SlashOutcome::Output(String::new());
                };
                let picked = match choice.trim().to_ascii_lowercase().as_str() {
                    "" | "q" | "cancel" => {
                        return SlashOutcome::Output(format!("{}\n\n", paint(Role::Dim, "Kept current mode.")));
                    }
                    "1" | "watch" => PaneMode::Watch,
                    "2" | "lab" => PaneMode::Lab,
                    other => {
                        return SlashOutcome::Output(note(&format!(
                            "unknown mode \"{other}\"; valid modes: {}",
                            PaneMode::VALID
                        )));
                    }
                };
                self.apply_mode_switch(picked, Vec::new())
            }
            SlashCmd::Mode(target, args) => self.apply_mode_switch(target, args),
            SlashCmd::Safety(arg) => {
                let next = match arg {
                    None => {
                        if self.safety.is_expert() {
                            Mode::Normal
                        } else {
                            Mode::Expert
                        }
                    }
                    Some(val) => match Mode::parse(&val) {
                        Ok(m) => m,
                        Err(err) => return SlashOutcome::Output(note(&format!("{err}"))),
                    },
                };
                self.safety = next;
                let name = if next.is_expert() { "expert" } else { "normal" };
                self.prefs.safety = Some(name.to_string());
                self.save_prefs();
                self.emit_header_refresh();
                let desc = if next.is_expert() {
                    "Safety mode: expert (fewer beginner safeguards; warnings still shown)."
                } else {
                    "Safety mode: normal."
                };
                SlashOutcome::Output(format!("{}\n\n", paint(Role::Success, desc)))
            }
            SlashCmd::Theme(None) => {
                if self.use_tui() {
                    let items: Vec<SelectItem> = style::ALL
                        .iter()
                        .enumerate()
                        .map(|(i, t)| SelectItem {
                            key: (i + 1).to_string(),
                            label: t.name().to_string(),
                            detail: if *t == self.theme {
                                format!("{} (current)", theme_summary(*t))
                            } else {
                                theme_summary(*t).to_string()
                            },
                            enabled: true,
                        })
                        .collect();
                    let initial = style::ALL.iter().position(|&t| t == self.theme).unwrap_or(0);
                    return match self.run_select_menu(
                        "Colour theme:",
                        &items,
                        initial,
                        "↑/↓ move · Enter apply · 1-9 hotkey · q/Esc cancel",
                    ) {
                        Ok(SelectOutcome::Picked(idx)) => {
                            self.exec_slash(SlashCmd::Theme(Some(style::ALL[idx].name().to_string())))
                        }
                        Ok(SelectOutcome::Slash(s)) => self.exec_slash_from_menu(&s),
                        Ok(SelectOutcome::Eof) => {
                            STOP.store(true, Ordering::Relaxed);
                            SlashOutcome::Output(String::new())
                        }
                        Ok(SelectOutcome::Cancelled) | Err(_) => {
                            SlashOutcome::Output(format!("{}\n\n", paint(Role::Dim, "Kept current theme.")))
                        }
                    };
                }
                let mut out = format!("{}\n", paint(Role::Header, "Themes:"));
                for t in style::ALL {
                    let mark = if t == self.theme { "*" } else { " " };
                    let _ = writeln!(out, "  {mark} {}", t.name());
                }
                let _ = writeln!(out, "{}\n", paint(Role::Dim, "Switch with `/theme <name>` (e.g. `/theme nord`)."));
                SlashOutcome::Output(out)
            }
            SlashCmd::Theme(Some(raw)) => {
                let next = match Theme::parse(&raw) {
                    Ok(t) => t,
                    Err(err) => return SlashOutcome::Output(note(&format!("{err}"))),
                };
                self.theme = next;
                if self.apply_global_style {
                    style::set(Style::for_terminal(next, self.color_on, TermEnv::detect()));
                }
                self.prefs.theme = Some(next.name().to_string());
                self.save_prefs();
                self.emit_header_refresh();
                SlashOutcome::Output(format!("{}\n\n", paint(Role::Success, &format!("Theme set to {}.", next.name()))))
            }
            SlashCmd::Pet(args) => self.exec_pet_slash(&args),
            SlashCmd::Speed(None) => {
                if self.use_tui() {
                    let presets = [
                        ("normal", "2000ms budget"),
                        ("fast", "800ms budget"),
                        ("slow", "3200ms budget"),
                        ("off", "instant (no typewriter)"),
                    ];
                    let items: Vec<SelectItem> = presets
                        .iter()
                        .enumerate()
                        .map(|(i, (name, desc))| SelectItem {
                            key: (i + 1).to_string(),
                            label: (*name).to_string(),
                            detail: (*desc).to_string(),
                            enabled: true,
                        })
                        .collect();
                    let initial = match self.speed {
                        Speed::Normal => 0,
                        Speed::Fast => 1,
                        Speed::Slow => 2,
                        Speed::Off => 3,
                        Speed::Custom(_) => 0,
                    };
                    return match self.run_select_menu(
                        &format!("Typewriter speed (current: {}):", self.speed.name()),
                        &items,
                        initial,
                        "↑/↓ move · Enter apply · q/Esc cancel",
                    ) {
                        Ok(SelectOutcome::Picked(idx)) => {
                            self.exec_slash(SlashCmd::Speed(Some(presets[idx].0.to_string())))
                        }
                        Ok(SelectOutcome::Slash(s)) => self.exec_slash_from_menu(&s),
                        Ok(SelectOutcome::Eof) => {
                            STOP.store(true, Ordering::Relaxed);
                            SlashOutcome::Output(String::new())
                        }
                        Ok(SelectOutcome::Cancelled) | Err(_) => {
                            SlashOutcome::Output(format!("{}\n\n", paint(Role::Dim, "Kept current typewriter speed.")))
                        }
                    };
                }
                SlashOutcome::Output(format!(
                    "{}\n{}\n\n",
                    paint(
                        Role::Header,
                        &format!("Typewriter speed: {} ({}ms)", self.speed.name(), self.speed.budget_ms())
                    ),
                    paint(Role::Dim, &format!("Options: {}", Speed::VALID))
                ))
            }
            SlashCmd::Speed(Some(raw)) => {
                let next = match Speed::parse(&raw) {
                    Ok(s) => s,
                    Err(err) => return SlashOutcome::Output(note(&format!("{err}"))),
                };
                self.speed = next;
                self.prefs.speed = Some(next.name());
                self.save_prefs();
                let msg = if next == Speed::Off {
                    "Typewriter speed: off (instant).".to_string()
                } else {
                    format!("Typewriter speed: {} ({}ms).", next.name(), next.budget_ms())
                };
                SlashOutcome::Output(format!("{}\n\n", paint(Role::Success, &msg)))
            }
            SlashCmd::Quiet(arg) => {
                let next = match arg.as_deref().map(str::to_ascii_lowercase).as_deref() {
                    None => !self.quiet,
                    Some("on" | "true" | "1") => true,
                    Some("off" | "false" | "0") => false,
                    Some(other) => {
                        return SlashOutcome::Output(note(&format!(
                            "unknown quiet setting \"{other}\"; use on or off"
                        )));
                    }
                };
                self.quiet = next;
                if next {
                    self.explained.get_or_insert_with(HashSet::new);
                } else {
                    self.explained = None;
                }
                self.prefs.quiet = Some(next);
                self.save_prefs();
                let msg =
                    if next { "Quiet mode: on (each program explained once per session)." } else { "Quiet mode: off." };
                SlashOutcome::Output(format!("{}\n\n", paint(Role::Success, msg)))
            }
            SlashCmd::Tips(arg) => match arg.as_deref().map(str::to_ascii_lowercase).as_deref() {
                None => {
                    self.tips_enabled = !self.tips_enabled;
                    self.tip_ticks = 0;
                    self.prefs.tips = Some(self.tips_enabled);
                    self.save_prefs();
                    self.emit_header_refresh();
                    let msg = if self.tips_enabled {
                        format!("Header tips: on ({}).", tips::tip_at(self.tip_index))
                    } else {
                        "Header tips: off.".to_string()
                    };
                    SlashOutcome::Output(format!("{}\n\n", paint(Role::Success, &msg)))
                }
                Some("on" | "true" | "1") => {
                    self.tips_enabled = true;
                    self.tip_ticks = 0;
                    self.prefs.tips = Some(true);
                    self.save_prefs();
                    self.emit_header_refresh();
                    SlashOutcome::Output(format!(
                        "{}\n\n",
                        paint(Role::Success, &format!("Header tips: on ({}).", tips::tip_at(self.tip_index)))
                    ))
                }
                Some("off" | "false" | "0") => {
                    self.tips_enabled = false;
                    self.tip_ticks = 0;
                    self.prefs.tips = Some(false);
                    self.save_prefs();
                    self.emit_header_refresh();
                    SlashOutcome::Output(format!("{}\n\n", paint(Role::Success, "Header tips: off.")))
                }
                Some("next") => {
                    if !self.tips_enabled {
                        self.tips_enabled = true;
                        self.prefs.tips = Some(true);
                        self.save_prefs();
                    }
                    self.tip_ticks = 0;
                    self.tip_index = (self.tip_index + 1) % tips::TIPS.len();
                    self.emit_header_refresh();
                    SlashOutcome::Output(format!(
                        "{}\n\n",
                        paint(Role::Success, &format!("Tip: {}", tips::tip_at(self.tip_index)))
                    ))
                }
                Some(other) => {
                    SlashOutcome::Output(note(&format!("unknown tips setting \"{other}\"; use on, off, or next")))
                }
            },
        }
    }

    fn apply_mode_switch(&mut self, target: PaneMode, args: Vec<String>) -> SlashOutcome {
        if target == PaneMode::Lab
            && let Err(err) = learn::parse_lab_args(&args)
        {
            return SlashOutcome::Output(note(&format!("{err}")));
        }
        let changed = self.mode != target;
        self.mode = target;
        self.prefs.last_mode = Some(target.name().to_string());
        self.save_prefs();
        self.refresh_lab_xp();
        self.emit_header_refresh();
        if !changed && args.is_empty() {
            let name = if target == PaneMode::Watch { "Watch" } else { "Lab" };
            return SlashOutcome::Output(format!("{}\n\n", paint(Role::Dim, &format!("Already in {name} mode."))));
        }
        SlashOutcome::SwitchMode(target, args)
    }

    fn exec_pet_slash(&mut self, args: &[String]) -> SlashOutcome {
        let now = crate::date::now_epoch();
        if args.is_empty() || args[0].eq_ignore_ascii_case("status") && args.len() == 1 {
            let card = self.pet_session.render_card(now);
            let mut out = card;
            let _ = writeln!(out, "{}", paint(Role::Header, "Species & Unlocks:"));
            for sp in Species::ALL {
                let marker = if sp == self.pet_session.pet.species { "*" } else { " " };
                let status = if sp == self.pet_session.pet.species {
                    paint(Role::Success, "active")
                } else if self.pet_session.pet.is_unlocked(sp) {
                    paint(Role::Description, "unlocked")
                } else if let Some(summary) = sp.unlock_summary(&self.pet_session.pet) {
                    paint(Role::Dim, &format!("locked — {summary}"))
                } else {
                    paint(Role::Description, "unlocked")
                };
                let _ = writeln!(out, "  {marker} {:<8} {:<14} {status}", sp.name(), sp.label());
            }
            let eff = match self.effective_pet_mode() {
                EffectiveMode::Pixel => "pixel (32x32)",
                EffectiveMode::Ascii => "ascii",
                EffectiveMode::Off => "off",
            };
            let _ = writeln!(out, "  Render style: {} (effective: {eff})", paint(Role::Value, self.pet_style.name()));
            let _ = writeln!(
                out,
                "{}\n",
                paint(
                    Role::Dim,
                    "  Commands: /pet name <NAME> · /pet species <SPECIES> · /pet style <auto|pixel|ascii|off> · /pet customize"
                )
            );
            return SlashOutcome::Output(out);
        }

        let sub = args[0].to_ascii_lowercase();
        match sub.as_str() {
            "name" => {
                let new_name = args[1..].join(" ");
                if new_name.trim().is_empty() {
                    return SlashOutcome::Output(note("usage: /pet name <NAME>"));
                }
                self.pet_session.merge_disk_metadata();
                if let Err(err) = self.pet_session.pet.rename(&new_name) {
                    return SlashOutcome::Output(note(&format!("{err}")));
                }
                self.pet_session.save_direct();
                self.emit_header_refresh();
                SlashOutcome::Output(format!(
                    "{}\n\n",
                    paint(Role::Success, &format!("Renamed companion to {}.", self.pet_session.pet.name))
                ))
            }
            "species" => {
                if args.len() == 1 {
                    if self.use_tui() {
                        self.pet_session.merge_disk_metadata();
                        let items: Vec<SelectItem> = Species::ALL
                            .iter()
                            .enumerate()
                            .map(|(i, &sp)| {
                                let unlocked = self.pet_session.pet.is_unlocked(sp);
                                let active = sp == self.pet_session.pet.species;
                                let detail = if active {
                                    format!("{} (active)", sp.label())
                                } else if unlocked {
                                    format!("{} — unlocked", sp.label())
                                } else {
                                    format!(
                                        "{} — locked ({})",
                                        sp.label(),
                                        sp.unlock_summary(&self.pet_session.pet).unwrap_or_default()
                                    )
                                };
                                SelectItem {
                                    key: (i + 1).to_string(),
                                    label: sp.name().to_string(),
                                    detail,
                                    enabled: unlocked,
                                }
                            })
                            .collect();
                        let initial = Species::ALL.iter().position(|&s| s == self.pet_session.pet.species).unwrap_or(0);
                        return match self.run_select_menu(
                            "Companion species:",
                            &items,
                            initial,
                            "↑/↓ move · Enter switch · q/Esc cancel",
                        ) {
                            Ok(SelectOutcome::Picked(idx)) => {
                                self.exec_pet_slash(&["species".into(), Species::ALL[idx].name().into()])
                            }
                            Ok(SelectOutcome::Slash(s)) => self.exec_slash_from_menu(&s),
                            Ok(SelectOutcome::Eof) => {
                                STOP.store(true, Ordering::Relaxed);
                                SlashOutcome::Output(String::new())
                            }
                            Ok(SelectOutcome::Cancelled) | Err(_) => {
                                SlashOutcome::Output(format!("{}\n\n", paint(Role::Dim, "Kept current species.")))
                            }
                        };
                    }
                    return SlashOutcome::Output(note(&format!("usage: /pet species <{}>", Species::VALID)));
                }
                if args.len() != 2 {
                    return SlashOutcome::Output(note(&format!("usage: /pet species <{}>", Species::VALID)));
                }
                let sp = match Species::parse(&args[1]) {
                    Ok(s) => s,
                    Err(err) => return SlashOutcome::Output(note(&format!("{err}"))),
                };
                self.pet_session.merge_disk_metadata();
                if !self.pet_session.pet.is_unlocked(sp) {
                    let how = sp.unlock_summary(&self.pet_session.pet).unwrap_or_default();
                    return SlashOutcome::Output(note(&format!("{} ({}) is locked — {how}", sp.name(), sp.label())));
                }
                if let Err(err) = self.pet_session.pet.set_species(sp) {
                    return SlashOutcome::Output(note(&format!("{err}")));
                }
                self.pet_session.save_direct();
                self.emit_header_refresh();
                SlashOutcome::Output(format!(
                    "{}\n\n",
                    paint(Role::Success, &format!("Switched companion to {} ({}).", sp.name(), sp.label()))
                ))
            }
            "style" => {
                if args.len() == 1 {
                    if self.use_tui() {
                        let styles = [
                            ("auto", "32x32 Kitty with ASCII fallback"),
                            ("pixel", "force 32x32 Kitty pixel sprite"),
                            ("ascii", "force 4x10 pure-ASCII art"),
                            ("off", "hide Pebble in header"),
                        ];
                        let items: Vec<SelectItem> = styles
                            .iter()
                            .enumerate()
                            .map(|(i, (name, desc))| SelectItem {
                                key: (i + 1).to_string(),
                                label: (*name).to_string(),
                                detail: (*desc).to_string(),
                                enabled: true,
                            })
                            .collect();
                        let initial = match self.pet_style {
                            RenderStyle::Auto => 0,
                            RenderStyle::Pixel => 1,
                            RenderStyle::Ascii => 2,
                            RenderStyle::Off => 3,
                        };
                        return match self.run_select_menu(
                            &format!("Pet render style (current: {}):", self.pet_style.name()),
                            &items,
                            initial,
                            "↑/↓ move · Enter apply · q/Esc cancel",
                        ) {
                            Ok(SelectOutcome::Picked(idx)) => {
                                self.exec_pet_slash(&["style".into(), styles[idx].0.into()])
                            }
                            Ok(SelectOutcome::Slash(s)) => self.exec_slash_from_menu(&s),
                            Ok(SelectOutcome::Eof) => {
                                STOP.store(true, Ordering::Relaxed);
                                SlashOutcome::Output(String::new())
                            }
                            Ok(SelectOutcome::Cancelled) | Err(_) => {
                                SlashOutcome::Output(format!("{}\n\n", paint(Role::Dim, "Kept current pet style.")))
                            }
                        };
                    }
                    return SlashOutcome::Output(note(&format!("usage: /pet style <{}>", RenderStyle::VALID)));
                }
                if args.len() != 2 {
                    return SlashOutcome::Output(note(&format!("usage: /pet style <{}>", RenderStyle::VALID)));
                }
                let style = match RenderStyle::parse(&args[1]) {
                    Ok(s) => s,
                    Err(err) => return SlashOutcome::Output(note(&format!("{err}"))),
                };
                self.pet_style = style;
                self.prefs.pet_style = Some(style.name().to_string());
                self.save_prefs();
                self.emit_header_refresh();
                let eff = match self.effective_pet_mode() {
                    EffectiveMode::Pixel => "pixel",
                    EffectiveMode::Ascii => "ascii",
                    EffectiveMode::Off => "off",
                };
                SlashOutcome::Output(format!(
                    "{}\n\n",
                    paint(Role::Success, &format!("Pet render style: {} ({eff}).", style.name()))
                ))
            }
            "customize" => {
                if args.len() != 1 {
                    return SlashOutcome::Output(note("usage: /pet customize"));
                }
                let Some(dir) = self.sprite_override_dir.clone().or_else(sprite::override_dir) else {
                    return SlashOutcome::Output(note(
                        "cannot determine sprite config directory ($XDG_CONFIG_HOME or $HOME)",
                    ));
                };
                self.pet_session.merge_disk_metadata();
                match sprite::export_template(self.pet_session.pet.species, &dir) {
                    Ok(path) => {
                        if self.pinned {
                            pin::reload_sprite_sheet();
                            self.emit_header_refresh();
                        }
                        SlashOutcome::Output(format!(
                            "{}\n{}\n\n",
                            paint(Role::Success, &format!("Sprite template ready: {}", path.display())),
                            paint(
                                Role::Dim,
                                "Edit the 32x32 grid or [palette] tokens; edits take effect on `/pet customize`, `/pet species <name>`, `/theme`, `/pet style pixel`, or restart."
                            )
                        ))
                    }
                    Err(err) => SlashOutcome::Output(note(&format!("cannot export sprite template: {err}"))),
                }
            }
            _ => SlashOutcome::Output(note(
                "usage: /pet [status | name <NAME> | species <penguin|cat|turtle|owl> | style <auto|pixel|ascii|off> | customize]",
            )),
        }
    }

    fn run_find(&mut self, words: &[String]) {
        if self.pinned {
            write_raw(&pin::suspend());
        }
        if let Err(error) = crate::find::run(words, &self.everything) {
            println!("huh find: {error:#}");
        }
        if self.pinned {
            write_raw(&pin::resume());
        }
    }

    fn run_watch_line(&mut self, line: &str) -> Option<(PaneMode, Vec<String>)> {
        let ctx = Input::new(&self.everything, &self.usable, self.safety, self.files.clone(), None);
        let reply = answer_line(line, &ctx, self.last_finished.as_ref());
        self.run_watch_reply(reply)
    }
}

fn already_in_lab_message(args: &[String]) -> &'static str {
    if args.is_empty() {
        "Already in Lab mode."
    } else {
        "Already in Lab mode — type 'q' to return to the Lab menu to switch units."
    }
}

/// Bridges `learn::LabHost` to `PaneState` inside the unified companion pane.
struct PaneHost<'a> {
    state: &'a mut PaneState,
    last_auto_report: Option<(PathBuf, Report)>,
}

impl PaneHost<'_> {
    fn run_lab_outcome(&mut self, outcome: SlashOutcome) -> SlashEffect {
        match outcome {
            SlashOutcome::Output(text) => {
                write_raw(&text);
                SlashEffect::Handled
            }
            SlashOutcome::SwitchMode(PaneMode::Watch, _) => SlashEffect::SwitchToWatch,
            SlashOutcome::SwitchMode(PaneMode::Lab, args) => {
                write_raw(&format!("{}\n\n", paint(Role::Dim, already_in_lab_message(&args))));
                SlashEffect::Handled
            }
        }
    }

    fn run_lab_reply(&mut self, reply: Reply) -> SlashEffect {
        match reply {
            Reply::Instant(text) | Reply::Text(text) => {
                write_raw(&text);
                SlashEffect::Handled
            }
            Reply::Find(words) => {
                self.state.run_find(&words);
                SlashEffect::Handled
            }
            Reply::Slash(cmd) => {
                let outcome = self.state.exec_slash(cmd);
                self.run_lab_outcome(outcome)
            }
            Reply::SlashNote { note, inner } => {
                write_raw(&format!("{}\n", paint(Role::Dim, &note)));
                self.run_lab_reply(*inner)
            }
            Reply::SlashPick { title, raw_line, defs } => {
                let outcome = self.state.exec_slash_pick(&title, &raw_line, &defs);
                self.run_lab_outcome(outcome)
            }
        }
    }
}

impl LabHost for PaneHost<'_> {
    fn write_narrative(&mut self, text: &str) -> std::io::Result<()> {
        self.state.write_typewritten(text);
        Ok(())
    }

    fn write_instant(&mut self, text: &str) -> std::io::Result<()> {
        write_raw(text);
        Ok(())
    }

    fn on_pet_event(&mut self, mood: Option<Mood>, celebrate: bool) {
        self.state.pet_session.reload_from_disk(celebrate);
        if !celebrate && mood.is_some() {
            self.state.pet_session.transient = mood;
        }
        self.state.refresh_lab_xp();
        self.state.emit_header_refresh();
    }

    fn handle_slash(&mut self, line: &str) -> std::io::Result<SlashEffect> {
        let Some(reply) = answer_slash(line) else {
            return Ok(SlashEffect::NotSlash);
        };
        Ok(self.run_lab_reply(reply))
    }

    fn choose_option(
        &mut self,
        title: &str,
        items: &[ChoiceOption],
        default_idx: usize,
        hint: &str,
        _reader: &mut dyn std::io::BufRead,
    ) -> std::io::Result<ChoiceResult> {
        if STOP.load(Ordering::Relaxed) {
            return Ok(ChoiceResult::Eof);
        }
        if !self.state.use_tui() {
            return Ok(ChoiceResult::Fallback);
        }
        let select_items: Vec<SelectItem> = items
            .iter()
            .map(|opt| SelectItem {
                key: opt.key.clone(),
                label: opt.label.clone(),
                detail: opt.detail.clone(),
                enabled: !opt.locked,
            })
            .collect();
        match self.state.run_select_menu(title, &select_items, default_idx, hint)? {
            SelectOutcome::Picked(idx) => Ok(ChoiceResult::Picked(idx)),
            SelectOutcome::Cancelled => Ok(ChoiceResult::Cancelled),
            SelectOutcome::Slash(s) => Ok(ChoiceResult::Slash(s)),
            SelectOutcome::Eof => {
                STOP.store(true, Ordering::Relaxed);
                Ok(ChoiceResult::Eof)
            }
        }
    }

    fn read_slash_from_menu(
        &mut self,
        prefill: &str,
        _reader: &mut dyn std::io::BufRead,
    ) -> std::io::Result<SlashEffect> {
        if STOP.load(Ordering::Relaxed) {
            return Ok(SlashEffect::NotSlash);
        }
        if !self.state.use_tui() {
            return self.handle_slash(prefill);
        }
        self.state.editor.push_prefill(prefill);
        match self.state.read_editor_line(PROMPT)? {
            EditorEvent::Submit(line) => {
                if line.trim().is_empty() {
                    Ok(SlashEffect::NotSlash)
                } else {
                    self.handle_slash(&line)
                }
            }
            EditorEvent::Interrupted => {
                STOP.store(true, Ordering::Relaxed);
                Ok(SlashEffect::NotSlash)
            }
            EditorEvent::Eof => Ok(SlashEffect::NotSlash),
        }
    }

    fn read_prompt(&mut self, prompt: &str, reader: &mut dyn std::io::BufRead) -> std::io::Result<Option<String>> {
        if STOP.load(Ordering::Relaxed) {
            return Ok(None);
        }
        if self.state.use_tui() {
            let p = if prompt.is_empty() { String::new() } else { format!("{prompt} ") };
            return match self.state.read_editor_line(&p)? {
                EditorEvent::Submit(line) => Ok(Some(line.trim().to_string())),
                EditorEvent::Interrupted => {
                    STOP.store(true, Ordering::Relaxed);
                    Ok(None)
                }
                EditorEvent::Eof => Ok(None),
            };
        }
        if !prompt.is_empty() {
            self.write_instant(&format!("{} ", paint(Role::Dim, prompt)))?;
        }
        self.read_line(reader)
    }

    fn handles_practice_prompt(&self) -> bool {
        self.state.use_tui()
    }

    fn read_line(&mut self, reader: &mut dyn std::io::BufRead) -> std::io::Result<Option<String>> {
        if self.state.use_tui() {
            return self.read_prompt("", reader);
        }
        Ok(self.state.read_blocking_line().map(|s| s.trim().to_string()))
    }

    fn wait_practice_turn(
        &mut self,
        sandbox_dir: &Path,
        practice: &Practice,
        reported_cmds: &mut u64,
    ) -> Option<PracticePoll> {
        if STOP.load(Ordering::Relaxed) {
            return Some(PracticePoll::Quit);
        }
        if self.state.use_tui() {
            let has_fallback = match &self.last_auto_report {
                Some((dir, rep)) if dir == sandbox_dir => rep.outcomes.iter().any(|o| !o.passed && o.needs_fallback),
                _ => practice.checks.iter().any(|c| matches!(c, crate::practice::Check::RecordedOutput { .. })),
            };
            let prompt = if has_fallback {
                "Press Enter to check, type answer, 'hint', 'reset', or 'q': "
            } else {
                "Press Enter to check ('hint', 'reset', 'q'): "
            };
            let cancel = AtomicBool::new(STOP.load(Ordering::Relaxed));
            let mut auto_update: Option<PracticePoll> = None;
            let mut relocate_ticks = 0u64;
            let mut editor = std::mem::take(&mut self.state.editor);
            let unlocked = Rc::new(RefCell::new(self.state.pet_session.pet.unlocked_species.clone()));
            let unlocked_for_comp = Rc::clone(&unlocked);
            let complete = move |line: &str| slash_completions(line, &unlocked_for_comp.borrow());
            let mut on_tick = || {
                if STOP.load(Ordering::Relaxed) {
                    cancel.store(true, Ordering::Relaxed);
                    return None;
                }
                if self.state.pinned
                    && let Some(redraw) = pin::on_resize()
                {
                    write_raw(&redraw);
                }
                if let Some(done) = self.state.follower.as_mut().and_then(Follower::poll) {
                    *reported_cmds += 1;
                    if *reported_cmds == 1 {
                        self.last_auto_report = None;
                    }
                    let explanation = (!done.command.trim().is_empty())
                        .then(|| crate::explain::explain_command(&self.state.everything, &done.command));
                    let has_warning = explanation.as_ref().is_some_and(|e| !e.warnings.is_empty());
                    let now = crate::date::now_epoch();
                    let (banner, unlocked_sp) = self.state.pet_session.observe_with_unlocks(&done, has_warning, now);
                    if banner.is_some() {
                        self.state.pet_session.save();
                        self.state.emit_header_refresh();
                    }
                    *unlocked.borrow_mut() = self.state.pet_session.pet.unlocked_species.clone();

                    let mut nudged = false;
                    let mut nudge_text = String::new();
                    if !unlocked_sp.is_empty() {
                        nudge_text.push_str(&format_watch_unlocks(&unlocked_sp));
                        nudged = true;
                    }
                    if done.exit.is_some_and(|c| c != 0)
                        && let Some(why) = brief_why_nudge(&done)
                    {
                        nudge_text.push_str(&why);
                        nudged = true;
                    }
                    let rec = RecordedCommand {
                        command: done.command.clone(),
                        exit: done.exit,
                        output: session::tail(&session::clean(&done.raw)),
                    };
                    if let Ok(report) = practice::evaluate(sandbox_dir, practice, Some(&rec), &[]) {
                        let key = (sandbox_dir.to_path_buf(), report.clone());
                        let changed = report.all_passed() || self.last_auto_report.as_ref() != Some(&key);
                        if changed || nudged {
                            self.last_auto_report = Some(key);
                            auto_update = Some(PracticePoll::AutoUpdate { report, recorded: Some(rec), nudged });
                            cancel.store(true, Ordering::Relaxed);
                            return (!nudge_text.is_empty()).then_some(nudge_text);
                        }
                    } else if nudged {
                        return Some(nudge_text);
                    }
                }
                self.state.tick_animation();
                relocate_ticks += 1;
                if self.state.follower.is_none() && relocate_ticks.is_multiple_of(RELOCATE_EVERY_TICKS) {
                    let _ = self.state.try_attach_follower();
                }
                None
            };
            let res = editor.read_line_with_cancel(prompt, &complete, &mut on_tick, &cancel);
            self.state.editor = editor;
            if let Some(poll) = auto_update.take() {
                return Some(poll);
            }
            if STOP.load(Ordering::Relaxed) {
                return Some(PracticePoll::Quit);
            }
            return match res {
                Ok(EditorEvent::Submit(line)) => Some(PracticePoll::UserLine(Some(line.trim().to_string()))),
                Ok(EditorEvent::Eof) => Some(PracticePoll::UserLine(None)),
                Ok(EditorEvent::Interrupted) => {
                    STOP.store(true, Ordering::Relaxed);
                    Some(PracticePoll::Quit)
                }
                Err(_) => Some(PracticePoll::UserLine(None)),
            };
        }

        let mut relocate_ticks = 0u64;
        loop {
            if STOP.load(Ordering::Relaxed) {
                return Some(PracticePoll::Quit);
            }
            if self.state.pinned
                && let Some(redraw) = pin::on_resize()
            {
                write_raw(&redraw);
            }

            if let Some(done) = self.state.follower.as_mut().and_then(Follower::poll) {
                *reported_cmds += 1;
                if *reported_cmds == 1 {
                    self.last_auto_report = None;
                }
                let explanation = (!done.command.trim().is_empty())
                    .then(|| crate::explain::explain_command(&self.state.everything, &done.command));
                let has_warning = explanation.as_ref().is_some_and(|e| !e.warnings.is_empty());
                let now = crate::date::now_epoch();
                let (banner, unlocked) = self.state.pet_session.observe_with_unlocks(&done, has_warning, now);
                if banner.is_some() {
                    self.state.pet_session.save();
                    self.state.emit_header_refresh();
                }

                let mut nudged = false;
                let mut nudge_text = String::new();
                if !unlocked.is_empty() {
                    nudge_text.push_str(&format_watch_unlocks(&unlocked));
                    nudged = true;
                }
                if done.exit.is_some_and(|c| c != 0)
                    && let Some(why) = brief_why_nudge(&done)
                {
                    nudge_text.push_str(&why);
                    nudged = true;
                }

                let rec = RecordedCommand {
                    command: done.command.clone(),
                    exit: done.exit,
                    output: session::tail(&session::clean(&done.raw)),
                };
                if let Ok(report) = practice::evaluate(sandbox_dir, practice, Some(&rec), &[]) {
                    let key = (sandbox_dir.to_path_buf(), report.clone());
                    let changed = report.all_passed() || self.last_auto_report.as_ref() != Some(&key);
                    if changed || nudged {
                        write_raw("\r\x1b[2K");
                        if !nudge_text.is_empty() {
                            write_raw(&nudge_text);
                        }
                        self.last_auto_report = Some(key);
                        return Some(PracticePoll::AutoUpdate { report, recorded: Some(rec), nudged });
                    }
                } else if nudged {
                    write_raw(&format!("\r\x1b[2K{nudge_text}"));
                }
            }

            match self.state.stdin.wait_tick(TICK_MS) {
                PollResult::Line => {
                    return Some(PracticePoll::UserLine(self.state.stdin.pop_line().map(|s| s.trim().to_string())));
                }
                PollResult::Eof => return Some(PracticePoll::UserLine(None)),
                PollResult::Timeout => {
                    self.state.tick_animation();
                    relocate_ticks += 1;
                    if self.state.follower.is_none() && relocate_ticks.is_multiple_of(RELOCATE_EVERY_TICKS) {
                        let _ = self.state.try_attach_follower();
                    }
                }
            }
        }
    }
}

fn write_raw(text: &str) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}

/// Runs the unified `huh` companion pane (`watch` + `lab`) until Ctrl+C or `Ctrl+D`.
pub fn run(catalog: &[Command], opts: RunOpts) -> Result<()> {
    STOP.store(false, Ordering::Relaxed);
    if let Some(dir) = opts.dir {
        set_recording_dir(dir);
    }
    if let Some(log) = opts.log {
        set_recording(log);
    }

    let mut log = locate(opts.tty.as_deref());
    if log.is_none() && opts.tty.is_some() && (RECORDING.get().is_some() || recording_dir().is_some_and(|d| d.is_dir()))
    {
        for _ in 0..4 {
            std::thread::sleep(Duration::from_millis(150));
            log = locate(opts.tty.as_deref());
            if log.is_some() {
                break;
            }
        }
    }

    let pinning = pin::wanted();
    let shown = crate::welcome::show_with(crate::welcome::Trigger::Pane, false, false, !pinning);
    if shown == crate::welcome::Shown::Interrupted {
        return Ok(());
    }
    catch_ctrl_c();

    let tty_label = match &log {
        Some(path) => tty_of(path).unwrap_or_else(|| path.display().to_string()),
        None => "no hook".to_string(),
    };
    let prefs = prefs::load_default();
    let prefs_path = prefs::default_state_path();
    let config_text = crate::mode::config_path().and_then(|p| std::fs::read_to_string(p).ok());
    let env_no_tw = std::env::var("HUH_NO_TYPEWRITER").is_ok_and(|v| !v.is_empty() && v != "0");
    let env_ms = std::env::var("HUH_TYPEWRITER_MS").ok();
    let speed = typewriter::resolve_speed(env_no_tw, env_ms.as_deref(), config_text.as_deref(), prefs.speed.as_deref());
    let env_no_tips = std::env::var("HUH_NO_TIPS").is_ok_and(|v| !v.is_empty() && v != "0");
    let tips_enabled = tips::resolve_tips_enabled(env_no_tips, config_text.as_deref(), prefs.tips)?;
    let pet_style = prefs.pet_style.as_deref().and_then(|s| RenderStyle::parse(s).ok()).unwrap_or_default();

    let platforms = crate::catalog::this_platform();
    let usable = crate::catalog::usable(catalog.to_vec(), &platforms, false);
    let typing = std::io::stdin().is_terminal();

    let mut state = PaneState {
        everything: catalog.to_vec(),
        usable,
        files: crate::candidates::current_folder_entries(),
        mode: opts.initial_mode,
        safety: opts.safety,
        theme: opts.theme,
        color_on: opts.color_on,
        apply_global_style: true,
        speed,
        pet_style,
        pet_enabled: crate::pet::enabled(),
        stdout_tty: std::io::stdout().is_terminal(),
        quiet: opts.quiet,
        tips_enabled,
        tip_index: 0,
        tip_ticks: 0,
        last_tip: None,
        explained: opts.quiet.then(HashSet::new),
        tty_arg: opts.tty,
        tty_label,
        tick: 0,
        last_ascii_frame: None,
        last_pixel_key: None,
        last_banner: None,
        last_pet_mode: EffectiveMode::Off,
        prefs,
        prefs_path,
        sprite_override_dir: sprite::override_dir(),
        pet_session: PetSession::load(crate::pet::enabled()),
        lab_xp: None,
        pinned: false,
        last_finished: None,
        follower: log.as_deref().map(Follower::new),
        stdin: StdinReader::new(typing),
        editor: LineEditor::new(),
    };
    state.refresh_lab_xp();

    let initial_info = state.header_info();
    let pinned_guard = if pinning { pin::Guard::start(initial_info.clone()) } else { None };
    state.pinned = pinned_guard.is_some();
    state.record_rendered_snapshot(&initial_info);

    if state.pinned && shown == crate::welcome::Shown::Played {
        crate::welcome::print_welcome(crate::welcome::Trigger::Pane);
    }

    if state.follower.is_none() {
        write_raw(&no_hook_notice());
    } else if !state.pinned {
        let suffix = if state.tty_arg.is_some() { "" } else { ", the terminal that finished a command most recently" };
        println!(
            "{}",
            paint(Role::Dim, &format!("huh: following {}{} (Ctrl+C stops). It only reads.", state.tty_label, suffix))
        );
    }

    if state.follower.is_some()
        && state.tty_arg.is_none()
        && let Some(current_log) = &log
    {
        let own = std::env::var_os("HUH_SESSION_LOG").map(PathBuf::from);
        let others: Vec<String> = recording_dir()
            .map(|dir| recordings(&dir, own.as_deref(), true))
            .unwrap_or_default()
            .iter()
            .filter(|p| *p != current_log)
            .filter_map(|p| tty_of(p))
            .collect();
        if !others.is_empty() {
            println!(
                "Other recorded terminals: {}. To follow one, run `tty` in it, then `huh --tty <number>`.",
                others.join(", ")
            );
        }
    }

    let mut pending_lab_args: Vec<String> = Vec::new();
    let mut watch_intro_printed = false;
    let mut prompt_shown = false;
    let mut relocate_ticks = 0u64;

    while !STOP.load(Ordering::Relaxed) {
        if state.mode == PaneMode::Lab {
            let args = std::mem::take(&mut pending_lab_args);
            let mut host = PaneHost { state: &mut state, last_auto_report: None };
            match learn::run_in_pane(&args, catalog, recording_dir(), typing, &mut host)? {
                PaneExit::SwitchToWatch => {
                    if state.mode != PaneMode::Watch {
                        let _ = state.apply_mode_switch(PaneMode::Watch, Vec::new());
                    }
                    prompt_shown = false;
                    continue;
                }
                PaneExit::Quit => break,
            }
        }

        if !watch_intro_printed {
            if typing {
                write_raw(&format!(
                    "{}\n\n",
                    paint(
                        Role::Dim,
                        "Type a command, ?word, why, / for commands (Tab completes), /mode lab, or /help; nothing is ever run."
                    )
                ));
            } else {
                write_raw("\n");
            }
            watch_intro_printed = true;
        }

        if state.use_tui() {
            if state.pinned
                && let Some(redraw) = pin::on_resize()
            {
                write_raw(&redraw);
            }
            if let Some(done) = state.follower.as_mut().and_then(Follower::poll) {
                let (text, has_warning) = render_block(&state.everything, &done, state.explained.as_mut());
                let now = crate::date::now_epoch();
                let (banner, unlocked) = state.pet_session.observe_with_unlocks(&done, has_warning, now);
                if banner.is_some() {
                    state.pet_session.save();
                    state.emit_header_refresh();
                }
                state.last_finished = Some(done);
                state.write_typewritten(&text);
                if !unlocked.is_empty() {
                    write_raw(&format_watch_unlocks(&unlocked));
                }
            }
            match state.read_watch_line(&mut relocate_ticks) {
                Ok(EditorEvent::Submit(line)) => {
                    if let Some((PaneMode::Lab, args)) = state.run_watch_line(&line) {
                        pending_lab_args = args;
                        continue;
                    }
                }
                Ok(EditorEvent::Interrupted) => {
                    STOP.store(true, Ordering::Relaxed);
                    break;
                }
                Ok(EditorEvent::Eof) | Err(_) => break,
            }
            continue;
        }

        if typing && !prompt_shown {
            write_raw(&paint(Role::Dim, PROMPT));
            prompt_shown = true;
        }

        if state.pinned
            && let Some(redraw) = pin::on_resize()
        {
            write_raw(&redraw);
        }

        if let Some(done) = state.follower.as_mut().and_then(Follower::poll) {
            let (text, has_warning) = render_block(&state.everything, &done, state.explained.as_mut());
            let now = crate::date::now_epoch();
            let (banner, unlocked) = state.pet_session.observe_with_unlocks(&done, has_warning, now);
            if banner.is_some() {
                state.pet_session.save();
                state.emit_header_refresh();
            }
            state.last_finished = Some(done);
            if typing && prompt_shown {
                write_raw("\r\x1b[2K");
                prompt_shown = false;
            }
            state.write_typewritten(&text);
            if !unlocked.is_empty() {
                write_raw(&format_watch_unlocks(&unlocked));
            }
            if typing {
                write_raw(&paint(Role::Dim, PROMPT));
                prompt_shown = true;
            }
        }

        match state.stdin.wait_tick(TICK_MS) {
            PollResult::Line => {
                let Some(line) = state.stdin.pop_line() else {
                    break;
                };
                prompt_shown = false;
                if let Some((PaneMode::Lab, args)) = state.run_watch_line(&line) {
                    pending_lab_args = args;
                    continue;
                }
            }
            PollResult::Eof => break,
            PollResult::Timeout => {
                state.tick_animation();
                relocate_ticks += 1;
                if state.follower.is_none()
                    && relocate_ticks.is_multiple_of(RELOCATE_EVERY_TICKS)
                    && let Some(msg) = state.try_attach_follower()
                {
                    if typing && prompt_shown {
                        write_raw("\r\x1b[2K");
                        prompt_shown = false;
                    }
                    write_raw(&msg);
                }
            }
        }
    }

    drop(pinned_guard);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: &str = "\x1b]7770;huh;start\x07";
    const END: &str = "\x1b]7770;huh;end\x07";

    fn catalog() -> Vec<Command> {
        crate::catalog::load_embedded().unwrap()
    }

    fn recording(name: &str) -> (PathBuf, PathBuf) {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/pane-tests")
            .join(format!("{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        (dir.clone(), dir.join("4242.log"))
    }

    fn finish(log: &Path, output: &str, exit: i32, command: &str) {
        let mut text = std::fs::read_to_string(log).unwrap_or_default();
        text.push_str(&format!("$ {command}\r\n{START}{output}{END}$ "));
        std::fs::write(log, text).unwrap();
        std::fs::write(log.with_extension("status"), format!("{exit}\n{command}\n")).unwrap();
    }

    fn test_state(dir: &Path, stdout_tty: bool) -> PaneState {
        let everything = catalog();
        let usable = crate::catalog::usable(everything.clone(), &["arch".to_string()], false);
        let prefs_path = dir.join("state/prefs.toml");
        let pet_path = dir.join("state/pet.toml");
        let sprite_dir = dir.join("config/sprites");
        PaneState {
            everything,
            usable,
            files: vec![],
            mode: PaneMode::Watch,
            safety: Mode::Normal,
            theme: Theme::BabyBlue,
            color_on: false,
            apply_global_style: false,
            speed: Speed::Normal,
            pet_style: RenderStyle::Auto,
            pet_enabled: true,
            stdout_tty,
            quiet: false,
            tips_enabled: true,
            tip_index: 0,
            tip_ticks: 0,
            last_tip: None,
            explained: None,
            tty_arg: None,
            tty_label: "no hook".into(),
            tick: 0,
            last_ascii_frame: None,
            last_pixel_key: None,
            last_banner: None,
            last_pet_mode: EffectiveMode::Off,
            prefs: Prefs::default(),
            prefs_path: Some(prefs_path),
            sprite_override_dir: Some(sprite_dir),
            pet_session: PetSession { pet: Pet::default(), transient: None, enabled: true, path: Some(pet_path) },
            lab_xp: None,
            pinned: false,
            last_finished: None,
            follower: None,
            stdin: StdinReader::new(false),
            editor: LineEditor::new(),
        }
    }

    #[test]
    fn follower_reports_each_new_command_once() {
        let (dir, log) = recording("follow");
        std::fs::write(&log, "$ ").unwrap();
        std::fs::write(log.with_extension("status"), "0\nold command\n").unwrap();
        let mut follower = Follower::new(&log);
        assert_eq!(follower.poll(), None, "command recorded before start is not replayed");
        assert!(!follower.has_new_activity());
        finish(&log, "a\r\nb\r\n", 0, "echo hi");
        assert!(follower.has_new_activity());
        let done = follower.poll().expect("finished command");
        assert_eq!((done.command.as_str(), done.exit, done.raw.as_str()), ("echo hi", Some(0), "a\r\nb\r\n"));
        assert_eq!(follower.poll(), None, "reported once");
        assert!(!follower.has_new_activity());
        finish(&log, "oops\r\n", 2, "false");
        assert_eq!(follower.poll().map(|d| (d.command, d.exit)), Some(("false".to_string(), Some(2))));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn follower_waits_for_the_end_marker() {
        let (dir, log) = recording("wait");
        std::fs::write(&log, "$ ").unwrap();
        let mut follower = Follower::new(&log);
        std::fs::write(log.with_extension("status"), "0\nsleep 1\n").unwrap();
        std::fs::write(&log, format!("$ sleep 1\r\n{START}")).unwrap();
        assert_eq!(follower.poll(), None, "the log has no end marker yet");
        std::fs::write(&log, format!("$ sleep 1\r\n{START}done\r\n{END}$ ")).unwrap();
        assert_eq!(follower.poll().map(|d| d.raw), Some("done\r\n".to_string()));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn explains_the_command_and_recognised_output() {
        let done = Finished {
            command: "df -h".into(),
            exit: Some(0),
            raw: "\x1b[1mFilesystem      Size  Used Avail Use% Mounted on\x1b[0m\r\n/dev/sda1       100G   50G   50G  50% /\r\n"
                .into(),
        };
        let text = block(&catalog(), &done, None);
        assert!(text.starts_with("$ df -h  (exit 0)\n"), "{text}");
        assert!(text.contains("-h") && text.contains("Use%") && text.contains("Mounted on"), "{text}");
        assert!(text.ends_with("\n\n"));
    }

    #[test]
    fn a_failure_gets_the_error_doctor_instead_of_field_explanations() {
        let done = Finished {
            command: "cat /nonexistent-huh-file".into(),
            exit: Some(1),
            raw: "cat: /nonexistent-huh-file: No such file or directory\r\n".into(),
        };
        let text = block(&catalog(), &done, None);
        assert!(text.starts_with("$ cat /nonexistent-huh-file  (exit 1)\n"), "{text}");
        assert!(text.to_lowercase().contains("does not exist") || text.contains("No such file"), "{text}");
        assert!(!text.contains("looks like the output"), "{text}");
    }

    #[test]
    fn unrecognised_success_prints_only_the_command_explanation() {
        let done = Finished { command: "echo hi".into(), exit: Some(0), raw: "hi\r\n".into() };
        let text = block(&catalog(), &done, None);
        assert!(text.contains("Print the given text"), "{text}");
        assert!(!text.contains("looks like"), "{text}");
    }

    #[test]
    fn full_screen_programs_are_only_named() {
        let done = Finished { command: "htop".into(), exit: Some(0), raw: "\x1b[?1049h\x1b[Hstuff\x1b[?1049l".into() };
        let text = block(&catalog(), &done, None);
        assert!(text.contains("full-screen program, nothing to explain"), "{text}");
        assert!(!text.contains("looks like"), "{text}");
    }

    #[test]
    fn quiet_explains_a_program_only_once() {
        let mut seen = HashSet::new();
        let done = Finished { command: "echo one".into(), exit: Some(0), raw: "one\r\n".into() };
        let first = block(&catalog(), &done, Some(&mut seen));
        let again = Finished { command: "echo two".into(), exit: Some(0), raw: "two\r\n".into() };
        let second = block(&catalog(), &again, Some(&mut seen));
        assert!(first.contains("Print the given text"));
        assert_eq!(second, "$ echo two  (exit 0)\n\n");
    }

    #[test]
    fn a_finished_command_counts_more_than_a_busy_log() {
        let (dir, busy) = recording("activity");
        let quiet = dir.join("2.log");
        std::fs::write(&busy, "x").unwrap();
        std::fs::write(busy.with_extension("status"), "0\nvim\n").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&quiet, "y").unwrap();
        std::fs::write(quiet.with_extension("status"), "0\nls\n").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&busy, "x more output").unwrap();
        assert_eq!(recordings(&dir, None, false).first(), Some(&quiet));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn picks_the_newest_other_recording() {
        let (dir, log) = recording("pick");
        let own = dir.join("1.log");
        std::fs::write(&log, "x").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&own, "y").unwrap();
        assert_eq!(recordings(&dir, Some(&own), false), vec![log.clone()]);
        assert_eq!(recordings(&dir, None, false).first(), Some(&own));
        assert!(recordings(&dir.join("missing"), None, false).is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn no_hook_notice_guides_setup_and_lab() {
        let text = no_hook_notice();
        assert!(!text.contains('\x1b'));
        assert!(text.contains("Live shell watching needs 'huh setup' in your shell."), "{text}");
        assert!(text.contains("/mode lab"), "{text}");
    }

    #[test]
    fn programs_skip_wrappers_and_cover_pipelines() {
        assert_eq!(programs("sudo ls -l | grep x"), vec!["ls", "grep"]);
        assert_eq!(programs("FOO=1 make && echo ok"), vec!["make", "echo"]);
    }

    fn tool(name: &str, description: &str) -> Tool {
        Tool { name: name.into(), description: description.into() }
    }

    fn answer(line: &str, mode: Mode, last: Option<&Finished>, tools: Vec<Tool>) -> Reply {
        let everything = catalog();
        let usable = crate::catalog::usable(everything.clone(), &["arch".to_string()], false);
        let ctx = Input::new(&everything, &usable, mode, vec![], Some(tools));
        answer_line(line, &ctx, last)
    }

    fn text(reply: Reply) -> String {
        match reply {
            Reply::Text(text) | Reply::Instant(text) => text,
            other => panic!("expected text, got {other:?}"),
        }
    }

    fn say(line: &str, last: Option<&Finished>) -> String {
        text(answer(line, Mode::Normal, last, vec![]))
    }

    fn df_output() -> Finished {
        Finished {
            command: "df -h".into(),
            exit: Some(0),
            raw: "\x1b[1mFilesystem      Size  Used Avail Use% Mounted on\x1b[0m\r\n/dev/sda1       100G   50G   50G  50% /\r\n"
                .into(),
        }
    }

    fn failure() -> Finished {
        Finished {
            command: "cat /nonexistent-huh-file".into(),
            exit: Some(1),
            raw: "cat: /nonexistent-huh-file: No such file or directory\r\n".into(),
        }
    }

    #[test]
    fn a_command_line_gets_the_explain_rendering() {
        let typed = say("tar -xzf backup.tar.gz", None);
        let explained = crate::render::render(&crate::explain::explain_command(&catalog(), "tar -xzf backup.tar.gz"));
        assert_eq!(typed, format!("{explained}\n"));
        assert!(typed.contains("-x") && typed.contains("backup.tar.gz"), "{typed}");
        let piped = say("ls -l | wc -l", None);
        assert!(piped.contains("wc"), "{piped}");
    }

    #[test]
    fn plain_words_get_the_normal_suggestions() {
        let typed = say("make deploy.sh executable", None);
        assert!(typed.starts_with("1. chmod +x deploy.sh"), "{typed}");
        assert!(typed.ends_with("\n\n"));
        let refused = say("wipe the drive /dev/sdb", None);
        assert!(refused.contains("No suggestion: formatting or wiping a drive"), "{refused}");
        let nothing = say("zzz qqq xxx", None);
        assert!(nothing.contains("No suggestion") || nothing.contains("hint"), "{nothing}");
    }

    #[test]
    fn plain_words_follow_the_mode_and_keep_warnings() {
        let expert = text(answer("wipe the drive /dev/sdb", Mode::Expert, None, vec![]));
        assert!(expert.starts_with("1. sudo wipefs -a /dev/sdb"), "{expert}");
        assert!(expert.contains("WARNING (destroys data)"), "{expert}");
        let normal = text(answer("wipe the drive /dev/sdb", Mode::Normal, None, vec![]));
        assert!(!normal.contains("wipefs"), "{normal}");
    }

    #[test]
    fn a_question_mark_filters_the_installed_programs_instantly() {
        let tools = vec![
            tool("gzip", "Compress or expand files"),
            tool("ping", "send ICMP echo to a network host"),
            tool("zstd", "Compress files"),
        ];
        let Reply::Instant(found) = answer("?compress", Mode::Normal, None, tools.clone()) else {
            panic!("?tools must be Instant (never typewritten)");
        };
        assert!(found.contains("gzip") && found.contains("zstd") && !found.contains("ping"), "{found}");
        assert!(found.contains("2 of 3 programs match"), "{found}");
        let both = text(answer("?  network   host ", Mode::Normal, None, tools.clone()));
        assert!(both.contains("ping") && !both.contains("gzip"), "{both}");
        assert!(text(answer("?zzzz", Mode::Normal, None, tools.clone())).contains("no program matches"));
        assert!(text(answer("?", Mode::Normal, None, tools)).contains("type ? and the words"));
    }

    #[test]
    fn a_long_tools_answer_is_cut_with_a_pointer_to_huh_tools() {
        let many: Vec<Tool> = (0..45).map(|n| tool(&format!("tool{n:02}"), "does a thing")).collect();
        let found = text(answer("?thing", Mode::Normal, None, many));
        assert_eq!(found.lines().filter(|l| l.starts_with("tool")).count(), MAX_TOOL_LINES, "{found}");
        assert!(found.contains("25 more: huh tools thing"), "{found}");
        assert!(!found.contains("programs match"), "{found}");
        assert!(found.ends_with("\n\n"));
    }

    #[test]
    fn why_answers_for_the_last_finished_command() {
        let bad = failure();
        let typed = say("why", Some(&bad));
        assert!(typed.starts_with("$ cat /nonexistent-huh-file (exit 1)\n"), "{typed}");
        assert!(typed.to_lowercase().contains("does not exist") || typed.contains("No such file"), "{typed}");
        assert!(typed.ends_with("\n\n"));
        let none = say("why", None);
        assert!(none.contains("nothing has finished"), "{none}");
        assert!(!say("why is the sky blue", Some(&bad)).starts_with("$ cat"));
    }

    #[test]
    fn an_empty_line_shows_the_last_block_in_full() {
        let done = df_output();
        let again = say("   ", Some(&done));
        assert_eq!(again, block(&catalog(), &done, None));
        assert!(again.starts_with("$ df -h  (exit 0)\n") && again.contains("Mounted on"), "{again}");
        let echo = Finished { command: "echo hi".into(), exit: Some(0), raw: "hi\r\n".into() };
        let mut seen = HashSet::new();
        block(&catalog(), &echo, Some(&mut seen));
        assert_eq!(block(&catalog(), &echo, Some(&mut seen)), "$ echo hi  (exit 0)\n\n");
        assert!(say("", Some(&echo)).contains("Print the given text"));
        assert_eq!(say("", Some(&done)), again);
        assert!(say("", None).contains("nothing has finished"));
    }

    #[test]
    fn find_and_mode_slash_commands_parse_cleanly() {
        let reply = |line: &str| answer(line, Mode::Normal, None, vec![]);
        assert_eq!(reply("/find"), Reply::Find(vec![]));
        assert_eq!(reply("/find  compress   files "), Reply::Find(vec!["compress".into(), "files".into()]));
        assert_eq!(reply("/find\tx"), Reply::Find(vec!["x".into()]));
        let Reply::Instant(unknown_finder) = reply("/finder") else { panic!("expected unknown slash note") };
        assert!(unknown_finder.contains("Unknown slash command \"/finder\""), "{unknown_finder}");

        assert_eq!(reply("/mode"), Reply::Slash(SlashCmd::ModeInteractive));
        assert_eq!(reply("/mode lab"), Reply::Slash(SlashCmd::Mode(PaneMode::Lab, vec![])));
        assert_eq!(reply("/mode watch"), Reply::Slash(SlashCmd::Mode(PaneMode::Watch, vec![])));
        assert_eq!(reply("/watch"), Reply::Slash(SlashCmd::Mode(PaneMode::Watch, vec![])));
        assert_eq!(reply("/lab"), Reply::Slash(SlashCmd::Mode(PaneMode::Lab, vec![])));
        assert_eq!(
            reply("/lab practice 2"),
            Reply::Slash(SlashCmd::Mode(PaneMode::Lab, vec!["practice".into(), "2".into()]))
        );
        let Reply::Instant(unknown_modeled) = reply("/modeled") else { panic!("expected unknown slash note") };
        assert!(unknown_modeled.contains("Unknown slash command \"/modeled\""), "{unknown_modeled}");

        // Multi-segment Unix paths never become slash commands.
        assert!(matches!(reply("/etc/hosts"), Reply::Text(_)));
        assert!(matches!(reply("/usr/bin/env"), Reply::Text(_)));

        // Option A fuzzy slash resolutions.
        assert_eq!(
            reply("/th cat"),
            Reply::SlashNote {
                note: "→ /theme catppuccin".into(),
                inner: Box::new(Reply::Slash(SlashCmd::Theme(Some("catppuccin".into())))),
            }
        );
        let Reply::SlashPick { title, defs, .. } = reply("/") else { panic!("expected palette") };
        assert_eq!(title, "Slash commands:");
        assert_eq!(defs.len(), 11);
        let Reply::SlashPick { title: amb_title, defs: amb_defs, .. } = reply("/s") else {
            panic!("expected ambiguous")
        };
        assert_eq!(amb_title, "Matching slash commands:");
        assert_eq!(amb_defs.iter().map(|d| d.name).collect::<Vec<_>>(), ["/safety", "/speed"]);
    }

    #[test]
    fn help_lists_commands_and_slash_toggles_instantly() {
        for line in ["help", "/help", "  help  "] {
            let Reply::Instant(help) = answer(line, Mode::Normal, None, vec![]) else {
                panic!("help must be Instant");
            };
            for word in [
                "type / for fuzzy commands",
                "/mode",
                "/safety",
                "/theme",
                "/pet",
                "/speed",
                "/quiet",
                "/tips",
                "/find",
                "?word",
                "why",
                "Enter",
            ] {
                assert!(help.contains(word), "{word}: {help}");
            }
        }
    }

    #[test]
    fn pet_command_triggers_pet_slash_while_plain_phrases_stay_suggestions() {
        let reply = |line: &str| answer(line, Mode::Normal, None, vec![]);
        assert_eq!(reply("pet"), Reply::Slash(SlashCmd::Pet(vec![])));
        assert_eq!(reply("  /pet "), Reply::Slash(SlashCmd::Pet(vec![])));
        assert_eq!(reply("/pet name Pip"), Reply::Slash(SlashCmd::Pet(vec!["name".into(), "Pip".into()])));
        assert_eq!(reply("/pet style ascii"), Reply::Slash(SlashCmd::Pet(vec!["style".into(), "ascii".into()])));
        assert_eq!(reply("/pet customize"), Reply::Slash(SlashCmd::Pet(vec!["customize".into()])));
        assert!(matches!(reply("pet a dog"), Reply::Text(_)));
    }

    #[test]
    fn pet_session_observes_success_failure_warning_and_watch_unlocks() {
        let epoch = 1_795_046_400;
        let mut session = PetSession { pet: Pet::default(), transient: None, enabled: true, path: None };

        let ok = Finished { command: "ls -la".into(), exit: Some(0), raw: String::new() };
        let b1 = session.observe(&ok, false, epoch).unwrap();
        assert_eq!(b1.mood, Mood::Happy);
        assert_eq!(session.pet.commands_fed, 1);

        // Warning with exit 0 still alerts and increments warnings_heeded.
        let happy_before = session.pet.happiness;
        let (b2, u2) = session.observe_with_unlocks(&ok, true, epoch + 10);
        assert_eq!(b2.unwrap().mood, Mood::Alert);
        assert!(u2.is_empty());
        assert_eq!(session.pet.cmd_streak, 0);
        assert_eq!(session.pet.warnings_heeded, 1);
        assert_eq!(session.pet.happiness, happy_before);

        // Next clean command clears transient alert and feeds again.
        let b3 = session.observe(&ok, false, epoch + 20).unwrap();
        assert_eq!(b3.mood, Mood::Happy);

        // Non-zero exit triggers Alert without incrementing warnings_heeded.
        let fail = Finished { command: "false".into(), exit: Some(1), raw: String::new() };
        let b4 = session.observe(&fail, false, epoch + 30).unwrap();
        assert_eq!(b4.mood, Mood::Alert);
        assert_eq!(session.pet.warnings_heeded, 1);

        // 4 more warnings unlock Snowy Owl via watch milestone (5 total).
        for i in 1..crate::pet::OWL_WATCH_WARNINGS {
            let cmd = Finished { command: "rm -rf /tmp/x".into(), exit: Some(0), raw: String::new() };
            let (banner, unlocked) = session.observe_with_unlocks(&cmd, true, epoch + 40 + u64::from(i));
            if i == crate::pet::OWL_WATCH_WARNINGS - 1 {
                assert_eq!(unlocked, vec![Species::Owl]);
                assert_eq!(banner.unwrap().mood, Mood::Celebrating);
            } else {
                assert!(unlocked.is_empty());
            }
        }
        assert!(session.pet.is_unlocked(Species::Owl));

        // 25 unique programs unlock Terminal Cat via watch milestone.
        let mut cat_session = PetSession { pet: Pet::default(), transient: None, enabled: true, path: None };
        for i in 0..crate::pet::CAT_WATCH_UNIQUE {
            let cmd = Finished { command: format!("prog{i} -x"), exit: Some(0), raw: String::new() };
            let (_, unlocked) = cat_session.observe_with_unlocks(&cmd, false, epoch + 100 + i as u64);
            if i + 1 == crate::pet::CAT_WATCH_UNIQUE {
                assert_eq!(unlocked, vec![Species::Cat]);
            } else {
                assert!(unlocked.is_empty());
            }
        }
        assert!(cat_session.pet.is_unlocked(Species::Cat));

        let mut off = PetSession { pet: Pet::default(), transient: None, enabled: false, path: None };
        assert_eq!(off.observe(&ok, false, epoch), None);
    }

    #[test]
    fn pet_session_merges_cross_process_metadata_on_observe_save_and_card() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/pane-tests")
            .join(format!("pet-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let pet_path = dir.join("pet.toml");

        let initial = Pet::default();
        crate::pet::save_to(&pet_path, &initial).unwrap();
        let mut session = PetSession {
            pet: crate::pet::load_from(&pet_path),
            transient: None,
            enabled: true,
            path: Some(pet_path.clone()),
        };

        let mut external = crate::pet::load_from(&pet_path);
        external.rename("Éclair").unwrap();
        external.unlock_species(Species::Cat);
        external.set_species(Species::Cat).unwrap();
        crate::pet::save_to(&pet_path, &external).unwrap();

        let epoch = 1_795_046_400;
        let ok = Finished { command: "ls -la".into(), exit: Some(0), raw: String::new() };
        let banner = session.observe(&ok, false, epoch).unwrap();
        session.save();
        assert_eq!(banner.name, "Éclair");
        assert_eq!(banner.species, Species::Cat);
        assert_eq!(banner.mood, Mood::Happy);

        let on_disk = crate::pet::load_from(&pet_path);
        assert_eq!(on_disk.name, "Éclair");
        assert_eq!(on_disk.species, Species::Cat);
        assert!(on_disk.is_unlocked(Species::Cat));
        assert_eq!(on_disk.commands_fed, 1);

        let mut external2 = Pet::default();
        external2.rename("Ștefan").unwrap();
        external2.unlock_species(Species::Turtle);
        external2.set_species(Species::Turtle).unwrap();
        crate::pet::save_to(&pet_path, &external2).unwrap();

        let card = session.render_card(epoch + 10);
        assert!(card.contains("Ștefan (Shell-Turtle)"), "{card}");
        assert!(session.pet.is_unlocked(Species::Cat));
        assert!(session.pet.is_unlocked(Species::Turtle));

        session.save();
        let final_disk = crate::pet::load_from(&pet_path);
        assert_eq!(final_disk.name, "Ștefan");
        assert_eq!(final_disk.species, Species::Turtle);
        assert!(final_disk.is_unlocked(Species::Cat));
        assert!(final_disk.is_unlocked(Species::Turtle));
        assert_eq!(final_disk.commands_fed, 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pet_session_reloads_from_disk_after_lab_with_celebrating_mood() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/pane-tests")
            .join(format!("pet-reload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let pet_path = dir.join("pet.toml");

        let mut session =
            PetSession { pet: Pet::default(), transient: None, enabled: true, path: Some(pet_path.clone()) };

        let epoch = 1_795_046_400;
        let mut learned = Pet::default();
        learned.feed(Some("pwd"), epoch);
        learned.unlock_species(Species::Cat);
        crate::pet::save_to(&pet_path, &learned).unwrap();

        session.reload_from_disk(true);
        assert_eq!(session.pet.commands_fed, 1);
        assert!(session.pet.is_unlocked(Species::Cat));
        let banner = session.banner(epoch).unwrap();
        assert_eq!(banner.mood, Mood::Celebrating);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn all_slash_commands_persist_to_prefs_without_touching_config_toml() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/pane-tests")
            .join(format!("slash-prefs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config_path = dir.join("config/config.toml");
        std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        let original_config = "theme = \"babyblue\"\nmode = \"normal\"\n";
        std::fs::write(&config_path, original_config).unwrap();

        let mut st = test_state(&dir, true);

        // /safety toggles and sets explicitly.
        let SlashOutcome::Output(s1) = st.exec_slash(SlashCmd::Safety(None)) else { panic!() };
        assert!(s1.contains("expert"), "{s1}");
        assert_eq!(st.safety, Mode::Expert);
        let SlashOutcome::Output(s2) = st.exec_slash(SlashCmd::Safety(Some("normal".into()))) else { panic!() };
        assert!(s2.contains("normal"), "{s2}");
        assert_eq!(st.safety, Mode::Normal);

        // /theme lists and updates theme in prefs (never calling style::set in tests).
        let SlashOutcome::Output(t_list) = st.exec_slash(SlashCmd::Theme(None)) else { panic!() };
        assert!(t_list.contains("* babyblue") && t_list.contains("nord"), "{t_list}");
        let SlashOutcome::Output(t_set) = st.exec_slash(SlashCmd::Theme(Some("nord".into()))) else { panic!() };
        assert!(t_set.contains("Theme set to nord"), "{t_set}");
        assert_eq!(st.theme, Theme::Nord);

        // /speed updates speed and persists to prefs.
        let SlashOutcome::Output(sp_list) = st.exec_slash(SlashCmd::Speed(None)) else { panic!() };
        assert!(sp_list.contains("normal (2000ms)"), "{sp_list}");
        let SlashOutcome::Output(sp_set) = st.exec_slash(SlashCmd::Speed(Some("fast".into()))) else { panic!() };
        assert!(sp_set.contains("fast (800ms)"), "{sp_set}");
        assert_eq!(st.speed, Speed::Fast);

        // /quiet toggles and sets explicitly.
        let SlashOutcome::Output(q1) = st.exec_slash(SlashCmd::Quiet(None)) else { panic!() };
        assert!(q1.contains("on"), "{q1}");
        assert!(st.quiet && st.explained.is_some());
        let SlashOutcome::Output(q2) = st.exec_slash(SlashCmd::Quiet(Some("off".into()))) else { panic!() };
        assert!(q2.contains("off"), "{q2}");
        assert!(!st.quiet && st.explained.is_none());

        // /tips toggles, advances, and persists to prefs.
        let SlashOutcome::Output(tip_off) = st.exec_slash(SlashCmd::Tips(Some("off".into()))) else { panic!() };
        assert!(tip_off.contains("off"), "{tip_off}");
        assert!(!st.tips_enabled);
        let SlashOutcome::Output(tip_on) = st.exec_slash(SlashCmd::Tips(None)) else { panic!() };
        assert!(tip_on.contains("on"), "{tip_on}");
        assert!(st.tips_enabled);
        let idx_before = st.tip_index;
        let SlashOutcome::Output(tip_next) = st.exec_slash(SlashCmd::Tips(Some("next".into()))) else { panic!() };
        assert_eq!(st.tip_index, (idx_before + 1) % tips::TIPS.len());
        assert!(tip_next.contains(tips::tip_at(st.tip_index)), "{tip_next}");

        // Non-TTY /s and /quiet o numbered lists render cleanly.
        let SlashOutcome::Output(ambig) = st.exec_submitted_slash("/s") else { panic!() };
        assert!(ambig.contains("/safety") && ambig.contains("/speed"), "{ambig}");
        let SlashOutcome::Output(sub_ambig) = st.exec_submitted_slash("/quiet o") else { panic!() };
        assert!(sub_ambig.contains("on") && sub_ambig.contains("off"), "{sub_ambig}");

        // /pet status, name, species (locked & unlocked), style, customize.
        let SlashOutcome::Output(p_status) = st.exec_slash(SlashCmd::Pet(vec![])) else { panic!() };
        assert!(p_status.contains("Pebble (Penguin chick)"), "{p_status}");
        assert!(p_status.contains("/mode lab") && p_status.contains("0/25"), "{p_status}");

        let SlashOutcome::Output(p_name) = st.exec_slash(SlashCmd::Pet(vec!["name".into(), "Biscuit".into()])) else {
            panic!()
        };
        assert!(p_name.contains("Renamed companion to Biscuit"), "{p_name}");
        assert_eq!(st.pet_session.pet.name, "Biscuit");

        let SlashOutcome::Output(p_locked) = st.exec_slash(SlashCmd::Pet(vec!["species".into(), "cat".into()])) else {
            panic!()
        };
        assert!(p_locked.contains("locked") && p_locked.contains("pathfinder"), "{p_locked}");
        st.pet_session.pet.unlock_species(Species::Cat);
        st.pet_session.save();
        let SlashOutcome::Output(p_cat) = st.exec_slash(SlashCmd::Pet(vec!["species".into(), "cat".into()])) else {
            panic!()
        };
        assert!(p_cat.contains("Switched companion to cat"), "{p_cat}");
        assert_eq!(st.pet_session.pet.species, Species::Cat);
        let SlashOutcome::Output(p_rename2) = st.exec_slash(SlashCmd::Pet(vec!["name".into(), "Mochi".into()])) else {
            panic!()
        };
        assert!(p_rename2.contains("Renamed companion to Mochi"), "{p_rename2}");
        let reloaded_pet = crate::pet::load_from(st.pet_session.path.as_deref().unwrap());
        assert_eq!(reloaded_pet.name, "Mochi");
        assert_eq!(reloaded_pet.species, Species::Cat);

        let SlashOutcome::Output(p_style) = st.exec_slash(SlashCmd::Pet(vec!["style".into(), "ascii".into()])) else {
            panic!()
        };
        assert!(p_style.contains("ascii"), "{p_style}");
        assert_eq!(st.pet_style, RenderStyle::Ascii);
        assert_eq!(st.effective_pet_mode(), EffectiveMode::Ascii);

        let SlashOutcome::Output(p_cust) = st.exec_slash(SlashCmd::Pet(vec!["customize".into()])) else { panic!() };
        assert!(p_cust.contains("cat.toml"), "{p_cust}");
        assert!(dir.join("config/sprites/cat.toml").is_file());

        // /mode switches Watch <-> Lab and persists last_mode.
        let SlashOutcome::SwitchMode(PaneMode::Lab, args) =
            st.exec_slash(SlashCmd::Mode(PaneMode::Lab, vec!["review".into()]))
        else {
            panic!()
        };
        assert_eq!(args, vec!["review"]);
        assert_eq!(st.mode, PaneMode::Lab);
        let SlashOutcome::Output(already_lab) = st.exec_slash(SlashCmd::Mode(PaneMode::Lab, vec![])) else { panic!() };
        assert!(already_lab.contains("Already in Lab mode"), "{already_lab}");
        let SlashOutcome::SwitchMode(PaneMode::Watch, _) = st.exec_slash(SlashCmd::Mode(PaneMode::Watch, vec![]))
        else {
            panic!()
        };
        assert_eq!(st.mode, PaneMode::Watch);

        // Verify prefs.toml captured everything and config.toml remained completely untouched.
        let saved_prefs = prefs::load_from(st.prefs_path.as_deref().unwrap());
        assert_eq!(saved_prefs.theme.as_deref(), Some("nord"));
        assert_eq!(saved_prefs.safety.as_deref(), Some("normal"));
        assert_eq!(saved_prefs.speed.as_deref(), Some("fast"));
        assert_eq!(saved_prefs.pet_style.as_deref(), Some("ascii"));
        assert_eq!(saved_prefs.quiet, Some(false));
        assert_eq!(saved_prefs.tips, Some(true));
        assert_eq!(saved_prefs.last_mode.as_deref(), Some("watch"));
        assert_eq!(std::fs::read_to_string(&config_path).unwrap(), original_config);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn brief_why_nudge_and_no_hook_header_defaults() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/pane-tests")
            .join(format!("nudge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut st = test_state(&dir, false);

        let info = st.header_info();
        assert_eq!(info.tty_label, "no hook");
        assert_eq!(info.mode, PaneMode::Watch);
        assert_eq!(info.pet_mode, EffectiveMode::Ascii);
        assert_eq!(info.tip.as_deref(), Some(tips::tip_at(0)));

        st.tick_animation();
        assert_eq!(st.tick, 1);
        assert_eq!(st.tip_index, 0);

        for _ in 1..tips::ROTATE_TICKS {
            st.tick_animation();
        }
        assert_eq!(st.tip_index, 1, "tips rotate after ROTATE_TICKS ticks");
        assert_eq!(st.header_info().tip.as_deref(), Some(tips::tip_at(1)));

        let failed = failure();
        let nudge = brief_why_nudge(&failed).unwrap();
        assert!(nudge.contains("cat /nonexistent-huh-file (exit 1)"), "{nudge}");
        assert!(nudge.lines().count() <= 4, "{nudge}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pane_host_trims_lab_inputs_handles_lab_args_and_caches_practice_by_sandbox() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/pane-tests")
            .join(format!("host-trim-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sb_a = dir.join("sb-a");
        let sb_b = dir.join("sb-b");
        std::fs::create_dir_all(&sb_a).unwrap();
        std::fs::create_dir_all(&sb_b).unwrap();

        let (rec_dir, log) = recording("host-cache");
        std::fs::write(&log, "$ ").unwrap();

        let mut st = test_state(&dir, false);
        st.mode = PaneMode::Lab;
        st.follower = Some(Follower::new(&log));
        let mut host = PaneHost { state: &mut st, last_auto_report: None };

        // Already in Lab: clarified message when extra args are given, watch switches cleanly.
        assert_eq!(already_in_lab_message(&[]), "Already in Lab mode.");
        assert_eq!(
            already_in_lab_message(&["review".into()]),
            "Already in Lab mode — type 'q' to return to the Lab menu to switch units."
        );
        assert!(matches!(host.handle_slash("/lab review").unwrap(), SlashEffect::Handled));
        assert!(matches!(host.handle_slash("/th nord").unwrap(), SlashEffect::Handled));
        assert_eq!(host.state.theme, Theme::Nord);
        assert!(matches!(host.handle_slash("/etc/hosts").unwrap(), SlashEffect::NotSlash));
        assert!(matches!(host.handle_slash("/watch").unwrap(), SlashEffect::SwitchToWatch));

        // PaneHost::read_line trims whitespace like PlainHost::read_line.
        let mut dummy = std::io::Cursor::new(&b""[..]);
        host.state.stdin.push_test_line("   ");
        assert_eq!(host.read_line(&mut dummy).unwrap(), Some(String::new()));
        host.state.stdin.push_test_line("  1  ");
        assert_eq!(host.read_line(&mut dummy).unwrap(), Some("1".to_string()));

        // Practice auto-report cache keys on (sandbox_dir, Report) and clears on restart.
        let practice = Practice {
            id: "unit-1-test".into(),
            title: "Test".into(),
            goal: "Create clue.txt".into(),
            hints: vec![],
            setup: Default::default(),
            checks: vec![crate::practice::Check::FileExists {
                path: "clue.txt".into(),
                label: "clue.txt exists".into(),
            }],
        };

        let mut reported = 0u64;
        finish(&log, "ok\r\n", 0, "ls");
        assert!(matches!(
            host.wait_practice_turn(&sb_a, &practice, &mut reported),
            Some(PracticePoll::AutoUpdate { .. })
        ));
        assert_eq!(reported, 1);

        // Identical failing outcome in the same sandbox suppresses duplicate AutoUpdate
        // and falls through to trimmed UserLine.
        host.state.stdin.push_test_line(" q \t");
        finish(&log, "ok\r\n", 0, "pwd");
        match host.wait_practice_turn(&sb_a, &practice, &mut reported) {
            Some(PracticePoll::UserLine(Some(line))) => assert_eq!(line, "q"),
            other => panic!("expected trimmed UserLine(\"q\"), got {other:?}"),
        }
        assert_eq!(reported, 2);

        // Different sandbox_dir with identical failing outcome still reports fresh AutoUpdate.
        finish(&log, "ok\r\n", 0, "ls");
        assert!(matches!(
            host.wait_practice_turn(&sb_b, &practice, &mut reported),
            Some(PracticePoll::AutoUpdate { .. })
        ));
        assert_eq!(reported, 3);

        // Restarting the same sandbox (reported resets to 0) reports fresh AutoUpdate again.
        reported = 0;
        finish(&log, "ok\r\n", 0, "ls");
        assert!(matches!(
            host.wait_practice_turn(&sb_b, &practice, &mut reported),
            Some(PracticePoll::AutoUpdate { .. })
        ));
        assert_eq!(reported, 1);

        let _ = std::fs::remove_dir_all(&rec_dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_typed_is_ever_run() {
        for line in
            ["rm -rf ~", "sudo dd if=/dev/zero of=/dev/sda", "delete everything", "why", "?rm", "!!", "/etc/hosts"]
        {
            assert!(matches!(answer(line, Mode::Normal, None, vec![]), Reply::Text(_) | Reply::Instant(_)), "{line}");
        }
        assert!(matches!(answer("/", Mode::Normal, None, vec![]), Reply::SlashPick { .. }));
    }
}
