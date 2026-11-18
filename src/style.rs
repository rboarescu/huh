//! Colour themes. Output is styled by semantic role (a command line, a flag, a WARNING label ...),
//! never by raw colour; a theme decides what each role looks like. With colour off every function
//! returns the plain text unchanged, so plain output is byte-identical with and without this module.
//!
//! Nothing is styled until `init` has run, which keeps unit tests and every pipe plain.

use anyhow::{Result, bail};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// A command line to run.
    Command,
    /// Plain description text.
    Description,
    /// A flag or argument being explained.
    Part,
    /// A value such as a confidence percentage or an exit code.
    Value,
    /// The WARNING label and its text.
    Warning,
    /// A risk category such as `[destroys data]`.
    Risk,
    /// A note or hint.
    Note,
    /// The cause given by `huh why`.
    Cause,
    /// The `Try:` line.
    Try,
    /// Secondary text.
    Dim,
    /// A block or section header (also the tutor's per-command header).
    Header,
    /// Something that went fine.
    Success,
    /// The logo and other accent.
    Accent,
}

const ROLES: usize = 13;

fn index(role: Role) -> usize {
    role as usize
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Col {
    Default,
    /// SGR foreground code: 30-37 or 90-97.
    Ansi(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Look {
    col: Col,
    bold: bool,
    dim: bool,
    underline: bool,
}

const PLAIN: Look = Look { col: Col::Default, bold: false, dim: false, underline: false };

impl Look {
    fn col(col: Col) -> Look {
        Look { col, ..PLAIN }
    }
    fn bold(col: Col) -> Look {
        Look { col, bold: true, ..PLAIN }
    }
    fn sgr(self, truecolor: bool) -> String {
        let mut codes: Vec<String> = Vec::new();
        if self.bold {
            codes.push("1".into());
        }
        if self.dim {
            codes.push("2".into());
        }
        if self.underline {
            codes.push("4".into());
        }
        match self.col {
            Col::Default => {}
            Col::Ansi(n) => codes.push(n.to_string()),
            Col::Rgb(r, g, b) if truecolor => codes.push(format!("38;2;{r};{g};{b}")),
            Col::Rgb(r, g, b) => codes.push(format!("38;5;{}", nearest_256(r, g, b))),
        }
        codes.join(";")
    }
}

/// The xterm-256 colour nearest to an RGB value: the best of the 6x6x6 cube (16-231) and the
/// grey ramp (232-255).
fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [i32; 6] = [0, 95, 135, 175, 215, 255];
    let level = |v: u8| (0..6).min_by_key(|&i| (LEVELS[i] - v as i32).abs()).unwrap_or(0);
    let dist = |(r2, g2, b2): (i32, i32, i32)| (r2 - r as i32).pow(2) + (g2 - g as i32).pow(2) + (b2 - b as i32).pow(2);
    let (ri, gi, bi) = (level(r), level(g), level(b));
    let cube = dist((LEVELS[ri], LEVELS[gi], LEVELS[bi]));
    let cube_index = 16 + 36 * ri + 6 * gi + bi;
    let grey_step = (0..24).min_by_key(|&i| dist(grey(i))).unwrap_or(0);
    if dist(grey(grey_step)) < cube { (232 + grey_step) as u8 } else { cube_index as u8 }
}

fn grey(step: usize) -> (i32, i32, i32) {
    let v = 8 + 10 * step as i32;
    (v, v, v)
}

/// What the terminal can show: whether it has 24-bit colour, and whether its background is light.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TermEnv {
    pub truecolor: bool,
    pub light: bool,
}

impl Default for TermEnv {
    /// Truecolour on a dark background: what most terminals are.
    fn default() -> TermEnv {
        TermEnv { truecolor: true, light: false }
    }
}

/// 24-bit colour: `COLORTERM` says truecolor or 24bit, or `TERM` names a direct-colour terminal.
pub fn truecolor_supported(colorterm: Option<&str>, term: Option<&str>) -> bool {
    colorterm.is_some_and(|c| matches!(c.trim().to_ascii_lowercase().as_str(), "truecolor" | "24bit"))
        || term.is_some_and(|t| t.contains("direct"))
}

/// A light background, from `COLORFGBG` ("15;0", "0;default;15"): the last field is the background
/// colour number, and 7 and 15 are the light ones. Absent or unreadable means "not known", so dark.
pub fn light_background(colorfgbg: Option<&str>) -> bool {
    colorfgbg
        .and_then(|v| v.rsplit(';').next())
        .and_then(|bg| bg.trim().parse::<u8>().ok())
        .is_some_and(|bg| bg == 7 || bg == 15)
}

impl TermEnv {
    pub fn detect() -> TermEnv {
        TermEnv {
            truecolor: truecolor_supported(
                std::env::var("COLORTERM").ok().as_deref(),
                std::env::var("TERM").ok().as_deref(),
            ),
            light: light_background(std::env::var("COLORFGBG").ok().as_deref()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    BabyBlue,
    Auto,
    Omarchy,
    Dark,
    Light,
    Nord,
    Gruvbox,
    Catppuccin,
    Mono,
}

pub const VALID: &str = "babyblue, auto, omarchy, dark, light, nord, gruvbox, catppuccin, mono";
pub const ALL: [Theme; 9] = [
    Theme::BabyBlue,
    Theme::Auto,
    Theme::Omarchy,
    Theme::Dark,
    Theme::Light,
    Theme::Nord,
    Theme::Gruvbox,
    Theme::Catppuccin,
    Theme::Mono,
];

impl Theme {
    pub fn parse(value: &str) -> Result<Theme> {
        Ok(match value.trim().to_ascii_lowercase().as_str() {
            "babyblue" => Theme::BabyBlue,
            "auto" => Theme::Auto,
            "omarchy" => Theme::Omarchy,
            "dark" => Theme::Dark,
            "light" => Theme::Light,
            "nord" => Theme::Nord,
            "gruvbox" => Theme::Gruvbox,
            "catppuccin" => Theme::Catppuccin,
            "mono" => Theme::Mono,
            other => bail!("unknown theme \"{other}\"; the valid themes are: {VALID}"),
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Theme::BabyBlue => "babyblue",
            Theme::Auto => "auto",
            Theme::Omarchy => "omarchy",
            Theme::Dark => "dark",
            Theme::Light => "light",
            Theme::Nord => "nord",
            Theme::Gruvbox => "gruvbox",
            Theme::Catppuccin => "catppuccin",
            Theme::Mono => "mono",
        }
    }
}

/// The colours a theme provides; roles are mapped onto them in one place (`Palette::looks`).
#[derive(Clone, Copy, Debug)]
struct Palette {
    accent: Col,
    red: Col,
    yellow: Col,
    green: Col,
    cyan: Col,
    blue: Col,
    magenta: Col,
    /// Secondary text: a colour, or `Default` meaning "the default colour, dimmed".
    gray: Col,
}

fn hex(value: u32) -> Col {
    Col::Rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

impl Palette {
    /// Only the terminal's own 16 colours, so the terminal's palette decides how it looks.
    fn auto() -> Palette {
        Palette {
            accent: Col::Ansi(35),
            red: Col::Ansi(31),
            yellow: Col::Ansi(33),
            green: Col::Ansi(32),
            cyan: Col::Ansi(36),
            blue: Col::Ansi(34),
            magenta: Col::Ansi(35),
            gray: Col::Default,
        }
    }

    fn builtin(theme: Theme) -> Option<Palette> {
        #[allow(clippy::too_many_arguments)]
        let p = |accent, red, yellow, green, cyan, blue, magenta, gray| Palette {
            accent: hex(accent),
            red: hex(red),
            yellow: hex(yellow),
            green: hex(green),
            cyan: hex(cyan),
            blue: hex(blue),
            magenta: hex(magenta),
            gray: hex(gray),
        };
        Some(match theme {
            // One Dark / One Light.
            Theme::Dark => p(0x61afef, 0xe06c75, 0xe5c07b, 0x98c379, 0x56b6c2, 0x61afef, 0xc678dd, 0x7f848e),
            Theme::Light => p(0x4078f2, 0xe45649, 0xc18401, 0x50a14f, 0x0184bc, 0x4078f2, 0xa626a4, 0x7f7f7f),
            Theme::Nord => p(0x88c0d0, 0xbf616a, 0xebcb8b, 0xa3be8c, 0x88c0d0, 0x81a1c1, 0xb48ead, 0x616e88),
            Theme::Gruvbox => p(0xfe8019, 0xfb4934, 0xfabd2f, 0xb8bb26, 0x8ec07c, 0x83a598, 0xd3869b, 0x928374),
            // Catppuccin Mocha.
            Theme::Catppuccin => p(0xcba6f7, 0xf38ba8, 0xf9e2af, 0xa6e3a1, 0x94e2d5, 0x89b4fa, 0xf5c2e7, 0x6c7086),
            _ => return None,
        })
    }

    fn looks(self) -> [Look; ROLES] {
        let mut l = [PLAIN; ROLES];
        l[index(Role::Command)] = Look::bold(Col::Default);
        l[index(Role::Part)] = Look::col(self.cyan);
        l[index(Role::Value)] = Look::col(self.magenta);
        l[index(Role::Warning)] = Look::bold(self.red);
        l[index(Role::Risk)] = Look::col(self.yellow);
        l[index(Role::Note)] = Look::col(self.blue);
        l[index(Role::Cause)] = Look::col(self.yellow);
        l[index(Role::Try)] = Look::bold(self.green);
        l[index(Role::Dim)] =
            if self.gray == Col::Default { Look { dim: true, ..PLAIN } } else { Look::col(self.gray) };
        l[index(Role::Header)] = Look::bold(self.accent);
        l[index(Role::Success)] = Look::col(self.green);
        l[index(Role::Accent)] = Look::bold(self.accent);
        l
    }
}

/// The default look: baby blue, in truecolour. The text roles use mid-tones that read on a black,
/// a #1e1e1e and a white background alike; the body text keeps the terminal's own colour. With a
/// light background (see `light_background`) every colour is a deeper shade. Danger (red, orange)
/// is never a blue.
fn babyblue_looks(light: bool) -> [Look; ROLES] {
    // accent, command, header and part, warning, risk, success, note and value, cause
    let (accent, command, header, warning, risk, success, note, cause) = if light {
        (0x1f7fb8, 0x2a7fc0, 0x1558a8, 0xc62828, 0xb85c00, 0x1b7f3e, 0x6a4fc4, 0x8a6100)
    } else {
        (0x89cff0, 0x3f97e4, 0x2f80d8, 0xe5484d, 0xe8750e, 0x2fae5e, 0x8a74e0, 0xc8922a)
    };
    let mut l = [PLAIN; ROLES];
    l[index(Role::Command)] = Look::bold(hex(command));
    l[index(Role::Part)] = Look::col(hex(header));
    l[index(Role::Value)] = Look::col(hex(note));
    l[index(Role::Warning)] = Look::bold(hex(warning));
    l[index(Role::Risk)] = Look::col(hex(risk));
    l[index(Role::Note)] = Look::col(hex(note));
    l[index(Role::Cause)] = Look::col(hex(cause));
    l[index(Role::Try)] = Look::bold(hex(success));
    l[index(Role::Dim)] = Look { dim: true, ..PLAIN };
    l[index(Role::Header)] = Look::bold(hex(header));
    l[index(Role::Success)] = Look::col(hex(success));
    l[index(Role::Accent)] = Look::bold(hex(accent));
    l
}

fn mono_looks() -> [Look; ROLES] {
    let mut l = [PLAIN; ROLES];
    for role in [Role::Command, Role::Warning, Role::Try, Role::Header, Role::Accent] {
        l[index(role)] = Look { bold: true, ..PLAIN };
    }
    l[index(Role::Risk)] = Look { underline: true, ..PLAIN };
    for role in [Role::Dim, Role::Value, Role::Note] {
        l[index(role)] = Look { dim: true, ..PLAIN };
    }
    l
}

// ---------------------------------------------------------------------------
// The omarchy theme: read from the desktop's current theme at run time
// ---------------------------------------------------------------------------

pub fn omarchy_colors_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("omarchy/current/theme/colors.toml"))
}

fn parse_hex(text: &str) -> Option<(u8, u8, u8)> {
    let digits = text.trim().strip_prefix('#')?;
    if digits.len() != 6 || !digits.is_ascii() {
        return None;
    }
    let n = u32::from_str_radix(digits, 16).ok()?;
    Some(((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

fn rgb(c: (u8, u8, u8)) -> Col {
    Col::Rgb(c.0, c.1, c.2)
}

fn reddish(c: (u8, u8, u8)) -> bool {
    c.0 as i32 > c.1 as i32 + 30 && c.0 as i32 > c.2 as i32 + 30
}

fn greenish(c: (u8, u8, u8)) -> bool {
    c.1 as i32 > c.0 as i32 + 20 && c.1 as i32 > c.2 as i32 + 20
}

/// The palette named by the text of an omarchy `colors.toml` (accent, foreground, background and
/// color1-color6 as `#rrggbb`). Keys that are missing fall back to the `dark` theme's colours.
/// A theme that has red and green swapped in its slots 1 and 2 is corrected, so WARNING stays red.
/// `None` when the text is not usable (no foreground, or not TOML).
fn omarchy_palette(text: &str) -> Option<Palette> {
    let table: toml::Table = toml::from_str(text).ok()?;
    let get = |key: &str| table.get(key).and_then(|v| v.as_str()).and_then(parse_hex);
    let fg = get("foreground")?;
    let bg = get("background").unwrap_or((0, 0, 0));
    let fallback = Palette::builtin(Theme::Dark)?;
    let pick = |key: &str, default: Col| get(key).map(rgb).unwrap_or(default);
    let mut red = get("color1");
    let mut green = get("color2");
    if let (Some(r), Some(g)) = (red, green)
        && greenish(r)
        && reddish(g)
    {
        red = Some(g);
        green = Some(r);
    }
    let mix = |a: u8, b: u8| ((a as u16 * 55 + b as u16 * 45) / 100) as u8;
    Some(Palette {
        accent: pick("accent", fallback.accent),
        red: red.map(rgb).unwrap_or(fallback.red),
        yellow: pick("color3", fallback.yellow),
        green: green.map(rgb).unwrap_or(fallback.green),
        cyan: pick("color6", fallback.cyan),
        blue: pick("color4", fallback.blue),
        magenta: pick("color5", fallback.magenta),
        gray: Col::Rgb(mix(bg.0, fg.0), mix(bg.1, fg.1), mix(bg.2, fg.2)),
    })
}

fn load_omarchy() -> Option<Palette> {
    omarchy_palette(&std::fs::read_to_string(omarchy_colors_path()?).ok()?)
}

// ---------------------------------------------------------------------------
// Style
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Style {
    on: bool,
    truecolor: bool,
    looks: [Look; ROLES],
}

impl Style {
    pub fn plain() -> Style {
        Style { on: false, truecolor: true, looks: [PLAIN; ROLES] }
    }

    /// `omarchy` falls back to `auto` when the desktop theme file is missing or unreadable.
    #[cfg(test)]
    pub fn new(theme: Theme, on: bool) -> Style {
        Style::for_terminal(theme, on, TermEnv::default())
    }

    /// Like `new`, for a terminal with the given colour depth and background.
    pub fn for_terminal(theme: Theme, on: bool, term: TermEnv) -> Style {
        // On Omarchy every terminal uses the theme's palette, whose slots may not mean what their
        // ANSI names say (a theme can put green in "red"): auto then uses the theme's own colours.
        let omarchy = if on && matches!(theme, Theme::Omarchy | Theme::Auto) { load_omarchy() } else { None };
        Style::build(theme, on, omarchy, term)
    }

    #[cfg(test)]
    fn with_omarchy(theme: Theme, on: bool, omarchy: Option<Palette>) -> Style {
        Style::build(theme, on, omarchy, TermEnv::default())
    }

    fn build(theme: Theme, on: bool, omarchy: Option<Palette>, term: TermEnv) -> Style {
        if !on {
            return Style::plain();
        }
        let looks = match theme {
            Theme::BabyBlue => babyblue_looks(term.light),
            Theme::Mono => mono_looks(),
            Theme::Auto | Theme::Omarchy => omarchy.unwrap_or_else(Palette::auto).looks(),
            other => Palette::builtin(other).unwrap_or_else(Palette::auto).looks(),
        };
        Style { on, truecolor: term.truecolor, looks }
    }

    pub fn paint(&self, role: Role, text: &str) -> String {
        let sgr = self.looks[index(role)].sgr(self.truecolor);
        if !self.on || text.is_empty() || sgr.is_empty() {
            return text.to_string();
        }
        format!("\x1b[{sgr}m{text}\x1b[0m")
    }

    /// The text of a `huh why` answer: cause lines, then the `Try:` line and its `Note (...)` lines.
    pub fn answer(&self, text: &str) -> String {
        if !self.on {
            return text.to_string();
        }
        let mut out = String::with_capacity(text.len() + 64);
        for (n, line) in text.split('\n').enumerate() {
            if n > 0 {
                out.push('\n');
            }
            let body = line.trim_start_matches(['-', '*', ' ']);
            let lead = &line[..line.len() - body.len()];
            if let Some(rest) = body.strip_prefix("Try:") {
                out.push_str(lead);
                out.push_str(&self.paint(Role::Try, "Try:"));
                out.push_str(&self.paint(Role::Command, rest));
            } else if body.starts_with("Note (")
                && let Some(close) = body.find("): ")
            {
                out.push_str(lead);
                out.push_str(&self.paint(Role::Risk, &body[..close + 2]));
                out.push_str(&self.paint(Role::Note, &body[close + 2..]));
            } else {
                out.push_str(&self.paint(Role::Cause, line));
            }
        }
        out
    }

    /// A short sample for `huh themes`: a suggestion with a WARNING and a hint.
    pub fn sample(&self) -> String {
        let p = |role, text: &str| self.paint(role, text);
        let mut out = String::new();
        let _ = writeln!(out, "   {}", p(Role::Command, "1. rm -r ~/Downloads/old-isos"));
        let _ = writeln!(
            out,
            "   {} {}",
            p(Role::Description, "Delete a folder and what is in it"),
            p(Role::Value, "(92%)")
        );
        let _ = writeln!(
            out,
            "     {}  {}",
            p(Role::Part, &format!("{:<4}", "-r")),
            p(Role::Description, "Also delete the folders inside")
        );
        let _ = writeln!(
            out,
            "   {}{}{} {}",
            p(Role::Warning, "WARNING ("),
            p(Role::Risk, "destroys data"),
            p(Role::Warning, "):"),
            p(Role::Description, "deleted files do not go to the bin")
        );
        let _ = writeln!(out, "   {}", p(Role::Note, "not included: `older than a month`"));
        let _ = writeln!(out, "   {} {}", p(Role::Try, "Try:"), p(Role::Command, "ls -la ~/Downloads/old-isos"));
        out
    }
}

static GLOBAL: OnceLock<Style> = OnceLock::new();

/// Sets the style used by `paint` for the rest of the run (the first call wins).
pub fn init(style: Style) {
    let _ = GLOBAL.set(style);
}

fn global() -> Option<&'static Style> {
    GLOBAL.get().filter(|s| s.on)
}

/// Whether output is being styled.
pub fn enabled() -> bool {
    global().is_some()
}

pub fn paint(role: Role, text: &str) -> String {
    match global() {
        Some(style) => style.paint(role, text),
        None => text.to_string(),
    }
}

pub fn answer(text: &str) -> String {
    match global() {
        Some(style) => style.answer(text),
        None => text.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Choosing the theme and whether to colour
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

/// Colour is off for pipes, `--first-line-only`, `NO_COLOR` (any value, even empty) and
/// `TERM=dumb`; `--color always` forces it on and `--color never` off.
pub fn color_enabled(
    choice: ColorChoice,
    stdout_tty: bool,
    first_line_only: bool,
    no_color_set: bool,
    term_dumb: bool,
) -> bool {
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => stdout_tty && !first_line_only && !no_color_set && !term_dumb,
    }
}

/// `theme = "nord"` from the text of the config file; other keys are ignored.
pub fn theme_from_config_text(text: &str) -> Result<Option<Theme>> {
    let table: toml::Table = toml::from_str(text).map_err(|e| anyhow::anyhow!("invalid config file: {e}"))?;
    match table.get("theme") {
        None => Ok(None),
        Some(toml::Value::String(s)) => Theme::parse(s).map(Some),
        Some(_) => bail!("config file: theme must be a string; the valid themes are: {VALID}"),
    }
}

/// Flag beats environment, environment beats the file, the file beats `babyblue`.
pub fn resolve_theme(flag: Option<&str>, env: Option<&str>, file: Option<&Path>) -> Result<Theme> {
    if let Some(value) = flag {
        return Theme::parse(value);
    }
    if let Some(value) = env.filter(|v| !v.trim().is_empty()) {
        return Theme::parse(value).map_err(|e| anyhow::anyhow!("HUH_THEME: {e}"));
    }
    if let Some(path) = file
        && let Ok(text) = std::fs::read_to_string(path)
    {
        return Ok(theme_from_config_text(&text)
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?
            .unwrap_or(Theme::BabyBlue));
    }
    Ok(Theme::BabyBlue)
}

/// `huh themes`: a sample of every theme.
pub fn themes_text(on: bool) -> String {
    let mut out = String::new();
    for theme in ALL {
        let style = Style::for_terminal(theme, on, TermEnv::detect());
        let label = match theme {
            Theme::BabyBlue => "babyblue (default): baby-blue accent, readable on dark and light terminals",
            Theme::Auto => {
                "auto: the current Omarchy theme's colours when there is one, else the terminal's own 16 colours"
            }
            Theme::Omarchy if load_omarchy().is_none() => "omarchy: desktop theme not found, so it looks like auto",
            Theme::Omarchy => "omarchy: colours of the current Omarchy theme",
            Theme::Mono => "mono: bold, dim and underline only",
            _ => theme.name(),
        };
        let _ = writeln!(out, "{}", style.paint(Role::Header, label));
        out.push_str(&style.sample());
        out.push('\n');
    }
    if !on {
        out.push_str("(colour is off here, so the samples are plain. Use --color always to see them.)\n");
    }
    out.push_str("Choose one with --theme NAME, HUH_THEME=NAME or theme = \"NAME\" in ~/.config/huh/config.toml.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_ROLES: [Role; 13] = [
        Role::Command,
        Role::Description,
        Role::Part,
        Role::Value,
        Role::Warning,
        Role::Risk,
        Role::Note,
        Role::Cause,
        Role::Try,
        Role::Dim,
        Role::Header,
        Role::Success,
        Role::Accent,
    ];

    fn scratch(name: &str, text: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(".scratch").join("style-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }

    fn strip(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for d in chars.by_ref() {
                    if d == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn unstyled_until_init_and_when_off() {
        assert_eq!(paint(Role::Warning, "WARNING"), "WARNING");
        assert_eq!(answer("a\nTry: b"), "a\nTry: b");
        let off = Style::new(Theme::Nord, false);
        for role in ALL_ROLES {
            assert_eq!(off.paint(role, "x"), "x");
        }
        assert_eq!(off.answer("It broke.\nTry: ls"), "It broke.\nTry: ls");
    }

    #[test]
    fn auto_uses_only_the_16_ansi_colours() {
        // Without an Omarchy theme (on Omarchy, auto takes the theme's own colours instead).
        let style = Style::with_omarchy(Theme::Auto, true, None);
        assert_eq!(style.paint(Role::Warning, "W"), "\x1b[1;31mW\x1b[0m");
        assert_eq!(style.paint(Role::Part, "-r"), "\x1b[36m-r\x1b[0m");
        assert_eq!(style.paint(Role::Dim, "d"), "\x1b[2md\x1b[0m");
        assert_eq!(style.paint(Role::Description, "plain"), "plain");
        for role in ALL_ROLES {
            assert!(!style.paint(role, "x").contains("38;2"), "{role:?}");
        }
    }

    #[test]
    fn truecolour_themes_and_mono() {
        let nord = Style::new(Theme::Nord, true);
        assert_eq!(nord.paint(Role::Warning, "W"), "\x1b[1;38;2;191;97;106mW\x1b[0m");
        let mono = Style::new(Theme::Mono, true);
        for role in ALL_ROLES {
            let painted = mono.paint(role, "x");
            assert!(!painted.contains("38;2") && !painted.contains(";3") && !painted.contains("[3"), "{painted:?}");
        }
        assert_eq!(mono.paint(Role::Command, "c"), "\x1b[1mc\x1b[0m");
    }

    #[test]
    fn styling_never_changes_the_text() {
        for theme in ALL {
            let style = Style::with_omarchy(theme, true, None);
            assert_eq!(strip(&style.sample()), Style::plain().sample(), "{}", theme.name());
            let answer = "It broke.\nTry: ls -la\nNote (destroys data): careful";
            assert_eq!(strip(&style.answer(answer)), answer);
        }
    }

    #[test]
    fn answer_marks_the_try_and_note_lines() {
        let style = Style::with_omarchy(Theme::Auto, true, None);
        let out = style.answer("Missing.\n- Try: ls\nNote (kills work): stops it");
        assert!(out.contains("\x1b[1;32mTry:\x1b[0m"), "{out:?}");
        assert!(out.contains("\x1b[33mNote (kills work):"), "{out:?}");
        assert!(out.starts_with("\x1b[33mMissing.\x1b[0m"), "{out:?}");
    }

    #[test]
    fn omarchy_reads_the_palette_and_corrects_swapped_red_and_green() {
        let text = "accent = \"#e2231a\"\nforeground = \"#ffffff\"\nbackground = \"#050709\"\n\
                    color1 = \"#66a773\"\ncolor2 = \"#d24646\"\ncolor3 = \"#b4b47b\"\n";
        let p = omarchy_palette(text).unwrap();
        assert_eq!(p.accent, Col::Rgb(0xe2, 0x23, 0x1a));
        assert_eq!(p.red, Col::Rgb(0xd2, 0x46, 0x46));
        assert_eq!(p.green, Col::Rgb(0x66, 0xa7, 0x73));
        assert_eq!(p.yellow, Col::Rgb(0xb4, 0xb4, 0x7b));
        // Normal order stays as it is; missing keys fall back to dark.
        let p = omarchy_palette("foreground = \"#eeeeee\"\ncolor1 = \"#ff0000\"\ncolor2 = \"#00ff00\"\n").unwrap();
        assert_eq!(p.red, Col::Rgb(255, 0, 0));
        assert_eq!(p.cyan, Palette::builtin(Theme::Dark).unwrap().cyan);
        assert!(omarchy_palette("accent = \"#ffffff\"\n").is_none());
        assert!(omarchy_palette("not toml [").is_none());
        // Without the file the omarchy theme looks like auto.
        let fallback = Style::with_omarchy(Theme::Omarchy, true, None);
        assert_eq!(fallback.looks, Style::with_omarchy(Theme::Auto, true, None).looks);
        let real = Style::with_omarchy(Theme::Omarchy, true, omarchy_palette(text));
        assert!(real.paint(Role::Warning, "W").contains("38;2;210;70;70"));
    }

    #[test]
    fn colour_decision() {
        use ColorChoice::*;
        assert!(color_enabled(Auto, true, false, false, false));
        assert!(!color_enabled(Auto, false, false, false, false), "pipe");
        assert!(!color_enabled(Auto, true, true, false, false), "first-line-only");
        assert!(!color_enabled(Auto, true, false, true, false), "NO_COLOR");
        assert!(!color_enabled(Auto, true, false, false, true), "TERM=dumb");
        assert!(!color_enabled(Never, true, false, false, false));
        assert!(color_enabled(Always, false, true, true, true));
    }

    #[test]
    fn theme_selection_precedence_and_errors() {
        let file = scratch("nord.toml", "theme = \"nord\"\nmode = \"expert\"\n");
        assert_eq!(resolve_theme(None, None, None).unwrap(), Theme::BabyBlue);
        assert_eq!(resolve_theme(None, None, Some(&file)).unwrap(), Theme::Nord);
        assert_eq!(resolve_theme(None, Some("gruvbox"), Some(&file)).unwrap(), Theme::Gruvbox);
        assert_eq!(resolve_theme(Some("MONO"), Some("gruvbox"), Some(&file)).unwrap(), Theme::Mono);
        assert_eq!(resolve_theme(None, Some(" "), Some(&file)).unwrap(), Theme::Nord);
        let err = resolve_theme(Some("neon"), None, None).unwrap_err().to_string();
        assert!(err.contains("neon") && err.contains(VALID), "{err}");
        assert!(resolve_theme(None, Some("neon"), None).unwrap_err().to_string().contains("HUH_THEME"));
        let bad = scratch("bad.toml", "theme = \"neon\"\n");
        assert!(resolve_theme(None, None, Some(&bad)).unwrap_err().to_string().contains(VALID));
        let bad = scratch("bad2.toml", "theme = 3\n");
        assert!(resolve_theme(None, None, Some(&bad)).is_err());
        let missing = Path::new(env!("CARGO_MANIFEST_DIR")).join(".scratch/none.toml");
        assert_eq!(resolve_theme(None, None, Some(&missing)).unwrap(), Theme::BabyBlue);
        assert_eq!(theme_from_config_text("mode = \"x\"").unwrap(), None);
    }

    #[test]
    fn themes_text_lists_every_theme() {
        let plain = themes_text(false);
        for theme in ALL {
            assert!(plain.contains(theme.name()), "{}", theme.name());
        }
        assert!(plain.contains("WARNING") && plain.contains("colour is off"));
        assert!(!plain.contains('\x1b'));
        assert!(themes_text(true).contains('\x1b'));
    }

    #[test]
    fn parse_names() {
        for theme in ALL {
            assert_eq!(Theme::parse(theme.name()).unwrap(), theme);
        }
        assert_eq!(parse_hex("#0a0B0c"), Some((10, 11, 12)));
        assert_eq!(parse_hex("0a0b0c"), None);
        assert_eq!(parse_hex("#12"), None);
    }

    #[test]
    fn babyblue_is_the_default_and_listed_first() {
        assert_eq!(ALL[0], Theme::BabyBlue);
        let text = themes_text(false);
        assert!(text.starts_with("babyblue (default)"), "{text}");
        assert_eq!(text.matches("(default)").count(), 1);
    }

    #[test]
    fn babyblue_accent_is_baby_blue_and_danger_is_not_blue() {
        let style = Style::new(Theme::BabyBlue, true);
        assert_eq!(style.paint(Role::Accent, "x"), "\x1b[1;38;2;137;207;240mx\x1b[0m");
        // Body text keeps the terminal's colour.
        assert_eq!(style.paint(Role::Description, "plain"), "plain");
        let bluish = |col: Col| matches!(col, Col::Rgb(r, g, b) if b as i32 > r as i32 + 40 && b >= g);
        for light in [false, true] {
            let style = Style::for_terminal(Theme::BabyBlue, true, TermEnv { truecolor: true, light });
            for role in [Role::Warning, Role::Risk, Role::Success, Role::Cause] {
                assert!(!bluish(style.looks[index(role)].col), "{role:?}");
            }
            assert_ne!(style.looks[index(Role::Warning)].col, style.looks[index(Role::Accent)].col);
            assert_ne!(style.looks[index(Role::Risk)].col, style.looks[index(Role::Accent)].col);
        }
        let light = Style::for_terminal(Theme::BabyBlue, true, TermEnv { truecolor: true, light: true });
        assert_ne!(light.looks, style.looks);
    }

    #[test]
    fn colour_depth_and_background_detection() {
        assert!(truecolor_supported(Some("truecolor"), None));
        assert!(truecolor_supported(Some("24bit"), Some("xterm")));
        assert!(truecolor_supported(None, Some("xterm-direct")));
        assert!(!truecolor_supported(None, Some("xterm-256color")));
        assert!(!truecolor_supported(Some(""), None));
        assert!(light_background(Some("0;15")));
        assert!(light_background(Some("0;default;7")));
        assert!(!light_background(Some("15;0")));
        assert!(!light_background(Some("15;default")));
        assert!(!light_background(None));
    }

    #[test]
    fn without_truecolour_rgb_falls_back_to_256_codes() {
        let term = TermEnv { truecolor: false, light: false };
        let style = Style::for_terminal(Theme::BabyBlue, true, term);
        // #89cff0 is nearest to 117 (#87d7ff).
        assert_eq!(style.paint(Role::Accent, "x"), "\x1b[1;38;5;117mx\x1b[0m");
        for role in ALL_ROLES {
            let painted = style.paint(role, "x");
            assert!(!painted.contains("38;2"), "{painted:?}");
        }
        assert!(style.paint(Role::Warning, "W").contains("38;5;"));
        // The cube, the grey ramp and the extremes.
        assert_eq!(nearest_256(0, 0, 0), 16);
        assert_eq!(nearest_256(255, 255, 255), 231);
        assert_eq!(nearest_256(128, 128, 128), 244);
        assert_eq!(nearest_256(255, 0, 0), 196);
        // Truecolour terminals are unchanged.
        assert!(Style::new(Theme::BabyBlue, true).paint(Role::Accent, "x").contains("38;2;137;207;240"));
    }
}
