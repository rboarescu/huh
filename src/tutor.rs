//! `huh tutor`: follows the recording of the terminal next to it (see session.rs) and prints,
//! for each finished command, what it does and what its output or failure means.
//! It only reads the recording and the status file. It never runs anything.

use crate::breakdown::{Block, Explanation, Row};
use crate::catalog::Command;
use crate::session;
use anyhow::{Result, bail};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

const POLL: Duration = Duration::from_millis(300);
/// Polls to wait for the end marker to reach the log after the status file changed (about 3 s).
const PATIENCE: u32 = 10;
/// Most flag or argument rows shown for a command.
const MAX_ROWS: usize = 5;

static STOP: AtomicBool = AtomicBool::new(false);

/// One finished command, as recorded.
#[derive(Debug, PartialEq)]
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
fn tty_of(log: &Path) -> Option<String> {
    let pid = log.file_stem()?;
    std::fs::read_link(Path::new("/proc").join(pid).join("fd/0")).ok().map(|t| t.display().to_string())
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
    recordings(dir, None, true).into_iter().find(|log| {
        let pid = log.file_stem().unwrap_or_default();
        std::fs::read_link(Path::new("/proc").join(pid).join("fd/0")).is_ok_and(|t| t == Path::new(tty))
    })
}

fn locate(tty: Option<&str>) -> Option<PathBuf> {
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
        crate::render::hanging(&mut out, 2, "! ", &format!("[{}] {}", w.risk.name(), w.text));
    }
    out
}

const ALT_SCREEN: [&str; 3] = ["\x1b[?1049h", "\x1b[?1047h", "\x1b[?47h"];

/// Everything printed for one finished command (ends with a blank line).
pub fn block(catalog: &[Command], done: &Finished, explained: Option<&mut HashSet<String>>) -> String {
    let exit = done.exit.map_or("?".to_string(), |c| c.to_string());
    let mut out = format!("$ {}  (exit {exit})\n", done.command);
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
        out.push_str("  full-screen program, nothing to explain\n\n");
        return out;
    }
    let cleaned = session::clean(&done.raw);
    if done.exit.is_some_and(|c| c != 0) {
        let last = session::Last { command: done.command.clone(), exit: done.exit, output: session::tail(&cleaned) };
        let mut answer = String::new();
        let _ = crate::doctor::explain(&last, &mut |text| answer = text.to_string());
        out.push_str(&format!("\n{}\n", answer.trim_end()));
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
pub fn run(catalog: &[Command], quiet: bool, tty: Option<&str>) -> Result<()> {
    use std::io::Write;
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
    catch_ctrl_c();
    let tty_name = tty_of(&log).unwrap_or_else(|| log.display().to_string());
    println!(
        "huh tutor: following {tty_name}, the terminal that finished a command most recently (Ctrl+C stops). It only reads."
    );
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
    println!();
    let mut follower = Follower::new(&log);
    let mut explained = quiet.then(HashSet::new);
    while !STOP.load(Ordering::Relaxed) {
        if let Some(done) = follower.poll() {
            print!("{}", block(catalog, &done, explained.as_mut()));
            std::io::stdout().flush().ok();
        }
        std::thread::sleep(POLL);
    }
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
}
