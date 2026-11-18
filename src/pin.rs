//! The tutor pane's pinned header: the "huh" logo and one status line stay at the top of the
//! terminal while everything the tutor prints scrolls underneath, using a terminal scroll region
//! (DECSTBM, `ESC[top;bottom r`) so the header is drawn once and never scrolls away.
//!
//! It is only used on a terminal at least `MIN_ROWS` tall (and wide enough for the logo), never with
//! `TERM=dumb`, `HUH_NO_PIN=1` or `pin = false` in the config file. The escape sequences are built by
//! the pure functions below; the `Guard` writes them and always puts the terminal back on drop.
//!
//! Cursor handling: the first draw clears the screen and leaves the cursor on the bottom row of the
//! region. A redraw (resize, or after `huh find`) saves the cursor with `ESC 7`, draws the header
//! with absolute positions, sets the region (which homes the cursor) and restores it with `ESC 8`,
//! so the text and the prompt below the header are not disturbed. Nothing else in the tutor uses
//! the saved-cursor slot.

use crate::style::{self, Role};
use crate::welcome;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// The terminal must be at least this tall for the header to be pinned.
pub const MIN_ROWS: u16 = 15;

/// Everything the decision depends on.
#[derive(Clone, Debug, Default)]
pub struct Inputs {
    pub stdout_tty: bool,
    pub rows: Option<u16>,
    pub cols: Option<u16>,
    pub term_dumb: bool,
    /// `HUH_NO_PIN` is set to something other than empty or 0.
    pub env_off: bool,
    /// `pin = false` in the config file.
    pub config_off: bool,
}

/// Whether to pin the header.
pub fn should_pin(inputs: &Inputs) -> bool {
    inputs.stdout_tty
        && !inputs.term_dumb
        && !inputs.env_off
        && !inputs.config_off
        && inputs.rows.is_some_and(|r| r >= MIN_ROWS)
        && inputs.cols.is_some_and(|c| c as usize >= welcome::art_width())
}

/// `pin = false` from the text of the config file; anything else (or an unreadable file) keeps it on.
pub fn pin_off_in_config(text: &str) -> bool {
    toml::from_str::<toml::Table>(text)
        .ok()
        .and_then(|t| t.get("pin").and_then(toml::Value::as_bool))
        .is_some_and(|on| !on)
}

fn real_inputs() -> Inputs {
    use std::io::IsTerminal;
    let size = size();
    Inputs {
        stdout_tty: std::io::stdout().is_terminal(),
        rows: size.map(|s| s.0),
        cols: size.map(|s| s.1),
        term_dumb: std::env::var("TERM").is_ok_and(|t| t == "dumb"),
        env_off: std::env::var("HUH_NO_PIN").is_ok_and(|v| !v.is_empty() && v != "0"),
        config_off: crate::mode::config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .is_some_and(|text| pin_off_in_config(&text)),
    }
}

/// Whether the header would be pinned right now in this terminal.
pub fn wanted() -> bool {
    should_pin(&real_inputs())
}

// ---------------------------------------------------------------------------
// Escape sequences (pure)
// ---------------------------------------------------------------------------

/// Lines of the header: the logo and its caption (the last frame of the welcome animation), then
/// one status line. Each is cut to `cols` so that it never wraps.
pub fn header_lines(cols: usize, tty: &str) -> Vec<String> {
    let clip = |text: &str| text.chars().take(cols).collect::<String>();
    let mut lines: Vec<String> = welcome::frames()
        .pop()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(row, line)| {
            let role = if row == welcome::HEIGHT - 1 { Role::Dim } else { Role::Accent };
            style::paint(role, &clip(line))
        })
        .collect();
    lines.push(style::paint(Role::Dim, &clip(&format!("  following {tty} · huh> to ask · Ctrl+C quits"))));
    lines
}

/// Each line written on its own row from the top, with absolute positions (no newlines, so nothing scrolls).
fn draw(lines: &[String]) -> String {
    let mut out = String::new();
    for (n, line) in lines.iter().enumerate() {
        out.push_str(&format!("\x1b[{};1H\x1b[2K{line}", n + 1));
    }
    out
}

/// Scroll region below a header of `header` lines on a terminal of `rows` rows. Setting it homes the cursor.
pub fn region_sequence(header: usize, rows: u16) -> String {
    format!("\x1b[{};{}r", header + 1, rows)
}

/// The first draw: clear the screen, draw the header, set the region and put the cursor on its
/// bottom row, where the prompt goes.
pub fn first_sequence(lines: &[String], rows: u16) -> String {
    format!("\x1b[2J{}{}\x1b[{rows};1H", draw(lines), region_sequence(lines.len(), rows))
}

/// A redraw from inside the region: the cursor is saved, restored afterwards and so ends up where it was.
pub fn redraw_sequence(lines: &[String], rows: u16) -> String {
    format!("\x1b7{}{}\x1b8", draw(lines), region_sequence(lines.len(), rows))
}

/// Region off, cursor on the bottom row (resetting the region homes the cursor, into the header).
pub fn unpin_sequence(rows: u16) -> String {
    format!("\x1b[r\x1b[{rows};1H")
}

/// The end: region off, cursor below everything, colours reset, cursor shown.
pub fn reset_sequence(rows: Option<u16>) -> String {
    let below = rows.map(unpin_sequence).unwrap_or_else(|| "\x1b[r".to_string());
    format!("{below}\x1b[0m\x1b[?25h\r\n")
}

// ---------------------------------------------------------------------------
// The terminal
// ---------------------------------------------------------------------------

struct State {
    tty: String,
    rows: u16,
    /// The region is set.
    active: bool,
    /// Taken off for a full-screen program (`huh find`).
    paused: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static RESIZED: AtomicBool = AtomicBool::new(false);

fn state() -> std::sync::MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn fits(size: (u16, u16)) -> bool {
    size.0 >= MIN_ROWS && size.1 as usize >= welcome::art_width()
}

/// While it lives the header is pinned; dropping it resets the scroll region and moves the cursor
/// below. Create it with `start` and keep it until the tutor ends.
pub struct Guard;

impl Guard {
    /// Pins the header now. `None` when the terminal size cannot be read or does not fit.
    pub fn start(tty: &str) -> Option<Guard> {
        let size = size().filter(|s| fits(*s))?;
        let lines = header_lines(size.1 as usize, tty);
        let mut out = std::io::stdout();
        let _ = write!(out, "{}", first_sequence(&lines, size.0));
        let _ = out.flush();
        *state() = Some(State { tty: tty.to_string(), rows: size.0, active: true, paused: false });
        RESIZED.store(false, Ordering::Relaxed);
        watch_resize();
        Some(Guard)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let rows = size().map(|s| s.0).or_else(|| state().as_ref().map(|s| s.rows));
        *state() = None;
        unwatch_resize();
        let mut out = std::io::stdout();
        let _ = write!(out, "{}", reset_sequence(rows));
        let _ = out.flush();
    }
}

/// If the terminal was resized since the last call, the sequence that redraws the header for the
/// new size (or takes the region off when the terminal became too small). Call it from the loop.
pub fn on_resize() -> Option<String> {
    if !RESIZED.swap(false, Ordering::Relaxed) {
        return None;
    }
    let size = size()?;
    let mut guard = state();
    let st = guard.as_mut()?;
    if st.paused {
        return None;
    }
    st.rows = size.0;
    if fits(size) {
        st.active = true;
        Some(redraw_sequence(&header_lines(size.1 as usize, &st.tty), size.0))
    } else if st.active {
        st.active = false;
        Some(unpin_sequence(size.0))
    } else {
        None
    }
}

/// Before a full-screen program takes the terminal (`huh find`): the sequence that turns the region off.
pub fn suspend() -> String {
    let mut guard = state();
    match guard.as_mut() {
        Some(st) if st.active && !st.paused => {
            st.paused = true;
            unpin_sequence(size().map_or(st.rows, |s| s.0))
        }
        _ => String::new(),
    }
}

/// After it: the sequence that draws the header and the region again, for the size the terminal has now.
pub fn resume() -> String {
    let mut guard = state();
    let Some(st) = guard.as_mut().filter(|st| st.paused) else { return String::new() };
    st.paused = false;
    match size().filter(|s| fits(*s)) {
        Some(size) => {
            st.rows = size.0;
            st.active = true;
            redraw_sequence(&header_lines(size.1 as usize, &st.tty), size.0)
        }
        None => {
            st.active = false;
            String::new()
        }
    }
}

/// (rows, columns) of the terminal on standard output.
#[cfg(target_os = "linux")]
fn size() -> Option<(u16, u16)> {
    #[repr(C)]
    #[derive(Default)]
    struct WinSize {
        rows: u16,
        cols: u16,
        x: u16,
        y: u16,
    }
    unsafe extern "C" {
        fn ioctl(fd: i32, request: usize, ...) -> i32;
    }
    const TIOCGWINSZ: usize = 0x5413;
    let mut size = WinSize::default();
    // SAFETY: TIOCGWINSZ writes a `winsize` struct, which `WinSize` matches.
    let ok = unsafe { ioctl(1, TIOCGWINSZ, &mut size as *mut WinSize) } == 0;
    (ok && size.rows > 0 && size.cols > 0).then_some((size.rows, size.cols))
}

#[cfg(not(target_os = "linux"))]
fn size() -> Option<(u16, u16)> {
    None
}

#[cfg(target_os = "linux")]
mod winch {
    use std::sync::atomic::Ordering;

    unsafe extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }
    const SIGWINCH: i32 = 28;

    extern "C" fn on_winch(_: i32) {
        super::RESIZED.store(true, Ordering::Relaxed);
    }

    pub fn watch() {
        // SAFETY: the handler only stores into an atomic.
        unsafe {
            signal(SIGWINCH, on_winch as *const () as usize);
        }
    }

    pub fn unwatch() {
        // SAFETY: 0 is SIG_DFL.
        unsafe {
            signal(SIGWINCH, 0);
        }
    }
}

#[cfg(target_os = "linux")]
fn watch_resize() {
    winch::watch();
}

#[cfg(target_os = "linux")]
fn unwatch_resize() {
    winch::unwatch();
}

#[cfg(not(target_os = "linux"))]
fn watch_resize() {}

#[cfg(not(target_os = "linux"))]
fn unwatch_resize() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn term() -> Inputs {
        Inputs { stdout_tty: true, rows: Some(30), cols: Some(100), ..Inputs::default() }
    }

    #[test]
    fn pins_only_on_a_tall_enough_terminal() {
        assert!(should_pin(&term()));
        assert!(should_pin(&Inputs { rows: Some(MIN_ROWS), ..term() }));
        assert!(!should_pin(&Inputs { rows: Some(MIN_ROWS - 1), ..term() }), "too short");
        assert!(!should_pin(&Inputs { rows: None, ..term() }), "size unknown");
        assert!(!should_pin(&Inputs { cols: Some(10), ..term() }), "too narrow for the logo");
        assert!(!should_pin(&Inputs { stdout_tty: false, ..term() }), "not a terminal");
        assert!(!should_pin(&Inputs { term_dumb: true, ..term() }), "TERM=dumb");
        assert!(!should_pin(&Inputs { env_off: true, ..term() }), "HUH_NO_PIN");
        assert!(!should_pin(&Inputs { config_off: true, ..term() }), "pin = false");
    }

    #[test]
    fn config_switch() {
        assert!(pin_off_in_config("pin = false\n"));
        assert!(!pin_off_in_config("pin = true\n"));
        assert!(!pin_off_in_config("animation = false\n"));
        assert!(!pin_off_in_config("pin = \"no\"\n"));
        assert!(!pin_off_in_config("not toml ["));
    }

    #[test]
    fn header_is_the_logo_and_a_status_line() {
        let lines = header_lines(100, "/dev/pts/7");
        assert_eq!(lines.len(), welcome::HEIGHT + 1);
        assert!(lines.len() <= 6);
        assert!(lines.iter().all(|l| !l.contains('\x1b')), "plain in tests");
        assert!(lines[0..4].iter().any(|l| l.contains('█')));
        assert!(lines[4].contains("huh"));
        assert_eq!(lines[5], "  following /dev/pts/7 · huh> to ask · Ctrl+C quits");
        // Never wider than the terminal.
        for line in header_lines(48, "/dev/pts/12") {
            assert!(line.chars().count() <= 48, "{line}");
        }
    }

    #[test]
    fn region_starts_below_the_header() {
        assert_eq!(region_sequence(6, 30), "\x1b[7;30r");
        assert_eq!(region_sequence(2, 15), "\x1b[3;15r");
    }

    #[test]
    fn first_draw_clears_draws_sets_the_region_and_parks_the_cursor() {
        let lines = vec!["A".to_string(), "B".to_string()];
        let seq = first_sequence(&lines, 20);
        assert_eq!(seq, "\x1b[2J\x1b[1;1H\x1b[2KA\x1b[2;1H\x1b[2KB\x1b[3;20r\x1b[20;1H");
        assert!(!seq.contains('\n'), "nothing may scroll");
    }

    #[test]
    fn redraw_saves_and_restores_the_cursor_and_does_not_clear() {
        let lines = vec!["A".to_string()];
        let seq = redraw_sequence(&lines, 24);
        assert_eq!(seq, "\x1b7\x1b[1;1H\x1b[2KA\x1b[2;24r\x1b8");
        assert!(seq.starts_with("\x1b7") && seq.ends_with("\x1b8"));
        assert!(!seq.contains("\x1b[2J") && !seq.contains('\n'));
    }

    #[test]
    fn reset_turns_the_region_off_and_moves_below() {
        assert_eq!(unpin_sequence(30), "\x1b[r\x1b[30;1H");
        assert_eq!(reset_sequence(Some(30)), "\x1b[r\x1b[30;1H\x1b[0m\x1b[?25h\r\n");
        assert_eq!(reset_sequence(None), "\x1b[r\x1b[0m\x1b[?25h\r\n");
        // The region is reset before the cursor is moved, because resetting homes the cursor.
        let seq = reset_sequence(Some(30));
        assert!(seq.find("\x1b[r").unwrap() < seq.find("\x1b[30;1H").unwrap());
    }
}
