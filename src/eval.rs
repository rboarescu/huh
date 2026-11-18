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
    /// Whether the first suggestion must carry the WARNING line (`true`) or must not (`false`).
    warn: Option<bool>,
    /// Pretend these are the entries of the current folder.
    #[serde(default)]
    files: Vec<String>,
    /// Package manager family ("debian", "alpine|void", ...) this case is about. Such a case runs
    /// only when scoring as that family (`huh eval --platform`); other cases run everywhere.
    platform: Option<String>,
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
pub fn run(matcher: &LocalMatcher, path: &Path, platforms: &[String]) -> Result<()> {
    let report = score_for(matcher, path, platforms)?;
    for line in &report.lines {
        println!("{line}");
    }
    println!("\n{report}");
    Ok(())
}

/// Scores as a system with these platform words (see platform.rs): package names are filled in
/// for it and cases tagged with its package manager family are included.
pub fn score_for(matcher: &LocalMatcher, path: &Path, platforms: &[String]) -> Result<Report> {
    crate::platform::use_platform_for_eval(platforms);
    let report = score_cases(matcher, path, platforms);
    crate::platform::use_platform_for_eval(&[]);
    report
}

fn score_cases(matcher: &LocalMatcher, path: &Path, platforms: &[String]) -> Result<Report> {
    let family = crate::platform::manager(platforms);
    let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let cases = toml::from_str::<CaseFile>(&text).with_context(|| format!("invalid {}", path.display()))?.case;

    let mut report = Report::default();
    let Report { top1, top3, exact, rejected, .. } = &mut report;
    let mut lines = Vec::new();
    let mut millis = Vec::new();

    for case in &cases {
        if let Some(wanted) = &case.platform
            && !family.as_ref().is_some_and(|family| wanted.split('|').any(|w| w == family))
        {
            continue;
        }
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
            let warn_ok = case.warn.is_none_or(|wanted| first.is_some_and(|s| s.destructive == wanted));
            match &case.expect {
                Some(expect) => {
                    let line_ok = first.is_some_and(|s| s.line == *expect);
                    exact.add(line_ok);
                    line_ok && warn_ok
                }
                None => command_ok && warn_ok,
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
    use crate::doctor;
    let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let cases = toml::from_str::<ErrorFile>(&text).with_context(|| format!("invalid {}", path.display()))?.case;

    // The recorded failures come from an Arch machine that lacked the program: score them
    // the same way on every machine.
    crate::platform::pin_for_eval(&["arch"]);
    let (mut cause, mut fix) = (Tally::default(), Tally::default());
    let mut total = Vec::new();
    for case in &cases {
        let last =
            crate::session::Last { command: case.command.clone(), exit: Some(case.exit), output: case.output.clone() };
        let answer = doctor::explain(&last, &mut |_| {})?;
        let cause_ok = case.mention.iter().all(|words| contains_any(&answer.text, words));
        let suggested = doctor::suggested(&answer.text);
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
        println!(
            "time per answer:             median {} ms, slowest {} ms",
            total[total.len() / 2],
            total[total.len() - 1]
        );
    }
    Ok(())
}
