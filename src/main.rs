mod assemble;
mod breakdown;
mod candidates;
mod catalog;
mod decode;
mod doctor;
mod eval;
mod explain;
mod extract;
mod find;
mod guard;
mod hint;
mod knowledge;
mod launch;
mod manpage;
mod matcher;
mod mode;
mod outputs;
mod platform;
mod readers;
mod render;
mod risk;
mod risk_extra;
mod risk_kinds;
mod session;
mod shell;
mod style;
mod sysinfo;
mod tools;
mod tutor;
mod values;
mod welcome;

use anyhow::{Result, bail};
use assemble::Suggestion;
use candidates::Candidates;
use catalog::Command;
use clap::{Parser, Subcommand};
use matcher::LocalMatcher;
use std::path::PathBuf;
use std::time::Instant;
use style::{Role, paint};

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
    /// Expert mode: fewer beginner safeguards; warnings are still shown (also HUH_MODE=expert or mode = "expert" in ~/.config/huh/config.toml)
    #[arg(long, global = true)]
    expert: bool,
    /// Normal mode, overriding HUH_MODE and the config file
    #[arg(long, global = true)]
    normal: bool,
    /// Colour theme: auto, omarchy, dark, light, nord, gruvbox, catppuccin or mono (also HUH_THEME or theme = "..." in the config file; see `huh themes`)
    #[arg(long, global = true, value_name = "NAME")]
    theme: Option<String>,
    /// When to use colour: auto (a terminal, no NO_COLOR, TERM not dumb), always or never
    #[arg(long, global = true, value_enum, default_value = "auto")]
    color: style::ColorChoice,
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
    /// Show a sample of every colour theme
    Themes,
    /// Replay the welcome animation
    Welcome,
    /// Explain why the last command failed (or pipe output in)
    Why,
    /// Explain what a shell command and each of its flags and arguments do
    Explain {
        /// Command line to break down; `-` (or nothing, with piped input) reads it from standard input
        command: Vec<String>,
    },
    /// List the programs on this machine with a one-line description; words narrow the list
    ///
    /// Example: `huh tools compress`. Nothing is run: names come from $PATH, descriptions from
    /// the catalog, the built-in table or man pages read from disk.
    Tools {
        /// Keep programs whose name or description contains every word
        words: Vec<String>,
        /// List names only
        #[arg(long)]
        no_desc: bool,
    },
    /// Search the catalog and every installed program by words (fuzzy); with fzf installed it is a picker
    ///
    /// Example: `huh find compress`. Piped, it prints the matching lines. Nothing is run: details
    /// (what it is, installed or not, common flags) come from the catalog, the built-in table and man pages.
    Find {
        /// Words to start the search with
        words: Vec<String>,
        /// Print the details of one catalog entry or program instead of searching (the fzf preview)
        #[arg(long, value_name = "NAME", conflicts_with = "words")]
        describe: Option<String>,
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
    /// Follow the recorded terminal next to this one and explain each command and its output
    ///
    /// Run it in a second pane. It only reads the recording (see `huh shell-init`); it never runs anything.
    Tutor {
        /// Hide the explanation of a program already explained in this run
        #[arg(long)]
        quiet: bool,
        /// Follow the recording of this terminal (e.g. /dev/pts/3) instead of the newest one
        #[arg(long, conflicts_with = "side")]
        tty: Option<String>,
        /// Open the tutor beside this terminal: a tmux split, a new window on a tiling desktop, or a tmux session
        #[arg(long)]
        side: bool,
        /// With --side: print what would be started instead of starting it (also HUH_DRY_RUN=1)
        #[arg(long, requires = "side")]
        dry_run: bool,
        /// The folder holding the recordings (set by --side for the window it opens)
        #[arg(long, hide = true)]
        dir: Option<PathBuf>,
        /// Keep the window open after an error until Enter is pressed (set by --side)
        #[arg(long, hide = true)]
        hold: bool,
        /// Follow exactly this recording (set by --side: the terminal it was started from)
        #[arg(long, hide = true)]
        log: Option<PathBuf>,
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
    /// "hint: ..." when nothing fits: the closest tool and where to read about it. Never a command line.
    pub pointer: Option<String>,
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

/// What a destructive suggestion would act on that is a whole folder the user never named
/// (`rm -r ~` for "delete user bob and home directory"), or a `find ... -delete` with no name or
/// age filter (every file under the folder). Normal mode drops such a suggestion, since it is worse
/// than none; expert mode keeps it and adds a note naming the target.
fn broad_target(suggestion: &Suggestion, request: &str) -> Option<&'static str> {
    if !suggestion.destructive {
        return None;
    }
    let typed: Vec<&str> = request.split_whitespace().map(|w| w.trim_matches(['"', '\'', ',', '?'])).collect();
    if suggestion.line.ends_with("-delete") && suggestion.line.starts_with("find ") {
        let filtered = suggestion.line.contains(" -name ")
            || suggestion.line.contains(" -mtime ")
            || suggestion.line.contains(" -size ");
        return (!filtered).then_some("every file under the starting folder");
    }
    // `find . -name node_modules -type d -prune -exec rm -rf {} +` only removes what the name matches;
    // without a name it would remove every folder.
    if suggestion.line.starts_with("find ") && suggestion.line.contains(" -exec rm ") {
        let named = suggestion.line.contains(" -name ") && !suggestion.line.contains(" -name <");
        return (!named).then_some("every file or folder under the starting folder");
    }
    let target = suggestion
        .line
        .split_whitespace()
        .map(|w| w.trim_matches(['"', '\'']))
        .find(|w| BROAD_TARGETS.contains(w) && !typed.contains(w))?;
    Some(match target {
        "." | "./" => "the whole current folder",
        "./*" | "*" => "everything in the current folder",
        "~" | "~/" | "$HOME" | "$HOME/" => "your whole home folder",
        "~/*" | "$HOME/*" => "everything in your home folder",
        "/" => "the whole filesystem root",
        _ => "everything at the top of the filesystem",
    })
}

/// "delete all files in Documents" empties Documents; `rm -r Documents` would delete the folder too.
/// Rewrites such an `rm` line to `rm -r Documents/*` and says what that leaves behind.
fn contents_of_folder(suggestion: &mut Suggestion, command: &Command, request: &str) {
    if command.name != "rm" {
        return;
    }
    let Some(folder) = guard::contents_target(request) else { return };
    let target = assemble::quote(&folder);
    let named =
        suggestion.line.rsplit(' ').next() == Some(target.as_str()) || suggestion.line.ends_with(&format!(" {target}"));
    if !named {
        return;
    }
    let contents = format!("{}/*", target.trim_end_matches('/'));
    suggestion.line = format!("rm -r {contents}");
    let recursive = suggestion.parts.iter().find(|(piece, _)| piece == "-r").map(|(_, why)| why.clone());
    suggestion.parts = vec![
        ("-r".into(), recursive.unwrap_or_else(|| "Also delete the folders inside".into())),
        (contents, format!("everything inside {folder}, but not {folder} itself")),
    ];
    suggestion.notes.push(format!(
        "not included: hidden files (names starting with a dot) in {folder}, and {folder} itself; they stay"
    ));
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
    suggest_in(mode::Mode::Normal, matcher, request, files, decide)
}

/// `suggest` in a given mode. Expert mode drops the beginner protections only (see mode.rs);
/// refusals, the explain redirects and every WARNING stay.
pub fn suggest_in(
    mode: mode::Mode,
    matcher: &LocalMatcher,
    request: &str,
    files: Vec<String>,
    decide: &mut dyn FnMut(&str) -> Option<bool>,
) -> Result<Outcome> {
    let expert = mode.is_expert();
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
            pointer: None,
        });
    }
    let known = |word: &str| matcher.knows_program(word) || COMMON_PROGRAMS.contains(&word);
    let refused = guard::command_in_safety_question(request, &known)
        .map(|command| {
            format!(
                "that asks whether a command is safe, not what to run. To see what it does and what could go wrong, use: huh explain \"{}\"",
                command.replace('"', "\\\"")
            )
        })
        .or_else(|| {
            // The request is already a command line: it is read, not matched, and one that would
            // destroy the system is never echoed back as a suggestion.
            let line = guard::command_line_request(request, &known)
                .or_else(|| guard::destroys_system(request).then_some(request.trim()));
            line.map(|line| {
                let explain = format!("huh explain \"{}\"", line.replace('"', "\\\""));
                if guard::destroys_system(line) {
                    format!("this would destroy the system (or a whole drive), so huh will not suggest it. To see what it does, use: {explain}")
                } else {
                    format!("that is already a command line, not a request in words. To see what it does, use: {explain}")
                }
            })
        })
        .or_else(|| guard::refusal(request, expert));
    if let Some(hint) = refused {
        return Ok(Outcome {
            suggestions: vec![],
            ranked: vec![],
            millis: started.elapsed().as_millis(),
            hint: Some(hint),
            pointer: None,
        });
    }
    let all_ranked = matcher.rank(request);
    let shortlist = assemble::shortlist(all_ranked.clone());
    let candidates = Candidates::new(request, files);
    let about_state = matcher.asks_about_state(request);

    let mut suggestions = Vec::new();
    let mut demoted = Vec::new();
    let mut hint = None;
    let mut refused_for_want_of_an_object = None;
    for &(command, probability) in &shortlist {
        // A question about state is answered by a query or not at all: never by an action.
        if about_state && !expert && matcher.is_action(&command.name) {
            hint.get_or_insert(
                "that asks about the current state, and no command in the catalog that only looks fits it.",
            );
            continue;
        }
        let Some(mut details) = matcher.details(command, request, &candidates) else { continue };
        readers::refine(command, request, &mut details);
        // Only the first suggestion is worth interrupting the user for.
        let mut silent = |_: &str| None;
        let ask: &mut dyn FnMut(&str) -> Option<bool> = if suggestions.is_empty() { &mut *decide } else { &mut silent };
        if let Some(mut suggestion) = assemble::build(command, probability, &details, ask)? {
            guard::tell_the_truth(&mut suggestion, command, &details, &candidates);
            // `rm` never wipes a drive (that is wipefs, mkfs or dd, shown in expert mode with a WARNING).
            if guard::deletes_a_drive(&suggestion.line) {
                continue;
            }
            // "everything in X" is the contents of X, not X.
            contents_of_folder(&mut suggestion, command, request);
            // A request to look never gets a destructive answer: normal mode abstains, expert mode
            // shows it below every command that only looks.
            // ("how big is x.tar.gz" must not unpack it; "show permissions of f" must not chmod it.)
            // Only the stricter question-or-look reading counts here: "get the latest changes" acts.
            let looks_only = matcher.asks_about_state(request);
            let destructive_look =
                looks_only && (suggestion.destructive || guard::changes_something(command, &details));
            if destructive_look && !expert {
                hint.get_or_insert(
                    "that asks to look at something, and no command in the catalog that only looks fits it.",
                );
                continue;
            }
            if command.name == "crontab edit"
                && let Some(line) = readers::cron_line(request)
            {
                suggestion
                    .notes
                    .push(format!("in the editor that opens, add this line (use the full path of the script): {line}"));
            }
            if let Some(target) = broad_target(&suggestion, request) {
                if !expert {
                    continue;
                }
                suggestion.notes.push(format!("note: this acts on {target}"));
            }
            // A dangerous command needs the verb and something specific to act on; without the
            // object the request is too vague to risk it, and nothing weaker should stand in for it.
            if suggestion.destructive && !expert && !guard::has_evidence(command, &details, request) {
                if suggestions.is_empty() {
                    hint = Some(
                        "that would change or delete something, but the request does not name what. Say which file, folder or item.",
                    );
                    refused_for_want_of_an_object = Some((command, probability));
                    break;
                }
                continue;
            }
            // A stated modifier the line has no place for must not vanish silently.
            let mut dropped = 0;
            for (phrase, value) in candidates::modifier_phrases(request) {
                if !values::modifier_present(&suggestion.line, &phrase, &value) {
                    suggestion.notes.push(format!("not included: `{phrase}`"));
                    dropped += 1;
                }
            }
            // Every other typed value must be in the line too, or be named as left out.
            let mut lost = values::lost(request, &suggestion.line, &suggestion.notes);
            let noted = |t: &values::Typed, notes: &[String]| {
                notes
                    .iter()
                    .any(|n| n.starts_with("not included") && n.to_lowercase().contains(&t.value.to_lowercase()))
            };
            lost.retain(|t| !noted(t, &suggestion.notes));
            values::fill_open_placeholder(&mut suggestion, command, &mut lost);
            for t in &lost {
                suggestion.notes.push(format!("not included: `{}`", t.phrase));
            }
            // Several values lost means the line is probably the wrong command.
            if dropped + lost.len() > 1 {
                suggestion.probability = suggestion.probability.min(values::LOST_CONFIDENCE);
            }
            if destructive_look {
                demoted.push(suggestion);
            } else {
                suggestions.push(suggestion);
            }
        }
    }
    suggestions.append(&mut demoted);
    let ranked = shortlist.iter().map(|(command, p)| (command.name.clone(), *p)).collect();
    let nothing = suggestions.is_empty();
    let pointer = if nothing {
        let commands: Vec<&Command> = matcher.commands().collect();
        hint::pointer(request, &all_ranked, &commands, refused_for_want_of_an_object)
    } else {
        None
    };
    let hint = if nothing { hint.map(str::to_string) } else { None };
    Ok(Outcome { suggestions, ranked, millis: started.elapsed().as_millis(), hint, pointer })
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
    print!("{}", render_outcome(outcome, catalog, brief));
}

/// The suggestions as printed text (every line ends with a newline).
pub fn render_outcome(outcome: &Outcome, catalog: &[Command], brief: bool) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    if package_task_without_manager(outcome, catalog) {
        let _ = writeln!(out, "{}", paint(Role::Note, &format!("No suggestion: {}", platform::no_manager_message())));
        return out;
    }
    if let Some(hint) = &outcome.hint {
        let _ = writeln!(out, "{}", paint(Role::Note, &format!("No suggestion: {hint}")));
        if let Some(pointer) = &outcome.pointer {
            let _ = writeln!(out, "{}", paint(Role::Note, pointer));
        }
        return out;
    }
    if outcome.suggestions.is_empty() {
        match &outcome.pointer {
            Some(pointer) => {
                let _ = writeln!(out, "{}", paint(Role::Note, pointer));
            }
            None => {
                let _ =
                    writeln!(out, "{}", paint(Role::Note, "No suggestion: nothing in the catalog fits that request."));
            }
        }
        return out;
    }
    let platforms = catalog::this_platform();
    for (number, s) in outcome.suggestions.iter().enumerate() {
        let _ = writeln!(out, "{}{}", paint(Role::Dim, &format!("{}. ", number + 1)), paint(Role::Command, &s.line));
        let _ = writeln!(
            out,
            "   {} {}",
            paint(Role::Description, &s.description),
            paint(Role::Value, &format!("({:.0}%)", s.probability * 100.0))
        );
        if number == 0 && !brief && !s.parts.is_empty() {
            let width = s.parts.iter().map(|(p, _)| p.chars().count()).max().unwrap_or(0).min(24);
            for (piece, explanation) in &s.parts {
                let _ = writeln!(
                    out,
                    "     {}  {}",
                    paint(Role::Part, &format!("{piece:<width$}")),
                    paint(Role::Description, explanation)
                );
            }
        }
        if let Some(why) = &s.warning {
            match s.risk {
                Some(risk) => {
                    let _ = writeln!(
                        out,
                        "   {}{}{} {}",
                        paint(Role::Warning, "WARNING ("),
                        paint(Role::Risk, risk.name()),
                        paint(Role::Warning, "):"),
                        paint(Role::Warning, why)
                    );
                }
                None => {
                    let _ = writeln!(out, "   {} {}", paint(Role::Warning, "WARNING:"), paint(Role::Warning, why));
                }
            }
        }
        // Hiding a missing program would push a worse command to the top, so say it instead.
        if let Some(note) = catalog::missing_note(&s.line, catalog.iter().find(|c| c.name == s.command), &platforms) {
            let _ = writeln!(out, "   {} {}", paint(Role::Note, "NOTE:"), paint(Role::Note, &note));
        }
        for note in &s.notes {
            let _ = writeln!(out, "   {}", paint(Role::Note, note));
        }
    }
    out
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
        let text = String::from_utf8_lossy(&piped);
        if let Some(described) = outputs::describe(&text) {
            print!("{}\n{}\n", described.text, paint(Role::Dim, "(huh only read this text. Nothing was run.)"));
            return Ok(());
        }
        let output = session::tail(&session::clean(&text));
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
        print!("{}", style::answer(piece));
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

/// `huh tutor --side`: starts the tutor viewer beside this terminal (see launch.rs).
fn side(quiet: bool, dry_run: bool) -> Result<()> {
    let dry_run = dry_run || std::env::var_os("HUH_DRY_RUN").is_some_and(|v| !v.is_empty() && v != "0");
    let tty = launch::stdin_tty().or_else(|| dry_run.then(|| "<this tty>".to_string()));
    let huh = std::env::current_exe()?;
    let path = platform::path_dirs();
    let have = |program: &str| !matches!(platform::locate_in(program, &path, &[]), platform::Found::Missing);
    let plan = launch::decide(&|name| std::env::var(name).ok(), &have, tty.as_deref(), &huh, quiet, std::process::id());
    let platforms = platform::detect();
    if dry_run {
        println!("{}", launch::describe(&plan, &platforms));
        return Ok(());
    }
    launch::start(plan, &platforms)
}

/// Chooses the theme and whether to colour, once, before anything is printed. The shell hooks and
/// `--first-line-only` (the Alt+h key) never read the theme or the config file, so a bad one cannot break them.
fn setup_style(cli: &Cli) -> Result<()> {
    use std::io::IsTerminal;
    if cli.first_line_only || matches!(cli.action, Some(Action::ShellInit { .. } | Action::RecordSink { .. })) {
        return Ok(());
    }
    let theme = style::resolve_theme(
        cli.theme.as_deref(),
        std::env::var("HUH_THEME").ok().as_deref(),
        mode::config_path().as_deref(),
    )?;
    let on = style::color_enabled(
        cli.color,
        std::io::stdout().is_terminal(),
        cli.first_line_only,
        std::env::var_os("NO_COLOR").is_some(),
        std::env::var("TERM").is_ok_and(|t| t == "dumb"),
    );
    style::init(style::Style::new(theme, on));
    Ok(())
}

/// The one-time welcome (see welcome.rs). Does nothing unless this is an interactive first run.
fn first_run_welcome(first_line_only: bool) {
    if welcome::show(welcome::Trigger::FirstRun, first_line_only, false) == welcome::Shown::Interrupted {
        std::process::exit(130);
    }
}

fn main() -> Result<()> {
    use std::io::IsTerminal;
    #[cfg(unix)]
    exit_quietly_on_closed_pipe();
    let cli = Cli::parse();
    setup_style(&cli)?;
    let first_line_only = cli.first_line_only;

    // These do not need the catalog, and the shell ones run in every new terminal.
    match &cli.action {
        Some(Action::ShellInit { hooks }) => {
            print!("{}", if *hooks { session::hooks_script() } else { session::record_script()? });
            return Ok(());
        }
        Some(Action::RecordSink { file, pipe }) => return session::sink(file, pipe),
        Some(Action::Tutor { quiet, side: true, dry_run, .. }) => return side(*quiet, *dry_run),
        Some(Action::Themes) => {
            print!("{}", style::themes_text(style::enabled()));
            return Ok(());
        }
        Some(Action::Welcome) => {
            if welcome::show(welcome::Trigger::Welcome, false, true) == welcome::Shown::Interrupted {
                std::process::exit(130);
            }
            return Ok(());
        }
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
        Some(
            Action::Why
            | Action::ShellInit { .. }
            | Action::EvalErrors { .. }
            | Action::RecordSink { .. }
            | Action::Themes
            | Action::Welcome,
        ) => {
            unreachable!("handled above")
        }
        Some(Action::Tutor { quiet, tty, dir, hold, log, .. }) => {
            if let Some(dir) = dir {
                tutor::set_recording_dir(dir);
            }
            if let Some(log) = log {
                tutor::set_recording(log);
            }
            let mode = mode::resolve(
                cli.expert,
                cli.normal,
                std::env::var("HUH_MODE").ok().as_deref(),
                mode::config_path().as_deref(),
            )?;
            if let Err(error) = tutor::run(&everything, quiet, tty.as_deref(), mode) {
                if !hold {
                    return Err(error);
                }
                // A window opened by --side closes when the tutor ends: keep the message readable.
                eprintln!("huh tutor: {error:#}\n\nPress Enter to close this window.");
                let mut line = String::new();
                std::io::stdin().read_line(&mut line).ok();
                std::process::exit(1);
            }
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
        Some(Action::Find { words, describe }) => match describe {
            Some(name) => print!("{}", find::describe_here(&name, &everything)),
            None => find::run(&words, &everything)?,
        },
        Some(Action::Tools { words, no_desc }) => {
            print!("{}", tools::run(&words, &everything, no_desc));
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
            if from_stdin && let Some(described) = outputs::describe(&cmd_str) {
                print!("{}\n{}\n", described.text, paint(Role::Dim, "(huh only read this text. Nothing was run.)"));
                return Ok(());
            }
            if from_stdin && let Some((plausible, total)) = explain::looks_like_program_output(&cmd_str, &everything) {
                println!(
                    "This looks like the output of a program, not commands to run: only {plausible} of its {total} lines \
                     start with something that could be a command.\n\n\
                     `huh explain` explains command lines, e.g. `huh explain \"lsblk -f\"`. If this output shows a \
                     failure, pipe it into `huh why` instead.\n\n(Nothing was run.)"
                );
                return Ok(());
            }
            if !from_stdin {
                first_run_welcome(first_line_only);
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
            first_run_welcome(first_line_only);
            let interactive = !cli.no_ask
                && !cli.first_line_only
                && std::io::stdin().is_terminal()
                && std::io::stderr().is_terminal();
            let mut decide = |question: &str| if interactive { ask_on_terminal(question) } else { None };
            let mode = mode::resolve(
                cli.expert,
                cli.normal,
                std::env::var("HUH_MODE").ok().as_deref(),
                mode::config_path().as_deref(),
            )?;
            if mode.is_expert() {
                eprintln!("expert mode: fewer beginner safeguards; warnings are still shown.");
            }
            let outcome = suggest_in(mode, &LocalMatcher::new(&catalog), &request, files, &mut decide)?;
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
