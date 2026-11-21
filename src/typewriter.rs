//! ANSI-safe typewriter streaming with bounded total duration and instant flush on input.
//!
//! Splits terminal output into indivisible ANSI escape sequences (`Token::Escape`) and visible
//! scalar characters (`Token::Char`), paces character output into a fixed total millisecond budget,
//! and can dump any remaining tail in a single unbroken write when interrupted.

use anyhow::{Result, bail};
use std::io::Write;
use std::time::Duration;

pub const VALID: &str = "off, fast, normal, slow, or milliseconds (0-5000)";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Speed {
    Off,
    Fast,
    #[default]
    Normal,
    Slow,
    Custom(u64),
}

impl Speed {
    pub const VALID: &'static str = VALID;

    pub fn parse(value: &str) -> Result<Speed> {
        let norm = value.trim().to_ascii_lowercase();
        match norm.as_str() {
            "off" => Ok(Speed::Off),
            "fast" => Ok(Speed::Fast),
            "normal" => Ok(Speed::Normal),
            "slow" => Ok(Speed::Slow),
            other => {
                let num = other.strip_suffix("ms").unwrap_or(other).trim();
                if let Ok(ms) = num.parse::<u64>()
                    && ms <= 5000
                {
                    return Ok(if ms == 0 { Speed::Off } else { Speed::Custom(ms) });
                }
                bail!("unknown typewriter speed \"{other}\"; valid speeds: {VALID}");
            }
        }
    }

    pub fn name(&self) -> String {
        match self {
            Speed::Off => "off".to_string(),
            Speed::Fast => "fast".to_string(),
            Speed::Normal => "normal".to_string(),
            Speed::Slow => "slow".to_string(),
            Speed::Custom(ms) => (*ms).min(5000).to_string(),
        }
    }

    pub fn budget_ms(&self) -> u64 {
        match self {
            Speed::Off => 0,
            Speed::Fast => 800,
            Speed::Normal => 2000,
            Speed::Slow => 3200,
            Speed::Custom(ms) => (*ms).min(5000),
        }
    }
}

/// Resolves typewriter speed: `env_no_tw` > `HUH_TYPEWRITER_MS` > `config.toml` > `prefs.toml` > `Normal`.
pub fn resolve_speed(
    env_no_tw: bool,
    env_ms: Option<&str>,
    config_text: Option<&str>,
    pref_speed: Option<&str>,
) -> Speed {
    if env_no_tw {
        return Speed::Off;
    }
    if let Some(raw) = env_ms.map(str::trim).filter(|s| !s.is_empty()) {
        if let Ok(speed) = Speed::parse(raw) {
            return speed;
        }
        if let Ok(ms) = raw.parse::<u64>() {
            return if ms == 0 { Speed::Off } else { Speed::Custom(ms.min(5000)) };
        }
    }
    if let Some(text) = config_text
        && let Ok(table) = toml::from_str::<toml::Table>(text)
    {
        if let Some(val) = table.get("typewriter") {
            match val {
                toml::Value::Boolean(false) => return Speed::Off,
                toml::Value::String(s) => {
                    if let Ok(speed) = Speed::parse(s) {
                        return speed;
                    }
                }
                _ => {}
            }
        }
        if let Some(ms) =
            table.get("typewriter_ms").and_then(toml::Value::as_integer).and_then(|n| u64::try_from(n).ok())
        {
            return if ms == 0 { Speed::Off } else { Speed::Custom(ms.min(5000)) };
        }
    }
    if let Some(raw) = pref_speed.map(str::trim).filter(|s| !s.is_empty())
        && let Ok(speed) = Speed::parse(raw)
    {
        return speed;
    }
    Speed::Normal
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token<'a> {
    Escape(&'a str),
    Char(char),
}

impl Token<'_> {
    fn byte_len(&self) -> usize {
        match self {
            Token::Escape(seq) => seq.len(),
            Token::Char(ch) => ch.len_utf8(),
        }
    }
}

fn scan_escape_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    if start + 1 >= bytes.len() {
        return bytes.len();
    }
    match bytes[start + 1] {
        b'[' => {
            let mut j = start + 2;
            while j < bytes.len() {
                let b = bytes[j];
                if (0x40..=0x7E).contains(&b) {
                    return j + 1;
                }
                if !(0x20..=0x3F).contains(&b) {
                    return j;
                }
                j += 1;
            }
            bytes.len()
        }
        b']' => {
            let mut j = start + 2;
            while j < bytes.len() {
                if bytes[j] == 0x07 {
                    return j + 1;
                }
                if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                    return j + 2;
                }
                j += 1;
            }
            bytes.len()
        }
        b'_' | b'P' | b'X' | b'^' => {
            let is_tmux = text[start..].starts_with("\x1bPtmux;");
            let mut j = if is_tmux { start + "\x1bPtmux;".len() } else { start + 2 };
            while j < bytes.len() {
                if is_tmux && bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == 0x1b {
                    j += 2;
                    continue;
                }
                if bytes[j] == 0x1b && j + 1 < bytes.len() && bytes[j + 1] == b'\\' {
                    return j + 2;
                }
                j += 1;
            }
            bytes.len()
        }
        _ => {
            let next_len = text[start + 1..].chars().next().map_or(0, char::len_utf8);
            start + 1 + next_len
        }
    }
}

/// Splits `text` into indivisible ANSI escape sequences and individual UTF-8 scalar characters.
pub fn tokenize(text: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if text.as_bytes()[i] == 0x1b {
            let end = scan_escape_end(text, i);
            tokens.push(Token::Escape(&text[i..end]));
            i = end;
        } else if let Some(ch) = text[i..].chars().next() {
            tokens.push(Token::Char(ch));
            i += ch.len_utf8();
        } else {
            break;
        }
    }
    tokens
}

/// Computes `(chars_per_step, step_delay)` so short outputs feel crisp and long outputs fit inside `budget_ms`.
pub fn pacing(visible_chars: usize, budget_ms: u64) -> (usize, Duration) {
    if budget_ms == 0 || visible_chars == 0 {
        return (visible_chars, Duration::ZERO);
    }
    let steps = (budget_ms / 16).max(1) as usize;
    if visible_chars <= steps {
        let delay_ms = (budget_ms / visible_chars as u64).clamp(16, 35);
        (1, Duration::from_millis(delay_ms))
    } else {
        let chars_per_step = visible_chars.div_ceil(steps).max(1);
        (chars_per_step, Duration::from_millis(16))
    }
}

/// Streams `text` to `out` under `budget_ms`, checking `should_flush(delay)` between paced steps.
/// When `should_flush` returns `true`, writes the remaining tail immediately with zero split escapes.
pub fn write_animated<W: Write>(
    out: &mut W,
    text: &str,
    budget_ms: u64,
    should_flush: &mut dyn FnMut(Duration) -> bool,
) -> std::io::Result<()> {
    if budget_ms == 0 || text.is_empty() {
        out.write_all(text.as_bytes())?;
        return out.flush();
    }

    let tokens = tokenize(text);
    let visible_chars = tokens.iter().filter(|t| matches!(t, Token::Char(_))).count();
    let (chars_per_step, delay) = pacing(visible_chars, budget_ms);
    if delay.is_zero() || chars_per_step >= visible_chars {
        out.write_all(text.as_bytes())?;
        return out.flush();
    }

    let mut step_start = 0usize;
    let mut bytes_written = 0usize;
    let mut chars_written = 0usize;
    let mut step_chars = 0usize;

    for token in tokens {
        bytes_written += token.byte_len();
        if matches!(token, Token::Char(_)) {
            chars_written += 1;
            step_chars += 1;
            if step_chars >= chars_per_step && chars_written < visible_chars {
                out.write_all(&text.as_bytes()[step_start..bytes_written])?;
                out.flush()?;
                step_start = bytes_written;
                step_chars = 0;
                if should_flush(delay) {
                    if step_start < text.len() {
                        out.write_all(&text.as_bytes()[step_start..])?;
                    }
                    return out.flush();
                }
            }
        }
    }

    if step_start < text.len() {
        out.write_all(&text.as_bytes()[step_start..])?;
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct RecordingWriter {
        chunks: Vec<Vec<u8>>,
        flushes: usize,
    }

    impl RecordingWriter {
        fn into_bytes(self) -> Vec<u8> {
            self.chunks.into_iter().flatten().collect()
        }
    }

    impl Write for RecordingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.chunks.push(buf.to_vec());
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    #[test]
    fn speed_parsing_names_and_budgets() {
        assert_eq!(Speed::Off.budget_ms(), 0);
        assert_eq!(Speed::Fast.budget_ms(), 800);
        assert_eq!(Speed::Normal.budget_ms(), 2000);
        assert_eq!(Speed::Slow.budget_ms(), 3200);
        assert_eq!(Speed::Custom(1500).budget_ms(), 1500);
        assert_eq!(Speed::Custom(9000).budget_ms(), 5000);

        assert_eq!(Speed::parse("OFF").unwrap(), Speed::Off);
        assert_eq!(Speed::parse("fast").unwrap(), Speed::Fast);
        assert_eq!(Speed::parse("normal").unwrap(), Speed::Normal);
        assert_eq!(Speed::parse("Slow").unwrap(), Speed::Slow);
        assert_eq!(Speed::parse("0").unwrap(), Speed::Off);
        assert_eq!(Speed::parse("1250").unwrap(), Speed::Custom(1250));
        assert_eq!(Speed::parse("1250ms").unwrap(), Speed::Custom(1250));
        assert_eq!(Speed::parse("5000").unwrap(), Speed::Custom(5000));

        assert_eq!(Speed::Off.name(), "off");
        assert_eq!(Speed::Fast.name(), "fast");
        assert_eq!(Speed::Normal.name(), "normal");
        assert_eq!(Speed::Slow.name(), "slow");
        assert_eq!(Speed::Custom(1250).name(), "1250");
        assert_eq!(Speed::Custom(9000).name(), "5000");

        for s in [Speed::Off, Speed::Fast, Speed::Normal, Speed::Slow, Speed::Custom(1250)] {
            assert_eq!(Speed::parse(&s.name()).unwrap(), s);
        }

        let err = Speed::parse("5001").unwrap_err().to_string();
        assert!(err.contains(VALID), "{err}");
        let err = Speed::parse("warp").unwrap_err().to_string();
        assert!(err.contains(VALID), "{err}");
    }

    #[test]
    fn speed_resolution_precedence() {
        assert_eq!(resolve_speed(true, Some("1000"), Some("typewriter_ms = 500\n"), Some("slow")), Speed::Off);
        assert_eq!(resolve_speed(false, Some("900"), Some("typewriter = false\n"), Some("slow")), Speed::Custom(900));
        assert_eq!(resolve_speed(false, Some("0"), Some("typewriter_ms = 500\n"), Some("slow")), Speed::Off);
        assert_eq!(resolve_speed(false, Some("9000"), None, None), Speed::Custom(5000));
        assert_eq!(
            resolve_speed(false, Some(" "), Some("typewriter = false\ntypewriter_ms = 700\n"), Some("fast")),
            Speed::Off
        );
        assert_eq!(resolve_speed(false, None, Some("typewriter_ms = 1400\n"), Some("fast")), Speed::Custom(1400));
        assert_eq!(resolve_speed(false, None, Some("typewriter = true\n"), Some("fast")), Speed::Fast);
        assert_eq!(resolve_speed(false, None, Some("theme = \"nord\"\n"), Some("slow")), Speed::Slow);
        assert_eq!(resolve_speed(false, None, None, None), Speed::Normal);
    }

    #[test]
    fn pacing_scales_for_short_and_long_outputs() {
        assert_eq!(pacing(0, 2000), (0, Duration::ZERO));
        assert_eq!(pacing(100, 0), (100, Duration::ZERO));

        // 5 chars: 1 char per step, clamped to 35ms (~175ms total, not 2s).
        assert_eq!(pacing(5, 2000), (1, Duration::from_millis(35)));
        // 60 chars: 2000 / 60 = 33ms per char.
        assert_eq!(pacing(60, 2000), (1, Duration::from_millis(33)));
        // 300 chars: 125 steps @ 16ms -> 3 chars/step.
        assert_eq!(pacing(300, 2000), (3, Duration::from_millis(16)));
        // 3000 chars: 125 steps @ 16ms -> 24 chars/step.
        assert_eq!(pacing(3000, 2000), (24, Duration::from_millis(16)));

        for n in [5, 60, 300, 3000] {
            let (cps, delay) = pacing(n, 2000);
            let steps = n.div_ceil(cps);
            let total_ms = steps as u64 * delay.as_millis() as u64;
            assert!(total_ms <= 2100, "n={n}: total_ms={total_ms}");
        }
    }

    #[test]
    fn tokenizer_never_splits_ansi_escapes_or_utf8() {
        let osc_bel = "\x1b]8;;https://example.com\x07link\x1b]8;;\x07";
        let osc_st = "\x1b]0;title — Å\x1b\\";
        let apc = "\x1b_Ga=T,f=32;AAAA\x1b\\";
        let dcs_tmux = "\x1bPtmux;\x1b\x1b_Ga=T;\x1b\x1b\\\x1b\\";
        let two_char = "\x1b7\x1b8";
        let input = format!("\x1b[1;38;2;137;207;240mHi\x1b[0m {osc_bel}{osc_st}{apc}{dcs_tmux}{two_char}🦀é");
        let tokens = tokenize(&input);

        let rebuilt: String = tokens
            .iter()
            .map(|t| match t {
                Token::Escape(s) => (*s).to_string(),
                Token::Char(c) => c.to_string(),
            })
            .collect();
        assert_eq!(rebuilt, input);

        assert_eq!(tokens[0], Token::Escape("\x1b[1;38;2;137;207;240m"));
        assert_eq!(tokens[1], Token::Char('H'));
        assert_eq!(tokens[2], Token::Char('i'));
        assert_eq!(tokens[3], Token::Escape("\x1b[0m"));
        assert_eq!(tokens[4], Token::Char(' '));
        assert_eq!(tokens[5], Token::Escape("\x1b]8;;https://example.com\x07"));
        assert!(tokens.contains(&Token::Escape(osc_st)));
        assert!(tokens.contains(&Token::Escape(apc)));
        assert!(tokens.contains(&Token::Escape(dcs_tmux)));
        assert!(tokens.contains(&Token::Escape("\x1b7")));
        assert!(tokens.contains(&Token::Escape("\x1b8")));
        assert!(tokens.contains(&Token::Char('🦀')));
        assert!(tokens.contains(&Token::Char('é')));
    }

    #[test]
    fn write_animated_preserves_escapes_utf8_and_early_flush() {
        let sample = "\x1b[1;36mls\x1b[0m — list files \x1b]8;;https://huh.local\x07docs\x1b]8;;\x07 · café 🚀\n";

        let mut full = RecordingWriter::default();
        let delays = RefCell::new(Vec::new());
        write_animated(&mut full, sample, 2000, &mut |d| {
            delays.borrow_mut().push(d);
            false
        })
        .unwrap();
        assert!(!delays.borrow().is_empty());
        assert!(full.flushes > 1);
        for chunk in &full.chunks {
            let s = std::str::from_utf8(chunk).expect("every streamed chunk must be valid UTF-8");
            assert_eq!(s.matches('\x1b').count(), tokenize(s).iter().filter(|t| matches!(t, Token::Escape(_))).count());
        }
        assert_eq!(String::from_utf8(full.into_bytes()).unwrap(), sample);

        // Early flush on step 2 must produce byte-identical output and write the remaining tail in one shot.
        let mut early = RecordingWriter::default();
        let mut calls = 0usize;
        write_animated(&mut early, sample, 2000, &mut |_| {
            calls += 1;
            calls == 2
        })
        .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(early.chunks.len(), 3);
        for chunk in &early.chunks {
            std::str::from_utf8(chunk).expect("early flush chunks must remain valid UTF-8");
        }
        assert_eq!(String::from_utf8(early.into_bytes()).unwrap(), sample);

        // Zero budget writes immediately in one chunk.
        let mut instant = RecordingWriter::default();
        let mut called = false;
        write_animated(&mut instant, sample, 0, &mut |_| {
            called = true;
            false
        })
        .unwrap();
        assert!(!called);
        assert_eq!(instant.chunks.len(), 1);
        assert_eq!(String::from_utf8(instant.into_bytes()).unwrap(), sample);
    }
}
