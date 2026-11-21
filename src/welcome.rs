//! The startup animation and the short welcome: a small "huh" logo that draws itself and turns its
//! "?" into "!" in well under a second, then at most five lines on how to use huh.
//!
//! It plays when the `huh` companion pane starts, the first time a person runs huh in a terminal, and on
//! `huh welcome`. It never plays into a pipe, with `--first-line-only`, in a terminal narrower than
//! the art, with `TERM=dumb`, `HUH_NO_ANIMATION=1` or `animation = false` in the config file.
//! Any key press skips the rest of the animation.

use crate::style::{self, Role};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Lines of every frame.
pub const HEIGHT: usize = 5;

const H: [&str; 4] = ["█   ", "█▀▀█", "█  █", "▀  ▀"];
const U: [&str; 4] = ["    ", "█  █", "█  █", "▀▀▀▀"];
const QUESTION: [&str; 4] = ["▄▀▀▄", "  ▄▀", "    ", "  ▀ "];
const BANG: [&str; 4] = [" ██ ", " ██ ", "    ", " ▀▀ "];
/// Columns of the logo itself: three letters of four columns and two gaps.
const LOGO_WIDTH: usize = 14;
const INDENT: &str = "  ";

/// What the animation is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// The unified `huh` companion pane starting.
    Pane,
    /// The first interactive run of huh.
    FirstRun,
    /// `huh welcome`.
    Welcome,
}

fn logo_row(row: usize, columns: usize) -> String {
    let full = format!("{} {} {}", H[row], U[row], H[row]);
    full.chars().take(columns).collect()
}

fn frame(columns: usize, glyph: Option<&[&str; 4]>, caption: bool) -> Vec<String> {
    let mut lines = Vec::with_capacity(HEIGHT);
    for row in 0..4 {
        let mut line = format!("{INDENT}{:<LOGO_WIDTH$}", logo_row(row, columns));
        if let Some(glyph) = glyph {
            line.push_str("  ");
            line.push_str(glyph[row]);
        }
        lines.push(line.trim_end().to_string());
    }
    lines.push(if caption {
        format!("{INDENT}huh {} · nothing is ever run", env!("CARGO_PKG_VERSION"))
    } else {
        String::new()
    });
    lines
}

/// Every frame of the animation, plain text, each `HEIGHT` lines tall: the letters are drawn in from
/// the left, then a "?" appears and turns into "!", and the last frame adds a caption.
pub fn frames() -> Vec<Vec<String>> {
    let mut all: Vec<Vec<String>> = (1..=LOGO_WIDTH.div_ceil(2)).map(|n| frame(n * 2, None, false)).collect();
    all.push(frame(LOGO_WIDTH, Some(&QUESTION), false));
    all.push(frame(LOGO_WIDTH, Some(&BANG), false));
    all.push(frame(LOGO_WIDTH, Some(&BANG), true));
    all
}

/// How long frame `n` stays on screen. The whole animation takes about 0.6 s.
pub fn frame_time(n: usize, total: usize) -> Duration {
    Duration::from_millis(match n {
        n if n + 3 < total => 45,
        n if n + 1 < total => 150,
        _ => 0,
    })
}

/// The lines printed after the animation: what huh is, three examples, and one more line.
pub fn welcome_lines(trigger: Trigger) -> Vec<String> {
    let last = match trigger {
        Trigger::Pane => "Pane: live watch + /mode lab lessons — /help for commands, Ctrl+C to quit",
        _ => "More: huh, huh setup, huh themes, huh --help",
    };
    vec![
        "huh turns plain words into terminal commands, and explains them.".to_string(),
        "  huh \"free up disk space\"".to_string(),
        "  huh explain \"tar -xzf x.tar.gz\"".to_string(),
        "  cmd 2>&1 | huh why".to_string(),
        last.to_string(),
    ]
}

// ---------------------------------------------------------------------------
// Deciding
// ---------------------------------------------------------------------------

/// Everything the decision depends on.
#[derive(Clone, Debug, Default)]
pub struct Inputs {
    pub stdout_tty: bool,
    pub stdin_tty: bool,
    pub width: Option<usize>,
    pub term_dumb: bool,
    /// `HUH_NO_ANIMATION` is set to something other than empty or 0.
    pub env_off: bool,
    /// `animation = false` in the config file.
    pub config_off: bool,
    pub first_line_only: bool,
}

pub fn art_width() -> usize {
    frames().iter().flatten().map(|l| l.chars().count()).max().unwrap_or(0)
}

/// Whether to play the animation. The first run plays only if it was never shown before.
pub fn should_animate(inputs: &Inputs, trigger: Trigger, welcomed: bool) -> bool {
    if !inputs.stdout_tty
        || !inputs.stdin_tty
        || inputs.first_line_only
        || inputs.term_dumb
        || inputs.env_off
        || inputs.config_off
    {
        return false;
    }
    if !inputs.width.is_some_and(|w| w >= art_width()) {
        return false;
    }
    !(trigger == Trigger::FirstRun && welcomed)
}

/// `animation = false` from the text of the config file; anything else (or an unreadable file) keeps it on.
pub fn animation_off_in_config(text: &str) -> bool {
    toml::from_str::<toml::Table>(text)
        .ok()
        .and_then(|t| t.get("animation").and_then(toml::Value::as_bool))
        .is_some_and(|on| !on)
}

/// `$XDG_STATE_HOME/huh/welcomed`, or `~/.local/state/huh/welcomed`.
pub fn welcomed_path(xdg_state: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    match xdg_state.filter(|v| !v.is_empty()) {
        Some(dir) => Some(Path::new(dir).join("huh/welcomed")),
        None => Some(Path::new(home.filter(|v| !v.is_empty())?).join(".local/state/huh/welcomed")),
    }
}

pub fn is_welcomed(path: &Path) -> bool {
    path.exists()
}

/// Records that the welcome was shown. Failing to write is not an error: the welcome just shows again.
pub fn mark_welcomed(path: &Path) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, format!("{}\n", env!("CARGO_PKG_VERSION")));
}

fn real_inputs(first_line_only: bool) -> Inputs {
    use std::io::IsTerminal;
    let config_off = crate::mode::config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|text| animation_off_in_config(&text));
    Inputs {
        stdout_tty: std::io::stdout().is_terminal(),
        stdin_tty: std::io::stdin().is_terminal(),
        width: terminal::width(),
        term_dumb: std::env::var("TERM").is_ok_and(|t| t == "dumb"),
        env_off: std::env::var("HUH_NO_ANIMATION").is_ok_and(|v| !v.is_empty() && v != "0"),
        config_off,
        first_line_only,
    }
}

// ---------------------------------------------------------------------------
// Playing
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    /// Nothing was shown (and nothing was due).
    No,
    Played,
    /// Ctrl+C arrived while playing; the terminal is restored. The caller should stop.
    Interrupted,
}

pub fn print_welcome(trigger: Trigger) {
    let mut out = std::io::stdout().lock();
    for (n, line) in welcome_lines(trigger).iter().enumerate() {
        let role = match n {
            0 => Role::Description,
            1..=3 => Role::Command,
            _ => Role::Dim,
        };
        let _ = writeln!(out, "{}", style::paint(role, line));
    }
    let _ = writeln!(out);
    let _ = out.flush();
}

fn animate() -> Shown {
    let all = frames();
    let guard = terminal::Guard::new();
    let mut out = std::io::stdout().lock();
    let _ = write!(out, "\x1b[?25l");
    for (n, lines) in all.iter().enumerate() {
        if n > 0 {
            let _ = write!(out, "\x1b[{HEIGHT}A");
        }
        for (row, line) in lines.iter().enumerate() {
            let role = if row == HEIGHT - 1 { Role::Dim } else { Role::Accent };
            let _ = writeln!(out, "\r\x1b[2K{}", style::paint(role, line));
        }
        let _ = out.flush();
        match guard.wait(frame_time(n, all.len())) {
            terminal::Wait::Elapsed => {}
            terminal::Wait::Key => {
                // Skip to the end: the last frame is what stays on screen.
                if n + 1 < all.len() {
                    let _ = write!(out, "\x1b[{HEIGHT}A");
                    for (row, line) in all[all.len() - 1].iter().enumerate() {
                        let role = if row == HEIGHT - 1 { Role::Dim } else { Role::Accent };
                        let _ = writeln!(out, "\r\x1b[2K{}", style::paint(role, line));
                    }
                }
                break;
            }
            terminal::Wait::Interrupted => {
                let _ = writeln!(out);
                return Shown::Interrupted;
            }
        }
    }
    let _ = out.flush();
    Shown::Played
}

/// Plays the animation and the welcome if `trigger` allows it right now. With `text_always` the
/// welcome lines are printed even when the animation is not (that is `huh welcome`).
pub fn show(trigger: Trigger, first_line_only: bool, text_always: bool) -> Shown {
    show_with(trigger, first_line_only, text_always, true)
}

/// `show`, but with `print_text` false the five welcome lines are left to the caller (the companion
/// pane prints them itself once its header is pinned). `Played` still says whether they are due.
pub fn show_with(trigger: Trigger, first_line_only: bool, text_always: bool, print_text: bool) -> Shown {
    let path = welcomed_path(std::env::var("XDG_STATE_HOME").ok().as_deref(), std::env::var("HOME").ok().as_deref());
    let welcomed = path.as_deref().is_some_and(is_welcomed);
    let inputs = real_inputs(first_line_only);
    let play = should_animate(&inputs, trigger, welcomed);
    if play && animate() == Shown::Interrupted {
        return Shown::Interrupted;
    }
    if (play || text_always) && print_text {
        print_welcome(trigger);
    }
    if play
        && trigger == Trigger::FirstRun
        && let Some(path) = &path
    {
        mark_welcomed(path);
    }
    if play || text_always { Shown::Played } else { Shown::No }
}

/// Raw terminal input just long enough to notice a key press; always put back afterwards.
#[cfg(target_os = "linux")]
mod terminal {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Termios {
        iflag: u32,
        oflag: u32,
        cflag: u32,
        lflag: u32,
        line: u8,
        cc: [u8; 32],
        ispeed: u32,
        ospeed: u32,
    }

    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }

    #[repr(C)]
    #[derive(Default)]
    struct WinSize {
        rows: u16,
        cols: u16,
        x: u16,
        y: u16,
    }

    unsafe extern "C" {
        fn tcgetattr(fd: i32, term: *mut Termios) -> i32;
        fn tcsetattr(fd: i32, action: i32, term: *const Termios) -> i32;
        fn poll(fds: *mut PollFd, count: usize, timeout_ms: i32) -> i32;
        fn read(fd: i32, buf: *mut std::ffi::c_void, count: usize) -> isize;
        fn ioctl(fd: i32, request: usize, ...) -> i32;
        fn signal(signum: i32, handler: usize) -> usize;
    }

    const ICANON: u32 = 0x2;
    const ECHO: u32 = 0x8;
    const TCSANOW: i32 = 0;
    const POLLIN: i16 = 1;
    const TIOCGWINSZ: usize = 0x5413;
    const SIGINT: i32 = 2;

    static INTERRUPTED: AtomicBool = AtomicBool::new(false);

    extern "C" fn on_interrupt(_: i32) {
        INTERRUPTED.store(true, Ordering::Relaxed);
    }

    pub enum Wait {
        Elapsed,
        Key,
        Interrupted,
    }

    pub fn width() -> Option<usize> {
        let mut size = WinSize::default();
        // SAFETY: TIOCGWINSZ writes a `winsize` struct, which `WinSize` matches.
        let ok = unsafe { ioctl(1, TIOCGWINSZ, &mut size as *mut WinSize) } == 0 && size.cols > 0;
        if ok { Some(size.cols as usize) } else { std::env::var("COLUMNS").ok()?.trim().parse().ok() }
    }

    /// Line editing and echo off while it lives (Ctrl+C still raises SIGINT, which is noticed);
    /// the cursor, colours, terminal mode and SIGINT handling are restored on drop.
    pub struct Guard {
        saved: Option<Termios>,
    }

    impl Guard {
        pub fn new() -> Guard {
            INTERRUPTED.store(false, Ordering::Relaxed);
            let mut term =
                Termios { iflag: 0, oflag: 0, cflag: 0, lflag: 0, line: 0, cc: [0; 32], ispeed: 0, ospeed: 0 };
            // SAFETY: plain libc calls on stdin with a correctly laid out `termios`; the handler
            // only stores into an atomic.
            let saved = unsafe {
                signal(SIGINT, on_interrupt as *const () as usize);
                if tcgetattr(0, &mut term) == 0 {
                    let saved = term;
                    term.lflag &= !(ICANON | ECHO);
                    (tcsetattr(0, TCSANOW, &term) == 0).then_some(saved)
                } else {
                    None
                }
            };
            Guard { saved }
        }

        /// Waits up to `time` for a key press (which is read and dropped) or Ctrl+C.
        pub fn wait(&self, time: Duration) -> Wait {
            if INTERRUPTED.load(Ordering::Relaxed) {
                return Wait::Interrupted;
            }
            if self.saved.is_none() {
                std::thread::sleep(time);
                return if INTERRUPTED.load(Ordering::Relaxed) { Wait::Interrupted } else { Wait::Elapsed };
            }
            let mut fd = PollFd { fd: 0, events: POLLIN, revents: 0 };
            // SAFETY: `fd` is a valid pollfd; the buffer is 64 bytes and `read` gets that length.
            let ready = unsafe { poll(&mut fd, 1, time.as_millis() as i32) };
            if ready > 0 {
                let mut buf = [0u8; 64];
                let got = unsafe { read(0, buf.as_mut_ptr().cast(), buf.len()) };
                if got > 0 {
                    return Wait::Key;
                }
                // End of input is not a key press; just let the frame run its time.
                std::thread::sleep(time);
            }
            if INTERRUPTED.load(Ordering::Relaxed) { Wait::Interrupted } else { Wait::Elapsed }
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            use std::io::Write;
            // SAFETY: restores what `new` saved; SIG_DFL is 0.
            unsafe {
                if let Some(saved) = &self.saved {
                    tcsetattr(0, TCSANOW, saved);
                }
                signal(SIGINT, 0);
            }
            let mut out = std::io::stdout();
            let _ = write!(out, "\x1b[0m\x1b[?25h");
            let _ = out.flush();
        }
    }
}

/// Elsewhere there is no raw mode: the animation just runs its short course.
#[cfg(not(target_os = "linux"))]
mod terminal {
    use std::time::Duration;

    pub enum Wait {
        Elapsed,
        Key,
        Interrupted,
    }

    pub fn width() -> Option<usize> {
        std::env::var("COLUMNS").ok()?.trim().parse().ok()
    }

    pub struct Guard;

    impl Guard {
        pub fn new() -> Guard {
            Guard
        }
        pub fn wait(&self, time: Duration) -> Wait {
            std::thread::sleep(time);
            Wait::Elapsed
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            use std::io::Write;
            let mut out = std::io::stdout();
            let _ = write!(out, "\x1b[0m\x1b[?25h");
            let _ = out.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The widest the art may be.
    const MAX_WIDTH: usize = 50;

    fn tty() -> Inputs {
        Inputs { stdout_tty: true, stdin_tty: true, width: Some(80), ..Inputs::default() }
    }

    #[test]
    fn frames_are_small_and_end_on_the_logo() {
        let all = frames();
        assert_eq!(all.len(), 10);
        for f in &all {
            assert_eq!(f.len(), HEIGHT);
            assert!(f.len() <= 7);
            for line in f {
                assert!(line.chars().count() <= MAX_WIDTH, "{line}");
            }
        }
        assert!(art_width() <= MAX_WIDTH);
        let final_frame = all.last().unwrap();
        let row0_chars: Vec<char> = final_frame[0].chars().collect();
        assert_eq!(row0_chars.get(2), Some(&'█'), "left h ascender: {:?}", final_frame[0]);
        assert_eq!(row0_chars.get(12), Some(&'█'), "right h ascender: {:?}", final_frame[0]);
        assert_eq!(BANG[0], " ██ ");
        let last = final_frame.join("\n");
        assert!(last.contains("huh"), "{last}");
        assert!(last.contains("█") && last.contains(" ██"), "the logo and the !");
        // The letters are drawn in: each frame shows at least as much as the one before.
        let ink = |f: &Vec<String>| f.iter().map(|l| l.chars().filter(|c| *c != ' ').count()).sum::<usize>();
        assert!(all[..8].windows(2).all(|w| ink(&w[0]) <= ink(&w[1])));
        assert!(ink(&all[0]) < ink(&all[6]));
        // The "?" turns into "!".
        assert_ne!(all[7], all[8]);
    }

    #[test]
    fn the_animation_is_quick() {
        let total: Duration = (0..frames().len()).map(|n| frame_time(n, frames().len())).sum();
        assert!(total < Duration::from_millis(800), "{total:?}");
    }

    #[test]
    fn welcome_is_short() {
        for trigger in [Trigger::Pane, Trigger::FirstRun, Trigger::Welcome] {
            let lines = welcome_lines(trigger);
            assert!(lines.len() <= 5);
            let text = lines.join("\n");
            for example in ["huh \"free up disk space\"", "huh explain \"tar -xzf x.tar.gz\"", "cmd 2>&1 | huh why"] {
                assert!(text.contains(example), "{example}");
            }
        }
        let pane = welcome_lines(Trigger::Pane).join("\n");
        assert!(pane.contains("live watch + /mode lab lessons") && pane.contains("Ctrl+C to quit"), "{pane}");
        assert!(!pane.contains("huh tutor"), "{pane}");
        let welcome = welcome_lines(Trigger::Welcome).join("\n");
        assert!(welcome.contains("More: huh, huh setup, huh themes, huh --help"), "{welcome}");
        assert!(!welcome.contains("Ctrl+C"));
    }

    #[test]
    fn animates_on_a_terminal() {
        for trigger in [Trigger::Pane, Trigger::FirstRun, Trigger::Welcome] {
            assert!(should_animate(&tty(), trigger, false), "{trigger:?}");
        }
    }

    #[test]
    fn never_animates_when_something_forbids_it() {
        let cases: Vec<(&str, Inputs)> = vec![
            ("stdout not a tty", Inputs { stdout_tty: false, ..tty() }),
            ("stdin not a tty", Inputs { stdin_tty: false, ..tty() }),
            ("first-line-only", Inputs { first_line_only: true, ..tty() }),
            ("TERM=dumb", Inputs { term_dumb: true, ..tty() }),
            ("HUH_NO_ANIMATION", Inputs { env_off: true, ..tty() }),
            ("config", Inputs { config_off: true, ..tty() }),
            ("unknown width", Inputs { width: None, ..tty() }),
            ("too narrow", Inputs { width: Some(art_width() - 1), ..tty() }),
        ];
        for (why, inputs) in cases {
            for trigger in [Trigger::Pane, Trigger::FirstRun, Trigger::Welcome] {
                assert!(!should_animate(&inputs, trigger, false), "{why} {trigger:?}");
            }
        }
        assert!(should_animate(&Inputs { width: Some(art_width()), ..tty() }, Trigger::Pane, false));
    }

    #[test]
    fn first_run_plays_once_but_the_others_replay() {
        assert!(!should_animate(&tty(), Trigger::FirstRun, true));
        assert!(should_animate(&tty(), Trigger::Pane, true));
        assert!(should_animate(&tty(), Trigger::Welcome, true));
    }

    #[test]
    fn config_switch() {
        assert!(animation_off_in_config("animation = false\n"));
        assert!(!animation_off_in_config("animation = true\n"));
        assert!(!animation_off_in_config("mode = \"expert\"\n"));
        assert!(!animation_off_in_config("animation = \"no\"\n"));
        assert!(!animation_off_in_config("not toml ["));
    }

    #[test]
    fn welcomed_file_logic() {
        assert_eq!(welcomed_path(Some("/s"), Some("/h")).unwrap(), Path::new("/s/huh/welcomed"));
        assert_eq!(welcomed_path(None, Some("/h")).unwrap(), Path::new("/h/.local/state/huh/welcomed"));
        assert_eq!(welcomed_path(Some(""), Some("/h")).unwrap(), Path::new("/h/.local/state/huh/welcomed"));
        assert_eq!(welcomed_path(None, None), None);
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(".scratch").join("welcome-tests");
        let _ = std::fs::remove_dir_all(&dir);
        let path = welcomed_path(dir.to_str(), None).unwrap();
        assert!(!is_welcomed(&path));
        mark_welcomed(&path);
        assert!(is_welcomed(&path));
        // Marking again, or into a place that cannot be written, never fails.
        mark_welcomed(&path);
        mark_welcomed(Path::new("/proc/huh/welcomed"));
    }
}
