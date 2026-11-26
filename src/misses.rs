//! Opt-in local miss log (`$XDG_STATE_HOME/huh/misses.jsonl` or `~/.local/state/huh/misses.jsonl`).
//!
//! Pure `std` implementation (zero external crate imports): records unmatched/refused plain-language
//! queries locally when enabled so users and maintainers can see what the catalog missed. Nothing is
//! ever sent over the network, and I/O errors during logging are ignored so `huh` never fails.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

pub const MAX_ENTRIES: usize = 1000;
pub const MAX_REQUEST_CHARS: usize = 240;

#[cfg_attr(not(test), allow(dead_code))]
pub const KIND_NO_MATCH: &str = "no_match";
#[cfg_attr(not(test), allow(dead_code))]
pub const KIND_HINT_ONLY: &str = "hint_only";
#[cfg_attr(not(test), allow(dead_code))]
pub const KIND_REFUSAL: &str = "refusal";

const SECS_PER_DAY: u64 = 86_400;
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissRecord {
    pub ts: String,
    pub request: String,
    pub kind: String,
    pub platform: String,
    pub pointer: Option<String>,
}

/// `$XDG_STATE_HOME/huh/misses.jsonl`, or `~/.local/state/huh/misses.jsonl`.
pub fn log_path(xdg_state: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    match xdg_state.filter(|v| !v.is_empty()) {
        Some(dir) => Some(Path::new(dir).join("huh/misses.jsonl")),
        None => Some(Path::new(home.filter(|v| !v.is_empty())?).join(".local/state/huh/misses.jsonl")),
    }
}

pub fn default_log_path() -> Option<PathBuf> {
    log_path(std::env::var("XDG_STATE_HOME").ok().as_deref(), std::env::var("HOME").ok().as_deref())
}

/// Resolves whether miss logging is enabled:
/// 1. `HUH_MISS_LOG` (`1`/`true`/`on`/`yes` vs `0`/`false`/`off`/`no`)
/// 2. `miss_log = true/false` in `config.toml`
/// 3. `prefs.miss_log` (`prefs.toml`)
/// 4. default `false`.
pub fn resolve_enabled(env_val: Option<&str>, config_path: Option<&Path>, prefs_val: Option<bool>) -> bool {
    if let Some(flag) = env_val.and_then(parse_toggle) {
        return flag;
    }
    if let Some(flag) =
        config_path.and_then(|path| fs::read_to_string(path).ok()).and_then(|text| miss_log_from_config_text(&text))
    {
        return flag;
    }
    if let Some(flag) = prefs_val {
        return flag;
    }
    false
}

fn parse_toggle(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => None,
    }
}

/// Minimal pure-std reader for top-level `miss_log = true/false` in `config.toml`.
/// Ignores `#` comments, string contents (single- and multi-line), and keys inside `[subtables]`.
fn miss_log_from_config_text(text: &str) -> Option<bool> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut in_ml_basic = false;
    let mut in_ml_literal = false;
    let mut bracket_depth: usize = 0;
    let mut in_top_level = true;
    let mut found = None;

    for raw_line in text.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let started_clean = !in_ml_basic && !in_ml_literal && bracket_depth == 0;
        let mut clean = String::new();
        let mut in_sl_basic = false;
        let mut in_sl_literal = false;
        let mut saw_non_ws = false;
        let mut is_table_header = false;

        let mut i = 0;
        while i < line.len() {
            let rest = &line[i..];
            if in_ml_basic {
                if rest.starts_with('\\') {
                    let mut chars = rest.chars();
                    let _ = chars.next();
                    let adv = chars.next().map_or(1, |c| 1 + c.len_utf8());
                    i += adv;
                } else if rest.starts_with("\"\"\"") {
                    in_ml_basic = false;
                    i += 3;
                } else {
                    i += rest.chars().next().map_or(1, char::len_utf8);
                }
                continue;
            }
            if in_ml_literal {
                if rest.starts_with("'''") {
                    in_ml_literal = false;
                    i += 3;
                } else {
                    i += rest.chars().next().map_or(1, char::len_utf8);
                }
                continue;
            }
            if in_sl_basic {
                if rest.starts_with('\\') {
                    let mut chars = rest.chars();
                    let _ = chars.next();
                    let adv = chars.next().map_or(1, |c| 1 + c.len_utf8());
                    clean.push_str(&rest[..adv]);
                    i += adv;
                } else if rest.starts_with('"') {
                    in_sl_basic = false;
                    clean.push('"');
                    i += 1;
                } else if let Some(ch) = rest.chars().next() {
                    clean.push(ch);
                    i += ch.len_utf8();
                }
                continue;
            }
            if in_sl_literal {
                if rest.starts_with('\'') {
                    in_sl_literal = false;
                    clean.push('\'');
                    i += 1;
                } else if let Some(ch) = rest.chars().next() {
                    clean.push(ch);
                    i += ch.len_utf8();
                }
                continue;
            }

            let Some(ch) = rest.chars().next() else {
                break;
            };
            if ch == '#' {
                break;
            }
            if !saw_non_ws && !ch.is_whitespace() {
                saw_non_ws = true;
                if started_clean && ch == '[' {
                    is_table_header = true;
                }
            }
            if rest.starts_with("\"\"\"") {
                in_ml_basic = true;
                i += 3;
                continue;
            }
            if rest.starts_with("'''") {
                in_ml_literal = true;
                i += 3;
                continue;
            }
            if ch == '"' {
                in_sl_basic = true;
                clean.push('"');
                i += 1;
                continue;
            }
            if ch == '\'' {
                in_sl_literal = true;
                clean.push('\'');
                i += 1;
                continue;
            }
            if ch == '[' && !is_table_header {
                bracket_depth += 1;
            } else if ch == ']' && !is_table_header {
                bracket_depth = bracket_depth.saturating_sub(1);
            }
            clean.push(ch);
            i += ch.len_utf8();
        }

        if is_table_header {
            in_top_level = false;
            continue;
        }
        if !started_clean || !in_top_level || in_ml_basic || in_ml_literal || bracket_depth != 0 {
            continue;
        }
        let Some((lhs, rhs)) = clean.split_once('=') else {
            continue;
        };
        let key = lhs.trim();
        if key == "miss_log" || key == "\"miss_log\"" || key == "'miss_log'" {
            found = match rhs.trim() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            };
        }
    }

    found
}

/// Current UTC timestamp formatted as ISO-8601 `YYYY-MM-DDTHH:MM:SSZ`.
pub fn now_iso_utc() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    format_iso_utc(secs)
}

/// Formats Unix epoch seconds as UTC `YYYY-MM-DDTHH:MM:SSZ` using Howard Hinnant's
/// proleptic Gregorian civil-from-days formula.
fn format_iso_utc(epoch_secs: u64) -> String {
    let days = (epoch_secs / SECS_PER_DAY) as i64;
    let rem = epoch_secs % SECS_PER_DAY;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + if m <= 2 { 1 } else { 0 };

    format!("{year:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Strips control characters and collapses whitespace/newlines to single spaces.
fn sanitize_single_line(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut pending_space = false;
    for ch in raw.chars() {
        if matches!(ch, '\n' | '\r' | '\t' | ' ') {
            if !out.is_empty() {
                pending_space = true;
            }
        } else if ch.is_control() {
            // Drop NUL, ESC, BEL, C1 controls, etc.
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(ch);
        }
    }
    out
}

fn sanitize_request(raw: &str) -> String {
    let clean = sanitize_single_line(raw);
    if clean.chars().count() <= MAX_REQUEST_CHARS {
        return clean;
    }
    clean.chars().take(MAX_REQUEST_CHARS).collect::<String>().trim_end().to_string()
}

fn sanitize_pointer(raw: &str) -> Option<String> {
    raw.lines().map(sanitize_single_line).find(|line| !line.is_empty())
}

fn sanitize_record(record: &MissRecord) -> MissRecord {
    let ts = sanitize_single_line(&record.ts);
    MissRecord {
        ts: if ts.is_empty() { now_iso_utc() } else { ts },
        request: sanitize_request(&record.request),
        kind: sanitize_single_line(&record.kind),
        platform: sanitize_single_line(&record.platform),
        pointer: record.pointer.as_deref().and_then(sanitize_pointer),
    }
}

fn push_json_str(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn to_json_line(record: &MissRecord) -> String {
    let mut out = String::with_capacity(128);
    out.push_str("{\"ts\":");
    push_json_str(&record.ts, &mut out);
    out.push_str(",\"request\":");
    push_json_str(&record.request, &mut out);
    out.push_str(",\"kind\":");
    push_json_str(&record.kind, &mut out);
    out.push_str(",\"platform\":");
    push_json_str(&record.platform, &mut out);
    if let Some(pointer) = &record.pointer {
        out.push_str(",\"pointer\":");
        push_json_str(pointer, &mut out);
    }
    out.push('}');
    out
}

struct JsonParser<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn new(src: &'a str) -> Self {
        Self { src, bytes: src.as_bytes(), pos: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if matches!(b, b' ' | b'\t' | b'\r' | b'\n') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn expect_byte(&mut self, expected: u8) -> Option<()> {
        if self.peek() == Some(expected) {
            self.pos += 1;
            Some(())
        } else {
            None
        }
    }

    fn parse_hex4(&mut self) -> Option<u16> {
        let slice = self.src.get(self.pos..self.pos + 4)?;
        if !slice.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let value = u16::from_str_radix(slice, 16).ok()?;
        self.pos += 4;
        Some(value)
    }

    fn parse_string(&mut self) -> Option<String> {
        self.expect_byte(b'"')?;
        let mut out = String::new();
        while let Some(b) = self.peek() {
            if b == b'"' {
                self.pos += 1;
                return Some(out);
            }
            if b == b'\\' {
                self.pos += 1;
                let esc = self.peek()?;
                self.pos += 1;
                match esc {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\x08'),
                    b'f' => out.push('\x0c'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let code = self.parse_hex4()?;
                        if (0xD800..=0xDBFF).contains(&code) {
                            self.expect_byte(b'\\')?;
                            self.expect_byte(b'u')?;
                            let low = self.parse_hex4()?;
                            if !(0xDC00..=0xDFFF).contains(&low) {
                                return None;
                            }
                            let cp = 0x10000 + ((u32::from(code) - 0xD800) << 10) + (u32::from(low) - 0xDC00);
                            out.push(char::from_u32(cp)?);
                        } else if (0xDC00..=0xDFFF).contains(&code) {
                            return None;
                        } else {
                            out.push(char::from_u32(u32::from(code))?);
                        }
                    }
                    _ => return None,
                }
                continue;
            }
            if b < 0x20 {
                return None;
            }
            let ch = self.src.get(self.pos..)?.chars().next()?;
            self.pos += ch.len_utf8();
            out.push(ch);
        }
        None
    }

    fn consume_literal(&mut self, lit: &str) -> Option<()> {
        if self.src.get(self.pos..)?.starts_with(lit) {
            self.pos += lit.len();
            Some(())
        } else {
            None
        }
    }

    fn skip_number(&mut self) -> Option<()> {
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek()? {
            b'0' => self.pos += 1,
            b'1'..=b'9' => {
                self.pos += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return None,
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return None;
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return None;
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        Some(())
    }

    fn skip_value(&mut self) -> Option<()> {
        self.skip_ws();
        match self.peek()? {
            b'"' => self.parse_string().map(|_| ()),
            b'{' => {
                self.pos += 1;
                self.skip_ws();
                if self.peek() == Some(b'}') {
                    self.pos += 1;
                    return Some(());
                }
                loop {
                    self.skip_ws();
                    self.parse_string()?;
                    self.skip_ws();
                    self.expect_byte(b':')?;
                    self.skip_value()?;
                    self.skip_ws();
                    match self.peek()? {
                        b',' => self.pos += 1,
                        b'}' => {
                            self.pos += 1;
                            return Some(());
                        }
                        _ => return None,
                    }
                }
            }
            b'[' => {
                self.pos += 1;
                self.skip_ws();
                if self.peek() == Some(b']') {
                    self.pos += 1;
                    return Some(());
                }
                loop {
                    self.skip_value()?;
                    self.skip_ws();
                    match self.peek()? {
                        b',' => self.pos += 1,
                        b']' => {
                            self.pos += 1;
                            return Some(());
                        }
                        _ => return None,
                    }
                }
            }
            b't' => self.consume_literal("true"),
            b'f' => self.consume_literal("false"),
            b'n' => self.consume_literal("null"),
            b'-' | b'0'..=b'9' => self.skip_number(),
            _ => None,
        }
    }

    fn parse_record(&mut self) -> Option<MissRecord> {
        self.skip_ws();
        self.expect_byte(b'{')?;
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return None;
        }

        let mut ts: Option<String> = None;
        let mut request: Option<String> = None;
        let mut kind: Option<String> = None;
        let mut platform: Option<String> = None;
        let mut pointer: Option<String> = None;

        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect_byte(b':')?;
            self.skip_ws();
            match key.as_str() {
                "ts" => ts = Some(self.parse_string()?),
                "request" => request = Some(self.parse_string()?),
                "kind" => kind = Some(self.parse_string()?),
                "platform" => platform = Some(self.parse_string()?),
                "pointer" => {
                    if self.peek() == Some(b'n') {
                        self.consume_literal("null")?;
                        pointer = None;
                    } else {
                        let value = self.parse_string()?;
                        pointer = (!value.is_empty()).then_some(value);
                    }
                }
                _ => self.skip_value()?,
            }
            self.skip_ws();
            match self.peek()? {
                b',' => self.pos += 1,
                b'}' => {
                    self.pos += 1;
                    break;
                }
                _ => return None,
            }
        }

        self.skip_ws();
        if self.pos != self.bytes.len() {
            return None;
        }

        Some(MissRecord { ts: ts?, request: request?, kind: kind?, platform: platform?, pointer })
    }
}

fn parse_json_line(line: &str) -> Option<MissRecord> {
    JsonParser::new(line).parse_record()
}

fn ensure_mode_0600(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mode = fs::metadata(path)?.permissions().mode() & 0o777;
        if mode != 0o600 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn write_atomic_0600(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("misses.jsonl");
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!("{file_name}.tmp.{}.{seq}", std::process::id()));

    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    opts.mode(0o600);

    let write_res = (|| -> std::io::Result<()> {
        let mut file = opts.open(&tmp)?;
        file.write_all(bytes)?;
        file.flush()?;
        ensure_mode_0600(&tmp)?;
        fs::rename(&tmp, path)?;
        Ok(())
    })();

    if write_res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    write_res
}

fn append_line_0600(path: &Path, encoded_line: &str, needs_leading_newline: bool) -> std::io::Result<()> {
    let mut opts = OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    opts.mode(0o600);

    let mut file = opts.open(path)?;
    if needs_leading_newline {
        file.write_all(b"\n")?;
    }
    file.write_all(encoded_line.as_bytes())?;
    file.write_all(b"\n")?;
    file.flush()?;
    ensure_mode_0600(path)?;
    Ok(())
}

fn try_record_miss(path: &Path, record: &MissRecord) -> std::io::Result<()> {
    let clean = sanitize_record(record);
    let encoded = to_json_line(&clean);

    if let Some(dir) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(dir)?;
    }

    let (existing_lines, needs_leading_newline) = match fs::read(path) {
        Ok(bytes) => {
            let needs_nl = !bytes.is_empty() && bytes.last() != Some(&b'\n');
            let lines: Vec<String> =
                String::from_utf8_lossy(&bytes).lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect();
            (lines, needs_nl)
        }
        Err(err) if err.kind() == ErrorKind::NotFound => (Vec::new(), false),
        Err(err) => return Err(err),
    };

    if existing_lines.len() >= MAX_ENTRIES {
        let keep = MAX_ENTRIES.saturating_sub(1);
        let start = existing_lines.len().saturating_sub(keep);
        let mut body = String::new();
        for line in &existing_lines[start..] {
            body.push_str(line);
            body.push('\n');
        }
        if MAX_ENTRIES > 0 {
            body.push_str(&encoded);
            body.push('\n');
        }
        write_atomic_0600(path, body.as_bytes())?;
    } else {
        append_line_0600(path, &encoded, needs_leading_newline)?;
    }
    Ok(())
}

/// Appends one miss record to `path` (`0600` on Unix), rotating atomically when exceeding
/// `MAX_ENTRIES`. All I/O errors are silently ignored so logging never breaks `huh`.
pub fn record_miss(path: &Path, record: &MissRecord) {
    let _ = try_record_miss(path, record);
}

/// Loads all valid `MissRecord` entries from `path` (oldest to newest), skipping blank or
/// malformed lines and returning an empty `Vec` if the file does not exist or cannot be read.
pub fn load_misses(path: &Path) -> Vec<MissRecord> {
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    bytes
        .split(|&b| b == b'\n')
        .filter_map(|raw_line| {
            let trimmed = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
            let line = std::str::from_utf8(trimmed).ok()?.trim();
            if line.is_empty() {
                return None;
            }
            parse_json_line(line)
        })
        .collect()
}

/// Removes the miss log file at `path`. Returns `Ok(true)` if a file existed and was removed,
/// `Ok(false)` if no file existed, or any other I/O error.
pub fn clear_misses(path: &Path) -> std::io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

/// Renders a human-readable summary of miss logging status, top repeated misses, and recent entries.
pub fn render_summary(records: &[MissRecord], enabled: bool, path: &Path, limit: usize) -> String {
    let mut out = String::new();
    if enabled {
        let _ = writeln!(out, "Miss logging: enabled (local only)");
        let _ =
            writeln!(out, "Disable with: huh misses --disable (or HUH_MISS_LOG=0, or miss_log = false in config.toml)");
    } else {
        let _ = writeln!(out, "Miss logging: disabled (opt-in, local only)");
        let _ =
            writeln!(out, "Enable with:  huh misses --enable (or HUH_MISS_LOG=1, or miss_log = true in config.toml)");
    }
    let _ = writeln!(out, "Log file:     {}", path.display());
    let _ = writeln!(out, "Entries:      {} (cap: {MAX_ENTRIES})", records.len());

    if records.is_empty() {
        out.push_str("\nNo missed requests recorded yet.\n");
        return out;
    }
    if limit == 0 {
        return out;
    }

    // Aggregate top repeated missed requests (case-insensitive trimmed key, preserving newest casing).
    struct Agg {
        count: usize,
        last_idx: usize,
        display: String,
        kind: String,
    }
    let mut grouped: HashMap<String, Agg> = HashMap::new();
    for (idx, rec) in records.iter().enumerate() {
        let display = sanitize_request(&rec.request);
        let key = display.to_ascii_lowercase();
        let kind = sanitize_single_line(&rec.kind);
        grouped
            .entry(key)
            .and_modify(|agg| {
                agg.count += 1;
                agg.last_idx = idx;
                agg.display = display.clone();
                agg.kind = kind.clone();
            })
            .or_insert(Agg { count: 1, last_idx: idx, display, kind });
    }
    let mut top: Vec<Agg> = grouped.into_values().collect();
    top.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| b.last_idx.cmp(&a.last_idx)));

    let shown_top = top.len().min(limit);
    let _ = writeln!(out, "\nTop missed requests (top {shown_top}):");
    for item in top.iter().take(limit) {
        let _ = writeln!(out, "  {:>3}x  {}  [{}]", item.count, item.display, item.kind);
    }

    let shown_recent = records.len().min(limit);
    let _ = writeln!(out, "\nRecent entries (newest {shown_recent} of {}):", records.len());
    for rec in records.iter().rev().take(limit) {
        let ts = sanitize_single_line(&rec.ts);
        let kind = sanitize_single_line(&rec.kind);
        let platform = sanitize_single_line(&rec.platform);
        let plat = if platform.is_empty() { "-" } else { platform.as_str() };
        let req = sanitize_request(&rec.request);
        let _ = writeln!(out, "  {ts}  [{kind}]  ({plat})  {req}");
        if let Some(ptr) = rec.pointer.as_deref().and_then(sanitize_pointer) {
            let _ = writeln!(out, "    -> {ptr}");
        }
    }
    out.push_str("\nTip: use `huh misses --raw` for JSONL lines, or `huh misses --clear` to delete the log.\n");
    out
}

/// Returns up to the newest `limit` non-empty JSONL lines from `path` (oldest-to-newest).
pub fn render_raw(path: &Path, limit: usize) -> String {
    if limit == 0 {
        return String::new();
    }
    let Ok(bytes) = fs::read(path) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(limit);
    let mut out = String::new();
    for line in &lines[start..] {
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("huh-misses-test-{label}-{}-{seq}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn sample(req: &str, kind: &str, pointer: Option<&str>) -> MissRecord {
        MissRecord {
            ts: "2026-11-26T10:00:00Z".into(),
            request: req.into(),
            kind: kind.into(),
            platform: "arch,omarchy".into(),
            pointer: pointer.map(str::to_string),
        }
    }

    #[test]
    fn path_resolution_uses_xdg_or_home() {
        assert_eq!(log_path(Some("/tmp/state"), Some("/home/u")).unwrap(), Path::new("/tmp/state/huh/misses.jsonl"));
        assert_eq!(log_path(None, Some("/home/u")).unwrap(), Path::new("/home/u/.local/state/huh/misses.jsonl"));
        assert_eq!(log_path(Some(""), Some("/home/u")).unwrap(), Path::new("/home/u/.local/state/huh/misses.jsonl"));
        assert_eq!(log_path(None, None), None);
        assert_eq!(log_path(None, Some("")), None);

        // Smoke-check default_log_path doesn't panic and ends with misses.jsonl when set.
        if let Some(p) = default_log_path() {
            assert!(p.ends_with("huh/misses.jsonl"), "unexpected default path: {}", p.display());
        }
    }

    #[test]
    fn utc_iso_timestamps_and_leap_years() {
        assert_eq!(format_iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_iso_utc(86_400 + 3661), "1970-01-02T01:01:01Z");
        assert_eq!(format_iso_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        // 2024-02-29T23:59:59Z (19782 days * 86400 + 86399)
        assert_eq!(format_iso_utc(19_782 * 86_400 + 86_399), "2024-02-29T23:59:59Z");
        // 2000-02-29T12:34:56Z (11016 days * 86400 + 45296)
        assert_eq!(format_iso_utc(11_016 * 86_400 + 45_296), "2000-02-29T12:34:56Z");

        let now = now_iso_utc();
        assert_eq!(now.len(), 20);
        assert!(now.ends_with('Z'));
        assert_eq!(&now[10..11], "T");
    }

    #[test]
    fn json_escaping_and_round_trip() {
        let dir = TempDir::new("roundtrip");
        let log = dir.path().join("misses.jsonl");

        let rec1 = MissRecord {
            ts: "2026-11-26T09:30:00Z".into(),
            request: "quote \"a\\b\" and emoji 🐧 / path".into(),
            kind: KIND_HINT_ONLY.into(),
            platform: "arch,omarchy".into(),
            pointer: Some("hint: no exact match. Closest: jq (\"json\")\nsecond line ignored".into()),
        };
        let rec2 = sample("order a pizza", KIND_NO_MATCH, None);
        let rec3 = sample("erase my drive", KIND_REFUSAL, Some("refused for safety"));

        record_miss(&log, &rec1);
        record_miss(&log, &rec2);
        record_miss(&log, &rec3);

        let loaded = load_misses(&log);
        assert_eq!(loaded.len(), 3);
        assert_eq!(
            loaded[0],
            MissRecord { pointer: Some("hint: no exact match. Closest: jq (\"json\")".into()), ..rec1 }
        );
        assert_eq!(loaded[1], rec2);
        assert_eq!(loaded[2], rec3);
    }

    #[test]
    fn json_parser_handles_edge_cases_and_malformed_lines() {
        // Reordered keys, whitespace, explicit null pointer, unicode escapes, surrogate pair, unknown nested fields.
        let tricky = r#"  { "extra" : [1, {"nested": "brace } here"}, true, -3.5e+2] , "platform" : "ubuntu" , "pointer" : null , "kind" : "no_match" , "request" : "find \u0041\uD83D\uDC27\/x" , "ts" : "2026-11-26T00:00:00Z" }  "#;
        let parsed = parse_json_line(tricky).unwrap();
        assert_eq!(parsed.ts, "2026-11-26T00:00:00Z");
        assert_eq!(parsed.request, "find A🐧/x");
        assert_eq!(parsed.kind, "no_match");
        assert_eq!(parsed.platform, "ubuntu");
        assert_eq!(parsed.pointer, None);

        // Escapes: \b \f \n \r \t \" \\
        let escapes = r#"{"ts":"2026-11-26T00:00:00Z","request":"a\bb\fc\nd\re\tf\"g\\h","kind":"hint_only","platform":"arch","pointer":"ptr"}"#;
        let parsed_esc = parse_json_line(escapes).unwrap();
        assert_eq!(parsed_esc.request, "a\x08b\x0cc\nd\re\tf\"g\\h");
        assert_eq!(parsed_esc.pointer.as_deref(), Some("ptr"));

        // Malformed lines must return None without panicking.
        for bad in [
            "",
            "   ",
            "[]",
            "{}",
            r#"{"ts":"t","request":"r","kind":"k"}"#, // missing platform
            r#"{"ts":"t","request":"r","kind":"k","platform":"p",}"#, // trailing comma
            r#"{"ts":"t","request":"r","kind":"k","platform":"p"} trailing"#, // trailing junk
            r#"{"ts":123,"request":"r","kind":"k","platform":"p"}"#, // wrong type
            r#"{"ts":"t","request":"unterminated"#,   // truncated
            r#"{"ts":"t","request":"bad\xescape","kind":"k","platform":"p"}"#, // invalid escape
            r#"{"ts":"t","request":"bad\uD800","kind":"k","platform":"p"}"#, // lone high surrogate
            r#"{"ts":"t","request":"bad\uDC00","kind":"k","platform":"p"}"#, // lone low surrogate
            "{\"ts\":\"t\",\"request\":\"raw\nnewline\",\"kind\":\"k\",\"platform\":\"p\"}", // raw control byte
        ] {
            assert!(parse_json_line(bad).is_none(), "expected None for: {bad}");
        }

        // load_misses skips malformed/non-UTF8 lines and preserves valid lines.
        let dir = TempDir::new("malformed-lines");
        let log = dir.path().join("misses.jsonl");
        let mut bytes = Vec::new();
        bytes
            .extend_from_slice(br#"{"ts":"2026-11-26T00:00:01Z","request":"ok1","kind":"no_match","platform":"arch"}"#);
        bytes.extend_from_slice(b"\n{broken json\n\xff\xfe\n");
        bytes.extend_from_slice(
            br#"{"ts":"2026-11-26T00:00:02Z","request":"ok2","kind":"refusal","platform":"arch","pointer":null}"#,
        );
        bytes.push(b'\n');
        fs::write(&log, bytes).unwrap();

        let loaded = load_misses(&log);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].request, "ok1");
        assert_eq!(loaded[1].request, "ok2");
    }

    #[test]
    fn sanitizes_control_chars_newlines_and_truncates_requests() {
        let dir = TempDir::new("sanitize");
        let log = dir.path().join("misses.jsonl");

        let long_prefix = "x".repeat(MAX_REQUEST_CHARS + 50);
        let raw_req = format!("  hello\x1b[31m\n\r\tworld\x00 {long_prefix}  ");
        let rec = MissRecord {
            ts: "2026-11-26T08:00:00Z\n".into(),
            request: raw_req,
            kind: "no_match\r".into(),
            platform: "arch,\x07omarchy".into(),
            pointer: Some("\n\t  first\x1b line  \nsecond line".into()),
        };

        record_miss(&log, &rec);
        let loaded = load_misses(&log);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].ts, "2026-11-26T08:00:00Z");
        assert_eq!(loaded[0].kind, "no_match");
        assert_eq!(loaded[0].platform, "arch,omarchy");
        assert_eq!(loaded[0].pointer.as_deref(), Some("first line"));
        assert!(loaded[0].request.starts_with("hello[31m world "));
        assert_eq!(loaded[0].request.chars().count(), MAX_REQUEST_CHARS);
        assert!(!loaded[0].request.chars().any(char::is_control));

        // Blank pointer normalizes to None.
        record_miss(&log, &sample("another", KIND_NO_MATCH, Some("  \n\t\x00 ")));
        let loaded = load_misses(&log);
        assert_eq!(loaded[1].pointer, None);
    }

    #[test]
    fn opt_in_precedence_and_toml_reader() {
        let dir = TempDir::new("precedence");
        let cfg_true = dir.path().join("true.toml");
        let cfg_false = dir.path().join("false.toml");
        let cfg_commented = dir.path().join("commented.toml");
        let cfg_subtable = dir.path().join("subtable.toml");
        let cfg_multiline = dir.path().join("multiline.toml");
        let cfg_missing = dir.path().join("does-not-exist.toml");

        fs::write(&cfg_true, "theme = \"nord\"\nmiss_log = true # inline comment\n").unwrap();
        fs::write(&cfg_false, "miss_log=false\nmode = \"normal\"\n").unwrap();
        fs::write(&cfg_commented, "# miss_log = true\ntheme = \"dark\"\n").unwrap();
        fs::write(&cfg_subtable, "[other]\nmiss_log = true\n").unwrap();
        fs::write(&cfg_multiline, "note = \"\"\"\nmiss_log = true\n\"\"\"\nitems = [\n  \"a\"\n]\nmiss_log = false\n")
            .unwrap();

        // Default is false.
        assert!(!resolve_enabled(None, None, None));
        assert!(!resolve_enabled(None, Some(&cfg_missing), None));
        assert!(!resolve_enabled(None, Some(&cfg_commented), None));
        assert!(!resolve_enabled(None, Some(&cfg_subtable), None));

        // prefs.miss_log is used when env & config don't set it.
        assert!(resolve_enabled(None, None, Some(true)));
        assert!(resolve_enabled(None, Some(&cfg_commented), Some(true)));
        assert!(resolve_enabled(None, Some(&cfg_subtable), Some(true)));
        assert!(!resolve_enabled(None, None, Some(false)));

        // config.toml beats prefs.miss_log.
        assert!(resolve_enabled(None, Some(&cfg_true), Some(false)));
        assert!(!resolve_enabled(None, Some(&cfg_false), Some(true)));
        assert!(!resolve_enabled(None, Some(&cfg_multiline), Some(true)));

        // HUH_MISS_LOG beats config.toml and prefs.miss_log.
        for truthy in ["1", "true", "TRUE", " on ", "Yes"] {
            assert!(resolve_enabled(Some(truthy), Some(&cfg_false), Some(false)), "{truthy}");
        }
        for falsy in ["0", "false", "FALSE", " off ", "No"] {
            assert!(!resolve_enabled(Some(falsy), Some(&cfg_true), Some(true)), "{falsy}");
        }
        // Empty or unrecognized env falls through to config/prefs.
        assert!(resolve_enabled(Some(""), Some(&cfg_true), Some(false)));
        assert!(!resolve_enabled(Some("   "), Some(&cfg_false), Some(true)));
        assert!(resolve_enabled(Some("maybe"), None, Some(true)));
    }

    #[test]
    fn rotation_caps_at_max_entries_keeping_newest() {
        let dir = TempDir::new("rotate");
        let log = dir.path().join("misses.jsonl");

        // Pre-seed MAX_ENTRIES - 1 lines directly, then record 6 more via record_miss
        // so we cross MAX_ENTRIES cleanly and quickly in a unit test.
        let mut seed = String::new();
        for i in 0..(MAX_ENTRIES - 1) {
            seed.push_str(&to_json_line(&sample(&format!("req-{i:04}"), KIND_NO_MATCH, None)));
            seed.push('\n');
        }
        fs::write(&log, seed).unwrap();

        for i in (MAX_ENTRIES - 1)..(MAX_ENTRIES + 5) {
            record_miss(&log, &sample(&format!("req-{i:04}"), KIND_NO_MATCH, None));
        }

        let loaded = load_misses(&log);
        assert_eq!(loaded.len(), MAX_ENTRIES);
        assert_eq!(loaded.first().unwrap().request, "req-0005");
        assert_eq!(loaded.last().unwrap().request, format!("req-{:04}", MAX_ENTRIES + 4));

        // Ensure no leftover .tmp files in directory.
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftovers.is_empty(), "leftover tmp files: {leftovers:?}");
    }

    #[cfg(unix)]
    #[test]
    fn unix_file_permissions_are_0600_and_io_errors_are_ignored() {
        let dir = TempDir::new("perms");
        let log = dir.path().join("nested/misses.jsonl");

        record_miss(&log, &sample("first", KIND_NO_MATCH, None));
        let mode = fs::metadata(&log).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {:o}", mode);

        // Even if permissions were loosened beforehand, recording tightens them back to 0600.
        fs::set_permissions(&log, fs::Permissions::from_mode(0o644)).unwrap();
        record_miss(&log, &sample("second", KIND_NO_MATCH, None));
        let mode_after = fs::metadata(&log).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode_after, 0o600, "expected 0600 after append, got {:o}", mode_after);

        // Unwritable path (parent is an existing regular file) silently does nothing.
        let blocker = dir.path().join("not-a-dir");
        fs::write(&blocker, b"x").unwrap();
        record_miss(&blocker.join("misses.jsonl"), &sample("ignored", KIND_NO_MATCH, None));
    }

    #[test]
    fn summary_aggregation_raw_and_clear() {
        let dir = TempDir::new("summary");
        let log = dir.path().join("misses.jsonl");

        // Empty state.
        let empty_summary = render_summary(&[], false, &log, 20);
        assert!(empty_summary.contains("disabled"));
        assert!(empty_summary.contains("--enable"));
        assert!(empty_summary.contains(&log.display().to_string()));
        assert!(empty_summary.contains("No missed requests recorded yet."));
        assert_eq!(render_raw(&log, 10), "");
        assert!(!clear_misses(&log).unwrap());

        record_miss(&log, &sample("pretty print json", KIND_HINT_ONLY, Some("hint: Closest: jq")));
        record_miss(&log, &sample("order a pizza", KIND_NO_MATCH, None));
        record_miss(&log, &sample("Pretty Print JSON", KIND_HINT_ONLY, Some("hint: Closest: jq")));
        record_miss(&log, &sample("format my ssd", KIND_REFUSAL, Some("refused")));

        let loaded = load_misses(&log);
        assert_eq!(loaded.len(), 4);

        let summary = render_summary(&loaded, true, &log, 2);
        assert!(summary.contains("enabled"));
        assert!(summary.contains("--disable"));
        assert!(summary.contains("Entries:      4"));
        assert!(summary.contains("2x  Pretty Print JSON"));
        // Top 2 limited to 2 items; recent limited to 2 newest entries (format my ssd + Pretty Print JSON).
        assert!(summary.contains("format my ssd"));
        assert!(summary.contains("-> refused"));
        assert!(!summary.contains("order a pizza"));

        let raw_2 = render_raw(&log, 2);
        let raw_lines: Vec<&str> = raw_2.lines().collect();
        assert_eq!(raw_lines.len(), 2);
        assert_eq!(parse_json_line(raw_lines[0]).unwrap().request, "Pretty Print JSON");
        assert_eq!(parse_json_line(raw_lines[1]).unwrap().request, "format my ssd");

        assert!(clear_misses(&log).unwrap());
        assert!(!log.exists());
        assert!(!clear_misses(&log).unwrap());
        assert!(load_misses(&log).is_empty());
    }
}
