//! Zero-crate Linux raw-mode key decoder, arrow-key inline selector (`select_menu`), and live
//! `/` fuzzy dropdown `LineEditor`.
//!
//! Cursor rule: only `src/pin.rs` uses saved-cursor slots for async header updates. Everything in
//! this module uses purely relative cursor movements (`\r`, `\x1b[2K`, `\x1b[J`, `\n`,
//! `\x1b[{n}A`, `\x1b[{col}G`) so background 220 ms animations and tip rotations never collide.

use crate::style::{self, Role};
use std::io::{BufRead, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::time::Instant;

const TICK_INTERVAL: Duration = Duration::from_millis(220);
const ESC_WAIT_MS: i32 = 25;
const MAX_DROPDOWN_ROWS: usize = 6;
const MAX_HISTORY: usize = 100;

// ---------------------------------------------------------------------------
// Terminal detection & Linux raw-mode RAII guard
// ---------------------------------------------------------------------------

fn parse_env_term_size(lines: Option<&str>, cols: Option<&str>) -> Option<(u16, u16)> {
    let rows = lines?.trim().parse::<u16>().ok()?;
    let cols = cols?.trim().parse::<u16>().ok()?;
    (rows > 0 && cols > 0).then_some((rows, cols))
}

/// Returns `(rows, cols)` of the terminal on stdout via `TIOCGWINSZ`, falling back to
/// `$LINES`/`$COLUMNS` when positive.
#[cfg(target_os = "linux")]
pub fn term_size() -> Option<(u16, u16)> {
    if let Some(size) = sys::winsize() {
        return Some(size);
    }
    parse_env_term_size(std::env::var("LINES").ok().as_deref(), std::env::var("COLUMNS").ok().as_deref())
}

/// Returns `(rows, cols)` from `$LINES`/`$COLUMNS` on non-Linux targets.
#[cfg(not(target_os = "linux"))]
pub fn term_size() -> Option<(u16, u16)> {
    parse_env_term_size(std::env::var("LINES").ok().as_deref(), std::env::var("COLUMNS").ok().as_deref())
}

fn interactive_from(stdin_tty: bool, stdout_tty: bool, term: Option<&str>) -> bool {
    stdin_tty && stdout_tty && term != Some("dumb")
}

/// True when both stdin and stdout are terminals and `TERM` is not `"dumb"`.
pub fn is_interactive_tty() -> bool {
    interactive_from(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
        std::env::var("TERM").ok().as_deref(),
    )
}

/// RAII guard that disables `ICANON | ECHO` on stdin while preserving `ISIG` and output flags,
/// restoring the original `termios` (and previous `SIGINT` handler) on drop.
#[cfg(target_os = "linux")]
pub struct RawModeGuard {
    saved: sys::Termios,
    prev_sigint: usize,
}

#[cfg(target_os = "linux")]
impl RawModeGuard {
    /// Enters non-canonical, no-echo input mode on fd 0. Returns `None` if stdin is not a TTY.
    pub fn new() -> Option<Self> {
        sys::enter_raw_mode().map(|(saved, prev_sigint)| Self { saved, prev_sigint })
    }

    fn is_interrupted(&self) -> bool {
        sys::is_interrupted()
    }
}

#[cfg(target_os = "linux")]
impl Drop for RawModeGuard {
    fn drop(&mut self) {
        sys::exit_raw_mode(&self.saved, self.prev_sigint);
    }
}

/// Safe no-op raw-mode stub on non-Linux targets.
#[cfg(not(target_os = "linux"))]
pub struct RawModeGuard;

#[cfg(not(target_os = "linux"))]
impl RawModeGuard {
    /// Always returns `None` on non-Linux targets.
    pub fn new() -> Option<Self> {
        None
    }
}

#[cfg(target_os = "linux")]
mod sys {
    use std::sync::atomic::{AtomicBool, Ordering};

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Termios {
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
    const POLLERR: i16 = 8;
    const POLLHUP: i16 = 16;
    const TIOCGWINSZ: usize = 0x5413;
    const SIGINT: i32 = 2;

    static INTERRUPTED: AtomicBool = AtomicBool::new(false);

    extern "C" fn on_sigint(_: i32) {
        INTERRUPTED.store(true, Ordering::Relaxed);
    }

    pub fn is_interrupted() -> bool {
        INTERRUPTED.load(Ordering::Relaxed)
    }

    pub fn winsize() -> Option<(u16, u16)> {
        let mut ws = WinSize::default();
        // SAFETY: `TIOCGWINSZ` writes a `winsize` struct matching `WinSize` on stdout fd 1.
        let ok = unsafe { ioctl(1, TIOCGWINSZ, &mut ws as *mut WinSize) } == 0;
        (ok && ws.rows > 0 && ws.cols > 0).then_some((ws.rows, ws.cols))
    }

    pub fn enter_raw_mode() -> Option<(Termios, usize)> {
        let mut term = Termios { iflag: 0, oflag: 0, cflag: 0, lflag: 0, line: 0, cc: [0; 32], ispeed: 0, ospeed: 0 };
        // SAFETY: `term` matches Linux `struct termios`; we clear only `ICANON | ECHO` on `lflag`.
        unsafe {
            if tcgetattr(0, &mut term) != 0 {
                return None;
            }
            let saved = term;
            term.lflag &= !(ICANON | ECHO);
            if tcsetattr(0, TCSANOW, &term) != 0 {
                return None;
            }
            INTERRUPTED.store(false, Ordering::Relaxed);
            let prev = signal(SIGINT, on_sigint as *const () as usize);
            if prev == 1 {
                signal(SIGINT, 1);
            }
            Some((saved, prev))
        }
    }

    pub fn exit_raw_mode(saved: &Termios, prev_sigint: usize) {
        // SAFETY: restores the saved `termios` and previous `SIGINT` disposition on drop.
        unsafe {
            tcsetattr(0, TCSANOW, saved);
            if prev_sigint != usize::MAX && prev_sigint != 1 {
                signal(SIGINT, prev_sigint);
            }
            if INTERRUPTED.swap(false, Ordering::Relaxed)
                && prev_sigint > 1
                && prev_sigint != on_sigint as *const () as usize
                && prev_sigint != usize::MAX
            {
                let prev_fn: extern "C" fn(i32) = std::mem::transmute(prev_sigint);
                prev_fn(SIGINT);
            }
        }
    }

    pub enum PollState {
        Ready,
        Timeout,
        Interrupted,
        Error(std::io::Error),
    }

    pub fn poll_stdin(timeout_ms: i32) -> PollState {
        if INTERRUPTED.load(Ordering::Relaxed) {
            return PollState::Interrupted;
        }
        let mut pfd = PollFd { fd: 0, events: POLLIN, revents: 0 };
        // SAFETY: `pfd` is a valid stack `PollFd` targeting stdin fd 0.
        let rc = unsafe { poll(&mut pfd, 1, timeout_ms.max(0)) };
        if INTERRUPTED.load(Ordering::Relaxed) {
            return PollState::Interrupted;
        }
        if rc > 0 && (pfd.revents & (POLLIN | POLLHUP | POLLERR)) != 0 {
            PollState::Ready
        } else if rc < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted { PollState::Timeout } else { PollState::Error(err) }
        } else {
            PollState::Timeout
        }
    }

    pub enum ReadChunk {
        Bytes(usize),
        Eof,
        Interrupted,
        Again,
        Error(std::io::Error),
    }

    pub fn read_stdin(buf: &mut [u8]) -> ReadChunk {
        if INTERRUPTED.load(Ordering::Relaxed) {
            return ReadChunk::Interrupted;
        }
        // SAFETY: `buf` is valid writable memory of `buf.len()` bytes for fd 0.
        let n = unsafe { read(0, buf.as_mut_ptr().cast(), buf.len()) };
        if INTERRUPTED.load(Ordering::Relaxed) {
            return ReadChunk::Interrupted;
        }
        if n > 0 {
            ReadChunk::Bytes(n as usize)
        } else if n == 0 {
            ReadChunk::Eof
        } else {
            let err = std::io::Error::last_os_error();
            match err.kind() {
                std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock => ReadChunk::Again,
                _ => ReadChunk::Error(err),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Key events & byte parser
// ---------------------------------------------------------------------------

/// Decoded terminal key event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Backspace,
    Delete,
    Enter,
    Tab,
    BackTab,
    Esc,
    CtrlC,
    CtrlD,
    CtrlA,
    CtrlE,
    CtrlU,
    CtrlW,
    Char(char),
}

fn parse_csi(buf: &[u8]) -> (Option<Key>, usize) {
    let mut i = 2;
    while i < buf.len() {
        let b = buf[i];
        if (0x40..=0x7e).contains(&b) {
            let params = &buf[2..i];
            let key = match (params, b) {
                (b"", b'A') => Some(Key::Up),
                (b"", b'B') => Some(Key::Down),
                (b"", b'C') => Some(Key::Right),
                (b"", b'D') => Some(Key::Left),
                (b"", b'H') => Some(Key::Home),
                (b"", b'F') => Some(Key::End),
                (b"", b'Z') => Some(Key::BackTab),
                (_, b'~') => match params.split(|&x| x == b';').next() {
                    Some(b"1" | b"7") => Some(Key::Home),
                    Some(b"3") => Some(Key::Delete),
                    Some(b"4" | b"8") => Some(Key::End),
                    _ => None,
                },
                _ => None,
            };
            return (key, i + 1);
        }
        if !(0x20..=0x3f).contains(&b) {
            return (None, i);
        }
        i += 1;
    }
    (None, 0)
}

fn parse_ss3(buf: &[u8]) -> (Option<Key>, usize) {
    if buf.len() < 3 {
        return (None, 0);
    }
    let key = match buf[2] {
        b'A' => Some(Key::Up),
        b'B' => Some(Key::Down),
        b'C' => Some(Key::Right),
        b'D' => Some(Key::Left),
        b'H' => Some(Key::Home),
        b'F' => Some(Key::End),
        _ => None,
    };
    (key, 3)
}

fn parse_utf8(buf: &[u8]) -> (Option<Key>, usize) {
    let expected = match buf[0] {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return (None, 1),
    };
    let check_len = buf.len().min(expected);
    match std::str::from_utf8(&buf[..check_len]) {
        Ok(s) if check_len == expected => {
            let Some(ch) = s.chars().next() else {
                return (None, 1);
            };
            let key = (!ch.is_control()).then_some(Key::Char(ch));
            (key, expected)
        }
        Ok(_) => (None, 0),
        Err(err) if err.error_len().is_none() => (None, 0),
        Err(_) => (None, 1),
    }
}

/// Parses one key event from the front of `buf`, returning `(decoded_key, bytes_consumed)`.
///
/// Returns `(None, 0)` when `buf` is empty or ends with an incomplete CSI/SS3/UTF-8 sequence.
pub fn parse_key_bytes(buf: &[u8]) -> (Option<Key>, usize) {
    let Some(&first) = buf.first() else {
        return (None, 0);
    };
    match first {
        b'\r' | b'\n' => (Some(Key::Enter), 1),
        b'\t' => (Some(Key::Tab), 1),
        0x7f | 0x08 => (Some(Key::Backspace), 1),
        0x01 => (Some(Key::CtrlA), 1),
        0x03 => (Some(Key::CtrlC), 1),
        0x04 => (Some(Key::CtrlD), 1),
        0x05 => (Some(Key::CtrlE), 1),
        0x15 => (Some(Key::CtrlU), 1),
        0x17 => (Some(Key::CtrlW), 1),
        0x1b => {
            if buf.len() == 1 || buf[1] == 0x1b {
                (Some(Key::Esc), 1)
            } else if buf[1] == b'[' {
                parse_csi(buf)
            } else if buf[1] == b'O' {
                parse_ss3(buf)
            } else {
                (None, 2)
            }
        }
        0x00..=0x1f => (None, 1),
        0x20..=0x7e => (Some(Key::Char(first as char)), 1),
        0x80..=0xff => parse_utf8(buf),
    }
}

// ---------------------------------------------------------------------------
// Plain-text clipping & viewport helpers
// ---------------------------------------------------------------------------

fn clip_chars(text: &str, max_cols: usize) -> String {
    text.chars().take(max_cols).collect()
}

fn push_clipped_segment(out: &mut String, remaining: &mut usize, text: &str, role: Option<Role>) {
    if *remaining == 0 || text.is_empty() {
        return;
    }
    let clipped = clip_chars(text, *remaining);
    *remaining -= clipped.chars().count();
    match role {
        Some(r) => out.push_str(&style::paint(r, &clipped)),
        None => out.push_str(&clipped),
    }
}

/// Returns a half-open `[start, end)` viewport slice keeping `selected` visible within `max_visible`.
pub fn viewport_window(total: usize, selected: usize, max_visible: usize) -> (usize, usize) {
    if total == 0 || max_visible == 0 {
        return (0, 0);
    }
    if total <= max_visible {
        return (0, total);
    }
    let sel = selected.min(total - 1);
    let start = sel.saturating_sub(max_visible / 2).min(total - max_visible);
    (start, start + max_visible)
}

#[cfg(target_os = "linux")]
fn remaining_tick_ms(last_tick: Instant) -> i32 {
    let rem = TICK_INTERVAL.saturating_sub(last_tick.elapsed());
    if rem.is_zero() {
        0
    } else {
        let ms = rem.as_millis() as u64 + u64::from(!rem.subsec_nanos().is_multiple_of(1_000_000));
        ms.min(TICK_INTERVAL.as_millis() as u64) as i32
    }
}

#[cfg(target_os = "linux")]
fn maybe_wait_esc_continuation(read_buf: &mut Vec<u8>) -> Option<bool> {
    let needs_wait = *read_buf == [0x1b] || parse_key_bytes(read_buf).1 == 0;
    if !needs_wait {
        return Some(false);
    }
    match sys::poll_stdin(ESC_WAIT_MS) {
        sys::PollState::Ready => {
            let mut tmp = [0u8; 64];
            match sys::read_stdin(&mut tmp) {
                sys::ReadChunk::Bytes(n) => {
                    read_buf.extend_from_slice(&tmp[..n]);
                    Some(true)
                }
                sys::ReadChunk::Interrupted => None,
                sys::ReadChunk::Eof | sys::ReadChunk::Again | sys::ReadChunk::Error(_) => Some(false),
            }
        }
        sys::PollState::Interrupted => None,
        sys::PollState::Timeout | sys::PollState::Error(_) => Some(false),
    }
}

#[cfg(target_os = "linux")]
fn drain_one_key(read_buf: &mut Vec<u8>) -> Option<Option<Key>> {
    maybe_wait_esc_continuation(read_buf)?;
    let (key_opt, consumed) = parse_key_bytes(read_buf);
    if consumed > 0 {
        read_buf.drain(..consumed);
        Some(key_opt)
    } else if read_buf.starts_with(b"\x1b[") || read_buf.starts_with(b"\x1bO") {
        read_buf.drain(..2.min(read_buf.len()));
        Some(None)
    } else {
        read_buf.drain(..1);
        Some(None)
    }
}

// ---------------------------------------------------------------------------
// Arrow-key inline selector (`select_menu`)
// ---------------------------------------------------------------------------

/// One selectable option shown by [`select_menu`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectItem {
    pub key: String,
    pub label: String,
    pub detail: String,
    pub enabled: bool,
}

/// Result of an inline [`select_menu`] prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectOutcome {
    Picked(usize),
    Cancelled,
    Slash(String),
    Eof,
}

fn unique_match<I, T>(iter: I, mut pred: impl FnMut(&T) -> bool) -> Option<usize>
where
    I: Iterator<Item = (usize, T)>,
{
    let mut found = None;
    for (idx, item) in iter {
        if pred(&item) {
            if found.is_some() {
                return None;
            }
            found = Some(idx);
        }
    }
    found
}

#[cfg(any(target_os = "linux", test))]
fn step_select(items: &[SelectItem], selected: &mut usize, key: Key) -> Option<SelectOutcome> {
    if items.is_empty() {
        return Some(SelectOutcome::Cancelled);
    }
    let len = items.len();
    match key {
        Key::Up | Key::Char('k') => {
            *selected = if *selected == 0 { len - 1 } else { *selected - 1 };
            None
        }
        Key::Down | Key::Char('j') => {
            *selected = (*selected + 1) % len;
            None
        }
        Key::Home => {
            *selected = 0;
            None
        }
        Key::End => {
            *selected = len - 1;
            None
        }
        Key::Enter => Some(SelectOutcome::Picked(*selected)),
        Key::Esc | Key::Char('q') => Some(SelectOutcome::Cancelled),
        Key::CtrlC | Key::CtrlD => Some(SelectOutcome::Eof),
        Key::Char('/') => Some(SelectOutcome::Slash("/".to_string())),
        Key::Char(c) => {
            let needle = c.to_lowercase().to_string();
            unique_match(items.iter().enumerate(), |it| !it.key.is_empty() && it.key.to_lowercase() == needle)
                .map(SelectOutcome::Picked)
        }
        _ => None,
    }
}

#[cfg(any(target_os = "linux", test))]
fn render_select_lines(
    title: &str,
    items: &[SelectItem],
    selected: usize,
    hint: &str,
    max_visible: usize,
    cols: u16,
) -> Vec<String> {
    let max_cols = cols.saturating_sub(1) as usize;
    let mut lines = Vec::new();
    if !title.is_empty() {
        lines.push(style::paint(Role::Header, &clip_chars(title, max_cols)));
    }
    let (start, end) = viewport_window(items.len(), selected, max_visible);
    if start > 0 {
        lines.push(style::paint(Role::Dim, &clip_chars(&format!("  ↑ {start} more"), max_cols)));
    }
    for (idx, item) in items.iter().enumerate().take(end).skip(start) {
        let is_sel = idx == selected;
        let mut row = String::new();
        let mut rem = max_cols;
        if is_sel {
            push_clipped_segment(&mut row, &mut rem, "▸ ", Some(Role::Accent));
        } else {
            push_clipped_segment(&mut row, &mut rem, "  ", None);
        }
        if !item.key.is_empty() {
            let key_role = if !item.enabled {
                Role::Dim
            } else if is_sel {
                Role::Value
            } else {
                Role::Dim
            };
            push_clipped_segment(&mut row, &mut rem, &format!("[{}] ", item.key), Some(key_role));
        }
        let label_role = if !item.enabled {
            Some(Role::Dim)
        } else if is_sel {
            Some(Role::Command)
        } else {
            None
        };
        push_clipped_segment(&mut row, &mut rem, &item.label, label_role);
        if !item.detail.is_empty() {
            push_clipped_segment(&mut row, &mut rem, &format!(" - {}", item.detail), Some(Role::Dim));
        }
        lines.push(row);
    }
    let remaining = items.len().saturating_sub(end);
    if remaining > 0 {
        lines.push(style::paint(Role::Dim, &clip_chars(&format!("  ↓ {remaining} more"), max_cols)));
    }
    if !hint.is_empty() {
        lines.push(style::paint(Role::Dim, &clip_chars(hint, max_cols)));
    }
    lines
}

#[cfg(target_os = "linux")]
fn clear_select_area(out: &mut impl Write, drawn_lines: usize) -> std::io::Result<()> {
    if drawn_lines > 0 {
        write!(out, "\r\x1b[{drawn_lines}A\x1b[J")?;
    } else {
        write!(out, "\r\x1b[2K\x1b[J")?;
    }
    out.flush()
}

#[cfg(target_os = "linux")]
fn draw_select_area(out: &mut impl Write, drawn_lines: &mut usize, lines: &[String]) -> std::io::Result<()> {
    if *drawn_lines > 0 {
        write!(out, "\r\x1b[{}A\x1b[J", *drawn_lines)?;
    }
    for line in lines {
        writeln!(out, "{line}")?;
    }
    *drawn_lines = lines.len();
    out.flush()
}

fn select_menu_plain<R: BufRead, W: Write>(
    title: &str,
    items: &[SelectItem],
    hint: &str,
    reader: &mut R,
    writer: &mut W,
) -> std::io::Result<SelectOutcome> {
    if items.is_empty() {
        return Ok(SelectOutcome::Cancelled);
    }
    if !title.is_empty() {
        writeln!(writer, "{}", style::paint(Role::Header, title))?;
    }
    for (idx, item) in items.iter().enumerate() {
        let num = idx + 1;
        let key_tag =
            if item.key.is_empty() || item.key == num.to_string() { String::new() } else { format!(" ({})", item.key) };
        let detail = if item.detail.is_empty() { String::new() } else { format!(" - {}", item.detail) };
        let line = format!("  [{num}]{key_tag} {}{detail}", item.label);
        if item.enabled {
            writeln!(writer, "{line}")?;
        } else {
            writeln!(writer, "{}", style::paint(Role::Dim, &line))?;
        }
    }
    if !hint.is_empty() {
        writeln!(writer, "{}", style::paint(Role::Dim, hint))?;
    }
    write!(writer, "Choice: ")?;
    writer.flush()?;

    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(SelectOutcome::Eof);
    }
    let trimmed = line.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("q")
        || trimmed.eq_ignore_ascii_case("b")
        || trimmed.eq_ignore_ascii_case("back")
    {
        return Ok(SelectOutcome::Cancelled);
    }
    if trimmed.starts_with('/') {
        return Ok(SelectOutcome::Slash(trimmed.to_string()));
    }
    if let Ok(num) = trimmed.parse::<usize>()
        && (1..=items.len()).contains(&num)
    {
        return Ok(SelectOutcome::Picked(num - 1));
    }
    let lower = trimmed.to_lowercase();
    if let Some(idx) = unique_match(items.iter().enumerate(), |it| !it.key.is_empty() && it.key.to_lowercase() == lower)
    {
        return Ok(SelectOutcome::Picked(idx));
    }
    if let Some(idx) = unique_match(items.iter().enumerate(), |it| it.label.to_lowercase() == lower) {
        return Ok(SelectOutcome::Picked(idx));
    }
    Ok(SelectOutcome::Cancelled)
}

#[cfg(target_os = "linux")]
fn select_menu_tty(
    guard: &RawModeGuard,
    title: &str,
    items: &[SelectItem],
    initial: usize,
    hint: &str,
    on_tick: &mut dyn FnMut() -> Option<String>,
) -> std::io::Result<SelectOutcome> {
    let mut selected = initial.min(items.len() - 1);
    let mut drawn_lines = 0usize;
    let mut read_buf: Vec<u8> = Vec::new();
    let mut last_size = term_size().unwrap_or((24, 80));
    let mut last_tick = Instant::now();
    let mut dirty = true;

    loop {
        let size = term_size().unwrap_or((24, 80));
        if size != last_size {
            last_size = size;
            dirty = true;
        }
        let max_visible = (size.0 as usize).saturating_sub(10).clamp(3, 10);
        if dirty {
            let lines = render_select_lines(title, items, selected, hint, max_visible, size.1);
            let mut out = std::io::stdout().lock();
            draw_select_area(&mut out, &mut drawn_lines, &lines)?;
            dirty = false;
        }

        if guard.is_interrupted() {
            let mut out = std::io::stdout().lock();
            clear_select_area(&mut out, drawn_lines)?;
            return Ok(SelectOutcome::Eof);
        }

        if read_buf.is_empty() {
            let wait_ms = remaining_tick_ms(last_tick);
            match sys::poll_stdin(wait_ms) {
                sys::PollState::Interrupted => {
                    let mut out = std::io::stdout().lock();
                    clear_select_area(&mut out, drawn_lines)?;
                    return Ok(SelectOutcome::Eof);
                }
                sys::PollState::Error(err) => {
                    let mut out = std::io::stdout().lock();
                    let _ = clear_select_area(&mut out, drawn_lines);
                    return Err(err);
                }
                sys::PollState::Ready => {
                    let mut tmp = [0u8; 64];
                    match sys::read_stdin(&mut tmp) {
                        sys::ReadChunk::Bytes(n) => read_buf.extend_from_slice(&tmp[..n]),
                        sys::ReadChunk::Eof => {
                            let mut out = std::io::stdout().lock();
                            clear_select_area(&mut out, drawn_lines)?;
                            return Ok(SelectOutcome::Eof);
                        }
                        sys::ReadChunk::Interrupted => {
                            let mut out = std::io::stdout().lock();
                            clear_select_area(&mut out, drawn_lines)?;
                            return Ok(SelectOutcome::Eof);
                        }
                        sys::ReadChunk::Again => {}
                        sys::ReadChunk::Error(err) => {
                            let mut out = std::io::stdout().lock();
                            let _ = clear_select_area(&mut out, drawn_lines);
                            return Err(err);
                        }
                    }
                }
                sys::PollState::Timeout => {}
            }
        }

        while !read_buf.is_empty() {
            let Some(key_opt) = drain_one_key(&mut read_buf) else {
                let mut out = std::io::stdout().lock();
                clear_select_area(&mut out, drawn_lines)?;
                return Ok(SelectOutcome::Eof);
            };
            if let Some(key) = key_opt {
                let prev = selected;
                if let Some(outcome) = step_select(items, &mut selected, key) {
                    let mut out = std::io::stdout().lock();
                    clear_select_area(&mut out, drawn_lines)?;
                    return Ok(outcome);
                }
                if selected != prev {
                    dirty = true;
                }
            }
        }

        if last_tick.elapsed() >= TICK_INTERVAL {
            last_tick = Instant::now();
            if let Some(msg) = on_tick().filter(|m| !m.is_empty()) {
                let mut out = std::io::stdout().lock();
                if drawn_lines > 0 {
                    write!(out, "\r\x1b[{drawn_lines}A\x1b[J")?;
                    drawn_lines = 0;
                }
                out.write_all(msg.as_bytes())?;
                if !msg.ends_with('\n') {
                    out.write_all(b"\n")?;
                }
                let size = term_size().unwrap_or((24, 80));
                last_size = size;
                let max_visible = (size.0 as usize).saturating_sub(10).clamp(3, 10);
                let lines = render_select_lines(title, items, selected, hint, max_visible, size.1);
                draw_select_area(&mut out, &mut drawn_lines, &lines)?;
                dirty = false;
            }
        }
    }
}

/// Shows an inline arrow-key selector (or numbered plain fallback when non-interactive), returning
/// the chosen index, cancel, slash escape, or EOF after cleaning up all temporary menu lines.
pub fn select_menu(
    title: &str,
    items: &[SelectItem],
    initial: usize,
    hint: &str,
    on_tick: &mut dyn FnMut() -> Option<String>,
) -> std::io::Result<SelectOutcome> {
    if items.is_empty() {
        return Ok(SelectOutcome::Cancelled);
    }
    #[cfg(target_os = "linux")]
    if is_interactive_tty()
        && let Some(guard) = RawModeGuard::new()
    {
        return select_menu_tty(&guard, title, items, initial, hint, on_tick);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (initial, on_tick);
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    select_menu_plain(title, items, hint, &mut stdin, &mut stdout)
}

// ---------------------------------------------------------------------------
// Live `/` fuzzy dropdown line editor (`LineEditor`)
// ---------------------------------------------------------------------------

/// One completion candidate shown in [`LineEditor`]'s live dropdown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    pub insert: String,
    pub display: String,
    pub summary: String,
    pub auto_submit: bool,
}

/// Result returned by [`LineEditor::read_line`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorEvent {
    Submit(String),
    Eof,
    Interrupted,
}

/// Pure, testable editing state for [`LineEditor`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditorState {
    pub buf: String,
    pub cursor: usize,
    pub dropdown_open: bool,
    pub selected: usize,
    pub history_pos: Option<usize>,
    pub saved_draft: String,
}

impl EditorState {
    /// Creates an empty editing state.
    pub fn new() -> Self {
        Self::default()
    }

    fn char_len(&self) -> usize {
        self.buf.chars().count()
    }

    fn byte_index(&self, char_idx: usize) -> usize {
        self.buf.char_indices().nth(char_idx).map_or(self.buf.len(), |(idx, _)| idx)
    }

    /// Replaces the buffer text and places the cursor at the end.
    pub fn set_text(&mut self, s: &str) {
        self.buf = s.to_string();
        self.cursor = self.char_len();
    }

    /// Returns the current buffer text.
    pub fn text(&self) -> &str {
        &self.buf
    }

    /// Returns the cursor position as a 0-based UTF-8 character offset.
    pub fn cursor_col(&self) -> usize {
        self.cursor.min(self.char_len())
    }

    /// Inserts `c` at the cursor, clears active history selection, and reopens the dropdown.
    pub fn insert_char(&mut self, c: char) {
        let pos = self.cursor_col();
        let idx = self.byte_index(pos);
        self.buf.insert(idx, c);
        self.cursor = pos + 1;
        self.history_pos = None;
        self.dropdown_open = true;
        self.selected = 0;
    }

    /// Deletes the character before the cursor.
    pub fn backspace(&mut self) {
        let pos = self.cursor_col();
        if pos > 0 {
            let end = self.byte_index(pos);
            let start = self.byte_index(pos - 1);
            self.buf.replace_range(start..end, "");
            self.cursor = pos - 1;
        }
        self.history_pos = None;
        self.dropdown_open = true;
        self.selected = 0;
    }

    /// Deletes the character under the cursor.
    pub fn delete(&mut self) {
        let pos = self.cursor_col();
        if pos < self.char_len() {
            let start = self.byte_index(pos);
            let end = self.byte_index(pos + 1);
            self.buf.replace_range(start..end, "");
        }
        self.cursor = pos;
        self.history_pos = None;
        self.dropdown_open = true;
        self.selected = 0;
    }

    /// Moves the cursor one character left.
    pub fn move_left(&mut self) {
        self.cursor = self.cursor_col().saturating_sub(1);
    }

    /// Moves the cursor one character right.
    pub fn move_right(&mut self) {
        self.cursor = (self.cursor_col() + 1).min(self.char_len());
    }

    /// Moves the cursor to the start of the buffer.
    pub fn home(&mut self) {
        self.cursor = 0;
    }

    /// Moves the cursor to the end of the buffer.
    pub fn end(&mut self) {
        self.cursor = self.char_len();
    }

    /// Clears the buffer and resets cursor, history cursor, and dropdown state.
    pub fn kill_line(&mut self) {
        self.buf.clear();
        self.cursor = 0;
        self.history_pos = None;
        self.dropdown_open = false;
        self.selected = 0;
    }

    /// Deletes the whitespace-delimited word immediately before the cursor.
    pub fn kill_word(&mut self) {
        let pos = self.cursor_col();
        if pos == 0 {
            return;
        }
        let chars: Vec<char> = self.buf.chars().collect();
        let mut new_cursor = pos;
        while new_cursor > 0 && chars[new_cursor - 1].is_whitespace() {
            new_cursor -= 1;
        }
        while new_cursor > 0 && !chars[new_cursor - 1].is_whitespace() {
            new_cursor -= 1;
        }
        let start = self.byte_index(new_cursor);
        let end = self.byte_index(pos);
        self.buf.replace_range(start..end, "");
        self.cursor = new_cursor;
        self.history_pos = None;
        self.dropdown_open = true;
        self.selected = 0;
    }

    /// Cycles backward through `history` (saving the active draft on first step).
    pub fn history_prev(&mut self, history: &[String]) {
        if history.is_empty() {
            return;
        }
        match self.history_pos {
            None => {
                self.saved_draft = self.buf.clone();
                let pos = history.len() - 1;
                self.history_pos = Some(pos);
                self.set_text(&history[pos]);
            }
            Some(pos) if pos > 0 => {
                let next_pos = pos - 1;
                self.history_pos = Some(next_pos);
                self.set_text(&history[next_pos]);
            }
            Some(_) => {}
        }
        self.dropdown_open = false;
        self.selected = 0;
    }

    /// Cycles forward through `history`, restoring the saved draft past the newest entry.
    pub fn history_next(&mut self, history: &[String]) {
        let Some(pos) = self.history_pos else {
            return;
        };
        if pos + 1 < history.len() {
            let next_pos = pos + 1;
            self.history_pos = Some(next_pos);
            self.set_text(&history[next_pos]);
        } else {
            self.history_pos = None;
            let draft = std::mem::take(&mut self.saved_draft);
            self.set_text(&draft);
        }
        self.dropdown_open = false;
        self.selected = 0;
    }

    /// Applies `c.insert` to the buffer with cursor at end; returns `true` if `c.auto_submit`.
    pub fn apply_completion(&mut self, c: &Completion) -> bool {
        self.set_text(&c.insert);
        self.history_pos = None;
        self.selected = 0;
        if c.auto_submit {
            self.dropdown_open = false;
            true
        } else {
            self.dropdown_open = true;
            false
        }
    }
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
enum EditorStep {
    Continue,
    SubmitAuto(String),
    SubmitNormal(String),
    Finish(EditorEvent),
}

#[cfg(any(target_os = "linux", test))]
fn step_editor(state: &mut EditorState, history: &[String], completions: &[Completion], key: Key) -> EditorStep {
    match key {
        Key::Left => state.move_left(),
        Key::Right => state.move_right(),
        Key::Home | Key::CtrlA => state.home(),
        Key::End | Key::CtrlE => state.end(),
        Key::Backspace => state.backspace(),
        Key::Delete => state.delete(),
        Key::CtrlU => state.kill_line(),
        Key::CtrlW => state.kill_word(),
        Key::Char(c) => state.insert_char(c),
        Key::Up => {
            if state.dropdown_open && !completions.is_empty() {
                let len = completions.len();
                let sel = state.selected.min(len - 1);
                state.selected = if sel == 0 { len - 1 } else { sel - 1 };
            } else {
                state.history_prev(history);
            }
        }
        Key::Down => {
            if state.dropdown_open && !completions.is_empty() {
                let len = completions.len();
                let sel = state.selected.min(len - 1);
                state.selected = (sel + 1) % len;
            } else {
                state.history_next(history);
            }
        }
        Key::BackTab => {
            if state.dropdown_open && !completions.is_empty() {
                let len = completions.len();
                let sel = state.selected.min(len - 1);
                state.selected = if sel == 0 { len - 1 } else { sel - 1 };
            }
        }
        Key::Tab => {
            if !completions.is_empty() {
                let sel = state.selected.min(completions.len() - 1);
                state.apply_completion(&completions[sel]);
                state.dropdown_open = true;
            }
        }
        Key::Esc => {
            if state.dropdown_open && !completions.is_empty() {
                state.dropdown_open = false;
            } else {
                state.kill_line();
            }
        }
        Key::CtrlC => return EditorStep::Finish(EditorEvent::Interrupted),
        Key::CtrlD => {
            if state.text().is_empty() {
                return EditorStep::Finish(EditorEvent::Eof);
            }
            state.delete();
        }
        Key::Enter => {
            if state.dropdown_open && !completions.is_empty() {
                let sel = state.selected.min(completions.len() - 1);
                let comp = &completions[sel];
                let exact_complete_match = state.text().trim() == comp.insert.trim();
                if comp.auto_submit && !exact_complete_match {
                    return EditorStep::SubmitAuto(comp.insert.trim().to_string());
                }
                if !comp.auto_submit
                    && !(state.text().starts_with(&comp.insert) && state.text().len() > comp.insert.len())
                {
                    state.apply_completion(comp);
                    state.dropdown_open = true;
                    return EditorStep::Continue;
                }
            }
            return EditorStep::SubmitNormal(state.text().to_string());
        }
    }
    EditorStep::Continue
}

#[cfg(any(target_os = "linux", test))]
fn format_prompt_and_col(prompt: &str, buf: &str, cursor_char_idx: usize, cols: u16) -> (String, usize) {
    let max_cols = cols.saturating_sub(1) as usize;
    let prompt_clipped = clip_chars(prompt, max_cols);
    let prompt_cols = prompt_clipped.chars().count();
    let avail = max_cols.saturating_sub(prompt_cols);
    let cursor = cursor_char_idx.min(buf.chars().count());

    let (visible_buf, visible_cursor) = if avail == 0 {
        (String::new(), 0)
    } else if cursor < avail {
        (clip_chars(buf, avail), cursor)
    } else {
        let h_start = cursor - avail + 1;
        (buf.chars().skip(h_start).take(avail).collect(), avail - 1)
    };

    let mut line = String::new();
    if !prompt_clipped.is_empty() {
        line.push_str(&style::paint(Role::Dim, &prompt_clipped));
    }
    line.push_str(&visible_buf);
    (line, (prompt_cols + visible_cursor + 1).max(1))
}

#[cfg(any(target_os = "linux", test))]
fn render_dropdown_lines(completions: &[Completion], selected: usize, cols: u16) -> Vec<String> {
    if completions.is_empty() {
        return Vec::new();
    }
    let max_cols = cols.saturating_sub(1) as usize;
    let sel = selected.min(completions.len() - 1);
    let (start, end) = viewport_window(completions.len(), sel, MAX_DROPDOWN_ROWS);
    let display_w = completions[start..end]
        .iter()
        .map(|c| c.display.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(8, 22)
        .min((max_cols / 2).max(1));

    let mut lines = Vec::with_capacity(end - start);
    for (idx, comp) in completions.iter().enumerate().take(end).skip(start) {
        let is_sel = idx == sel;
        let mut row = String::new();
        let mut rem = max_cols;
        if is_sel {
            push_clipped_segment(&mut row, &mut rem, "▸ ", Some(Role::Accent));
        } else {
            push_clipped_segment(&mut row, &mut rem, "  ", None);
        }
        let disp_role = if is_sel { Role::Command } else { Role::Part };
        let disp_len = comp.display.chars().count();
        push_clipped_segment(&mut row, &mut rem, &comp.display, Some(disp_role));
        if !comp.summary.is_empty() && rem > 2 {
            let pad = display_w.saturating_sub(disp_len) + 2;
            let spaces = " ".repeat(pad.min(rem));
            push_clipped_segment(&mut row, &mut rem, &spaces, None);
            push_clipped_segment(&mut row, &mut rem, &comp.summary, Some(Role::Dim));
        }
        lines.push(row);
    }
    lines
}

#[cfg(target_os = "linux")]
fn draw_editor(
    out: &mut impl Write,
    prompt: &str,
    state: &EditorState,
    completions: &[Completion],
    cols: u16,
) -> std::io::Result<()> {
    let (prompt_line, col) = format_prompt_and_col(prompt, state.text(), state.cursor_col(), cols);
    let dropdown =
        if state.dropdown_open { render_dropdown_lines(completions, state.selected, cols) } else { Vec::new() };
    write!(out, "\r\x1b[2K\x1b[J{prompt_line}")?;
    for row in &dropdown {
        write!(out, "\n{row}")?;
    }
    if !dropdown.is_empty() {
        write!(out, "\x1b[{}A", dropdown.len())?;
    }
    write!(out, "\r\x1b[{col}G")?;
    out.flush()
}

#[cfg(target_os = "linux")]
fn clear_prompt_and_dropdown(out: &mut impl Write) -> std::io::Result<()> {
    write!(out, "\r\x1b[2K\x1b[J")?;
    out.flush()
}

#[cfg(target_os = "linux")]
fn commit_prompt_and_newline(out: &mut impl Write, prompt: &str, submitted: &str, cols: u16) -> std::io::Result<()> {
    let (prompt_line, _) = format_prompt_and_col(prompt, submitted, submitted.chars().count(), cols);
    writeln!(out, "\r\x1b[2K\x1b[J{prompt_line}")?;
    out.flush()
}

/// Interactive single-line editor with command history, typewriter prefill preservation, and live
/// `/` fuzzy completion dropdowns.
#[derive(Clone, Debug, Default)]
pub struct LineEditor {
    history: Vec<String>,
    prefill: Vec<u8>,
}

impl LineEditor {
    /// Creates an empty `LineEditor`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends `text` bytes to be consumed at the start of the next [`LineEditor::read_line`].
    pub fn push_prefill(&mut self, text: &str) {
        self.prefill.extend_from_slice(text.as_bytes());
    }

    fn push_history(&mut self, line: &str) {
        if line.trim().is_empty() {
            return;
        }
        if self.history.last().is_some_and(|last| last == line) {
            return;
        }
        if self.history.len() >= MAX_HISTORY {
            let excess = self.history.len() + 1 - MAX_HISTORY;
            self.history.drain(..excess);
        }
        self.history.push(line.to_string());
    }

    /// Reads one submitted line, handling raw-mode editing, live completions, and `on_tick` async
    /// output on interactive Linux TTYs, or falling back to plain `stdin().read_line`.
    #[allow(dead_code)]
    pub fn read_line(
        &mut self,
        prompt: &str,
        complete: &dyn Fn(&str) -> Vec<Completion>,
        on_tick: &mut dyn FnMut() -> Option<String>,
    ) -> std::io::Result<EditorEvent> {
        let never = AtomicBool::new(false);
        self.read_line_with_cancel(prompt, complete, on_tick, &never)
    }

    /// Same as [`LineEditor::read_line`], aborting cleanly when `cancel` becomes `true` (saving any
    /// partial input into `prefill` for the next read).
    pub fn read_line_with_cancel(
        &mut self,
        prompt: &str,
        complete: &dyn Fn(&str) -> Vec<Completion>,
        on_tick: &mut dyn FnMut() -> Option<String>,
        cancel: &AtomicBool,
    ) -> std::io::Result<EditorEvent> {
        if cancel.load(Ordering::Relaxed) {
            return Ok(EditorEvent::Interrupted);
        }
        #[cfg(target_os = "linux")]
        if is_interactive_tty()
            && let Some(guard) = RawModeGuard::new()
        {
            return self.read_line_tty(&guard, prompt, complete, on_tick, cancel);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = (complete, on_tick);
        let mut stdin = std::io::stdin().lock();
        let mut stdout = std::io::stdout().lock();
        read_line_plain(self, prompt, &mut stdin, &mut stdout)
    }

    #[cfg(target_os = "linux")]
    fn read_line_tty(
        &mut self,
        guard: &RawModeGuard,
        prompt: &str,
        complete: &dyn Fn(&str) -> Vec<Completion>,
        on_tick: &mut dyn FnMut() -> Option<String>,
        cancel: &AtomicBool,
    ) -> std::io::Result<EditorEvent> {
        let mut state = EditorState::new();
        let mut read_buf = std::mem::take(&mut self.prefill);

        while !read_buf.is_empty() {
            let (key_opt, consumed) = parse_key_bytes(&read_buf);
            if consumed == 0 {
                break;
            }
            read_buf.drain(..consumed);
            if let Some(key) = key_opt {
                let completions = complete(state.text());
                match step_editor(&mut state, &self.history, &completions, key) {
                    EditorStep::Continue => {}
                    EditorStep::SubmitAuto(submitted) => {
                        self.prefill = read_buf;
                        self.push_history(&submitted);
                        return Ok(EditorEvent::Submit(submitted));
                    }
                    EditorStep::SubmitNormal(submitted) => {
                        self.prefill = read_buf;
                        let cols = term_size().map_or(80, |(_, c)| c);
                        let mut out = std::io::stdout().lock();
                        commit_prompt_and_newline(&mut out, prompt, &submitted, cols)?;
                        self.push_history(&submitted);
                        return Ok(EditorEvent::Submit(submitted));
                    }
                    EditorStep::Finish(event) => {
                        self.prefill = read_buf;
                        return Ok(event);
                    }
                }
            }
        }

        let mut last_size = term_size().unwrap_or((24, 80));
        let mut last_tick = Instant::now();
        let mut dirty = true;

        loop {
            if cancel.load(Ordering::Relaxed) {
                let mut out = std::io::stdout().lock();
                clear_prompt_and_dropdown(&mut out)?;
                if !state.text().is_empty() || !read_buf.is_empty() {
                    let mut saved = state.text().as_bytes().to_vec();
                    saved.extend_from_slice(&read_buf);
                    self.prefill = saved;
                }
                return Ok(EditorEvent::Interrupted);
            }
            let size = term_size().unwrap_or((24, 80));
            if size != last_size {
                last_size = size;
                dirty = true;
            }
            if dirty {
                let completions = complete(state.text());
                let mut out = std::io::stdout().lock();
                draw_editor(&mut out, prompt, &state, &completions, size.1)?;
                dirty = false;
            }

            if guard.is_interrupted() {
                let mut out = std::io::stdout().lock();
                clear_prompt_and_dropdown(&mut out)?;
                return Ok(EditorEvent::Interrupted);
            }

            if read_buf.is_empty() {
                let wait_ms = remaining_tick_ms(last_tick);
                match sys::poll_stdin(wait_ms) {
                    sys::PollState::Interrupted => {
                        let mut out = std::io::stdout().lock();
                        clear_prompt_and_dropdown(&mut out)?;
                        return Ok(EditorEvent::Interrupted);
                    }
                    sys::PollState::Error(err) => {
                        let mut out = std::io::stdout().lock();
                        let _ = clear_prompt_and_dropdown(&mut out);
                        return Err(err);
                    }
                    sys::PollState::Ready => {
                        let mut tmp = [0u8; 128];
                        match sys::read_stdin(&mut tmp) {
                            sys::ReadChunk::Bytes(n) => read_buf.extend_from_slice(&tmp[..n]),
                            sys::ReadChunk::Eof => {
                                let mut out = std::io::stdout().lock();
                                clear_prompt_and_dropdown(&mut out)?;
                                return Ok(EditorEvent::Eof);
                            }
                            sys::ReadChunk::Interrupted => {
                                let mut out = std::io::stdout().lock();
                                clear_prompt_and_dropdown(&mut out)?;
                                return Ok(EditorEvent::Interrupted);
                            }
                            sys::ReadChunk::Again => {}
                            sys::ReadChunk::Error(err) => {
                                let mut out = std::io::stdout().lock();
                                let _ = clear_prompt_and_dropdown(&mut out);
                                return Err(err);
                            }
                        }
                    }
                    sys::PollState::Timeout => {}
                }
            }

            while !read_buf.is_empty() {
                let Some(key_opt) = drain_one_key(&mut read_buf) else {
                    let mut out = std::io::stdout().lock();
                    clear_prompt_and_dropdown(&mut out)?;
                    return Ok(EditorEvent::Interrupted);
                };
                if let Some(key) = key_opt {
                    let completions = complete(state.text());
                    match step_editor(&mut state, &self.history, &completions, key) {
                        EditorStep::Continue => dirty = true,
                        EditorStep::SubmitAuto(submitted) => {
                            self.prefill = read_buf;
                            let mut out = std::io::stdout().lock();
                            clear_prompt_and_dropdown(&mut out)?;
                            self.push_history(&submitted);
                            return Ok(EditorEvent::Submit(submitted));
                        }
                        EditorStep::SubmitNormal(submitted) => {
                            self.prefill = read_buf;
                            let cols = term_size().map_or(80, |(_, c)| c);
                            let mut out = std::io::stdout().lock();
                            commit_prompt_and_newline(&mut out, prompt, &submitted, cols)?;
                            self.push_history(&submitted);
                            return Ok(EditorEvent::Submit(submitted));
                        }
                        EditorStep::Finish(event) => {
                            self.prefill = read_buf;
                            let mut out = std::io::stdout().lock();
                            clear_prompt_and_dropdown(&mut out)?;
                            return Ok(event);
                        }
                    }
                }
            }

            if last_tick.elapsed() >= TICK_INTERVAL {
                last_tick = Instant::now();
                let async_out = on_tick().filter(|m| !m.is_empty());
                if cancel.load(Ordering::Relaxed) {
                    let mut out = std::io::stdout().lock();
                    if let Some(msg) = async_out {
                        write!(out, "\r\x1b[2K\x1b[J")?;
                        out.write_all(msg.as_bytes())?;
                        if !msg.ends_with('\n') {
                            out.write_all(b"\n")?;
                        }
                        out.flush()?;
                    } else {
                        clear_prompt_and_dropdown(&mut out)?;
                    }
                    if !state.text().is_empty() || !read_buf.is_empty() {
                        let mut saved = state.text().as_bytes().to_vec();
                        saved.extend_from_slice(&read_buf);
                        self.prefill = saved;
                    }
                    return Ok(EditorEvent::Interrupted);
                }
                if let Some(async_out) = async_out {
                    let mut out = std::io::stdout().lock();
                    write!(out, "\r\x1b[2K\x1b[J")?;
                    out.write_all(async_out.as_bytes())?;
                    if !async_out.ends_with('\n') {
                        out.write_all(b"\n")?;
                    }
                    let cols = term_size().map_or(80, |(_, c)| c);
                    last_size.1 = cols;
                    let completions = complete(state.text());
                    draw_editor(&mut out, prompt, &state, &completions, cols)?;
                    dirty = false;
                }
            }
        }
    }

    /// Sleeps up to `delay` while watching for a key press to flush typewriter output.
    ///
    /// On an interactive Linux TTY, drops standalone skip keys (`\r`, `\n`, `' '`, lone `0x1b`) and
    /// preserves any other typed bytes in `prefill` before returning `true`.
    pub fn wait_typewriter_skip(&mut self, delay: Duration) -> bool {
        #[cfg(target_os = "linux")]
        if is_interactive_tty()
            && let Some(guard) = RawModeGuard::new()
        {
            match sys::poll_stdin(delay.as_millis() as i32) {
                sys::PollState::Interrupted => return true,
                sys::PollState::Ready => {
                    let mut tmp = [0u8; 128];
                    match sys::read_stdin(&mut tmp) {
                        sys::ReadChunk::Bytes(n) if n > 0 => {
                            let bytes = &tmp[..n];
                            self.ingest_typewriter_bytes(bytes);
                            return true;
                        }
                        sys::ReadChunk::Interrupted => return true,
                        _ => return guard.is_interrupted(),
                    }
                }
                sys::PollState::Timeout | sys::PollState::Error(_) => return guard.is_interrupted(),
            }
        }
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        false
    }

    #[cfg(any(target_os = "linux", test))]
    fn ingest_typewriter_bytes(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if bytes == [0x1b] || bytes == b"\r\n" || (bytes.len() == 1 && matches!(bytes[0], b'\r' | b'\n' | b' ' | 0x1b))
        {
            return;
        }
        if matches!(bytes[0], b'\r' | b'\n' | b' ') {
            let skip = if bytes.starts_with(b"\r\n") { 2 } else { 1 };
            self.prefill.extend_from_slice(&bytes[skip..]);
        } else {
            self.prefill.extend_from_slice(bytes);
        }
    }
}

fn read_line_plain<R: BufRead, W: Write>(
    editor: &mut LineEditor,
    prompt: &str,
    reader: &mut R,
    writer: &mut W,
) -> std::io::Result<EditorEvent> {
    if !prompt.is_empty() {
        write!(writer, "{}", style::paint(Role::Dim, prompt))?;
        writer.flush()?;
    }
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(EditorEvent::Eof);
    }
    while line.ends_with('\n') || line.ends_with('\r') {
        line.pop();
    }
    editor.push_history(&line);
    Ok(EditorEvent::Submit(line))
}

// ---------------------------------------------------------------------------
// Tests (never touch global `style::init`/`style::set` or real fd 0 termios)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn item(key: &str, label: &str, detail: &str, enabled: bool) -> SelectItem {
        SelectItem { key: key.into(), label: label.into(), detail: detail.into(), enabled }
    }

    fn comp(insert: &str, display: &str, summary: &str, auto_submit: bool) -> Completion {
        Completion { insert: insert.into(), display: display.into(), summary: summary.into(), auto_submit }
    }

    #[test]
    fn parses_control_arrow_ss3_csi_and_utf8_keys() {
        assert_eq!(parse_key_bytes(b""), (None, 0));
        assert_eq!(parse_key_bytes(b"\r"), (Some(Key::Enter), 1));
        assert_eq!(parse_key_bytes(b"\n"), (Some(Key::Enter), 1));
        assert_eq!(parse_key_bytes(b"\t"), (Some(Key::Tab), 1));
        assert_eq!(parse_key_bytes(&[0x7f]), (Some(Key::Backspace), 1));
        assert_eq!(parse_key_bytes(&[0x08]), (Some(Key::Backspace), 1));
        assert_eq!(parse_key_bytes(&[0x01]), (Some(Key::CtrlA), 1));
        assert_eq!(parse_key_bytes(&[0x03]), (Some(Key::CtrlC), 1));
        assert_eq!(parse_key_bytes(&[0x04]), (Some(Key::CtrlD), 1));
        assert_eq!(parse_key_bytes(&[0x05]), (Some(Key::CtrlE), 1));
        assert_eq!(parse_key_bytes(&[0x15]), (Some(Key::CtrlU), 1));
        assert_eq!(parse_key_bytes(&[0x17]), (Some(Key::CtrlW), 1));
        assert_eq!(parse_key_bytes(&[0x0c]), (None, 1));

        // Esc, CSI, SS3, Alt+key
        assert_eq!(parse_key_bytes(&[0x1b]), (Some(Key::Esc), 1));
        assert_eq!(parse_key_bytes(b"\x1b\x1b[A"), (Some(Key::Esc), 1));
        assert_eq!(parse_key_bytes(b"\x1b[A"), (Some(Key::Up), 3));
        assert_eq!(parse_key_bytes(b"\x1b[B"), (Some(Key::Down), 3));
        assert_eq!(parse_key_bytes(b"\x1b[C"), (Some(Key::Right), 3));
        assert_eq!(parse_key_bytes(b"\x1b[D"), (Some(Key::Left), 3));
        assert_eq!(parse_key_bytes(b"\x1b[H"), (Some(Key::Home), 3));
        assert_eq!(parse_key_bytes(b"\x1b[F"), (Some(Key::End), 3));
        assert_eq!(parse_key_bytes(b"\x1b[Z"), (Some(Key::BackTab), 3));
        assert_eq!(parse_key_bytes(b"\x1b[1~"), (Some(Key::Home), 4));
        assert_eq!(parse_key_bytes(b"\x1b[3~"), (Some(Key::Delete), 4));
        assert_eq!(parse_key_bytes(b"\x1b[4~"), (Some(Key::End), 4));
        assert_eq!(parse_key_bytes(b"\x1b[7~"), (Some(Key::Home), 4));
        assert_eq!(parse_key_bytes(b"\x1b[8~"), (Some(Key::End), 4));
        assert_eq!(parse_key_bytes(b"\x1b[1;5A"), (None, 6));
        assert_eq!(parse_key_bytes(b"\x1b[<0;12;5M"), (None, 10));
        assert_eq!(parse_key_bytes(b"\x1bOA"), (Some(Key::Up), 3));
        assert_eq!(parse_key_bytes(b"\x1bOB"), (Some(Key::Down), 3));
        assert_eq!(parse_key_bytes(b"\x1bOC"), (Some(Key::Right), 3));
        assert_eq!(parse_key_bytes(b"\x1bOD"), (Some(Key::Left), 3));
        assert_eq!(parse_key_bytes(b"\x1bOH"), (Some(Key::Home), 3));
        assert_eq!(parse_key_bytes(b"\x1bOF"), (Some(Key::End), 3));
        assert_eq!(parse_key_bytes(b"\x1bOP"), (None, 3));
        assert_eq!(parse_key_bytes(b"\x1bx"), (None, 2));

        // Incomplete CSI/SS3 waits for remaining bytes.
        assert_eq!(parse_key_bytes(b"\x1b["), (None, 0));
        assert_eq!(parse_key_bytes(b"\x1b[1;"), (None, 0));
        assert_eq!(parse_key_bytes(b"\x1bO"), (None, 0));

        // UTF-8 multi-byte & truncated sequences.
        let crab = "🦀".as_bytes();
        assert_eq!(parse_key_bytes(crab), (Some(Key::Char('🦀')), 4));
        assert_eq!(parse_key_bytes(&crab[..2]), (None, 0));
        assert_eq!(parse_key_bytes("é".as_bytes()), (Some(Key::Char('é')), 2));
        assert_eq!(parse_key_bytes(b"\xffa"), (None, 1));
        assert_eq!(parse_key_bytes("\u{0085}".as_bytes()), (None, 2));
    }

    #[test]
    fn terminal_size_and_tty_helpers() {
        assert_eq!(parse_env_term_size(Some("32"), Some("120")), Some((32, 120)));
        assert_eq!(parse_env_term_size(Some("0"), Some("80")), None);
        assert_eq!(parse_env_term_size(Some("24"), None), None);
        assert_eq!(parse_env_term_size(Some("bad"), Some("80")), None);

        assert!(interactive_from(true, true, Some("xterm-256color")));
        assert!(interactive_from(true, true, None));
        assert!(!interactive_from(true, true, Some("dumb")));
        assert!(!interactive_from(false, true, Some("xterm")));
        assert!(!interactive_from(true, false, Some("xterm")));
    }

    #[test]
    fn viewport_window_keeps_selection_bounded() {
        assert_eq!(viewport_window(0, 0, 5), (0, 0));
        assert_eq!(viewport_window(4, 2, 0), (0, 0));
        assert_eq!(viewport_window(3, 2, 6), (0, 3));
        assert_eq!(viewport_window(10, 0, 5), (0, 5));
        assert_eq!(viewport_window(10, 3, 5), (1, 6));
        assert_eq!(viewport_window(10, 9, 5), (5, 10));
        assert_eq!(viewport_window(10, 99, 5), (5, 10));
    }

    #[test]
    fn select_step_wraps_matches_keys_and_respects_uniqueness() {
        let items = vec![
            item("1", "watch", "follow terminal", true),
            item("2", "lab", "interactive lessons", true),
            item("c", "cat", "locked", false),
            item("d", "dup", "first", true),
            item("d", "dup2", "second", true),
        ];
        let mut sel = 0;
        assert_eq!(step_select(&items, &mut sel, Key::Up), None);
        assert_eq!(sel, 4);
        assert_eq!(step_select(&items, &mut sel, Key::Down), None);
        assert_eq!(sel, 0);
        assert_eq!(step_select(&items, &mut sel, Key::Char('j')), None);
        assert_eq!(sel, 1);
        assert_eq!(step_select(&items, &mut sel, Key::Char('k')), None);
        assert_eq!(sel, 0);
        assert_eq!(step_select(&items, &mut sel, Key::End), None);
        assert_eq!(sel, 4);
        assert_eq!(step_select(&items, &mut sel, Key::Home), None);
        assert_eq!(sel, 0);

        assert_eq!(step_select(&items, &mut sel, Key::Enter), Some(SelectOutcome::Picked(0)));
        assert_eq!(step_select(&items, &mut sel, Key::Esc), Some(SelectOutcome::Cancelled));
        assert_eq!(step_select(&items, &mut sel, Key::Char('q')), Some(SelectOutcome::Cancelled));
        assert_eq!(step_select(&items, &mut sel, Key::CtrlC), Some(SelectOutcome::Eof));
        assert_eq!(step_select(&items, &mut sel, Key::CtrlD), Some(SelectOutcome::Eof));
        assert_eq!(step_select(&items, &mut sel, Key::Char('/')), Some(SelectOutcome::Slash("/".into())));
        assert_eq!(step_select(&items, &mut sel, Key::Char('C')), Some(SelectOutcome::Picked(2)));
        assert_eq!(step_select(&items, &mut sel, Key::Char('d')), None);
    }

    #[test]
    fn select_render_clips_plain_text_and_shows_scroll_indicators() {
        let items: Vec<SelectItem> = (1..=8)
            .map(|i| item(&i.to_string(), &format!("item-{i}"), "very long description detail", i != 4))
            .collect();
        let lines = render_select_lines("Pick an option with a very long title", &items, 4, "↑/↓ move", 4, 24);
        assert!(lines.iter().any(|l| l.contains("↑ ")));
        assert!(lines.iter().any(|l| l.contains("↓ ")));
        assert!(lines.iter().any(|l| l.starts_with("▸ [5]")));
        for line in &lines {
            assert!(line.chars().count() <= 23, "line exceeded cols-1: {line:?}");
            assert!(!line.contains(concat!("\x1b", "7")) && !line.contains(concat!("\x1b", "8")));
        }
    }

    #[test]
    fn select_menu_plain_fallback_covers_all_outcomes() {
        let items = vec![item("1", "watch", "default", true), item("l", "lab", "course", true)];
        let run = |input: &str| {
            let mut r = Cursor::new(input.as_bytes());
            let mut w = Vec::new();
            let out = select_menu_plain("Mode:", &items, "hint", &mut r, &mut w).unwrap();
            (out, String::from_utf8(w).unwrap())
        };
        let (o, printed) = run("2\n");
        assert_eq!(o, SelectOutcome::Picked(1));
        assert!(printed.contains("[1] watch - default") && printed.contains("[2] (l) lab - course"));
        assert_eq!(run("l\n").0, SelectOutcome::Picked(1));
        assert_eq!(run("watch\n").0, SelectOutcome::Picked(0));
        assert_eq!(run("/theme\n").0, SelectOutcome::Slash("/theme".into()));
        assert_eq!(run("\n").0, SelectOutcome::Cancelled);
        assert_eq!(run("q\n").0, SelectOutcome::Cancelled);
        assert_eq!(run("back\n").0, SelectOutcome::Cancelled);
        assert_eq!(run("").0, SelectOutcome::Eof);
    }

    #[test]
    fn editor_state_editing_utf8_and_history_cycle() {
        let mut st = EditorState::new();
        for ch in "café🦀".chars() {
            st.insert_char(ch);
        }
        assert_eq!(st.text(), "café🦀");
        assert_eq!(st.cursor_col(), 5);
        st.move_left();
        st.backspace();
        assert_eq!(st.text(), "caf🦀");
        assert_eq!(st.cursor_col(), 3);
        st.delete();
        assert_eq!(st.text(), "caf");
        st.home();
        st.insert_char('!');
        assert_eq!(st.text(), "!caf");
        st.end();
        assert_eq!(st.cursor_col(), 4);

        st.set_text("/pet species turtle");
        st.kill_word();
        assert_eq!(st.text(), "/pet species ");
        st.kill_word();
        assert_eq!(st.text(), "/pet ");
        st.kill_line();
        assert_eq!(st.text(), "");

        // History preserves draft and wraps cleanly.
        let history = vec!["first".to_string(), "second".to_string()];
        st.set_text("draft");
        st.history_prev(&history);
        assert_eq!(st.text(), "second");
        assert!(!st.dropdown_open);
        st.history_prev(&history);
        assert_eq!(st.text(), "first");
        st.history_prev(&history);
        assert_eq!(st.text(), "first");
        st.history_next(&history);
        assert_eq!(st.text(), "second");
        st.history_next(&history);
        assert_eq!(st.text(), "draft");
    }

    #[test]
    fn editor_step_dropdown_tab_enter_esc_and_history() {
        let mut st = EditorState::new();
        let history = vec!["old cmd".to_string()];

        // Empty buffer Up cycles history.
        assert_eq!(step_editor(&mut st, &history, &[], Key::Up), EditorStep::Continue);
        assert_eq!(st.text(), "old cmd");
        assert_eq!(step_editor(&mut st, &history, &[], Key::Down), EditorStep::Continue);
        assert_eq!(st.text(), "");

        // Typing opens dropdown; Tab applies completion and keeps dropdown open.
        st.insert_char('/');
        st.insert_char('t');
        st.insert_char('h');
        let stage1 = vec![comp("/theme ", "/theme", "Preview or switch theme", true)];
        assert_eq!(step_editor(&mut st, &history, &stage1, Key::Tab), EditorStep::Continue);
        assert_eq!(st.text(), "/theme ");
        assert!(st.dropdown_open);

        // Stage 2: Up/Down moves dropdown highlight; Enter auto-submits selected choice.
        let themes =
            vec![comp("/theme babyblue", "babyblue", "default", true), comp("/theme nord", "nord", "arctic", true)];
        assert_eq!(step_editor(&mut st, &history, &themes, Key::Down), EditorStep::Continue);
        assert_eq!(st.selected, 1);
        assert_eq!(step_editor(&mut st, &history, &themes, Key::Enter), EditorStep::SubmitAuto("/theme nord".into()));

        // Non-auto-submit completion fills buffer instead of submitting partial prefix.
        st.set_text("/pet n");
        st.dropdown_open = true;
        let name_comp = vec![comp("/pet name ", "name ", "Rename Pebble", false)];
        assert_eq!(step_editor(&mut st, &history, &name_comp, Key::Enter), EditorStep::Continue);
        assert_eq!(st.text(), "/pet name ");
        assert!(st.dropdown_open);

        // Esc closes dropdown first, second Esc clears line.
        assert_eq!(step_editor(&mut st, &history, &name_comp, Key::Esc), EditorStep::Continue);
        assert!(!st.dropdown_open);
        assert_eq!(step_editor(&mut st, &history, &name_comp, Key::Esc), EditorStep::Continue);
        assert_eq!(st.text(), "");

        // CtrlD on empty -> Eof; CtrlC -> Interrupted.
        assert_eq!(step_editor(&mut st, &history, &[], Key::CtrlD), EditorStep::Finish(EditorEvent::Eof));
        assert_eq!(step_editor(&mut st, &history, &[], Key::CtrlC), EditorStep::Finish(EditorEvent::Interrupted));
    }

    #[test]
    fn prompt_cursor_dropdown_clipping_and_plain_line_editor() {
        let (line, col) = format_prompt_and_col("huh> ", "ls -la", 6, 80);
        assert_eq!(line, "huh> ls -la");
        assert_eq!(col, 12);

        // Narrow terminal horizontally scrolls without overflowing cols-1.
        let (clipped_line, narrow_col) = format_prompt_and_col("huh> ", "0123456789abcdefghij", 18, 16);
        assert!(clipped_line.chars().count() <= 15);
        assert!(narrow_col <= 16);

        // Dropdown capped at 6 rows and clipped to cols-1.
        let comps: Vec<Completion> = (0..10)
            .map(|i| comp(&format!("/cmd{i}"), &format!("/cmd{i}"), "a rather long summary text", true))
            .collect();
        let rows = render_dropdown_lines(&comps, 2, 26);
        assert_eq!(rows.len(), 6);
        for row in &rows {
            assert!(row.chars().count() <= 25);
        }

        // Plain LineEditor fallback + history dedup & 100-item cap.
        let mut editor = LineEditor::new();
        let mut r = Cursor::new(b"first\r\nfirst\nsecond\n");
        let mut w = Vec::new();
        assert_eq!(read_line_plain(&mut editor, "huh> ", &mut r, &mut w).unwrap(), EditorEvent::Submit("first".into()));
        assert_eq!(read_line_plain(&mut editor, "huh> ", &mut r, &mut w).unwrap(), EditorEvent::Submit("first".into()));
        assert_eq!(
            read_line_plain(&mut editor, "huh> ", &mut r, &mut w).unwrap(),
            EditorEvent::Submit("second".into())
        );
        assert_eq!(read_line_plain(&mut editor, "huh> ", &mut r, &mut w).unwrap(), EditorEvent::Eof);
        assert_eq!(editor.history, vec!["first".to_string(), "second".to_string()]);

        for i in 0..110 {
            editor.push_history(&format!("cmd-{i}"));
        }
        assert_eq!(editor.history.len(), 100);
        assert_eq!(editor.history.first().map(String::as_str), Some("cmd-10"));
        assert_eq!(editor.history.last().map(String::as_str), Some("cmd-109"));
    }

    #[test]
    fn typewriter_skip_preserves_typed_text_and_drops_skip_bytes() {
        let mut editor = LineEditor::new();
        editor.ingest_typewriter_bytes(b" ");
        assert!(editor.prefill.is_empty());
        editor.ingest_typewriter_bytes(b"\r");
        assert!(editor.prefill.is_empty());
        editor.ingest_typewriter_bytes(b"\x1b");
        assert!(editor.prefill.is_empty());
        editor.ingest_typewriter_bytes(b" why");
        assert_eq!(editor.prefill, b"why");
        editor.prefill.clear();
        editor.ingest_typewriter_bytes(b"/theme");
        assert_eq!(editor.prefill, b"/theme");
        assert!(!editor.wait_typewriter_skip(Duration::ZERO));
    }
}
