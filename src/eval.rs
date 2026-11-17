//! `huh eval`: run test requests through the matcher and score the results.

use crate::matcher::LocalMatcher;
use crate::suggest;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
struct CaseFile {
    case: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    request: String,
    /// Expected command name, alternatives as "a|b", or "none" when nothing should be suggested.
    command: String,
    /// Expected full command line of the first suggestion (optional).
    expect: Option<String>,
    /// Pretend these are the entries of the current folder.
    #[serde(default)]
    files: Vec<String>,
}

#[derive(Default)]
pub struct Tally {
    pub hit: usize,
    pub total: usize,
}

impl Tally {
    fn add(&mut self, hit: bool) {
        self.total += 1;
        self.hit += hit as usize;
    }

    /// Whole percent of hits; 100 when there were no cases.
    #[cfg(test)]
    pub fn percent(&self) -> usize {
        (100 * self.hit).checked_div(self.total).unwrap_or(100)
    }
}

impl std::fmt::Display for Tally {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.total == 0 {
            return write!(f, "no cases");
        }
        write!(f, "{}/{} ({:.0}%)", self.hit, self.total, 100.0 * self.hit as f64 / self.total as f64)
    }
}

/// The scores of one cases file, with one "ok"/"MISS" line per case.
#[derive(Default)]
pub struct Report {
    pub top1: Tally,
    pub top3: Tally,
    pub exact: Tally,
    pub rejected: Tally,
    pub millis: Vec<u128>,
    pub lines: Vec<String>,
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "right command first:        {}", self.top1)?;
        writeln!(f, "right command in top 3:     {}", self.top3)?;
        writeln!(f, "exact command line:         {}", self.exact)?;
        write!(f, "\"nothing fits\" recognised:  {}", self.rejected)?;
        let mut millis = self.millis.clone();
        millis.sort();
        if let (Some(median), Some(slowest)) = (millis.get(millis.len() / 2), millis.last()) {
            write!(f, "\ntime per request:           median {median} ms, slowest {slowest} ms")?;
        }
        Ok(())
    }
}

/// `huh eval`: prints every case, then the scores.
pub fn run(matcher: &LocalMatcher, path: &Path) -> Result<()> {
    let report = score(matcher, path)?;
    for line in &report.lines {
        println!("{line}");
    }
    println!("\n{report}");
    Ok(())
}

pub fn score(matcher: &LocalMatcher, path: &Path) -> Result<Report> {
    let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let cases = toml::from_str::<CaseFile>(&text).with_context(|| format!("invalid {}", path.display()))?.case;

    let mut report = Report::default();
    let Report { top1, top3, exact, rejected, .. } = &mut report;
    let mut lines = Vec::new();
    let mut millis = Vec::new();

    for case in &cases {
        let outcome = suggest(matcher, &case.request, case.files.clone(), &mut |_| None)?;
        millis.push(outcome.millis);
        let first = outcome.suggestions.first();
        let got = first.map_or("(no suggestion)", |s| s.line.as_str());

        let ok = if case.command == "none" {
            let ok = outcome.suggestions.is_empty();
            rejected.add(ok);
            ok
        } else {
            // "top|ps" means either command is a right answer.
            let accepted: Vec<&str> = case.command.split('|').collect();
            let command_ok = first.is_some_and(|s| accepted.contains(&s.command.as_str()));
            top1.add(command_ok);
            top3.add(outcome.ranked.iter().any(|(name, _)| accepted.contains(&name.as_str())));
            match &case.expect {
                Some(expect) => {
                    let line_ok = first.is_some_and(|s| s.line == *expect);
                    exact.add(line_ok);
                    line_ok
                }
                None => command_ok,
            }
        };
        let wanted = case.expect.as_deref().unwrap_or(&case.command);
        lines.push(if ok {
            format!("ok    {}  ->  {got}", case.request)
        } else {
            format!("MISS  {}  ->  {got}   (wanted: {wanted})", case.request)
        });
    }
    report.lines = lines;
    report.millis = millis;
    Ok(report)
}

#[derive(Deserialize)]
struct ErrorFile {
    case: Vec<ErrorCase>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorCase {
    command: String,
    exit: i32,
    output: String,
    /// Words the explanation must contain; "a|b" accepts either.
    mention: Vec<String>,
    /// Text the `Try:` command must contain; "a|b" accepts either. "nothing" expects no command.
    fix: String,
}

fn contains_any(text: &str, alternatives: &str) -> bool {
    let text = text.to_lowercase();
    alternatives.split('|').any(|word| text.contains(&word.to_lowercase()))
}

/// `huh eval-errors`: a first-pass score only. The answers are printed in
/// full because whether an explanation is right has to be read, not matched.
pub fn run_errors(path: &Path) -> Result<()> {
    use crate::explain;
    let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let cases = toml::from_str::<ErrorFile>(&text).with_context(|| format!("invalid {}", path.display()))?.case;

    let (mut cause, mut fix) = (Tally::default(), Tally::default());
    let mut total = Vec::new();
    for case in &cases {
        let last = crate::session::Last { command: case.command.clone(), exit: Some(case.exit), output: case.output.clone() };
        let answer = explain::explain(&last, &mut |_| {})?;
        let cause_ok = case.mention.iter().all(|words| contains_any(&answer.text, words));
        let suggested = explain::suggested(&answer.text);
        let fix_ok = match (case.fix.as_str(), suggested) {
            (wanted, Some(command)) => contains_any(command, wanted),
            (wanted, None) => wanted.split('|').any(|alternative| alternative == "nothing"),
        };
        cause.add(cause_ok);
        fix.add(fix_ok);
        total.push(answer.total.as_millis());
        println!("=== $ {}   (exit {})", case.command, case.exit);
        println!("{}", answer.text.trim());
        println!(
            "--- cause {}, fix {} (wanted: {}), {} ms\n",
            if cause_ok { "ok" } else { "MISS" },
            if fix_ok { "ok" } else { "MISS" },
            case.fix,
            answer.total.as_millis()
        );
    }
    total.sort();
    println!("explanation names the cause: {cause}");
    println!("suggested command fits:      {fix}");
    if !total.is_empty() {
        println!("time per answer:             median {} ms, slowest {} ms", total[total.len() / 2], total[total.len() - 1]);
    }
    Ok(())
}
