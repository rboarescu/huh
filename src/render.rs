//! Turns an `Explanation` into text that is readable in an 80-column terminal.

use crate::breakdown::{Block, CmdBlock, ConnKind, Explanation, Row};
use std::fmt::Write;

pub const WIDTH: usize = 80;

fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(20);
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let wl = word.chars().count();
        let cl = cur.chars().count();
        if cl > 0 && cl + 1 + wl > width {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn flat(text: &str, max: usize) -> String {
    let one: String = text.split('\n').map(str::trim).collect::<Vec<_>>().join(" / ");
    if one.chars().count() <= max {
        one
    } else {
        let cut: String = one.chars().take(max.saturating_sub(3)).collect();
        format!("{cut}...")
    }
}

fn hanging(out: &mut String, indent: usize, first: &str, text: &str) {
    let lead = first.chars().count();
    let lines = wrap(text, WIDTH.saturating_sub(indent + lead));
    for (n, line) in lines.iter().enumerate() {
        if n == 0 {
            let _ = writeln!(out, "{}{first}{line}", " ".repeat(indent));
        } else {
            let _ = writeln!(out, "{}{}{line}", " ".repeat(indent), " ".repeat(lead));
        }
    }
}

fn conn_step(kind: ConnKind) -> &'static str {
    match kind {
        ConnKind::Pipe => "takes that output as its input: ",
        ConnKind::PipeErr => "takes that output and error messages as its input: ",
        ConnKind::And => "only if that worked: ",
        ConnKind::Or => "only if that failed: ",
        ConnKind::Seq => "then: ",
        ConnKind::Background => "meanwhile: ",
    }
}

fn conn_line(kind: ConnKind) -> &'static str {
    match kind {
        ConnKind::Pipe => "| the output of the command above becomes the input of the next one",
        ConnKind::PipeErr => "|& the output and error messages above become the input of the next command",
        ConnKind::And => "&& the next command runs only if the one above succeeded",
        ConnKind::Or => "|| the next command runs only if the one above failed",
        ConnKind::Seq => ";  the next command runs after the one above has finished",
        ConnKind::Background => {
            "&  the command above was started in the background; the shell moved on without waiting"
        }
    }
}

fn rows(out: &mut String, rows: &[Row], indent: usize) {
    if rows.is_empty() {
        return;
    }
    let longest = rows.iter().map(|r| r.piece.chars().count()).max().unwrap_or(0);
    let col = longest.clamp(6, 22);
    for r in rows {
        let pl = r.piece.chars().count();
        if pl > col {
            let _ = writeln!(out, "{}{}", " ".repeat(indent), r.piece);
            let pad = " ".repeat(col + 2);
            hanging(out, indent, &pad, &r.meaning);
        } else {
            let first = format!("{:<width$}  ", r.piece, width = col);
            hanging(out, indent, &first, &r.meaning);
        }
    }
}

fn cmd_block(out: &mut String, c: &CmdBlock, indent: usize) {
    rows(out, &c.rows, indent);
    for inner in &c.inner {
        hanging(out, indent, "> ", &inner.label);
        blocks(out, &inner.blocks, indent + 4, false);
    }
}

fn blocks(out: &mut String, list: &[Block], indent: usize, top: bool) {
    let mut n = 0;
    for b in list {
        match b {
            Block::Connector(kind) => {
                if top {
                    hanging(out, indent + 3, "", conn_line(*kind));
                } else {
                    hanging(out, indent, "", conn_line(*kind));
                }
            }
            Block::Cmd(c) => {
                n += 1;
                if top {
                    let first = format!("{n}. ");
                    hanging(out, indent, &first, &flat(&c.raw, 200));
                    cmd_block(out, c, indent + 3);
                } else {
                    hanging(out, indent, "$ ", &flat(&c.raw, 200));
                    cmd_block(out, c, indent + 2);
                }
            }
        }
    }
}

/// Renders the explanation for a terminal.
pub fn render(e: &Explanation) -> String {
    let mut out = String::new();
    let n = e.steps.len();
    if n == 0 {
        return "There is nothing to explain in that text.\n".to_string();
    }
    let noun = if e.multiline { "script" } else { "line" };
    let what = if n == 1 {
        format!("This {noun} does 1 thing:")
    } else {
        format!("This {noun} does {n} things, in this order:")
    };
    let _ = writeln!(out, "{what}");
    for (i, s) in e.steps.iter().enumerate() {
        let conn = s.conn.map(conn_step).unwrap_or("");
        let first = format!("{}. ", i + 1);
        hanging(&mut out, 2, &first, &format!("{conn}{}", s.text));
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "Part by part:");
    blocks(&mut out, &e.blocks, 0, true);
    if !e.warnings.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "WARNINGS");
        for w in &e.warnings {
            let part = flat(&w.part, 70);
            let head = if w.context.is_empty() || w.context == w.part {
                format!("`{part}`: ")
            } else {
                format!("`{part}` (in `{}`): ", flat(&w.context, 50))
            };
            hanging(&mut out, 2, "! ", &format!("[{}] {head}{}", w.risk.name(), w.text));
        }
    }
    if !e.notes.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "NOTES");
        for note in &e.notes {
            hanging(&mut out, 2, "- ", &capitalize(note));
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "(huh only read this text and its own notes. Nothing was run.)");
    out
}

fn capitalize(text: &str) -> String {
    let mut c = text.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
