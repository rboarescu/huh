//! The companion pane's pinned header: the "huh" logo, optional `#!` Pebble companion (32x32
//! Kitty pixel art or 4x10 pure ASCII), and one status line stay at the top of the terminal while
//! everything printed scrolls underneath, using a terminal scroll region (DECSTBM, `ESC[top;bottom r`).
//!
//! It is only used on a terminal at least `MIN_ROWS` tall (and wide enough for the logo), never with
//! `TERM=dumb`, `HUH_NO_PIN=1` or `pin = false` in the config file. The escape sequences are built by
//! the pure functions below; the `Guard` writes them and always puts the terminal back on drop.
//!
//! Cursor handling: the first draw clears the screen and leaves the cursor on the bottom row of the
//! region. A redraw (resize, or after `huh find`) saves the cursor with `ESC 7`, draws the header
//! with absolute positions, sets the region (which homes the cursor) and restores it with `ESC 8`,
//! so the text and the prompt below the header are not disturbed. Nothing else in the pane uses the
//! saved-cursor slot.

use crate::mode::Mode;
use crate::pet::Banner;
use crate::prefs::PaneMode;
use crate::sprite::{self, EffectiveMode, SpriteSheet, ThemeColors};
use crate::style::{self, Role, SpriteRgb};
use crate::welcome;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// The terminal must be at least this tall for the header to be pinned.
pub const MIN_ROWS: u16 = 15;
/// The terminal must be at least this wide for the `#!` pet to appear beside the logo.
pub const PET_MIN_COLS: usize = 38;
/// Left column slot width before the pet starts (22-col L2b logo + 2-space gap -> col 25, 1-based).
const LOGO_SLOT_COLS: usize = 24;
/// 1-based column where the pet starts beside the logo.
const PET_START_COL: u16 = 25;
/// Cell footprint (cols x rows) of the 32x32 Kitty sprite in the header.
const KITTY_COLS: u16 = 8;
const KITTY_ROWS: u16 = 4;

/// Everything needed to render the pinned header (6 or 7 rows) and its `#!` companion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderInfo {
    pub mode: PaneMode,
    pub safety: Mode,
    pub tty_label: String,
    pub pet: Option<Banner>,
    pub pet_mode: EffectiveMode,
    pub tick: u64,
    pub lab_xp: Option<(u64, u32)>,
    pub tip: Option<String>,
}

/// Everything the pin decision depends on.
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

/// Converts `style::SpriteRgb` to `sprite::ThemeColors`.
pub fn theme_colors(rgb: SpriteRgb) -> ThemeColors {
    ThemeColors {
        light: rgb.light,
        accent: rgb.accent,
        warning: rgb.warning,
        success: rgb.success,
        note: rgb.note,
        dim: rgb.dim,
    }
}

// ---------------------------------------------------------------------------
// Escape sequences & header lines (pure)
// ---------------------------------------------------------------------------

fn clip_chars(text: &str, max_cols: usize) -> String {
    text.chars().take(max_cols).collect()
}

fn paint_status_line(clipped: &str) -> String {
    if let Some(rest) = clipped.strip_prefix("  [WATCH]") {
        format!("  {}{}", style::paint(Role::Accent, "[WATCH]"), style::paint(Role::Dim, rest))
    } else if let Some(rest) = clipped.strip_prefix("  [LAB]") {
        format!("  {}{}", style::paint(Role::Accent, "[LAB]"), style::paint(Role::Dim, rest))
    } else {
        style::paint(Role::Dim, clipped)
    }
}

fn paint_tip_line(cols: usize, tip: &str) -> String {
    let clipped = clip_chars(&format!("  tip: {tip}"), cols);
    if let Some(rest) = clipped.strip_prefix("  tip:") {
        format!("  {}{}", style::paint(Role::Note, "tip:"), style::paint(Role::Dim, rest))
    } else if let Some(rest) = clipped.strip_prefix("  ") {
        format!("  {}", style::paint(Role::Note, rest))
    } else {
        clipped
    }
}

/// Lines of the header: the L2b logo (with optional ASCII pet beside it at col 25), its caption,
/// one mode-specific status line, and an optional row-7 tip. Every row is safely clipped by UTF-8
/// char count before painting.
pub fn header_lines_for(cols: usize, info: &HeaderInfo) -> Vec<String> {
    let frame = welcome::frames().pop().unwrap_or_default();
    let pet_shown = info.pet.as_ref().filter(|_| cols >= PET_MIN_COLS && info.pet_mode != EffectiveMode::Off);
    let mut lines: Vec<String> = frame
        .iter()
        .enumerate()
        .map(|(row, line)| {
            if row == welcome::HEIGHT - 1 {
                let caption = match pet_shown {
                    Some(p) => {
                        let suffix = format!(" · {} ({})", p.name, p.mood.name());
                        if line.chars().count() + suffix.chars().count() <= cols {
                            format!("{line}{suffix}")
                        } else {
                            clip_chars(line, cols)
                        }
                    }
                    None => clip_chars(line, cols),
                };
                return style::paint(Role::Dim, &caption);
            }
            match pet_shown {
                Some(p) if info.pet_mode == EffectiveMode::Ascii && row < crate::pet::ART_HEIGHT => {
                    let left = format!("{line:<LOGO_SLOT_COLS$}");
                    let right = crate::pet::art_frame(p.species, p.mood, info.tick)[row].trim_end();
                    let left_len = left.chars().count();
                    if left_len >= cols {
                        style::paint(Role::Accent, &clip_chars(&left, cols))
                    } else {
                        let right_budget = cols - left_len;
                        let right_clipped = clip_chars(right, right_budget);
                        format!("{}{}", style::paint(Role::Accent, &left), style::paint(p.mood.role(), &right_clipped))
                    }
                }
                _ => style::paint(Role::Accent, &clip_chars(line, cols)),
            }
        })
        .collect();

    let safety_label = if info.safety.is_expert() { "expert" } else { "normal" };
    let status = match info.mode {
        PaneMode::Watch => format!("  [WATCH] {} · {safety_label} · /mode · /help", info.tty_label),
        PaneMode::Lab => match info.lab_xp {
            Some((xp, streak)) => format!("  [LAB] XP {xp} · {streak}d · {} · /mode · /help", info.tty_label),
            None => format!("  [LAB] {} · /mode · /help", info.tty_label),
        },
    };
    lines.push(paint_status_line(&clip_chars(&status, cols)));
    if let Some(tip) = &info.tip {
        lines.push(paint_tip_line(cols, tip));
    }
    lines
}

/// Kitty Graphics APC sequence placing the 32x32 sprite at row 1, col 25 (`ESC 7 ... ESC 8`).
pub fn kitty_place_sequence(
    cols: u16,
    pet: &Banner,
    sheet: &SpriteSheet,
    tick: u64,
    colors: &ThemeColors,
    in_tmux: bool,
) -> String {
    if (cols as usize) < PET_MIN_COLS {
        return String::new();
    }
    let rgba = sprite::render_rgba(sheet, pet.mood, tick, colors);
    let encoded = sprite::encode_kitty(&rgba, KITTY_COLS, KITTY_ROWS, in_tmux);
    format!("\x1b7\x1b[1;{PET_START_COL}H{encoded}\x1b8")
}

/// Kitty Graphics APC sequence clearing the header sprite (`i=1`).
pub fn kitty_clear_sequence(in_tmux: bool) -> String {
    sprite::delete_kitty(in_tmux)
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

/// An in-place header update from inside the region without re-emitting DECSTBM (so scroll state stays untouched).
pub fn refresh_sequence(lines: &[String]) -> String {
    format!("\x1b7{}\x1b8", draw(lines))
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
    info: HeaderInfo,
    sheet: Option<SpriteSheet>,
    colors: ThemeColors,
    in_tmux: bool,
    kitty_drawn: bool,
    rows: u16,
    cols: u16,
    header_rows: usize,
    /// The region is set.
    active: bool,
    /// Taken off for a full-screen program (`huh find`).
    paused: bool,
}

impl State {
    fn apply_refresh(&mut self, info: &HeaderInfo, new_colors: ThemeColors, cols: u16) -> Option<String> {
        let species_changed = self.info.pet.as_ref().map(|p| p.species) != info.pet.as_ref().map(|p| p.species);
        if species_changed || (self.sheet.is_none() && info.pet.is_some()) {
            self.sheet = load_sheet_for(info.pet.as_ref());
        }
        let prev_info = self.info.clone();
        let prev_colors = self.colors;
        let was_kitty = self.kitty_drawn;
        let prev_header_rows = self.header_rows;
        self.info = info.clone();
        self.colors = new_colors;

        if !self.active || self.paused {
            return None;
        }

        let cols_changed = cols != self.cols;
        self.cols = cols;

        let want_kitty = wants_kitty(info, cols) && self.sheet.is_some();
        let mut seq = String::new();
        if was_kitty && !want_kitty {
            seq.push_str(&kitty_clear_sequence(self.in_tmux));
            self.kitty_drawn = false;
        }

        let lines = header_lines_for(cols as usize, info);
        let new_rows = lines.len();

        let ascii_active = info.pet_mode == EffectiveMode::Ascii && cols as usize >= PET_MIN_COLS && info.pet.is_some();
        let ascii_art_changed = ascii_active
            && (prev_info.pet_mode != EffectiveMode::Ascii
                || prev_info.pet != info.pet
                || info.pet.as_ref().zip(prev_info.pet.as_ref()).is_some_and(|(a, b)| {
                    crate::pet::art_frame(a.species, a.mood, info.tick)
                        != crate::pet::art_frame(b.species, b.mood, prev_info.tick)
                }));

        let base_text_changed = cols_changed
            || prev_info.mode != info.mode
            || prev_info.safety != info.safety
            || prev_info.tty_label != info.tty_label
            || prev_info.pet != info.pet
            || prev_info.pet_mode != info.pet_mode
            || prev_info.lab_xp != info.lab_xp
            || prev_colors != new_colors
            || (was_kitty && !want_kitty)
            || ascii_art_changed;

        let tip_changed = prev_info.tip != info.tip;
        if new_rows != prev_header_rows {
            self.header_rows = new_rows;
            seq.push_str(&redraw_sequence(&lines, self.rows));
            if new_rows < prev_header_rows {
                seq.push_str("\x1b7");
                for r in new_rows..prev_header_rows {
                    seq.push_str(&format!("\x1b[{};1H\x1b[2K", r + 1));
                }
                seq.push_str("\x1b8");
            }
            if want_kitty && let (Some(pet), Some(sheet)) = (&info.pet, &self.sheet) {
                seq.push_str(&kitty_place_sequence(cols, pet, sheet, info.tick, &self.colors, self.in_tmux));
                self.kitty_drawn = true;
            }
            return (!seq.is_empty()).then_some(seq);
        }

        if base_text_changed {
            seq.push_str(&refresh_sequence(&lines));
        } else if tip_changed && let Some(line7) = lines.last() {
            seq.push_str(&format!("\x1b7\x1b[{new_rows};1H\x1b[2K{line7}\x1b8"));
        }

        if want_kitty && let (Some(pet), Some(sheet)) = (&info.pet, &self.sheet) {
            let pixel_changed = !was_kitty
                || cols_changed
                || prev_colors != new_colors
                || prev_info.pet != info.pet
                || prev_info.pet.as_ref().is_none_or(|prev| {
                    sprite::frame_key(prev.mood, prev_info.tick) != sprite::frame_key(pet.mood, info.tick)
                });
            if pixel_changed || base_text_changed {
                seq.push_str(&kitty_place_sequence(cols, pet, sheet, info.tick, &self.colors, self.in_tmux));
                self.kitty_drawn = true;
            }
        }

        (!seq.is_empty()).then_some(seq)
    }

    fn apply_resize(&mut self, size: (u16, u16)) -> Option<String> {
        if self.paused {
            return None;
        }
        self.rows = size.0;
        self.cols = size.1;
        if fits(size) {
            self.active = true;
            let lines = header_lines_for(size.1 as usize, &self.info);
            self.header_rows = lines.len();
            let mut seq = String::new();
            if self.kitty_drawn && !wants_kitty(&self.info, size.1) {
                seq.push_str(&kitty_clear_sequence(self.in_tmux));
                self.kitty_drawn = false;
            }
            seq.push_str(&redraw_sequence(&lines, size.0));
            if wants_kitty(&self.info, size.1) {
                if self.sheet.is_none() {
                    self.sheet = load_sheet_for(self.info.pet.as_ref());
                }
                if let (Some(pet), Some(sheet)) = (&self.info.pet, &self.sheet) {
                    seq.push_str(&kitty_place_sequence(size.1, pet, sheet, self.info.tick, &self.colors, self.in_tmux));
                    self.kitty_drawn = true;
                }
            }
            Some(seq)
        } else if self.active {
            self.active = false;
            let mut seq = String::new();
            if self.kitty_drawn {
                seq.push_str(&kitty_clear_sequence(self.in_tmux));
                self.kitty_drawn = false;
            }
            seq.push_str(&unpin_sequence(size.0));
            Some(seq)
        } else {
            None
        }
    }

    fn apply_suspend(&mut self, rows: u16) -> String {
        if !self.active || self.paused {
            return String::new();
        }
        self.paused = true;
        let mut seq = String::new();
        if self.kitty_drawn {
            seq.push_str(&kitty_clear_sequence(self.in_tmux));
            self.kitty_drawn = false;
        }
        seq.push_str(&unpin_sequence(rows));
        seq
    }

    fn apply_resume(&mut self, size: Option<(u16, u16)>, new_colors: ThemeColors) -> String {
        if !self.paused {
            return String::new();
        }
        self.paused = false;
        self.colors = new_colors;
        match size.filter(|s| fits(*s)) {
            Some(size) => {
                self.rows = size.0;
                self.cols = size.1;
                self.active = true;
                let lines = header_lines_for(size.1 as usize, &self.info);
                self.header_rows = lines.len();
                let mut seq = redraw_sequence(&lines, size.0);
                if wants_kitty(&self.info, size.1) {
                    if self.sheet.is_none() {
                        self.sheet = load_sheet_for(self.info.pet.as_ref());
                    }
                    if let (Some(pet), Some(sheet)) = (&self.info.pet, &self.sheet) {
                        seq.push_str(&kitty_place_sequence(
                            size.1,
                            pet,
                            sheet,
                            self.info.tick,
                            &self.colors,
                            self.in_tmux,
                        ));
                        self.kitty_drawn = true;
                    }
                }
                seq
            }
            None => {
                self.active = false;
                String::new()
            }
        }
    }

    fn drop_sequence(&self, rows: Option<u16>) -> String {
        let mut seq = String::new();
        if self.kitty_drawn {
            seq.push_str(&kitty_clear_sequence(self.in_tmux));
        }
        seq.push_str(&reset_sequence(rows));
        seq
    }
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static RESIZED: AtomicBool = AtomicBool::new(false);

fn state() -> std::sync::MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn fits(size: (u16, u16)) -> bool {
    size.0 >= MIN_ROWS && size.1 as usize >= welcome::art_width()
}

fn load_sheet_for(pet: Option<&Banner>) -> Option<SpriteSheet> {
    let override_dir = sprite::override_dir();
    pet.map(|p| sprite::load_sheet(p.species, override_dir.as_deref()))
}

fn wants_kitty(info: &HeaderInfo, cols: u16) -> bool {
    info.pet_mode == EffectiveMode::Pixel && cols as usize >= PET_MIN_COLS && info.pet.is_some()
}

/// While it lives the header is pinned; dropping it deletes any active Kitty image, resets the
/// scroll region, and moves the cursor below.
pub struct Guard;

impl Guard {
    /// Pins the header now using `info`. `None` when the terminal size cannot be read or does not fit.
    pub fn start(info: HeaderInfo) -> Option<Guard> {
        let size = size().filter(|s| fits(*s))?;
        let sheet = load_sheet_for(info.pet.as_ref());
        let colors = theme_colors(style::active_sprite_rgb());
        let in_tmux = sprite::in_tmux_env();
        let lines = header_lines_for(size.1 as usize, &info);
        let header_rows = lines.len();
        let mut seq = first_sequence(&lines, size.0);
        let mut kitty_drawn = false;
        if wants_kitty(&info, size.1)
            && let (Some(pet), Some(sheet)) = (&info.pet, &sheet)
        {
            seq.push_str(&kitty_place_sequence(size.1, pet, sheet, info.tick, &colors, in_tmux));
            kitty_drawn = true;
        }
        let mut out = std::io::stdout();
        let _ = write!(out, "{seq}");
        let _ = out.flush();
        *state() = Some(State {
            info,
            sheet,
            colors,
            in_tmux,
            kitty_drawn,
            rows: size.0,
            cols: size.1,
            header_rows,
            active: true,
            paused: false,
        });
        RESIZED.store(false, Ordering::Relaxed);
        watch_resize();
        Some(Guard)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let rows = size().map(|s| s.0).or_else(|| state().as_ref().map(|s| s.rows));
        let seq = state().as_ref().map_or_else(|| reset_sequence(rows), |st| st.drop_sequence(rows));
        *state() = None;
        unwatch_resize();
        let mut out = std::io::stdout();
        let _ = write!(out, "{seq}");
        let _ = out.flush();
    }
}

/// If the terminal was resized since the last call, the sequence that redraws the header for the
/// new size (or takes the region off and clears any Kitty sprite when the terminal became too small).
pub fn on_resize() -> Option<String> {
    if !RESIZED.swap(false, Ordering::Relaxed) {
        return None;
    }
    let size = size()?;
    let mut guard = state();
    let st = guard.as_mut()?;
    st.apply_resize(size)
}

/// Forces the active Kitty sprite sheet to reload from `~/.config/huh/sprites` on the next refresh.
pub fn reload_sprite_sheet() {
    if let Some(st) = state().as_mut() {
        st.sheet = load_sheet_for(st.info.pet.as_ref());
    }
}

/// Updates the pinned header in place using `ESC 7 ... ESC 8` (never touching DECSTBM!).
/// Emits `delete_kitty` when leaving `Pixel` mode or shrinking below `PET_MIN_COLS`.
pub fn refresh_pet_frame(info: &HeaderInfo) -> Option<String> {
    let mut guard = state();
    let st = guard.as_mut()?;
    let cols = size().map_or(st.cols, |s| s.1);
    st.apply_refresh(info, theme_colors(style::active_sprite_rgb()), cols)
}

/// Before a full-screen program takes the terminal (`huh find`): clears any Kitty sprite and turns the region off.
pub fn suspend() -> String {
    let mut guard = state();
    let Some(st) = guard.as_mut() else { return String::new() };
    let rows = size().map_or(st.rows, |s| s.0);
    st.apply_suspend(rows)
}

/// After it: the sequence that draws the header, Kitty sprite (if applicable), and scroll region again.
pub fn resume() -> String {
    let mut guard = state();
    let Some(st) = guard.as_mut() else { return String::new() };
    st.apply_resume(size(), theme_colors(style::active_sprite_rgb()))
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

    fn watch_info(pet: Option<Banner>, pet_mode: EffectiveMode) -> HeaderInfo {
        HeaderInfo {
            mode: PaneMode::Watch,
            safety: Mode::Normal,
            tty_label: "/dev/pts/7".into(),
            pet,
            pet_mode,
            tick: 0,
            lab_xp: None,
            tip: None,
        }
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
    fn header_watch_and_lab_status_lines_and_logo_bounds() {
        let watch = header_lines_for(100, &watch_info(None, EffectiveMode::Off));
        assert_eq!(watch.len(), welcome::HEIGHT + 1);
        assert!(watch.iter().all(|l| !l.contains('\x1b')), "plain in tests");
        assert!(watch[0..4].iter().any(|l| l.contains('█')));
        assert!(watch[4].contains("huh"));
        assert_eq!(watch[5], "  [WATCH] /dev/pts/7 · normal · /mode · /help");

        let expert_watch = header_lines_for(
            100,
            &HeaderInfo { safety: Mode::Expert, tty_label: "no hook".into(), ..watch_info(None, EffectiveMode::Off) },
        );
        assert_eq!(expert_watch[5], "  [WATCH] no hook · expert · /mode · /help");

        let lab_xp = header_lines_for(
            100,
            &HeaderInfo {
                mode: PaneMode::Lab,
                safety: Mode::Normal,
                tty_label: "/dev/pts/7".into(),
                pet: None,
                pet_mode: EffectiveMode::Off,
                tick: 0,
                lab_xp: Some((250, 4)),
                tip: None,
            },
        );
        assert_eq!(lab_xp[5], "  [LAB] XP 250 · 4d · /dev/pts/7 · /mode · /help");

        let lab_plain = header_lines_for(
            100,
            &HeaderInfo {
                mode: PaneMode::Lab,
                safety: Mode::Normal,
                tty_label: "no hook".into(),
                pet: None,
                pet_mode: EffectiveMode::Off,
                tick: 0,
                lab_xp: None,
                tip: None,
            },
        );
        assert_eq!(lab_plain[5], "  [LAB] no hook · /mode · /help");

        // Never wider than the terminal.
        for line in header_lines_for(32, &watch_info(None, EffectiveMode::Off)) {
            assert!(line.chars().count() <= 32, "{line}");
        }
    }

    #[test]
    fn header_with_pet_layout_col_25_and_narrow_fallback() {
        let banner = Banner {
            name: crate::pet::DEFAULT_NAME.into(),
            species: crate::pet::Species::Penguin,
            mood: crate::pet::Mood::Happy,
        };
        let wide = header_lines_for(60, &watch_info(Some(banner.clone()), EffectiveMode::Ascii));
        assert_eq!(wide.len(), welcome::HEIGHT + 1);
        assert!(wide.iter().all(|l| !l.contains('\x1b')), "plain in tests");
        // Pet starts at 0-based index 24 (1-based column 25): left L2b slot padded to 24 chars.
        let row2_chars: Vec<char> = wide[2].chars().collect();
        assert_eq!(row2_chars.get(24), Some(&' '), "col 25 starts penguin body row 2: {:?}", wide[2]);
        assert_eq!(row2_chars.get(25), Some(&'/'), "col 26 has penguin slash: {:?}", wide[2]);
        assert!(wide[0..4].join("\n").contains("#!"), "shows ASCII pet #!");
        assert!(wide[4].contains("Pebble (happy)"), "{}", wide[4]);
        for line in &wide {
            assert!(line.chars().count() <= 60, "{line}");
        }

        // Pixel mode keeps rows 0..4 clear on the right while still showing Pebble in row 4 caption.
        let pixel = header_lines_for(60, &watch_info(Some(banner.clone()), EffectiveMode::Pixel));
        assert!(!pixel[0..4].join("\n").contains("#!"), "pixel mode leaves right side clear");
        assert!(pixel[4].contains("Pebble (happy)"), "{}", pixel[4]);

        // Off mode hides pet everywhere.
        let off = header_lines_for(60, &watch_info(Some(banner.clone()), EffectiveMode::Off));
        assert_eq!(off, header_lines_for(60, &watch_info(None, EffectiveMode::Off)));

        let exact = header_lines_for(PET_MIN_COLS, &watch_info(Some(banner.clone()), EffectiveMode::Ascii));
        assert!(exact[0..4].join("\n").contains("#!"));
        for line in &exact {
            assert!(line.chars().count() <= PET_MIN_COLS, "{line}");
        }

        let narrow = header_lines_for(PET_MIN_COLS - 3, &watch_info(Some(banner), EffectiveMode::Ascii));
        assert_eq!(narrow, header_lines_for(PET_MIN_COLS - 3, &watch_info(None, EffectiveMode::Off)));
        assert!(!narrow[0..4].join("\n").contains("#!"));
    }

    #[test]
    fn header_with_accented_utf8_pet_name_never_overflows_across_widths() {
        let mut pet = crate::pet::Pet::default();
        let name = "Éclair-Ștefan-Crème!";
        assert_eq!(name.chars().count(), 20);
        assert!(name.len() > 20, "must exercise multi-byte UTF-8");
        pet.rename(name).unwrap();

        for species in crate::pet::Species::ALL {
            for mood in crate::pet::Mood::ALL {
                let banner = Banner { name: pet.name.clone(), species, mood };
                for cols in 33..=120 {
                    let info = HeaderInfo {
                        mode: PaneMode::Lab,
                        safety: Mode::Expert,
                        tty_label: "/dev/pts/12".into(),
                        pet: Some(banner.clone()),
                        pet_mode: EffectiveMode::Ascii,
                        tick: 7,
                        lab_xp: Some((999, 12)),
                        tip: None,
                    };
                    let lines = header_lines_for(cols, &info);
                    assert_eq!(lines.len(), welcome::HEIGHT + 1, "{species:?} {mood:?} cols={cols}");
                    for (row, line) in lines.iter().enumerate() {
                        assert!(!line.contains('\x1b'), "plain in tests");
                        assert!(
                            line.chars().count() <= cols,
                            "{species:?} {mood:?} cols={cols} row={row} overflowed ({}): {line:?}",
                            line.chars().count()
                        );
                    }
                    if cols < PET_MIN_COLS {
                        assert!(!lines[0..4].join("\n").contains("#!"), "{species:?} {mood:?} cols={cols}");
                    } else {
                        assert!(lines[0..4].join("\n").contains("#!"), "{species:?} {mood:?} cols={cols}");
                    }
                    if cols == 120 {
                        assert!(lines[4].contains(name), "expected full name at cols=120: {}", lines[4]);
                        assert!(lines[4].contains(mood.name()), "expected mood at cols=120: {}", lines[4]);
                    }
                }
            }
        }
    }

    #[test]
    fn kitty_place_and_clear_sequences_respect_width_and_tmux() {
        let banner = Banner {
            name: crate::pet::DEFAULT_NAME.into(),
            species: crate::pet::Species::Penguin,
            mood: crate::pet::Mood::Idle,
        };
        let sheet = sprite::load_sheet(crate::pet::Species::Penguin, None);
        let colors = ThemeColors::default();

        assert_eq!(kitty_place_sequence((PET_MIN_COLS - 1) as u16, &banner, &sheet, 0, &colors, false), "");

        let direct = kitty_place_sequence(PET_MIN_COLS as u16, &banner, &sheet, 0, &colors, false);
        assert!(direct.starts_with("\x1b7\x1b[1;25H\x1b_Ga=T,f=32,s=32,v=32,o=z,i=1,r=4,c=8,C=1,q=2;"), "{direct}");
        assert!(direct.ends_with("\x1b\\\x1b8"), "{direct}");
        assert!(!direct.contains("r\x1b"), "must never touch DECSTBM");

        let tmux = kitty_place_sequence(80, &banner, &sheet, 0, &colors, true);
        assert!(tmux.starts_with("\x1b7\x1b[1;25H\x1bPtmux;\x1b\x1b_G"), "{tmux}");
        assert!(tmux.ends_with("\x1b\x1b\\\x1b\\\x1b8"), "{tmux}");

        assert_eq!(kitty_clear_sequence(false), "\x1b_Ga=d,d=i,i=1,q=2\x1b\\");
        assert_eq!(kitty_clear_sequence(true), "\x1bPtmux;\x1b\x1b_Ga=d,d=i,i=1,q=2\x1b\x1b\\\x1b\\");
    }

    #[test]
    fn kitty_lifecycle_emits_delete_on_shrink_switch_suspend_and_drop() {
        let banner = Banner {
            name: crate::pet::DEFAULT_NAME.into(),
            species: crate::pet::Species::Penguin,
            mood: crate::pet::Mood::Happy,
        };
        let sheet = sprite::load_sheet(crate::pet::Species::Penguin, None);
        let colors = ThemeColors::default();
        let pixel_info = watch_info(Some(banner.clone()), EffectiveMode::Pixel);

        let mut st = State {
            info: pixel_info.clone(),
            sheet: Some(sheet),
            colors,
            in_tmux: false,
            kitty_drawn: true,
            rows: 30,
            cols: 80,
            header_rows: welcome::HEIGHT + 1,
            active: true,
            paused: false,
        };

        // Tick animation in Pixel mode updates Kitty without touching DECSTBM or erasing rows.
        let tick1 = HeaderInfo { tick: 1, ..pixel_info.clone() };
        let tick_seq = st.apply_refresh(&tick1, colors, 80).unwrap();
        assert!(tick_seq.contains("\x1b[1;25H\x1b_Ga=T"), "{tick_seq}");
        assert!(!tick_seq.contains("\x1b[2K"), "pixel tick avoids clearing text rows");
        assert!(!tick_seq.contains(";30r"), "must never emit DECSTBM");

        // Switch from Pixel -> Ascii clears Kitty image first, then refreshes ASCII header.
        let ascii_info = HeaderInfo { pet_mode: EffectiveMode::Ascii, ..tick1.clone() };
        let to_ascii = st.apply_refresh(&ascii_info, colors, 80).unwrap();
        assert!(to_ascii.starts_with("\x1b_Ga=d,d=i,i=1,q=2\x1b\\"), "{to_ascii}");
        assert!(!st.kitty_drawn);
        assert!(to_ascii.contains("#!"));

        // Switch Ascii -> Off stops showing pet.
        let off_info = HeaderInfo { pet_mode: EffectiveMode::Off, ..ascii_info.clone() };
        let to_off = st.apply_refresh(&off_info, colors, 80).unwrap();
        assert!(!to_off.contains("#!"));
        assert!(!st.kitty_drawn);

        // Back to Pixel, then resize < PET_MIN_COLS clears Kitty image.
        let back_pixel = st.apply_refresh(&pixel_info, colors, 80).unwrap();
        assert!(back_pixel.contains("\x1b_Ga=T"));
        assert!(st.kitty_drawn);
        let narrow = st.apply_resize((30, (PET_MIN_COLS - 2) as u16)).unwrap();
        assert!(narrow.starts_with("\x1b_Ga=d,d=i,i=1,q=2\x1b\\"), "{narrow}");
        assert!(!st.kitty_drawn);

        // Resize back >= PET_MIN_COLS restores Kitty image; suspend clears it; resume restores it.
        let wide = st.apply_resize((30, 80)).unwrap();
        assert!(wide.contains("\x1b[1;25H\x1b_Ga=T"), "{wide}");
        assert!(st.kitty_drawn);

        let suspended = st.apply_suspend(30);
        assert!(suspended.starts_with("\x1b_Ga=d,d=i,i=1,q=2\x1b\\"), "{suspended}");
        assert!(!st.kitty_drawn);
        assert!(st.paused);

        let resumed = st.apply_resume(Some((30, 80)), colors);
        assert!(resumed.contains("\x1b[1;25H\x1b_Ga=T"), "{resumed}");
        assert!(st.kitty_drawn);

        // Drop clears Kitty image before resetting scroll region.
        let dropped = st.drop_sequence(Some(30));
        let del_pos = dropped.find("\x1b_Ga=d,d=i,i=1,q=2\x1b\\").unwrap();
        let reset_pos = dropped.find("\x1b[r").unwrap();
        assert!(del_pos < reset_pos, "{dropped}");
    }

    #[test]
    fn refresh_updates_in_place_without_decstbm() {
        let lines = vec!["A".to_string(), "B".to_string()];
        let seq = refresh_sequence(&lines);
        assert_eq!(seq, "\x1b7\x1b[1;1H\x1b[2KA\x1b[2;1H\x1b[2KB\x1b8");
        assert!(seq.starts_with("\x1b7") && seq.ends_with("\x1b8"));
        assert!(!seq.contains("\x1b[2J") && !seq.contains('\n') && !seq.contains('r'));
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

    #[test]
    fn header_tip_row_clips_plain_text_across_widths() {
        let tip = "Éclair tip: press / to browse commands and ↑/↓ to navigate selectors!";
        for cols in 33..=120 {
            let info = HeaderInfo { tip: Some(tip.to_string()), ..watch_info(None, EffectiveMode::Off) };
            let lines = header_lines_for(cols, &info);
            assert_eq!(lines.len(), welcome::HEIGHT + 2, "cols={cols}");
            for (row, line) in lines.iter().enumerate() {
                assert!(!line.contains('\x1b'), "plain in tests");
                assert!(
                    line.chars().count() <= cols,
                    "cols={cols} row={row} overflowed ({}): {line:?}",
                    line.chars().count()
                );
            }
            assert!(lines[6].starts_with("  tip: "), "{:?}", lines[6]);
            if cols == 120 {
                assert_eq!(lines[6], format!("  tip: {tip}"));
            }
        }
        assert_eq!(paint_tip_line(5, tip), "  tip");
        assert_eq!(paint_tip_line(2, tip), "  ");
    }

    #[test]
    fn tip_rotation_refreshes_only_row_seven_without_flicker_or_kitty_reemit() {
        let banner = Banner {
            name: crate::pet::DEFAULT_NAME.into(),
            species: crate::pet::Species::Penguin,
            mood: crate::pet::Mood::Happy,
        };
        let sheet = sprite::load_sheet(crate::pet::Species::Penguin, None);
        let colors = ThemeColors::default();
        let info_a = HeaderInfo { tip: Some("First tip".into()), ..watch_info(Some(banner), EffectiveMode::Pixel) };

        let mut st = State {
            info: info_a.clone(),
            sheet: Some(sheet),
            colors,
            in_tmux: false,
            kitty_drawn: true,
            rows: 30,
            cols: 80,
            header_rows: welcome::HEIGHT + 2,
            active: true,
            paused: false,
        };

        let info_b = HeaderInfo { tip: Some("Second tip".into()), ..info_a };
        let seq = st.apply_refresh(&info_b, colors, 80).unwrap();
        assert_eq!(seq, "\x1b7\x1b[7;1H\x1b[2K  tip: Second tip\x1b8");
        assert!(!seq.contains("[1;1H") && !seq.contains(";30r") && !seq.contains("_Ga=T"));
        assert_eq!(st.apply_refresh(&info_b, colors, 80), None);
    }

    #[test]
    fn tip_toggle_transitions_six_and_seven_rows_with_decstbm_and_kitty() {
        let banner = Banner {
            name: crate::pet::DEFAULT_NAME.into(),
            species: crate::pet::Species::Penguin,
            mood: crate::pet::Mood::Happy,
        };
        let sheet = sprite::load_sheet(crate::pet::Species::Penguin, None);
        let colors = ThemeColors::default();
        let base = watch_info(Some(banner), EffectiveMode::Pixel);

        let mut st = State {
            info: base.clone(),
            sheet: Some(sheet),
            colors,
            in_tmux: false,
            kitty_drawn: true,
            rows: 30,
            cols: 80,
            header_rows: welcome::HEIGHT + 1,
            active: true,
            paused: false,
        };

        // 6 -> 7 transition updates header_rows, emits DECSTBM 8;30r, and re-emits Kitty sprite.
        let with_tip = HeaderInfo { tip: Some("Use /theme nord".into()), ..base.clone() };
        let grow = st.apply_refresh(&with_tip, colors, 80).unwrap();
        assert_eq!(st.header_rows, 7);
        assert!(grow.contains("\x1b[8;30r"), "{grow}");
        assert!(grow.contains("\x1b[7;1H\x1b[2K  tip: Use /theme nord"), "{grow}");
        assert!(grow.contains("\x1b[1;25H\x1b_Ga=T"), "{grow}");

        // 7 -> 6 transition updates header_rows, emits DECSTBM 7;30r, clears stale row 7, and re-emits Kitty sprite.
        let shrink = st.apply_refresh(&base, colors, 80).unwrap();
        assert_eq!(st.header_rows, 6);
        assert!(shrink.contains("\x1b[7;30r"), "{shrink}");
        assert!(shrink.contains("\x1b[7;1H\x1b[2K"), "{shrink}");
        assert!(shrink.contains("\x1b[1;25H\x1b_Ga=T"), "{shrink}");
    }
}
