//! Turns an `Explanation` into text that is readable in an 80-column terminal.

use crate::breakdown::{Block, CmdBlock, ConnKind, Explanation, Row};
use crate::style::{Role, paint};
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

/// Wraps `text` after `first` (a lead such as `"1. "`, painted `lead_role`) with a hanging indent;
/// each wrapped line of the text is painted `role`. Widths are measured on plain text.
pub(crate) fn hanging_as(out: &mut String, indent: usize, first: (&str, Role), text: (&str, Role)) {
    let lead = first.0.chars().count();
    let lines = wrap(text.0, WIDTH.saturating_sub(indent + lead));
    for (n, line) in lines.iter().enumerate() {
        if n == 0 {
            let _ = writeln!(out, "{}{}{}", " ".repeat(indent), paint(first.1, first.0), paint(text.1, line));
        } else {
            let _ = writeln!(out, "{}{}{}", " ".repeat(indent), " ".repeat(lead), paint(text.1, line));
        }
    }
}

pub(crate) fn hanging(out: &mut String, indent: usize, first: &str, text: &str) {
    hanging_as(out, indent, (first, Role::Dim), (text, Role::Description));
}

/// Like `hanging`, with `tag` (the start of `text`, such as `[destroys data]`) painted `Role::Risk`.
pub(crate) fn hanging_tagged(out: &mut String, indent: usize, first: (&str, Role), tag: &str, text: &str) {
    let lead = first.0.chars().count();
    for (n, line) in wrap(text, WIDTH.saturating_sub(indent + lead)).iter().enumerate() {
        let pad = if n == 0 { paint(first.1, first.0) } else { " ".repeat(lead) };
        let body = match line.strip_prefix(tag) {
            Some(rest) if n == 0 => format!("{}{}", paint(Role::Risk, tag), paint(Role::Description, rest)),
            _ => paint(Role::Description, line),
        };
        let _ = writeln!(out, "{}{pad}{body}", " ".repeat(indent));
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

pub(crate) fn rows(out: &mut String, rows: &[Row], indent: usize) {
    if rows.is_empty() {
        return;
    }
    let longest = rows.iter().map(|r| r.piece.chars().count()).max().unwrap_or(0);
    let col = longest.clamp(6, 22);
    for r in rows {
        let pl = r.piece.chars().count();
        if pl > col {
            let _ = writeln!(out, "{}{}", " ".repeat(indent), paint(Role::Part, &r.piece));
            let pad = " ".repeat(col + 2);
            hanging(out, indent, &pad, &r.meaning);
        } else {
            let first = format!("{:<width$}  ", r.piece, width = col);
            hanging_as(out, indent, (&first, Role::Part), (&r.meaning, Role::Description));
        }
    }
}

fn cmd_block(out: &mut String, c: &CmdBlock, indent: usize) {
    rows(out, &c.rows, indent);
    for inner in &c.inner {
        hanging_as(out, indent, ("> ", Role::Dim), (&inner.label, Role::Note));
        blocks(out, &inner.blocks, indent + 4, false);
    }
}

fn blocks(out: &mut String, list: &[Block], indent: usize, top: bool) {
    let mut n = 0;
    for b in list {
        match b {
            Block::Connector(kind) => {
                let at = if top { indent + 3 } else { indent };
                hanging_as(out, at, ("", Role::Dim), (conn_line(*kind), Role::Dim));
            }
            Block::Cmd(c) => {
                n += 1;
                if top {
                    let first = format!("{n}. ");
                    hanging_as(out, indent, (&first, Role::Dim), (&flat(&c.raw, 200), Role::Command));
                    cmd_block(out, c, indent + 3);
                } else {
                    hanging_as(out, indent, ("$ ", Role::Dim), (&flat(&c.raw, 200), Role::Command));
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
    let _ = writeln!(out, "{}", paint(Role::Header, &what));
    for (i, s) in e.steps.iter().enumerate() {
        let conn = s.conn.map(conn_step).unwrap_or("");
        let first = format!("{}. ", i + 1);
        hanging(&mut out, 2, &first, &format!("{conn}{}", s.text));
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", paint(Role::Header, "Part by part:"));
    blocks(&mut out, &e.blocks, 0, true);
    if !e.warnings.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "{}", paint(Role::Warning, "WARNINGS"));
        for w in &e.warnings {
            let part = flat(&w.part, 70);
            let head = if w.context.is_empty() || w.context == w.part {
                format!("`{part}`: ")
            } else {
                format!("`{part}` (in `{}`): ", flat(&w.context, 50))
            };
            let tag = format!("[{}]", w.risk.name());
            hanging_tagged(&mut out, 2, ("! ", Role::Warning), &tag, &format!("{tag} {head}{}", w.text));
        }
    }
    if !e.notes.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "{}", paint(Role::Header, "NOTES"));
        for note in &e.notes {
            hanging_as(&mut out, 2, ("- ", Role::Dim), (&capitalize(note), Role::Note));
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", paint(Role::Dim, "(huh only read this text and its own notes. Nothing was run.)"));
    out
}

fn capitalize(text: &str) -> String {
    let mut c = text.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
