//! `huh eval`: run test requests through the matcher and score the results.

use crate::matcher::LocalMatcher;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

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
    /// Pretend these are the entries of the current folder; folders end in `/`.
    #[serde(default)]
    files: Vec<String>,
    /// Text the notes under the first suggestion must contain (optional).
    note: Option<String>,
    /// Package manager family ("debian", "alpine|void", ...) this case is about. Such a case runs
    /// only when scoring as that family (`huh eval --platform`); other cases run everywhere.
    platform: Option<String>,
    /// "expert" scores the case in expert mode (default: normal).
    mode: Option<String>,
    /// Typed values the first suggestion must keep: each must appear in its line or in one of its
    /// notes (case-insensitive); "a|b" accepts either spelling. Nothing may vanish silently.
    #[serde(default)]
    values: Vec<String>,
    /// Text the answer's hint ("No suggestion: ...") must contain (optional).
    hint: Option<String>,
    /// Text the "hint:" pointer must contain ("a|b" accepts either); nothing is suggested.
    pointer: Option<String>,
    /// `true`: the answer must be a plain "No suggestion" with no pointer at all.
    no_pointer: Option<bool>,
    /// The confidence of the first suggestion must not exceed this (optional).
    max_confidence: Option<f64>,
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
    // Users, groups, shells and services come from the fixture, never from the machine running the eval.
    crate::sysinfo::use_fixture(true);
    let report = score_cases(matcher, path, platforms);
    crate::sysinfo::use_fixture(false);
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
        let mode = match &case.mode {
            Some(m) => crate::mode::Mode::parse(m)?,
            None => crate::mode::Mode::Normal,
        };
        let outcome = crate::suggest_in(mode, matcher, &case.request, case.files.clone(), &mut |_| None)?;
        millis.push(outcome.millis);
        let first = outcome.suggestions.first();
        let got = first.map_or("(no suggestion)", |s| s.line.as_str());

        let ok = if case.command == "none" {
            let ok = outcome.suggestions.is_empty()
                && case.hint.as_ref().is_none_or(|h| outcome.hint.as_ref().is_some_and(|got| got.contains(h.as_str())))
                && case.pointer.as_ref().is_none_or(|p| {
                    outcome.pointer.as_ref().is_some_and(|got| {
                        let got = got.to_lowercase();
                        p.split('|').any(|alt| got.contains(&alt.to_lowercase()))
                    })
                })
                && case.no_pointer.is_none_or(|none| none == outcome.pointer.is_none());
            rejected.add(ok);
            ok
        } else {
            // "top|ps" means either command is a right answer.
            let accepted: Vec<&str> = case.command.split('|').collect();
            // "any" accepts whatever comes first, or nothing (an honest abstention): such cases only
            // check what a shown suggestion does with the typed values.
            let any = case.command == "any";
            let command_ok = any || first.is_some_and(|s| accepted.contains(&s.command.as_str()));
            top1.add(command_ok);
            top3.add(outcome.ranked.iter().any(|(name, _)| accepted.contains(&name.as_str())));
            let kept = |value: &String| {
                first.map_or(any, |s| {
                    let shown: Vec<String> =
                        std::iter::once(&s.line).chain(&s.notes).map(|text| text.to_lowercase()).collect();
                    value.split('|').any(|alt| shown.iter().any(|text| text.contains(&alt.to_lowercase())))
                })
            };
            let warn_ok = case.warn.is_none_or(|wanted| first.is_some_and(|s| s.destructive == wanted))
                && case.values.iter().all(kept)
                && case.max_confidence.is_none_or(|max| first.map_or(any, |s| s.probability <= max + 1e-9));
            match &case.expect {
                Some(expect) => {
                    let line_ok = first.is_some_and(|s| {
                        s.line == *expect
                            && case.note.as_ref().is_none_or(|note| s.notes.iter().any(|n| n.contains(note.as_str())))
                    });
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
    /// The recorded exit code; leave it out for output that was piped in.
    exit: Option<i32>,
    output: String,
    /// Words the explanation must contain; "a|b" accepts either.
    mention: Vec<String>,
    /// Text the `Try:` command must contain; "a|b" accepts either. "nothing" expects no command.
    fix: String,
    /// Words the explanation must not contain.
    #[serde(default)]
    forbid: Vec<String>,
    /// Files to create in a fresh folder, as "name:mode" (octal; a trailing "/" on the name makes
    /// a folder). `$TMP` in the command and output stands for that folder.
    #[serde(default)]
    files: Vec<String>,
}

/// Creates a case's files in a new folder and returns its path.
fn make_files(files: &[String], index: usize) -> Result<Option<PathBuf>> {
    use std::os::unix::fs::PermissionsExt;
    if files.is_empty() {
        return Ok(None);
    }
    let dir = std::env::temp_dir().join(format!("huh-eval-errors-{}-{index}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    for spec in files {
        let (name, mode) = spec.rsplit_once(':').with_context(|| format!("bad file spec {spec}"))?;
        let mode = u32::from_str_radix(mode, 8).with_context(|| format!("bad mode in {spec}"))?;
        if let Some((link, target)) = name.split_once("->") {
            std::os::unix::fs::symlink(target, dir.join(link))?;
            continue;
        }
        let path = dir.join(name.trim_end_matches('/'));
        if name.ends_with('/') {
            std::fs::create_dir_all(&path)?;
        } else {
            std::fs::write(&path, "x\n")?;
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(Some(dir))
}

fn remove_files(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let _ = std::fs::set_permissions(entry.path(), std::fs::Permissions::from_mode(0o755));
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

fn contains_any(text: &str, alternatives: &str) -> bool {
    let text = text.to_lowercase();
    alternatives.split('|').any(|word| text.contains(&word.to_lowercase()))
}

/// `huh eval-errors`: a first-pass score only. The answers are printed in
/// full because whether an explanation is right has to be read, not matched.
pub fn run_errors(path: &Path) -> Result<()> {
    score_errors(path, true).map(|_| ())
}

/// Scores the cases in `path`; returns how many explanations named the cause and how many
/// suggested a fitting command.
pub fn score_errors(path: &Path, verbose: bool) -> Result<(Tally, Tally)> {
    use crate::doctor;
    let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let cases = toml::from_str::<ErrorFile>(&text).with_context(|| format!("invalid {}", path.display()))?.case;

    // The recorded failures come from an Arch machine that lacked the program: score them
    // the same way on every machine.
    crate::platform::pin_for_eval(&["arch"]);
    let (mut cause, mut fix) = (Tally::default(), Tally::default());
    let mut total = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let dir = make_files(&case.files, index)?;
        let fill = |text: &str| match &dir {
            Some(dir) => text.replace("$TMP", &dir.display().to_string()),
            None => text.to_string(),
        };
        let (command, output) = (fill(&case.command), fill(&case.output));
        let last = crate::session::Last { command: command.clone(), exit: case.exit, output };
        let answer = doctor::explain(&last, &mut |_| {})?;
        if let Some(dir) = &dir {
            remove_files(dir);
        }
        let lower = answer.text.to_lowercase();
        let cause_ok = case.mention.iter().all(|words| contains_any(&answer.text, words))
            && !case.forbid.iter().any(|word| lower.contains(&word.to_lowercase()));
        let suggested = doctor::suggested(&answer.text);
        let fix_ok = match (case.fix.as_str(), suggested) {
            (wanted, Some(command)) => contains_any(command, wanted),
            (wanted, None) => wanted.split('|').any(|alternative| alternative == "nothing"),
        };
        cause.add(cause_ok);
        fix.add(fix_ok);
        total.push(answer.total.as_millis());
        if verbose {
            let exit = case.exit.map_or("no exit code".to_string(), |code| format!("exit {code}"));
            println!("=== $ {command}   ({exit})");
            println!("{}", answer.text.trim());
            println!(
                "--- cause {}, fix {} (wanted: {}), {} ms\n",
                if cause_ok { "ok" } else { "MISS" },
                if fix_ok { "ok" } else { "MISS" },
                case.fix,
                answer.total.as_millis()
            );
        }
    }
    total.sort();
    if verbose {
        println!("explanation names the cause: {cause}");
        println!("suggested command fits:      {fix}");
        if !total.is_empty() {
            println!(
                "time per answer:             median {} ms, slowest {} ms",
                total[total.len() / 2],
                total[total.len() - 1]
            );
        }
    }
    Ok((cause, fix))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Expert mode (evals/expert.toml): relaxed protections answer, refusals and warnings stay.
    #[test]
    fn expert_evals_all_pass() {
        let arch = vec!["arch".to_string(), "omarchy".to_string()];
        let catalog = crate::catalog::usable(crate::catalog::load_embedded().unwrap(), &arch, false);
        let matcher = LocalMatcher::new(&catalog);
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("evals/expert.toml");
        let report = score_for(&matcher, &file, &arch).unwrap();
        let misses: Vec<&String> = report.lines.iter().filter(|line| line.starts_with("MISS")).collect();
        for line in &misses {
            println!("{line}");
        }
        assert!(report.lines.len() >= 30, "expert.toml should hold 30+ cases");
        assert!(misses.is_empty(), "expert evals must be at 100%: {} misses", misses.len());
    }

    /// Hints (evals/hints.toml): a request with no confident command but an honest closest tool gets
    /// a pointer naming it; off-topic requests get a plain "No suggestion".
    #[test]
    fn hint_evals_all_pass() {
        let arch = vec!["arch".to_string(), "omarchy".to_string()];
        let catalog = crate::catalog::usable(crate::catalog::load_embedded().unwrap(), &arch, false);
        let matcher = LocalMatcher::new(&catalog);
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("evals/hints.toml");
        let report = score_for(&matcher, &file, &arch).unwrap();
        let misses: Vec<&String> = report.lines.iter().filter(|line| line.starts_with("MISS")).collect();
        for line in &misses {
            println!("{line}");
        }
        println!("{} cases, {} misses", report.lines.len(), misses.len());
        assert!(report.lines.len() >= 55, "hints.toml should hold 40+ hint and 15 off-topic cases");
        assert!(misses.is_empty(), "hint evals must be at 100%: {} misses", misses.len());
    }

    #[test]
    fn recorded_failures_are_all_explained() {
        for file in [
            "evals/errors.toml",
            "evals/errors-heldout.toml",
            "evals/errors-real.toml",
            "evals/errors-measured.toml",
            "evals/errors-round3.toml",
            "evals/errors-round4.toml",
        ] {
            let (cause, fix) = score_errors(Path::new(file), false).unwrap();
            assert_eq!(cause.percent(), 100, "{file}: causes {cause}");
            assert_eq!(fix.percent(), 100, "{file}: fixes {fix}");
        }
    }
}
