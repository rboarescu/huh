mod assemble;
mod breakdown;
mod candidates;
mod catalog;
mod decode;
mod doctor;
mod eval;
mod explain;
mod extract;
mod knowledge;
mod manpage;
mod matcher;
mod platform;
mod render;
mod risk;
mod session;
mod shell;

use anyhow::{Result, bail};
use assemble::Suggestion;
use candidates::Candidates;
use catalog::Command;
use clap::{Parser, Subcommand};
use matcher::LocalMatcher;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Parser)]
#[command(name = "huh", version, about = "Say what you want to do; get a terminal command back. Nothing is ever run.")]
#[command(args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    action: Option<Action>,
    /// What you want to do, in plain language
    request: Vec<String>,
    /// Print ranked candidate scores and timing
    #[arg(long, global = true)]
    debug: bool,
    /// Folder holding the catalog *.toml files
    #[arg(long, global = true)]
    catalog: Option<PathBuf>,
    /// Do not offer names from the current folder as candidates
    #[arg(long)]
    no_cwd: bool,
    /// Never ask a y/n question; list uncertain flags as notes instead
    #[arg(long)]
    no_ask: bool,
    /// Hide the part-by-part explanation under the first suggestion
    #[arg(long)]
    brief: bool,
    /// Print only the first suggested command line (or exit 1 with no output)
    #[arg(long, hide = true)]
    first_line_only: bool,
    /// Also offer commands that belong to another kind of system (e.g. apt on Arch)
    #[arg(long)]
    all_commands: bool,
}

#[derive(Subcommand)]
enum Action {
    /// Score the test requests in a cases file against the local matcher
    Eval {
        #[arg(long, default_value = "evals/cases.toml")]
        cases: PathBuf,
        /// Score as if on these systems (comma separated). Unlike normal use,
        /// eval does not hide commands that are missing from this machine.
        #[arg(long, default_value = "arch,omarchy", value_delimiter = ',')]
        platform: Vec<String>,
    },
    /// Validate the catalog and list its commands
    Check,
    /// Explain why the last command failed (or pipe output in)
    Why,
    /// Explain what a shell command and each of its flags and arguments do
    Explain {
        /// Command line to break down; `-` (or nothing, with piped input) reads it from standard input
        command: Vec<String>,
    },
    /// Print the shell lines that record a terminal and bind prompt completion for `huh`
    ShellInit {
        /// Print the second part, which goes at the end of ~/.bashrc
        #[arg(long)]
        hooks: bool,
    },
    /// Score the error doctor on recorded failures
    EvalErrors {
        #[arg(long, default_value = "evals/errors.toml")]
        cases: PathBuf,
    },
    #[command(hide = true)]
    RecordSink { file: PathBuf, pipe: PathBuf },
}

pub struct Outcome {
    pub suggestions: Vec<Suggestion>,
    /// The command shortlist before fit filtering, best first.
    pub ranked: Vec<(String, f64)>,
    pub millis: u128,
    /// Set instead of suggestions when the request asks what a command does ("what does rm -rf do"):
    /// the answer is `huh explain`, and suggesting the command itself would be backwards.
    pub hint: Option<String>,
}

/// Programs people ask about that the catalog may not hold.
const COMMON_PROGRAMS: &[&str] = &[
    "sudo", "su", "dd", "mkfs", "fdisk", "parted", "fsck", "mount", "umount", "sed", "awk", "xargs", "tee", "shred",
    "wget", "nc", "nmap", "iptables", "ufw", "crontab", "env", "export", "alias", "eval", "exec", "nohup", "screen",
    "make", "gcc", "pip", "npm", "yarn", "apt", "dnf", "pacman", "chgrp", "umask", "tr", "rev", "tac", "yes", "wall",
    "mkswap", "swapon", "fuser", "lsof", "strace", "ldd", "ln", "truncate", "split", "install", "mkdir", "rmdir",
];

/// Whole-folder targets a destructive line must never get unless the request typed them.
const BROAD_TARGETS: &[&str] = &[".", "./", "./*", "*", "~", "~/", "~/*", "/", "/*", "$HOME", "$HOME/", "$HOME/*"];

/// A destructive suggestion that would act on a whole folder the user never named (`rm -r ~`
/// for "delete user bob and home directory"), or a `find ... -delete` with no name or age filter
/// (every file under the folder), is worse than no suggestion: drop it.
fn destroys_more_than_named(suggestion: &Suggestion, request: &str) -> bool {
    if !suggestion.destructive {
        return false;
    }
    let typed: Vec<&str> = request.split_whitespace().map(|w| w.trim_matches(['"', '\'', ',', '?'])).collect();
    if suggestion.line.ends_with("-delete") && suggestion.line.starts_with("find ") {
        return !(suggestion.line.contains(" -name ")
            || suggestion.line.contains(" -mtime ")
            || suggestion.line.contains(" -size "));
    }
    suggestion
        .line
        .split_whitespace()
        .map(|w| w.trim_matches(['"', '\'']))
        .any(|w| BROAD_TARGETS.contains(&w) && !typed.contains(&w))
}

/// "what does rm -rf do": the command being asked about, as written in the request.
fn command_in_question<'a>(request: &'a str, matcher: &LocalMatcher) -> Option<&'a str> {
    let trimmed = request.trim().trim_end_matches(['?', '.', '!']).trim();
    let lower = trimmed.to_ascii_lowercase();
    // "explain X" and "what is X" ask about X as much as "what does X do".
    let (rest, tail_len) = if let Some(rest) = lower.strip_prefix("explain ").or_else(|| lower.strip_prefix("what is "))
    {
        (rest, 0)
    } else {
        let rest = lower.strip_prefix("what does ").or_else(|| lower.strip_prefix("what do "))?;
        let tail = [" actually do", " do", " mean", " means", " actually mean", " stand for"]
            .iter()
            .find(|tail| rest.ends_with(**tail))?;
        (rest, tail.len())
    };
    let start = lower.len() - rest.len();
    let inner = trimmed[start..start + rest.len() - tail_len].trim();
    let mut words = inner.split_whitespace().peekable();
    if words.peek() == Some(&"sudo") && inner.split_whitespace().count() > 1 {
        words.next();
    }
    let first = words.next()?;
    let known = matcher.knows_program(first) || COMMON_PROGRAMS.contains(&first);
    (known || inner.split_whitespace().any(|w| w.len() > 1 && w.starts_with('-'))).then_some(inner)
}

/// The whole pipeline: rank the catalog, shortlist, then build a line for each
/// shortlisted command that fits. `files` are offered as path values.
///
/// `decide` is asked a yes/no question when a flag or value of the first
/// suggestion is uncertain; return `None` when there is nobody to ask.
pub fn suggest(
    matcher: &LocalMatcher,
    request: &str,
    files: Vec<String>,
    decide: &mut dyn FnMut(&str) -> Option<bool>,
) -> Result<Outcome> {
    let started = Instant::now();
    if let Some(command) = command_in_question(request, matcher) {
        let hint = format!(
            "that asks what a command does, not what to run. To see what it does, use: huh explain \"{}\"",
            command.replace('"', "\\\"")
        );
        return Ok(Outcome {
            suggestions: vec![],
            ranked: vec![],
            millis: started.elapsed().as_millis(),
            hint: Some(hint),
        });
    }
    let shortlist = assemble::shortlist(matcher.rank(request));
    let candidates = Candidates::new(request, files);

    let mut suggestions = Vec::new();
    for &(command, probability) in &shortlist {
        let Some(details) = matcher.details(command, request, &candidates) else { continue };
        // Only the first suggestion is worth interrupting the user for.
        let mut silent = |_: &str| None;
        let ask: &mut dyn FnMut(&str) -> Option<bool> = if suggestions.is_empty() { &mut *decide } else { &mut silent };
        if let Some(mut suggestion) = assemble::build(command, probability, &details, ask)? {
            if destroys_more_than_named(&suggestion, request) {
                continue;
            }
            // A stated modifier the line has no place for must not vanish silently.
            for (phrase, value) in candidates::modifier_phrases(request) {
                if !suggestion.line.to_lowercase().contains(&value) {
                    suggestion.notes.push(format!("not included: `{phrase}`"));
                }
            }
            suggestions.push(suggestion);
        }
    }
    let ranked = shortlist.iter().map(|(command, p)| (command.name.clone(), *p)).collect();
    Ok(Outcome { suggestions, ranked, millis: started.elapsed().as_millis(), hint: None })
}

/// Re-quotes multi-word arguments when multiple CLI tokens are supplied so
/// shell-quoted values stay grouped for `Candidates::tokens()`.
fn join_argv(args: &[String]) -> String {
    if args.len() <= 1 {
        return args.first().cloned().unwrap_or_default();
    }
    args.iter()
        .map(|arg| {
            if arg.is_empty() || arg.chars().any(char::is_whitespace) {
                if !arg.contains('"') {
                    format!("\"{arg}\"")
                } else if !arg.contains('\'') {
                    format!("'{arg}'")
                } else {
                    format!("\"{}\"", arg.replace('"', "\\\""))
                }
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn print(outcome: &Outcome, catalog: &[Command], brief: bool) {
    if package_task_without_manager(outcome, catalog) {
        println!("No suggestion: {}", platform::no_manager_message());
        return;
    }
    if let Some(hint) = &outcome.hint {
        println!("No suggestion: {hint}");
        return;
    }
    if outcome.suggestions.is_empty() {
        println!("No suggestion: nothing in the catalog fits that request.");
        return;
    }
    let platforms = catalog::this_platform();
    for (number, s) in outcome.suggestions.iter().enumerate() {
        println!("{}. {}", number + 1, s.line);
        println!("   {} ({:.0}%)", s.description, s.probability * 100.0);
        if number == 0 && !brief && !s.parts.is_empty() {
            let width = s.parts.iter().map(|(p, _)| p.chars().count()).max().unwrap_or(0).min(24);
            for (piece, explanation) in &s.parts {
                println!("     {piece:<width$}  {explanation}");
            }
        }
        if let Some(why) = &s.warning {
            println!("   WARNING: {why}");
        }
        // Hiding a missing program would push a worse command to the top, so say it instead.
        if let Some(note) = catalog::missing_note(&s.line, catalog.iter().find(|c| c.name == s.command), &platforms) {
            println!("   NOTE: {note}");
        }
        for note in &s.notes {
            println!("   {note}");
        }
    }
}

/// The best match is a package task, but this system has no package manager huh knows.
fn package_task_without_manager(outcome: &Outcome, catalog: &[Command]) -> bool {
    outcome.suggestions.first().is_some_and(|s| catalog.iter().any(|c| c.name == s.command && c.without_manager()))
}

/// Asks on the terminal and reads a line; Enter alone means no.
fn ask_on_terminal(question: &str) -> Option<bool> {
    use std::io::Write;
    eprint!("{question} [y/N] ");
    std::io::stderr().flush().ok()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok()?;
    Some(matches!(line.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// `huh why`: the recorded last command, or whatever was piped in.
fn why() -> Result<()> {
    use std::io::{IsTerminal, Read};
    let last = if std::io::stdin().is_terminal() {
        session::last()?
    } else {
        let mut piped = Vec::new();
        std::io::stdin().read_to_end(&mut piped)?;
        let output = session::tail(&session::clean(&String::from_utf8_lossy(&piped)));
        session::Last { command: String::new(), exit: None, output }
    };
    why_last(last)
}

fn why_last(last: session::Last) -> Result<()> {
    use std::io::Write;
    if !last.command.is_empty() {
        let exit = last.exit.map_or(String::new(), |code| format!(" (exit code {code})"));
        eprintln!("Last command: {}{exit}", last.command);
    }
    if last.output.trim().is_empty() && last.command.is_empty() {
        bail!("there is no output to explain");
    }
    eprintln!();
    doctor::explain(&last, &mut |piece| {
        print!("{piece}");
        std::io::stdout().flush().ok();
    })?;
    println!();
    eprintln!("\n(Nothing was run.)");
    Ok(())
}

/// Rust ignores SIGPIPE, so printing into a closed pipe (`huh check | head`) panics.
/// Restoring the default makes huh exit quietly instead, like other Unix tools.
#[cfg(unix)]
fn exit_quietly_on_closed_pipe() {
    unsafe extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }
    const SIGPIPE: i32 = 13;
    const SIG_DFL: usize = 0;
    // SAFETY: called once at startup, before any other thread exists.
    unsafe {
        signal(SIGPIPE, SIG_DFL);
    }
}

fn main() -> Result<()> {
    use std::io::IsTerminal;
    #[cfg(unix)]
    exit_quietly_on_closed_pipe();
    let cli = Cli::parse();

    // These do not need the catalog, and the shell ones run in every new terminal.
    match &cli.action {
        Some(Action::ShellInit { hooks }) => {
            print!("{}", if *hooks { session::hooks_script() } else { session::record_script()? });
            return Ok(());
        }
        Some(Action::RecordSink { file, pipe }) => return session::sink(file, pipe),
        Some(Action::Why) => return why(),
        Some(Action::EvalErrors { cases }) => return eval::run_errors(cases),
        None if cli.request.is_empty() => {
            if cli.first_line_only {
                std::process::exit(1);
            }
            if !std::io::stdin().is_terminal() {
                return why();
            }
            if let Ok(last) = session::last()
                && last.exit.is_some_and(|code| code != 0)
            {
                return why_last(last);
            }
            bail!(
                "say what you want to do, e.g. `huh \"make deploy.sh executable\"`, \
                 `huh explain \"<command>\"`, or `huh why`"
            );
        }
        _ => {}
    }

    let everything = catalog::load_default(cli.catalog)?;

    match cli.action {
        Some(Action::Why | Action::ShellInit { .. } | Action::EvalErrors { .. } | Action::RecordSink { .. }) => {
            unreachable!("handled above")
        }
        Some(Action::Check) => {
            let everything = catalog::matchable(everything);
            let total = everything.len();
            let here = catalog::usable(everything.clone(), &catalog::this_platform(), true);
            let offered = catalog::usable(everything.clone(), &catalog::this_platform(), false).len();
            for command in &everything {
                let mark = if here.iter().any(|c| c.name == command.name) { " " } else { "-" };
                println!("{mark} {:28} {}", command.name, command.description);
            }
            println!(
                "{total} commands, all valid. {offered} are offered on this system; {} of those are installed (- marks the rest).",
                here.len()
            );
        }
        Some(Action::Explain { command }) => {
            use std::io::Read;
            // `huh explain -` (or no argument with piped input) reads the text from standard input.
            let from_stdin = command == ["-"] || (command.is_empty() && !std::io::stdin().is_terminal());
            let cmd_str = if from_stdin {
                let mut piped = String::new();
                std::io::stdin().take(1 << 20).read_to_string(&mut piped)?;
                piped
            } else {
                join_argv(&command)
            };
            if cmd_str.trim().is_empty() {
                bail!("provide a command to explain, e.g. `huh explain \"du -sh * | sort -h\"`");
            }
            let explanation = explain::explain_command(&everything, &cmd_str);
            print!("{}", render::render(&explanation));
        }
        Some(Action::Eval { cases, platform }) => {
            let catalog = catalog::usable(everything, &platform, false);
            eval::run(&LocalMatcher::new(&catalog), &cases, &platform)?;
        }
        None => {
            let request = join_argv(&cli.request);
            if request.trim().is_empty() {
                if cli.first_line_only {
                    std::process::exit(1);
                }
                bail!(
                    "say what you want to do, e.g. `huh \"make deploy.sh executable\"`, \
                     `huh explain \"<command>\"`, or `huh why`"
                );
            }
            let catalog = if cli.all_commands {
                catalog::resolve(catalog::matchable(everything), &catalog::this_platform())
            } else {
                catalog::usable(everything, &catalog::this_platform(), false)
            };
            let files = if cli.no_cwd { vec![] } else { candidates::current_folder_entries() };
            let interactive = !cli.no_ask
                && !cli.first_line_only
                && std::io::stdin().is_terminal()
                && std::io::stderr().is_terminal();
            let mut decide = |question: &str| if interactive { ask_on_terminal(question) } else { None };
            let outcome = suggest(&LocalMatcher::new(&catalog), &request, files, &mut decide)?;
            if cli.debug {
                eprintln!("[debug] {} ms, ranked: {:?}", outcome.millis, outcome.ranked);
            }
            if cli.first_line_only {
                if package_task_without_manager(&outcome, &catalog) {
                    std::process::exit(1);
                }
                if let Some(first) = outcome.suggestions.first() {
                    println!("{}", first.line);
                    return Ok(());
                }
                std::process::exit(1);
            }
            print(&outcome, &catalog, cli.brief);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions_about_a_command_point_to_explain() {
        let catalog = catalog::load_embedded().unwrap();
        let matcher = LocalMatcher::new(&catalog::usable(catalog, &["arch".to_string()], false));
        for (request, command) in [
            ("what does rm -rf do", "rm -rf"),
            ("What does chmod 777 do?", "chmod 777"),
            ("what does sudo dd if=/dev/zero of=/dev/sda do", "sudo dd if=/dev/zero of=/dev/sda"),
            ("what does tar -xzf mean", "tar -xzf"),
            ("explain sudo", "sudo"),
            ("what is grep", "grep"),
            ("explain cat notes.txt | wc -l", "cat notes.txt | wc -l"),
        ] {
            let out = suggest(&matcher, request, vec![], &mut |_| None).unwrap();
            assert!(out.suggestions.is_empty(), "{request}");
            let hint = out.hint.unwrap_or_default();
            assert!(hint.contains(&format!("huh explain \"{command}\"")), "{request}: {hint}");
        }
        // Not about a command: the normal pipeline answers.
        for request in ["what does the weather do", "delete old.log", "what does my disk look like"] {
            assert!(suggest(&matcher, request, vec![], &mut |_| None).unwrap().hint.is_none(), "{request}");
        }
    }

    #[test]
    fn preserves_single_arg_and_quotes_multi_word_argv() {
        assert_eq!(join_argv(&["make deploy.sh executable".into()]), "make deploy.sh executable");
        assert_eq!(
            join_argv(&["switch".into(), "theme".into(), "to".into(), "Tokyo Night".into()]),
            r#"switch theme to "Tokyo Night""#
        );
        assert_eq!(
            join_argv(&["git".into(), "commit".into(), "-m".into(), "fix bug".into()]),
            r#"git commit -m "fix bug""#
        );
    }
}
