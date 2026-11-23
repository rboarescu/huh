//! 32x32 pixel pet sprite sheets, capability detection, and Kitty Graphics APC rendering.
//!
//! Built-in sheets live in `sprites/*.toml` (embedded at compile time via `build.rs`), with optional
//! user overrides under `$XDG_CONFIG_HOME/huh/sprites/<species>.toml` (or `~/.config/huh/sprites`).
//! Invalid or missing override files silently fall back to the embedded sheet.

use crate::pet::{Mood, Species};
use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::write::ZlibEncoder;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

include!(concat!(env!("OUT_DIR"), "/embedded_sprites.rs"));

pub const WIDTH: usize = 32;
pub const HEIGHT: usize = 32;
pub const RGBA_BYTES: usize = WIDTH * HEIGHT * 4;

const MAX_OVERRIDE_BYTES: u64 = 256 * 1024;
const KITTY_CHUNK_LEN: usize = 4096;

const SHEBANG_7X5: [&[u8]; 5] = [b".#.#..#", b"#####.#", b".#.#..#", b"#####..", b".#.#..#"];

/// Requested pet render style from CLI, environment, or config.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RenderStyle {
    #[default]
    Auto,
    Pixel,
    Ascii,
    Off,
}

impl RenderStyle {
    pub const VALID: &'static str = "auto, pixel, ascii, off";

    pub fn parse(value: &str) -> Result<RenderStyle> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(RenderStyle::Auto),
            "pixel" => Ok(RenderStyle::Pixel),
            "ascii" => Ok(RenderStyle::Ascii),
            "off" => Ok(RenderStyle::Off),
            other => bail!("unknown pet render style \"{other}\"; valid styles: {}", Self::VALID),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            RenderStyle::Auto => "auto",
            RenderStyle::Pixel => "pixel",
            RenderStyle::Ascii => "ascii",
            RenderStyle::Off => "off",
        }
    }
}

/// Effective pet display mode resolved for the current terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectiveMode {
    Pixel,
    Ascii,
    Off,
}

/// Theme RGB colors used to resolve semantic sprite tokens and mood effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeColors {
    pub light: bool,
    pub accent: (u8, u8, u8),
    pub warning: (u8, u8, u8),
    pub success: (u8, u8, u8),
    pub note: (u8, u8, u8),
    pub dim: (u8, u8, u8),
}

impl Default for ThemeColors {
    /// Default babyblue dark palette.
    fn default() -> ThemeColors {
        ThemeColors {
            light: false,
            accent: (0x89, 0xcf, 0xf0),
            warning: (0xe5, 0x48, 0x4d),
            success: (0x2f, 0xae, 0x5e),
            note: (0x8a, 0x74, 0xe0),
            dim: (0x70, 0x84, 0x9c),
        }
    }
}

/// Resolves whether the pet should render as Kitty 32x32 pixels, ASCII art, or stay hidden.
pub fn detect_mode(
    requested: RenderStyle,
    env: &dyn Fn(&str) -> Option<String>,
    stdout_tty: bool,
    pet_enabled: bool,
) -> EffectiveMode {
    if !pet_enabled || requested == RenderStyle::Off {
        return EffectiveMode::Off;
    }
    if !stdout_tty || env("TERM").as_deref() == Some("dumb") || requested == RenderStyle::Ascii {
        return EffectiveMode::Ascii;
    }
    if requested == RenderStyle::Pixel {
        return EffectiveMode::Pixel;
    }

    let nonempty = |name: &str| env(name).is_some_and(|v| !v.is_empty());
    let term_program_capable = env("TERM_PROGRAM")
        .is_some_and(|tp| matches!(tp.trim().to_ascii_lowercase().as_str(), "ghostty" | "kitty" | "wezterm"));
    let term_capable = env("TERM").is_some_and(|t| {
        let lower = t.to_ascii_lowercase();
        lower.contains("kitty") || lower.contains("ghostty")
    });
    let terminal_capable =
        nonempty("GHOSTTY_RESOURCES_DIR") || nonempty("KITTY_WINDOW_ID") || term_program_capable || term_capable;
    if !terminal_capable {
        return EffectiveMode::Ascii;
    }

    if let Some(tmux) = env("TMUX").filter(|v| !v.is_empty()) {
        let socket_base = Path::new(tmux.split(',').next().unwrap_or("")).file_name().and_then(|n| n.to_str());
        let passthrough_ok = socket_base == Some("huh") || env("HUH_KITTY_GRAPHICS").is_some_and(|v| v.trim() == "1");
        if !passthrough_ok {
            return EffectiveMode::Ascii;
        }
    }

    EffectiveMode::Pixel
}

pub fn in_tmux_env() -> bool {
    std::env::var_os("TMUX").is_some_and(|v| !v.is_empty())
}

fn override_dir_from(xdg_config: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    let base = match xdg_config.filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(home.filter(|v| !v.is_empty())?).join(".config"),
    };
    Some(base.join("huh").join("sprites"))
}

pub fn override_dir() -> Option<PathBuf> {
    override_dir_from(std::env::var("XDG_CONFIG_HOME").ok().as_deref(), std::env::var("HOME").ok().as_deref())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EyeStyle {
    Chibi,
    ChibiSingle,
    Rpg,
}

impl EyeStyle {
    fn parse(value: &str) -> Result<EyeStyle> {
        match value.trim() {
            "chibi" => Ok(EyeStyle::Chibi),
            "chibi_single" => Ok(EyeStyle::ChibiSingle),
            "rpg" => Ok(EyeStyle::Rpg),
            other => bail!("unknown eye_style \"{other}\""),
        }
    }

    fn bounds(self) -> (usize, usize) {
        match self {
            EyeStyle::Chibi | EyeStyle::ChibiSingle => (3, 4),
            EyeStyle::Rpg => (3, 3),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ColorSpec {
    Rgba([u8; 4]),
    Accent,
    Warning,
    Success,
    Note,
    Dim,
    Shadow,
}

impl ColorSpec {
    fn parse(value: &str) -> Result<ColorSpec> {
        let trimmed = value.trim();
        match trimmed.to_ascii_lowercase().as_str() {
            "accent" => return Ok(ColorSpec::Accent),
            "warning" => return Ok(ColorSpec::Warning),
            "success" => return Ok(ColorSpec::Success),
            "note" => return Ok(ColorSpec::Note),
            "dim" => return Ok(ColorSpec::Dim),
            "shadow" => return Ok(ColorSpec::Shadow),
            _ => {}
        }
        let hex = trimmed.strip_prefix('#').with_context(|| format!("invalid sprite color \"{trimmed}\""))?;
        anyhow::ensure!(hex.is_ascii() && (hex.len() == 6 || hex.len() == 8), "invalid hex color \"{trimmed}\"");
        let r = u8::from_str_radix(&hex[0..2], 16)?;
        let g = u8::from_str_radix(&hex[2..4], 16)?;
        let b = u8::from_str_radix(&hex[4..6], 16)?;
        let a = if hex.len() == 8 { u8::from_str_radix(&hex[6..8], 16)? } else { 255 };
        Ok(ColorSpec::Rgba([r, g, b, a]))
    }

    fn resolve(self, colors: &ThemeColors) -> [u8; 4] {
        match self {
            ColorSpec::Rgba(rgba) => rgba,
            ColorSpec::Accent => [colors.accent.0, colors.accent.1, colors.accent.2, 255],
            ColorSpec::Warning => [colors.warning.0, colors.warning.1, colors.warning.2, 255],
            ColorSpec::Success => [colors.success.0, colors.success.1, colors.success.2, 255],
            ColorSpec::Note => [colors.note.0, colors.note.1, colors.note.2, 255],
            ColorSpec::Dim => [colors.dim.0, colors.dim.1, colors.dim.2, 255],
            ColorSpec::Shadow => {
                if colors.light {
                    [19, 26, 42, 42]
                } else {
                    [0, 0, 0, 89]
                }
            }
        }
    }
}

/// Parsed and bounds-validated 32x32 pixel sprite sheet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpriteSheet {
    #[cfg_attr(not(test), allow(dead_code))]
    pub species: Species,
    #[cfg_attr(not(test), allow(dead_code))]
    pub style: String,
    pub eye_style: EyeStyle,
    pub eye_left: (usize, usize),
    pub eye_right: Option<(usize, usize)>,
    pub shebang: (usize, usize),
    grid: [[u8; WIDTH]; HEIGHT],
    palette: [Option<ColorSpec>; 128],
    eye_ink: ColorSpec,
    eye_glint: ColorSpec,
    shebang_fg: ColorSpec,
}

fn parse_coords(val: &toml::Value, field: &str, max_w: usize, max_h: usize) -> Result<(usize, usize)> {
    let arr = val.as_array().with_context(|| format!("{field} must be a 2-element [x, y] array"))?;
    anyhow::ensure!(arr.len() == 2, "{field} must have exactly 2 elements");
    let x = usize::try_from(arr[0].as_integer().with_context(|| format!("{field}[0] must be an integer"))?)
        .with_context(|| format!("{field}[0] must be non-negative"))?;
    let y = usize::try_from(arr[1].as_integer().with_context(|| format!("{field}[1] must be an integer"))?)
        .with_context(|| format!("{field}[1] must be non-negative"))?;
    anyhow::ensure!(x + max_w <= WIDTH && y + max_h <= HEIGHT, "{field} [{x}, {y}] out of 32x32 bounds");
    Ok((x, y))
}

impl SpriteSheet {
    fn parse(toml_text: &str) -> Result<SpriteSheet> {
        let root: toml::Table = toml::from_str(toml_text).context("invalid sprite TOML")?;
        let species =
            root.get("species").and_then(toml::Value::as_str).context("missing species").and_then(Species::parse)?;
        let style = root.get("style").and_then(toml::Value::as_str).unwrap_or("custom").to_string();
        let eye_style = root
            .get("eye_style")
            .and_then(toml::Value::as_str)
            .context("missing eye_style")
            .and_then(EyeStyle::parse)?;
        let (eye_w, eye_h) = eye_style.bounds();
        let eye_left =
            root.get("eye_left").context("missing eye_left").and_then(|v| parse_coords(v, "eye_left", eye_w, eye_h))?;
        let eye_right = match eye_style {
            EyeStyle::ChibiSingle => None,
            EyeStyle::Chibi | EyeStyle::Rpg => Some(
                root.get("eye_right")
                    .context("missing eye_right")
                    .and_then(|v| parse_coords(v, "eye_right", eye_w, eye_h))?,
            ),
        };
        let shebang = root.get("shebang").context("missing shebang").and_then(|v| parse_coords(v, "shebang", 7, 5))?;

        let palette_tbl = root.get("palette").and_then(toml::Value::as_table).context("missing [palette] table")?;
        let mut palette = [None; 128];
        let mut eye_ink = None;
        let mut eye_glint = None;
        let mut shebang_fg = None;
        for (k, v) in palette_tbl {
            if k == "grid" {
                continue;
            }
            let raw = v.as_str().with_context(|| format!("palette.{k} must be a string"))?;
            let spec = ColorSpec::parse(raw)?;
            match k.as_str() {
                "eye_ink" => eye_ink = Some(spec),
                "eye_glint" => eye_glint = Some(spec),
                "shebang_fg" => shebang_fg = Some(spec),
                key if key.len() == 1 && key.is_ascii() => {
                    let b = key.as_bytes()[0];
                    anyhow::ensure!(b != b'.' && b.is_ascii_graphic(), "invalid palette token \"{key}\"");
                    palette[b as usize] = Some(spec);
                }
                other => bail!("unknown palette key \"{other}\""),
            }
        }
        let eye_ink = eye_ink.context("missing palette.eye_ink")?;
        let eye_glint = eye_glint.context("missing palette.eye_glint")?;
        let shebang_fg = shebang_fg.context("missing palette.shebang_fg")?;

        // Accept top-level `grid` (preferred) or `palette.grid` if written under `[palette]`.
        let grid_arr = root
            .get("grid")
            .or_else(|| palette_tbl.get("grid"))
            .and_then(toml::Value::as_array)
            .context("missing grid array")?;
        anyhow::ensure!(grid_arr.len() == HEIGHT, "grid must have {HEIGHT} rows, got {}", grid_arr.len());

        let mut grid = [[b'.'; WIDTH]; HEIGHT];
        for (y, row_val) in grid_arr.iter().enumerate() {
            let row = row_val.as_str().with_context(|| format!("grid[{y}] must be a string"))?;
            anyhow::ensure!(row.is_ascii() && row.len() == WIDTH, "grid[{y}] must be {WIDTH} ASCII chars");
            for (x, &b) in row.as_bytes().iter().enumerate() {
                anyhow::ensure!(b == b'.' || b.is_ascii_graphic(), "grid[{y}][{x}] must be printable ASCII");
                if b != b'.' {
                    anyhow::ensure!(
                        palette[b as usize].is_some(),
                        "grid[{y}][{x}] '{}' missing from [palette]",
                        b as char
                    );
                }
                grid[y][x] = b;
            }
        }

        Ok(SpriteSheet {
            species,
            style,
            eye_style,
            eye_left,
            eye_right,
            shebang,
            grid,
            palette,
            eye_ink,
            eye_glint,
            shebang_fg,
        })
    }
}

fn embedded_toml(species: Species) -> &'static str {
    let target = format!("{}.toml", species.name());
    EMBEDDED_SPRITES
        .iter()
        .find(|(name, _)| *name == target)
        .map(|(_, text)| *text)
        .unwrap_or_else(|| panic!("missing embedded sprite for {}", species.name()))
}

fn read_bounded(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut buf = Vec::new();
    file.take(MAX_OVERRIDE_BYTES + 1).read_to_end(&mut buf).ok()?;
    if buf.len() as u64 > MAX_OVERRIDE_BYTES {
        return None;
    }
    String::from_utf8(buf).ok()
}

/// Loads the sprite sheet for `species` from `override_dir` if present and valid, else embedded.
pub fn load_sheet(species: Species, override_dir: Option<&Path>) -> SpriteSheet {
    if let Some(dir) = override_dir {
        let path = dir.join(format!("{}.toml", species.name()));
        if let Some(text) = read_bounded(&path)
            && let Ok(sheet) = SpriteSheet::parse(&text)
        {
            return sheet;
        }
    }
    SpriteSheet::parse(embedded_toml(species)).expect("embedded sprite sheet must be valid")
}

/// Writes the embedded TOML template to `<dir>/<species>.toml` if missing; preserves existing files.
#[cfg_attr(not(test), allow(dead_code))]
pub fn export_template(species: Species, dir: &Path) -> Result<PathBuf> {
    let path = dir.join(format!("{}.toml", species.name()));
    if path.symlink_metadata().is_ok() {
        return Ok(path);
    }
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            file.write_all(embedded_toml(species).as_bytes())
                .with_context(|| format!("cannot write {}", path.display()))?;
            Ok(path)
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(path),
        Err(err) => Err(err).with_context(|| format!("cannot create {}", path.display())),
    }
}

/// Returns a distinct key per visual animation frame so the pane avoids redundant APC transfers.
pub fn frame_key(mood: Mood, tick: u64) -> u64 {
    match mood {
        Mood::Idle => {
            let breathe = u64::from(tick % 10 >= 5);
            let blink = u64::from(tick % 24 == 23);
            breathe | (blink << 1)
        }
        Mood::Happy => (1 << 8) | (tick % 4),
        Mood::Sleepy => {
            let bob = u64::from(tick % 6 >= 3);
            let z_step = (tick % 6) / 2;
            (2 << 8) | bob | (z_step << 1)
        }
        Mood::Hungry => (3 << 8) | u64::from(tick % 4 >= 2),
        Mood::Alert => (4 << 8) | (tick % 2),
        Mood::Celebrating => (5 << 8) | (tick % 30),
    }
}

fn plot(buf: &mut [u8; RGBA_BYTES], x: isize, y: isize, rgba: [u8; 4]) {
    if (0..WIDTH as isize).contains(&x) && (0..HEIGHT as isize).contains(&y) {
        let idx = (y as usize * WIDTH + x as usize) * 4;
        buf[idx..idx + 4].copy_from_slice(&rgba);
    }
}

fn spark(buf: &mut [u8; RGBA_BYTES], cx: isize, cy: isize, color: [u8; 4], big: bool) {
    plot(buf, cx, cy, color);
    if big {
        plot(buf, cx - 1, cy, color);
        plot(buf, cx + 1, cy, color);
        plot(buf, cx, cy - 1, color);
        plot(buf, cx, cy + 1, color);
    }
}

#[derive(Clone, Copy)]
struct EyePalette {
    ink: [u8; 4],
    glint: [u8; 4],
    accent: [u8; 4],
}

fn draw_chibi_eye(buf: &mut [u8; RGBA_BYTES], x: isize, y: isize, mood: Mood, blink: bool, pal: EyePalette) {
    if mood == Mood::Sleepy || blink {
        for dx in 0..3 {
            plot(buf, x + dx, y + 2, pal.ink);
        }
    } else if matches!(mood, Mood::Happy | Mood::Celebrating) {
        plot(buf, x + 1, y + 1, pal.ink);
        plot(buf, x, y + 2, pal.ink);
        plot(buf, x + 2, y + 2, pal.ink);
    } else if matches!(mood, Mood::Alert | Mood::Hungry) {
        for r in 0..4 {
            for c in 0..3 {
                plot(buf, x + c, y + r, [255, 255, 255, 255]);
            }
        }
        plot(buf, x + 1, y + 1, pal.ink);
        plot(buf, x + 1, y + 2, pal.ink);
        plot(buf, x, y, pal.ink);
        plot(buf, x + 2, y, pal.ink);
        plot(buf, x, y + 3, pal.ink);
        plot(buf, x + 2, y + 3, pal.ink);
    } else {
        for r in 0..3 {
            for c in 0..3 {
                plot(buf, x + c, y + r, pal.ink);
            }
        }
        plot(buf, x, y, pal.glint);
        plot(buf, x + 2, y + 2, pal.accent);
    }
}

fn draw_rpg_eye(buf: &mut [u8; RGBA_BYTES], x: isize, y: isize, mood: Mood, blink: bool, pal: EyePalette) {
    if mood == Mood::Sleepy || blink {
        plot(buf, x, y + 1, pal.ink);
        plot(buf, x + 1, y + 1, pal.ink);
    } else if matches!(mood, Mood::Happy | Mood::Celebrating) {
        plot(buf, x, y + 1, pal.ink);
        plot(buf, x + 1, y, pal.ink);
        plot(buf, x + 2, y + 1, pal.ink);
    } else if matches!(mood, Mood::Alert | Mood::Hungry) {
        plot(buf, x, y, pal.ink);
        plot(buf, x + 1, y, pal.ink);
        plot(buf, x, y + 1, pal.glint);
        plot(buf, x + 1, y + 1, pal.ink);
        plot(buf, x, y + 2, pal.ink);
        plot(buf, x + 1, y + 2, pal.ink);
    } else {
        plot(buf, x, y, pal.glint);
        plot(buf, x + 1, y, pal.ink);
        plot(buf, x, y + 1, pal.ink);
        plot(buf, x + 1, y + 1, pal.ink);
    }
}

fn draw_eyes(
    buf: &mut [u8; RGBA_BYTES],
    sheet: &SpriteSheet,
    mood: Mood,
    tick: u64,
    dx: isize,
    dy: isize,
    colors: &ThemeColors,
) {
    let blink = mood == Mood::Idle && tick % 24 == 23;
    let ink = sheet.eye_ink.resolve(colors);
    let glint = sheet.eye_glint.resolve(colors);
    let pal = EyePalette { ink, glint, accent: sheet.palette[b'3' as usize].map_or(glint, |s| s.resolve(colors)) };
    let lx = sheet.eye_left.0 as isize + dx;
    let ly = sheet.eye_left.1 as isize + dy;

    match sheet.eye_style {
        EyeStyle::Chibi => {
            draw_chibi_eye(buf, lx, ly, mood, blink, pal);
            if let Some((rx, _)) = sheet.eye_right {
                draw_chibi_eye(buf, rx as isize + dx, ly, mood, blink, pal);
            }
        }
        EyeStyle::ChibiSingle => draw_chibi_eye(buf, lx, ly, mood, blink, pal),
        EyeStyle::Rpg => {
            draw_rpg_eye(buf, lx, ly, mood, blink, pal);
            if let Some((rx, _)) = sheet.eye_right {
                draw_rpg_eye(buf, rx as isize + dx, ly, mood, blink, pal);
            }
        }
    }
}

fn draw_mood_fx(buf: &mut [u8; RGBA_BYTES], mood: Mood, tick: u64, colors: &ThemeColors) {
    let fx_accent = [colors.accent.0, colors.accent.1, colors.accent.2, 255];
    let fx_gold = if colors.light { [0xd9, 0x8a, 0x00, 255] } else { [0xff, 0xd5, 0x5a, 255] };
    let fx_warn = [colors.warning.0, colors.warning.1, colors.warning.2, 255];
    let fx_success = [colors.success.0, colors.success.1, colors.success.2, 255];
    let fx_note = [colors.note.0, colors.note.1, colors.note.2, 255];

    match mood {
        Mood::Idle => {}
        Mood::Sleepy => {
            let step = (tick % 6) as isize;
            let (zx, zy) = (25, 6 - step / 2);
            plot(buf, zx, zy, fx_accent);
            plot(buf, zx + 1, zy, fx_accent);
            plot(buf, zx + 2, zy, fx_accent);
            plot(buf, zx + 1, zy + 1, fx_accent);
            plot(buf, zx, zy + 2, fx_accent);
            plot(buf, zx + 1, zy + 2, fx_accent);
            plot(buf, zx + 2, zy + 2, fx_accent);
            if step >= 2 {
                let (zx2, zy2) = (28, 2 - (step - 2) / 2);
                plot(buf, zx2, zy2, fx_accent);
                plot(buf, zx2 + 1, zy2, fx_accent);
                plot(buf, zx2, zy2 + 1, fx_accent);
                plot(buf, zx2 + 1, zy2 + 1, fx_accent);
            }
        }
        Mood::Hungry => {
            let qy = if tick % 4 >= 2 { 1 } else { 2 };
            plot(buf, 27, qy, fx_note);
            plot(buf, 28, qy, fx_note);
            plot(buf, 29, qy, fx_note);
            plot(buf, 27, qy + 1, fx_note);
            plot(buf, 29, qy + 1, fx_note);
            plot(buf, 29, qy + 2, fx_note);
            plot(buf, 28, qy + 3, fx_note);
            plot(buf, 28, qy + 5, fx_note);
        }
        Mood::Alert => {
            let ac = if tick.is_multiple_of(2) { fx_warn } else { fx_gold };
            for r in 1..=5 {
                plot(buf, 27, r, ac);
                plot(buf, 28, r, ac);
            }
            plot(buf, 27, 7, ac);
            plot(buf, 28, 7, ac);
        }
        Mood::Happy => {
            let phase = tick % 4;
            spark(buf, 4, 5, fx_accent, phase < 2);
            spark(buf, 27, 4, fx_gold, phase >= 2);
        }
        Mood::Celebrating => {
            let pts = [
                (3_isize, 2_isize, fx_accent),
                (8, 1, fx_gold),
                (23, 1, fx_success),
                (28, 3, fx_accent),
                (2, 9, fx_success),
                (29, 8, fx_gold),
                (5, 5, fx_gold),
                (26, 6, fx_accent),
            ];
            for (i, (px, py, col)) in pts.into_iter().enumerate() {
                let t = tick + i as u64;
                let cy = (py + (t % 5) as isize) % 12;
                plot(buf, px, cy, col);
                if t.is_multiple_of(3) {
                    plot(buf, px + 1, cy, col);
                }
            }
        }
    }
}

/// Renders one 32x32 RGBA animation frame for `sheet`, `mood`, `tick`, and `colors`.
pub fn render_rgba(sheet: &SpriteSheet, mood: Mood, tick: u64, colors: &ThemeColors) -> [u8; RGBA_BYTES] {
    let mut buf = [0u8; RGBA_BYTES];

    let (dx, dy): (isize, isize) = match mood {
        Mood::Happy => (0, if matches!(tick % 4, 1 | 2) { -1 } else { 0 }),
        Mood::Celebrating => {
            const ARC: [isize; 6] = [0, -2, -3, -2, 0, -1];
            (0, ARC[(tick % 6) as usize])
        }
        Mood::Alert => (if tick.is_multiple_of(2) { 0 } else { 1 }, 0),
        Mood::Sleepy => (0, if tick % 6 >= 3 { 1 } else { 0 }),
        Mood::Idle | Mood::Hungry => (0, 0),
    };
    let breathe: isize = if mood == Mood::Idle && tick % 10 >= 5 { 1 } else { 0 };

    // Static ground shadow (`S`) first so animated feet stay above it cleanly.
    if let Some(shadow_spec) = sheet.palette[b'S' as usize] {
        let shadow = shadow_spec.resolve(colors);
        for (y, row) in sheet.grid.iter().enumerate() {
            for (x, &b) in row.iter().enumerate() {
                if b == b'S' {
                    plot(&mut buf, x as isize, y as isize, shadow);
                }
            }
        }
    }

    for (y, row) in sheet.grid.iter().enumerate() {
        let row_dy = dy + if breathe != 0 && y < 17 { 1 } else { 0 };
        for (x, &b) in row.iter().enumerate() {
            if b != b'.'
                && b != b'S'
                && let Some(spec) = sheet.palette[b as usize]
            {
                plot(&mut buf, x as isize + dx, y as isize + row_dy, spec.resolve(colors));
            }
        }
    }

    let sb_col = sheet.shebang_fg.resolve(colors);
    for (r, row) in SHEBANG_7X5.iter().enumerate() {
        for (c, &ch) in row.iter().enumerate() {
            if ch == b'#' {
                plot(
                    &mut buf,
                    sheet.shebang.0 as isize + c as isize + dx,
                    sheet.shebang.1 as isize + r as isize + dy,
                    sb_col,
                );
            }
        }
    }

    draw_eyes(&mut buf, sheet, mood, tick, dx, dy + breathe, colors);
    draw_mood_fx(&mut buf, mood, tick, colors);
    buf
}

fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(triple >> 18) & 0x3f] as char);
        out.push(TABLE[(triple >> 12) & 0x3f] as char);
        out.push(if chunk.len() > 1 { TABLE[(triple >> 6) & 0x3f] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[triple & 0x3f] as char } else { '=' });
    }
    out
}

fn wrap_tmux_dcs(seq: &str) -> String {
    let mut out = String::with_capacity(seq.len() + 16);
    out.push_str("\x1bPtmux;");
    for ch in seq.chars() {
        if ch == '\x1b' {
            out.push_str("\x1b\x1b");
        } else {
            out.push(ch);
        }
    }
    out.push_str("\x1b\\");
    out
}

fn encode_kitty_chunks(b64: &str, cols: u16, rows: u16, in_tmux: bool) -> String {
    let total = b64.len().div_ceil(KITTY_CHUNK_LEN).max(1);
    let mut out = String::with_capacity(b64.len() + total * 64);
    for idx in 0..total {
        let start = idx * KITTY_CHUNK_LEN;
        let end = ((idx + 1) * KITTY_CHUNK_LEN).min(b64.len());
        let chunk = &b64[start..end];
        let apc = if total == 1 {
            format!("\x1b_Ga=T,f=32,s=32,v=32,o=z,i=1,r={rows},c={cols},C=1,q=2;{chunk}\x1b\\")
        } else if idx == 0 {
            format!("\x1b_Ga=T,f=32,s=32,v=32,o=z,i=1,r={rows},c={cols},C=1,q=2,m=1;{chunk}\x1b\\")
        } else if idx + 1 < total {
            format!("\x1b_Gq=2,m=1;{chunk}\x1b\\")
        } else {
            format!("\x1b_Gq=2,m=0;{chunk}\x1b\\")
        };
        if in_tmux {
            out.push_str(&wrap_tmux_dcs(&apc));
        } else {
            out.push_str(&apc);
        }
    }
    out
}

/// Encodes a 32x32 RGBA frame as a zlib-compressed Kitty Graphics APC sequence (tmux-wrapped when needed).
pub fn encode_kitty(rgba: &[u8; RGBA_BYTES], cols: u16, rows: u16, in_tmux: bool) -> String {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(rgba).expect("zlib write to Vec cannot fail");
    let compressed = encoder.finish().expect("zlib finish on Vec cannot fail");
    let b64 = encode_base64(&compressed);
    encode_kitty_chunks(&b64, cols, rows, in_tmux)
}

/// Returns the Kitty Graphics sequence to delete image `i=1`.
pub fn delete_kitty(in_tmux: bool) -> String {
    let seq = "\x1b_Ga=d,d=i,i=1,q=2\x1b\\";
    if in_tmux { wrap_tmux_dcs(seq) } else { seq.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::ZlibDecoder;

    fn env_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| (*v).to_string())
    }

    fn pixel(buf: &[u8; RGBA_BYTES], x: usize, y: usize) -> [u8; 4] {
        let idx = (y * WIDTH + x) * 4;
        [buf[idx], buf[idx + 1], buf[idx + 2], buf[idx + 3]]
    }

    fn decode_base64(input: &str) -> Vec<u8> {
        let val = |b: u8| -> u8 {
            match b {
                b'A'..=b'Z' => b - b'A',
                b'a'..=b'z' => b - b'a' + 26,
                b'0'..=b'9' => b - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                _ => panic!("invalid base64 byte {b}"),
            }
        };
        let bytes = input.as_bytes();
        let (chunks, rem) = bytes.as_chunks::<4>();
        assert!(rem.is_empty());
        let mut out = Vec::with_capacity(chunks.len() * 3);
        for chunk in chunks {
            let v0 = val(chunk[0]) as u32;
            let v1 = val(chunk[1]) as u32;
            let v2 = if chunk[2] == b'=' { 0 } else { val(chunk[2]) as u32 };
            let v3 = if chunk[3] == b'=' { 0 } else { val(chunk[3]) as u32 };
            let n = (v0 << 18) | (v1 << 12) | (v2 << 6) | v3;
            out.push((n >> 16) as u8);
            if chunk[2] != b'=' {
                out.push((n >> 8) as u8);
            }
            if chunk[3] != b'=' {
                out.push(n as u8);
            }
        }
        out
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/sprite-tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn embedded_sprites_all_parse_and_match_spec() {
        assert_eq!(EMBEDDED_SPRITES.len(), 9);
        let names: Vec<&str> = EMBEDDED_SPRITES.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            vec![
                "bear.toml",
                "beaver.toml",
                "bee.toml",
                "cat.toml",
                "dolphin.toml",
                "owl.toml",
                "penguin.toml",
                "snake.toml",
                "turtle.toml",
            ]
        );

        let penguin = load_sheet(Species::Penguin, None);
        assert_eq!(penguin.species, Species::Penguin);
        assert_eq!(penguin.style, "P1");
        assert_eq!(penguin.eye_style, EyeStyle::Chibi);
        assert_eq!(penguin.eye_left, (9, 10));
        assert_eq!(penguin.eye_right, Some((19, 10)));
        assert_eq!(penguin.shebang, (12, 20));

        let cat = load_sheet(Species::Cat, None);
        assert_eq!(cat.species, Species::Cat);
        assert_eq!(cat.style, "P4");
        assert_eq!(cat.eye_style, EyeStyle::Rpg);
        assert_eq!(cat.eye_left, (9, 10));
        assert_eq!(cat.eye_right, Some((17, 10)));
        assert_eq!(cat.shebang, (10, 20));

        let beaver = load_sheet(Species::Beaver, None);
        assert_eq!(beaver.species, Species::Beaver);
        assert_eq!(beaver.style, "P3");
        assert_eq!(beaver.eye_style, EyeStyle::Chibi);
        assert_eq!(beaver.eye_left, (7, 8));
        assert_eq!(beaver.eye_right, Some((14, 8)));
        assert_eq!(beaver.shebang, (8, 19));

        let turtle = load_sheet(Species::Turtle, None);
        assert_eq!(turtle.species, Species::Turtle);
        assert_eq!(turtle.style, "P1");
        assert_eq!(turtle.eye_style, EyeStyle::ChibiSingle);
        assert_eq!(turtle.eye_left, (24, 13));
        assert_eq!(turtle.eye_right, None);
        assert_eq!(turtle.shebang, (8, 15));

        let dolphin = load_sheet(Species::Dolphin, None);
        assert_eq!(dolphin.species, Species::Dolphin);
        assert_eq!(dolphin.style, "P2");
        assert_eq!(dolphin.eye_style, EyeStyle::ChibiSingle);
        assert_eq!(dolphin.eye_left, (8, 12));
        assert_eq!(dolphin.eye_right, None);
        assert_eq!(dolphin.shebang, (14, 13));

        let bear = load_sheet(Species::Bear, None);
        assert_eq!(bear.species, Species::Bear);
        assert_eq!(bear.style, "P1");
        assert_eq!(bear.eye_style, EyeStyle::Chibi);
        assert_eq!(bear.eye_left, (10, 9));
        assert_eq!(bear.eye_right, Some((19, 9)));
        assert_eq!(bear.shebang, (12, 20));

        let snake = load_sheet(Species::Snake, None);
        assert_eq!(snake.species, Species::Snake);
        assert_eq!(snake.style, "P1");
        assert_eq!(snake.eye_style, EyeStyle::Chibi);
        assert_eq!(snake.eye_left, (10, 6));
        assert_eq!(snake.eye_right, Some((18, 6)));
        assert_eq!(snake.shebang, (12, 20));

        let bee = load_sheet(Species::Bee, None);
        assert_eq!(bee.species, Species::Bee);
        assert_eq!(bee.style, "P3");
        assert_eq!(bee.eye_style, EyeStyle::Chibi);
        assert_eq!(bee.eye_left, (10, 10));
        assert_eq!(bee.eye_right, Some((19, 10)));
        assert_eq!(bee.shebang, (12, 19));

        let owl = load_sheet(Species::Owl, None);
        assert_eq!(owl.species, Species::Owl);
        assert_eq!(owl.style, "P4");
        assert_eq!(owl.eye_style, EyeStyle::Rpg);
        assert_eq!(owl.eye_left, (10, 11));
        assert_eq!(owl.eye_right, Some((18, 11)));
        assert_eq!(owl.shebang, (12, 19));
    }

    #[test]
    fn render_style_parse_and_names() {
        for style in [RenderStyle::Auto, RenderStyle::Pixel, RenderStyle::Ascii, RenderStyle::Off] {
            assert_eq!(RenderStyle::parse(style.name()).unwrap(), style);
            assert_eq!(RenderStyle::parse(&format!(" {} ", style.name().to_uppercase())).unwrap(), style);
        }
        let err = RenderStyle::parse("sixel").unwrap_err().to_string();
        assert!(err.contains("sixel") && err.contains(RenderStyle::VALID), "{err}");
    }

    #[test]
    fn detect_mode_handles_capabilities_and_tmux_passthrough() {
        let ghostty = [("GHOSTTY_RESOURCES_DIR", "/usr/share/ghostty")];
        assert_eq!(detect_mode(RenderStyle::Auto, &env_from(&ghostty), true, false), EffectiveMode::Off);
        assert_eq!(detect_mode(RenderStyle::Off, &env_from(&ghostty), true, true), EffectiveMode::Off);
        assert_eq!(detect_mode(RenderStyle::Auto, &env_from(&ghostty), false, true), EffectiveMode::Ascii);
        assert_eq!(
            detect_mode(
                RenderStyle::Pixel,
                &env_from(&[("GHOSTTY_RESOURCES_DIR", "/x"), ("TERM", "dumb")]),
                true,
                true
            ),
            EffectiveMode::Ascii
        );
        assert_eq!(detect_mode(RenderStyle::Ascii, &env_from(&ghostty), true, true), EffectiveMode::Ascii);
        assert_eq!(detect_mode(RenderStyle::Pixel, &env_from(&[]), true, true), EffectiveMode::Pixel);

        // Auto detection across capable terminals.
        assert_eq!(detect_mode(RenderStyle::Auto, &env_from(&ghostty), true, true), EffectiveMode::Pixel);
        assert_eq!(
            detect_mode(RenderStyle::Auto, &env_from(&[("KITTY_WINDOW_ID", "1")]), true, true),
            EffectiveMode::Pixel
        );
        assert_eq!(
            detect_mode(RenderStyle::Auto, &env_from(&[("TERM_PROGRAM", "WezTerm")]), true, true),
            EffectiveMode::Pixel
        );
        assert_eq!(
            detect_mode(RenderStyle::Auto, &env_from(&[("TERM_PROGRAM", "Ghostty")]), true, true),
            EffectiveMode::Pixel
        );
        assert_eq!(
            detect_mode(RenderStyle::Auto, &env_from(&[("TERM", "xterm-kitty")]), true, true),
            EffectiveMode::Pixel
        );
        assert_eq!(
            detect_mode(RenderStyle::Auto, &env_from(&[("TERM", "xterm-ghostty")]), true, true),
            EffectiveMode::Pixel
        );
        assert_eq!(
            detect_mode(RenderStyle::Auto, &env_from(&[("TERM_PROGRAM", "kitty")]), true, true),
            EffectiveMode::Pixel
        );
        assert_eq!(
            detect_mode(RenderStyle::Auto, &env_from(&[("TERM", "xterm-256color")]), true, true),
            EffectiveMode::Ascii
        );
        assert_eq!(
            detect_mode(RenderStyle::Auto, &env_from(&[("GHOSTTY_RESOURCES_DIR", "")]), true, true),
            EffectiveMode::Ascii
        );

        // Inside tmux: default user tmux falls back to Ascii unless HUH_KITTY_GRAPHICS=1; `huh` socket uses Pixel.
        let user_tmux = [("GHOSTTY_RESOURCES_DIR", "/x"), ("TMUX", "/tmp/tmux-1000/default,123,0")];
        assert_eq!(detect_mode(RenderStyle::Auto, &env_from(&user_tmux), true, true), EffectiveMode::Ascii);
        let forced_tmux =
            [("GHOSTTY_RESOURCES_DIR", "/x"), ("TMUX", "/tmp/tmux-1000/default,123,0"), ("HUH_KITTY_GRAPHICS", "1")];
        assert_eq!(detect_mode(RenderStyle::Auto, &env_from(&forced_tmux), true, true), EffectiveMode::Pixel);
        let huh_tmux = [("GHOSTTY_RESOURCES_DIR", "/x"), ("TMUX", "/tmp/tmux-1000/huh,99,0")];
        assert_eq!(detect_mode(RenderStyle::Auto, &env_from(&huh_tmux), true, true), EffectiveMode::Pixel);
        // `huh` tmux inside a non-capable outer terminal still degrades to Ascii.
        let non_kitty_huh_tmux = [("TERM", "tmux-256color"), ("TMUX", "/tmp/tmux-1000/huh,99,0")];
        assert_eq!(detect_mode(RenderStyle::Auto, &env_from(&non_kitty_huh_tmux), true, true), EffectiveMode::Ascii);
    }

    #[test]
    fn override_dir_uses_xdg_or_home() {
        assert_eq!(override_dir_from(Some("/cfg"), Some("/home/u")), Some(PathBuf::from("/cfg/huh/sprites")));
        assert_eq!(override_dir_from(Some(""), Some("/home/u")), Some(PathBuf::from("/home/u/.config/huh/sprites")));
        assert_eq!(override_dir_from(None, Some("")), None);
    }

    #[test]
    fn load_sheet_supports_valid_override_and_falls_back_on_errors() {
        let dir = scratch_dir("overrides");
        let target = dir.join("penguin.toml");

        // Valid custom override using semantic palette tokens.
        let custom = embedded_toml(Species::Penguin).replace("#141c2e", "accent");
        std::fs::write(&target, &custom).unwrap();
        let loaded = load_sheet(Species::Penguin, Some(&dir));
        let colors = ThemeColors { accent: (10, 20, 30), ..ThemeColors::default() };
        let rgba = render_rgba(&loaded, Mood::Idle, 0, &colors);
        // Penguin row 2 col 14 is 'K', which now resolves to `accent`.
        assert_eq!(pixel(&rgba, 14, 2), [10, 20, 30, 255]);

        // Also accept `grid` placed after `[palette]`.
        let grid_after_palette = r##"
species = "penguin"
style = "custom"
eye_style = "chibi"
eye_left = [9, 10]
eye_right = [19, 10]
shebang = [12, 20]

[palette]
K = "#112233"
S = "shadow"
eye_ink = "#112233"
eye_glint = "#ffffff"
shebang_fg = "success"
"##
        .to_string()
            + "\ngrid = [\n"
            + &"  \"KKKKKKKKKKKKKKKKKKKKKKKKKKKKKKKK\",\n".repeat(32)
            + "]\n";
        std::fs::write(&target, grid_after_palette).unwrap();
        let after_pal = load_sheet(Species::Penguin, Some(&dir));
        assert_eq!(after_pal.style, "custom");

        // Corrupt or out-of-bounds overrides fall back cleanly to the embedded sheet.
        for bad in [
            "not valid toml [[[".to_string(),
            embedded_toml(Species::Penguin).replace("species = \"penguin\"", "species = \"dragon\""),
            embedded_toml(Species::Penguin).replace("[9, 10]", "[31, 10]"),
            embedded_toml(Species::Penguin).replace("[12, 20]", "[26, 20]"),
            embedded_toml(Species::Penguin).replace("eye_right = [19, 10]", ""),
            embedded_toml(Species::Penguin).replace("\"................................\",\n", ""),
            embedded_toml(Species::Penguin).replace("#141c2e", "not_a_color"),
            embedded_toml(Species::Penguin).replace("K = \"#141c2e\"", ""),
            format!("{}\n# {}", embedded_toml(Species::Penguin), "x".repeat(MAX_OVERRIDE_BYTES as usize + 64)),
        ] {
            std::fs::write(&target, bad).unwrap();
            let fallback = load_sheet(Species::Penguin, Some(&dir));
            assert_eq!(fallback, load_sheet(Species::Penguin, None));
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_template_creates_and_never_overwrites() {
        let dir = scratch_dir("export");
        let path = export_template(Species::Owl, &dir).unwrap();
        assert_eq!(path, dir.join("owl.toml"));
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, embedded_toml(Species::Owl));

        std::fs::write(&path, "# user customized\n").unwrap();
        let second = export_template(Species::Owl, &dir).unwrap();
        assert_eq!(second, path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# user customized\n");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_rgba_moods_and_effects_match_spec() {
        let colors = ThemeColors::default();
        let penguin = load_sheet(Species::Penguin, None);

        // Idle tick 0: shadow on row 30, #! chest badge at (12, 20), open eyes at (9, 10).
        let idle0 = render_rgba(&penguin, Mood::Idle, 0, &colors);
        assert_eq!(pixel(&idle0, 8, 30), [0, 0, 0, 89]);
        // Row 0 col 1 of SHEBANG_7X5 is '#' -> (13, 20) has shebang_fg #226bb8.
        assert_eq!(pixel(&idle0, 13, 20), [0x22, 0x6b, 0xb8, 255]);
        // Penguin open eye glint at (9, 10) is #ffffff; ink at (10, 10) is #141c2e.
        assert_eq!(pixel(&idle0, 9, 10), [0xff, 0xff, 0xff, 255]);
        assert_eq!(pixel(&idle0, 10, 10), [0x14, 0x1c, 0x2e, 255]);

        // Light theme adjusts shadow alpha/color.
        let light = ThemeColors { light: true, ..colors };
        let idle_light = render_rgba(&penguin, Mood::Idle, 0, &light);
        assert_eq!(pixel(&idle_light, 8, 30), [19, 26, 42, 42]);

        // Idle breathe at tick 5 shifts top rows (y < 17) down 1px: crest 'K' moves from y=2 to y=3.
        let idle5 = render_rgba(&penguin, Mood::Idle, 5, &colors);
        assert_eq!(pixel(&idle5, 14, 2), [0, 0, 0, 0]);
        assert_eq!(pixel(&idle5, 14, 3), [0x14, 0x1c, 0x2e, 255]);

        // Idle blink at tick 23 closes eyes into a horizontal line at y + 2 + breathe(=0) = 12.
        let blink = render_rgba(&penguin, Mood::Idle, 23, &colors);
        assert_eq!(pixel(&blink, 9, 12), [0x14, 0x1c, 0x2e, 255]);
        assert_eq!(pixel(&blink, 10, 12), [0x14, 0x1c, 0x2e, 255]);
        assert_eq!(pixel(&blink, 11, 12), [0x14, 0x1c, 0x2e, 255]);

        // Happy: sparkles in accent + gold and bounce dy = -1 on tick 1.
        let happy1 = render_rgba(&penguin, Mood::Happy, 1, &colors);
        assert_eq!(pixel(&happy1, 4, 5), [colors.accent.0, colors.accent.1, colors.accent.2, 255]);
        assert_eq!(pixel(&happy1, 3, 5), [colors.accent.0, colors.accent.1, colors.accent.2, 255]);
        assert_eq!(pixel(&happy1, 27, 4), [0xff, 0xd5, 0x5a, 255]);
        assert_eq!(pixel(&happy1, 14, 1), [0x14, 0x1c, 0x2e, 255]);

        // Sleepy: floating zZ in accent.
        let sleepy2 = render_rgba(&penguin, Mood::Sleepy, 2, &colors);
        assert_eq!(pixel(&sleepy2, 25, 5), [colors.accent.0, colors.accent.1, colors.accent.2, 255]);
        assert_eq!(pixel(&sleepy2, 28, 2), [colors.accent.0, colors.accent.1, colors.accent.2, 255]);

        // Hungry: widened eyes (white sclera) + '?' spark in colors.note.
        let hungry = render_rgba(&penguin, Mood::Hungry, 0, &colors);
        assert_eq!(pixel(&hungry, 9, 11), [255, 255, 255, 255]);
        assert_eq!(pixel(&hungry, 27, 2), [colors.note.0, colors.note.1, colors.note.2, 255]);
        assert_eq!(pixel(&hungry, 28, 7), [colors.note.0, colors.note.1, colors.note.2, 255]);

        // Alert: horizontal 1px shake moves body + eyes + #! together, alternating red/gold '!'.
        let alert0 = render_rgba(&penguin, Mood::Alert, 0, &colors);
        let alert1 = render_rgba(&penguin, Mood::Alert, 1, &colors);
        assert_eq!(pixel(&alert0, 27, 1), [colors.warning.0, colors.warning.1, colors.warning.2, 255]);
        assert_eq!(pixel(&alert1, 27, 1), [0xff, 0xd5, 0x5a, 255]);
        assert_eq!(pixel(&alert1, 14, 20), [0x22, 0x6b, 0xb8, 255]);

        // Celebrating: jump arc dy = -3 at tick 2 (crest y=2 moves off top or feet move up 3px) + confetti.
        let celeb2 = render_rgba(&penguin, Mood::Celebrating, 2, &colors);
        assert_eq!(pixel(&celeb2, 9, 26), [0x14, 0x1c, 0x2e, 255]);
        assert_eq!(pixel(&celeb2, 3, 4), [colors.accent.0, colors.accent.1, colors.accent.2, 255]);

        // RPG eyes (Cat & Owl) and ChibiSingle (Turtle) across moods + #! badge check.
        let cat = load_sheet(Species::Cat, None);
        let cat_idle = render_rgba(&cat, Mood::Idle, 0, &colors);
        assert_eq!(pixel(&cat_idle, 9, 10), [0xff, 0xfd, 0xf7, 255]); // eye_glint
        assert_eq!(pixel(&cat_idle, 10, 10), [0x1c, 0x42, 0x30, 255]); // eye_ink
        assert_eq!(pixel(&cat_idle, 11, 20), [0xc9, 0x88, 0x16, 255]); // shebang_fg
        let cat_happy = render_rgba(&cat, Mood::Happy, 0, &colors);
        assert_eq!(pixel(&cat_happy, 10, 10), [0x1c, 0x42, 0x30, 255]); // chevron tip (x+1, y)
        let cat_hungry = render_rgba(&cat, Mood::Hungry, 0, &colors);
        assert_eq!(pixel(&cat_hungry, 9, 12), [0x1c, 0x42, 0x30, 255]); // 3rd row of widened RPG eye

        let turtle = load_sheet(Species::Turtle, None);
        let turtle_idle = render_rgba(&turtle, Mood::Idle, 0, &colors);
        assert_eq!(pixel(&turtle_idle, 24, 13), [0xff, 0xff, 0xff, 255]); // single chibi eye glint
        assert_eq!(pixel(&turtle_idle, 9, 15), [0x19, 0x6e, 0x5d, 255]); // shebang_fg

        let owl = load_sheet(Species::Owl, None);
        let owl_sleepy = render_rgba(&owl, Mood::Sleepy, 0, &colors);
        assert_eq!(pixel(&owl_sleepy, 10, 12), [0x1e, 0x27, 0x45, 255]); // sleepy closed RPG eye
        assert_eq!(pixel(&owl_sleepy, 13, 19), [0x26, 0x65, 0xa6, 255]); // shebang_fg
    }

    #[test]
    fn frame_key_matches_visual_frames_across_all_species_and_moods() {
        let colors = ThemeColors::default();
        for species in Species::ALL {
            let sheet = load_sheet(species, None);
            for mood in Mood::ALL {
                let mut seen: Vec<(u64, [u8; RGBA_BYTES])> = Vec::new();
                for tick in 0..60 {
                    let key = frame_key(mood, tick);
                    let rgba = render_rgba(&sheet, mood, tick, &colors);
                    for (prev_key, prev_rgba) in &seen {
                        if *prev_key == key {
                            assert_eq!(rgba, *prev_rgba, "{species:?} {mood:?} tick {tick} collided key");
                        } else {
                            assert_ne!(rgba, *prev_rgba, "{species:?} {mood:?} tick {tick} redundant key");
                        }
                    }
                    seen.push((key, rgba));
                }
            }
            // Different moods never collide on frame_key.
            let keys: Vec<u64> = Mood::ALL.iter().map(|&m| frame_key(m, 0)).collect();
            for i in 0..keys.len() {
                for j in (i + 1)..keys.len() {
                    assert_ne!(keys[i], keys[j]);
                }
            }
        }
    }

    #[test]
    fn base64_kitty_and_tmux_dcs_round_trip() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(encode_base64(b"foob"), "Zm9vYg==");
        assert_eq!(encode_base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");

        let colors = ThemeColors::default();
        let sheet = load_sheet(Species::Cat, None);
        let rgba = render_rgba(&sheet, Mood::Idle, 0, &colors);
        let seq = encode_kitty(&rgba, 8, 4, false);
        assert!(seq.starts_with("\x1b_Ga=T,f=32,s=32,v=32,o=z,i=1,r=4,c=8,C=1,q=2;"), "{seq}");
        assert!(seq.ends_with("\x1b\\"), "{seq}");

        let payload = seq
            .strip_prefix("\x1b_Ga=T,f=32,s=32,v=32,o=z,i=1,r=4,c=8,C=1,q=2;")
            .unwrap()
            .strip_suffix("\x1b\\")
            .unwrap();
        let compressed = decode_base64(payload);
        let mut decoded = Vec::new();
        ZlibDecoder::new(compressed.as_slice()).read_to_end(&mut decoded).unwrap();
        assert_eq!(decoded.as_slice(), rgba.as_slice());

        // Multi-chunk splitting (>4096 base64 chars) via end-to-end encode_kitty on incompressible RGBA bytes.
        let mut noisy = [0u8; RGBA_BYTES];
        let mut state: u32 = 0x1234_5678;
        for byte in &mut noisy {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            *byte = (state >> 24) as u8;
        }
        let chunked = encode_kitty(&noisy, 8, 4, false);
        let parts: Vec<&str> = chunked.split("\x1b\\").filter(|s| !s.is_empty()).collect();
        assert_eq!(parts.len(), 2);
        assert!(parts[0].starts_with("\x1b_Ga=T,f=32,s=32,v=32,o=z,i=1,r=4,c=8,C=1,q=2,m=1;"));
        let p0 = parts[0].split_once(';').unwrap().1;
        assert_eq!(p0.len(), 4096);
        assert!(parts[1].starts_with("\x1b_Gq=2,m=0;"));
        let p1 = parts[1].split_once(';').unwrap().1;
        assert!(!p1.is_empty() && p1.len() <= 4096);
        let mut combined_decoded = Vec::new();
        ZlibDecoder::new(decode_base64(&format!("{p0}{p1}")).as_slice()).read_to_end(&mut combined_decoded).unwrap();
        assert_eq!(combined_decoded.as_slice(), noisy.as_slice());

        // Also test 3-chunk middle-chunk (`q=2,m=1;`) formatting.
        let three_chunks = encode_kitty_chunks(&"ABCD".repeat(2500), 8, 4, false);
        let three_parts: Vec<&str> = three_chunks.split("\x1b\\").filter(|s| !s.is_empty()).collect();
        assert_eq!(three_parts.len(), 3);
        assert!(three_parts[0].starts_with("\x1b_Ga=T,f=32,s=32,v=32,o=z,i=1,r=4,c=8,C=1,q=2,m=1;"));
        assert!(three_parts[1].starts_with("\x1b_Gq=2,m=1;"));
        assert!(three_parts[2].starts_with("\x1b_Gq=2,m=0;"));

        // Tmux DCS wrapping doubles interior ESCs cleanly.
        let tmux_seq = encode_kitty(&rgba, 8, 4, true);
        assert!(tmux_seq.starts_with("\x1bPtmux;\x1b\x1b_Ga=T,"), "{tmux_seq}");
        assert!(tmux_seq.ends_with("\x1b\x1b\\\x1b\\"), "{tmux_seq}");

        assert_eq!(delete_kitty(false), "\x1b_Ga=d,d=i,i=1,q=2\x1b\\");
        assert_eq!(delete_kitty(true), "\x1bPtmux;\x1b\x1b_Ga=d,d=i,i=1,q=2\x1b\x1b\\\x1b\\");

        // Exercise helpers cleanly.
        let _ = in_tmux_env();
        let _ = override_dir();
    }
}
