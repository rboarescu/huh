//! `huh tutor`: follows the recording of the terminal next to it (see session.rs) and prints,
//! for each finished command, what it does and what its output or failure means.
//! It only reads the recording and the status file. It never runs anything.

use crate::breakdown::{Block, Explanation, Row};
use crate::catalog::Command;
use crate::matcher::LocalMatcher;
use crate::mode::Mode;
use crate::session;
use crate::style::{Role, paint};
use crate::tools::Tool;
use anyhow::{Result, bail};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

const POLL: Duration = Duration::from_millis(300);
/// Polls to wait for the end marker to reach the log after the status file changed (about 3 s).
const PATIENCE: u32 = 10;
/// Most flag or argument rows shown for a command.
const MAX_ROWS: usize = 5;

static STOP: AtomicBool = AtomicBool::new(false);

/// Held while anything is printed, so that a block from the followed terminal and the answer to a
/// typed line never mix.
static PRINT: Mutex<()> = Mutex::new(());

/// The line shown where you can type.
const PROMPT: &str = "huh> ";
/// Most lines of a `?words` answer.
const MAX_TOOL_LINES: usize = 20;

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

    /// The command that finished since the last call, if any. When several finish between two
    /// calls only the newest is reported.
    pub fn poll(&mut self) -> Option<Finished> {
        let now = stamp(&self.status)?;
        if Some(&now) == self.seen.as_ref() || now.1.trim().is_empty() {
            return None;
        }
        let log = String::from_utf8_lossy(&std::fs::read(&self.log).ok()?).into_owned();
        // The status file is written before the recorder's pipe has delivered the end marker.
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

/// `<dir>/<pid>.log` files of recordings still running, newest first. The caller's own
/// recording is left out: that is the pane `huh tutor` runs in.
/// When the terminal last finished a command: its status file is written then (and when it
/// starts). The log itself changes all the time under a full-screen program, so it only counts
/// when there is no status file.
fn last_activity(log: &Path) -> Option<SystemTime> {
    let status = log.with_extension("status");
    std::fs::metadata(&status).or_else(|_| std::fs::metadata(log)).ok()?.modified().ok()
}

/// The terminal a recording belongs to (`/dev/pts/3`): the recorder's standard input.
/// The recorder reads the outer terminal; the shell it records runs on a pseudo-terminal of its
/// own. `tty` in that shell prints the inner one, so that is the name shown, and both match.
fn ttys_of(log: &Path) -> Vec<String> {
    let Some(pid) = log.file_stem() else { return Vec::new() };
    let proc_dir = Path::new("/proc").join(pid);
    let stdin_of = |dir: &Path| std::fs::read_link(dir.join("fd/0")).ok().map(|t| t.display().to_string());
    let mut out = Vec::new();
    // Children first: the recorded shell, whose terminal is the one people see.
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

/// Set by `--dir`: the folder `--side` found the recordings in, in the shell that started it. A
/// window opened by the desktop does not inherit that shell's environment.
static RECORDING_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Set by `--log`: the exact recording of the terminal `--side` was started from.
static RECORDING: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub fn set_recording(log: PathBuf) {
    RECORDING.set(log).ok();
}

pub fn set_recording_dir(dir: PathBuf) {
    RECORDING_DIR.set(dir).ok();
}

fn recording_dir() -> Option<PathBuf> {
    if let Some(dir) = RECORDING_DIR.get() {
        return Some(dir.clone());
    }
    // The folder this pane's own recording is in: the terminals next to it are recorded there too,
    // whichever recorder set them up.
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

/// The recording of the terminal `tty` (e.g. /dev/pts/3): the recorder's standard input is that terminal.
fn recording_of_tty(dir: &Path, tty: &str) -> Option<PathBuf> {
    recordings(dir, None, true).into_iter().find(|log| ttys_of(log).iter().any(|t| t == tty))
}

fn locate(tty: Option<&str>) -> Option<PathBuf> {
    // The terminal --side was started from, by its own recording: nothing to match or guess.
    if let Some(log) = RECORDING.get().filter(|log| log.is_file()) {
        return Some(log.clone());
    }
    let dir = recording_dir()?;
    match tty {
        // `--tty 8` is short for `--tty /dev/pts/8`.
        Some(tty) if tty.chars().all(|c| c.is_ascii_digit()) => recording_of_tty(&dir, &format!("/dev/pts/{tty}")),
        Some(tty) => recording_of_tty(&dir, tty),
        None => {
            let own = std::env::var_os("HUH_SESSION_LOG").map(PathBuf::from);
            recordings(&dir, own.as_deref(), true).into_iter().next()
        }
    }
}

// ---------------------------------------------------------------------------
// What is printed
// ---------------------------------------------------------------------------

/// Programs a command line starts with, one per pipeline stage (for `--quiet`).
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

/// The explanation of the command line cut down to a few lines: what it does, its first flags
/// and arguments, and the first warning.
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

const ALT_SCREEN: [&str; 3] = ["\x1b[?1049h", "\x1b[?1047h", "\x1b[?47h"];

/// Everything printed for one finished command (ends with a blank line).
pub fn block(catalog: &[Command], done: &Finished, explained: Option<&mut HashSet<String>>) -> String {
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
    let mut explain_command = !done.command.trim().is_empty();
    if let Some(seen) = explained {
        let names = programs(&done.command);
        explain_command = explain_command && !names.iter().all(|n| seen.contains(n));
        seen.extend(names);
    }
    if explain_command {
        out.push_str(&condensed(&crate::explain::explain_command(catalog, &done.command)));
    }
    if ALT_SCREEN.iter().any(|s| done.raw.contains(s)) {
        out.push_str(&paint(Role::Dim, "  full-screen program, nothing to explain"));
        out.push_str("\n\n");
        return out;
    }
    let cleaned = session::clean(&done.raw);
    if done.exit.is_some_and(|c| c != 0) {
        let last = session::Last { command: done.command.clone(), exit: done.exit, output: session::tail(&cleaned) };
        let mut answer = String::new();
        let _ = crate::doctor::explain(&last, &mut |text| answer = text.to_string());
        out.push_str(&format!("\n{}\n", crate::style::answer(answer.trim_end())));
    } else if let Some(described) = crate::outputs::describe(&cleaned) {
        out.push_str(&format!("\n{}", described.text));
    }
    out.push('\n');
    out
}

extern "C" fn on_interrupt(_: i32) {
    STOP.store(true, Ordering::Relaxed);
}

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

fn how_to_record() -> String {
    "no recorded terminal found. `huh tutor` follows the recording that `huh shell-init` sets up:\n\
     \x20 1. Put `eval \"$(huh shell-init)\"` right after the interactive check in ~/.bashrc, and\n\
     \x20    `eval \"$(huh shell-init --hooks)\"` as its last line (see \"Terminal recording\" in the README).\n\
     \x20 2. Open a new terminal, then run `huh tutor` in a second pane next to it."
        .to_string()
}

/// `huh tutor`: runs until Ctrl+C.
pub fn run(catalog: &[Command], quiet: bool, tty: Option<&str>, mode: Mode) -> Result<()> {
    use std::io::IsTerminal;
    // A pane that `--side` has just opened can be ahead of the recorder of the shell next to it.
    let mut log = locate(tty);
    for _ in 0..if tty.is_some() { 20 } else { 0 } {
        if log.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
        log = locate(tty);
    }
    let Some(log) = log else { bail!("{}", how_to_record()) };
    // With a pinned header the welcome lines are printed after it is drawn, under it.
    let pinning = crate::pin::wanted();
    let shown = crate::welcome::show_with(crate::welcome::Trigger::Tutor, false, false, !pinning);
    if shown == crate::welcome::Shown::Interrupted {
        return Ok(());
    }
    catch_ctrl_c();
    let tty_name = tty_of(&log).unwrap_or_else(|| log.display().to_string());
    // Dropped when `run` ends, however it ends: resets the scroll region and moves the cursor below.
    let pinned = if pinning { crate::pin::Guard::start(&tty_name) } else { None };
    if pinning && shown == crate::welcome::Shown::Played {
        crate::welcome::print_welcome(crate::welcome::Trigger::Tutor);
    }
    if pinned.is_none() {
        println!(
            "{}",
            paint(
                Role::Dim,
                &format!(
                    "huh tutor: following {tty_name}{} (Ctrl+C stops). It only reads.",
                    if tty.is_some() { "" } else { ", the terminal that finished a command most recently" }
                )
            )
        );
    }
    if tty.is_none() {
        let own = std::env::var_os("HUH_SESSION_LOG").map(PathBuf::from);
        let others: Vec<String> = recording_dir()
            .map(|dir| recordings(&dir, own.as_deref(), true))
            .unwrap_or_default()
            .iter()
            .filter(|p| **p != log)
            .filter_map(|p| tty_of(p))
            .collect();
        if !others.is_empty() {
            println!(
                "Other recorded terminals: {}. To follow one, run `tty` in it, then `huh tutor --tty <number>`.",
                others.join(", ")
            );
        }
    }
    let last: Arc<Mutex<Option<Finished>>> = Arc::new(Mutex::new(None));
    // Lines typed here are read only from a terminal; a pipe is left alone.
    let typing = std::io::stdin().is_terminal();
    if typing {
        println!("{}", paint(Role::Dim, "Type a command, ?word, why, /find or help here; nothing you type is run."));
    }
    println!();
    if typing {
        emit(&paint(Role::Dim, PROMPT));
        spawn_input(catalog.to_vec(), mode, Arc::clone(&last));
    }
    let mut follower = Follower::new(&log);
    let mut explained = quiet.then(HashSet::new);
    while !STOP.load(Ordering::Relaxed) {
        if pinned.is_some()
            && let Some(redraw) = crate::pin::on_resize()
        {
            emit(&redraw);
        }
        if let Some(done) = follower.poll() {
            let text = block(catalog, &done, explained.as_mut());
            *lock(&last) = Some(done);
            // The prompt is on the line the cursor is on: wipe it so the block starts at the margin.
            emit(&format!("{}{text}", if typing { "\r\x1b[2K" } else { "" }));
            if typing {
                emit(&paint(Role::Dim, PROMPT));
            }
        }
        std::thread::sleep(POLL);
    }
    drop(pinned);
    Ok(())
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Prints `text` as one piece.
fn emit(text: &str) {
    use std::io::Write;
    let _guard = lock(&PRINT);
    print!("{text}");
    std::io::stdout().flush().ok();
}

/// Reads the lines typed in this pane until the input ends (Ctrl+D), answering each one. It lives
/// as long as the tutor does: the tutor's end takes it along.
fn spawn_input(everything: Vec<Command>, mode: Mode, last: Arc<Mutex<Option<Finished>>>) {
    std::thread::spawn(move || {
        use std::io::BufRead;
        let platforms = crate::catalog::this_platform();
        let usable = crate::catalog::usable(everything.clone(), &platforms, false);
        let ctx = Input::new(&everything, &usable, mode, crate::candidates::current_folder_entries(), None);
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            let finished = lock(&last).clone();
            match answer_line(&line, &ctx, finished.as_ref()) {
                Reply::Text(text) => emit(&format!("{text}{}", paint(Role::Dim, PROMPT))),
                Reply::Find(words) => {
                    // Nothing else prints while the finder has the terminal.
                    let _guard = lock(&PRINT);
                    // The pinned header's scroll region is off while the finder runs.
                    print!("{}", crate::pin::suspend());
                    if let Err(error) = crate::find::run(&words, &everything) {
                        println!("huh find: {error:#}");
                    }
                    print!("{}", crate::pin::resume());
                    print!("{}", paint(Role::Dim, PROMPT));
                    std::io::Write::flush(&mut std::io::stdout()).ok();
                }
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Lines typed in the tutor pane
// ---------------------------------------------------------------------------

/// What a typed line leads to.
#[derive(Debug, PartialEq)]
pub enum Reply {
    /// Text to print (ends with a blank line).
    Text(String),
    /// `/find`: open the finder with these words, then come back.
    Find(Vec<String>),
}

/// What answering a typed line needs. Nothing in it runs anything.
pub struct Input<'a> {
    everything: &'a [Command],
    matcher: LocalMatcher,
    mode: Mode,
    files: Vec<String>,
    /// Loaded on the first `?words` (it takes a moment).
    tools: std::cell::OnceCell<Vec<Tool>>,
}

impl<'a> Input<'a> {
    /// `usable` is the catalog for this system (what suggestions come from); `tools` the list of
    /// installed programs when it is already known.
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
    "Type here (nothing you type is run):\n\
     \x20 a command line      what it does, as `huh explain`; plain words: suggestions, as `huh \"...\"`\n\
     \x20 ?word [word...]     programs that match, as `huh tools`\n\
     \x20 why                 why the last command in the followed terminal failed\n\
     \x20 Enter alone         the last explanation again, in full;  /find [words]: search commands and programs\n\n"
        .to_string()
}

fn note(text: &str) -> String {
    format!("{}\n\n", paint(Role::Note, text))
}

/// The answer to `why`: the error doctor on the last finished command.
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

/// `?words`: the `huh tools` filter, cut to `MAX_TOOL_LINES` lines.
fn tools_text(words: &[String], tools: &[Tool]) -> String {
    let matches = crate::tools::filter(tools, words);
    if matches.is_empty() {
        return note("no program matches");
    }
    let shown = &matches[..matches.len().min(MAX_TOOL_LINES)];
    let text = crate::tools::render(shown, tools.len(), true);
    // The count line of `render` counts the lines shown; this one counts the matches.
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

/// What one typed line does: the line, the tutor's state (`ctx`) and the last finished command
/// in the followed terminal give the text to print, or a request to open the finder.
pub fn answer_line(line: &str, ctx: &Input, last: Option<&Finished>) -> Reply {
    let line = line.trim();
    if line.is_empty() {
        return Reply::Text(match last {
            // Everything, even when `--quiet` hides what was explained before.
            Some(done) => block(ctx.everything, done, None),
            None => note("nothing has finished in the followed terminal yet"),
        });
    }
    if line == "/find" || line.strip_prefix("/find").is_some_and(|rest| rest.starts_with(char::is_whitespace)) {
        return Reply::Find(line["/find".len()..].split_whitespace().map(str::to_string).collect());
    }
    if line == "help" || line == "/help" {
        return Reply::Text(help_text());
    }
    if let Some(words) = line.strip_prefix('?') {
        let words: Vec<String> = words.split_whitespace().map(str::to_string).collect();
        if words.is_empty() {
            return Reply::Text(note("type ? and the words to look for, e.g. ?compress"));
        }
        return Reply::Text(tools_text(&words, ctx.tools()));
    }
    if line == "why" {
        return Reply::Text(match last {
            Some(done) => why_text(done),
            None => note("nothing has finished in the followed terminal yet, so there is nothing to explain"),
        });
    }
    let known = |word: &str| ctx.matcher.knows_program(word) || crate::COMMON_PROGRAMS.contains(&word);
    if let Some(command) = crate::guard::command_line_request(line, &known) {
        let explanation = crate::explain::explain_command(ctx.everything, command);
        return Reply::Text(format!("{}\n", crate::render::render(&explanation)));
    }
    // Plain words: the same suggestions as `huh "..."`, without questions.
    let outcome = crate::suggest_in(ctx.mode, &ctx.matcher, line, ctx.files.clone(), &mut |_| None);
    Reply::Text(match outcome {
        Ok(outcome) => format!("{}\n", crate::render_outcome(&outcome, ctx.everything, false)),
        Err(error) => note(&format!("huh: {error:#}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: &str = "\x1b]7770;huh;start\x07";
    const END: &str = "\x1b]7770;huh;end\x07";

    fn catalog() -> Vec<Command> {
        crate::catalog::load_embedded().unwrap()
    }

    /// A fresh folder holding `<pid>.log` and `<pid>.status`.
    fn recording(name: &str) -> (PathBuf, PathBuf) {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/tutor-tests")
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

    #[test]
    fn follower_reports_each_new_command_once() {
        let (dir, log) = recording("follow");
        std::fs::write(&log, "$ ").unwrap();
        std::fs::write(log.with_extension("status"), "0\nold command\n").unwrap();
        let mut follower = Follower::new(&log);
        assert_eq!(follower.poll(), None, "the command recorded before the tutor started is not replayed");
        finish(&log, "a\r\nb\r\n", 0, "echo hi");
        let done = follower.poll().expect("finished command");
        assert_eq!((done.command.as_str(), done.exit, done.raw.as_str()), ("echo hi", Some(0), "a\r\nb\r\n"));
        assert_eq!(follower.poll(), None, "reported once");
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
        // A full-screen program keeps writing to the busy log, but no command finishes there.
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
    fn no_recording_says_how_to_enable_it() {
        let text = how_to_record();
        assert!(text.contains("huh shell-init") && text.contains("README"));
    }

    #[test]
    fn programs_skip_wrappers_and_cover_pipelines() {
        assert_eq!(programs("sudo ls -l | grep x"), vec!["ls", "grep"]);
        assert_eq!(programs("FOO=1 make && echo ok"), vec!["make", "echo"]);
    }

    // --- typed lines -----------------------------------------------------------------------

    fn tool(name: &str, description: &str) -> Tool {
        Tool { name: name.into(), description: description.into() }
    }

    /// Answers `line` with a fixed list of installed programs and the folder's files.
    fn answer(line: &str, mode: Mode, last: Option<&Finished>, tools: Vec<Tool>) -> Reply {
        let everything = catalog();
        let usable = crate::catalog::usable(everything.clone(), &["arch".to_string()], false);
        let ctx = Input::new(&everything, &usable, mode, vec![], Some(tools));
        answer_line(line, &ctx, last)
    }

    fn text(reply: Reply) -> String {
        match reply {
            Reply::Text(text) => text,
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
        // A request is not mistaken for a command line, and a refusal shows its reason.
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
    fn a_question_mark_filters_the_installed_programs() {
        let tools = vec![
            tool("gzip", "Compress or expand files"),
            tool("ping", "send ICMP echo to a network host"),
            tool("zstd", "Compress files"),
        ];
        let found = text(answer("?compress", Mode::Normal, None, tools.clone()));
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
        // `why` alone is the word; a request that starts with it is plain words.
        assert!(!say("why is the sky blue", Some(&bad)).starts_with("$ cat"));
    }

    #[test]
    fn an_empty_line_shows_the_last_block_in_full() {
        let done = df_output();
        let again = say("   ", Some(&done));
        assert_eq!(again, block(&catalog(), &done, None));
        assert!(again.starts_with("$ df -h  (exit 0)\n") && again.contains("Mounted on"), "{again}");
        // Under --quiet a program is explained once; the repeat is still in full.
        let echo = Finished { command: "echo hi".into(), exit: Some(0), raw: "hi\r\n".into() };
        let mut seen = HashSet::new();
        block(&catalog(), &echo, Some(&mut seen));
        assert_eq!(block(&catalog(), &echo, Some(&mut seen)), "$ echo hi  (exit 0)\n\n");
        assert!(say("", Some(&echo)).contains("Print the given text"));
        assert_eq!(say("", Some(&done)), again);
        assert!(say("", None).contains("nothing has finished"));
    }

    #[test]
    fn find_opens_the_finder_with_the_words() {
        let reply = |line: &str| answer(line, Mode::Normal, None, vec![]);
        assert_eq!(reply("/find"), Reply::Find(vec![]));
        assert_eq!(reply("/find  compress   files "), Reply::Find(vec!["compress".into(), "files".into()]));
        assert_eq!(reply("/find\tx"), Reply::Find(vec!["x".into()]));
        // Not the command: another word that starts the same way.
        assert!(matches!(reply("/finder"), Reply::Text(_)));
    }

    #[test]
    fn help_lists_what_can_be_typed_in_five_lines() {
        for line in ["help", "/help", "  help  "] {
            let help = say(line, None);
            assert_eq!(help.trim_end().lines().count(), 5, "{help}");
            for word in ["explain", "?word", "why", "Enter", "/find"] {
                assert!(help.contains(word), "{word}: {help}");
            }
        }
    }

    #[test]
    fn nothing_typed_is_ever_run() {
        // Every branch only returns text (or a request to open the finder); a dangerous line is
        // explained or refused, not executed.
        for line in ["rm -rf ~", "sudo dd if=/dev/zero of=/dev/sda", "delete everything", "why", "?rm", "!!", "/"] {
            assert!(matches!(answer(line, Mode::Normal, None, vec![]), Reply::Text(_)), "{line}");
        }
    }
}
