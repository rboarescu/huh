//! Deterministic Error Doctor (`huh why`): explains why a command failed.
//!
//! No network calls are made here: errors are diagnosed with live
//! machine checks and structured diagnostic rules.

use crate::session::Last;
use anyhow::Result;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Standard shell exit codes and their plain-English meanings.
pub const EXIT_CODES: &[(i32, &str)] = &[
    (0, "succeeded"),
    (1, "general error"),
    (2, "incorrect command usage or syntax"),
    (126, "command found but not executable"),
    (127, "command not found"),
    (128, "fatal error or signal"),
    (130, "interrupted with Ctrl+C"),
    (137, "killed"),
    (143, "terminated"),
    (255, "connection or remote command error"),
];

pub fn exit_meaning(code: i32) -> Option<&'static str> {
    EXIT_CODES.iter().find(|(c, _)| *c == code).map(|(_, desc)| *desc)
}

/// Well-known commands used as a fallback typo dictionary alongside `$PATH`.
const COMMON_COMMANDS: &[&str] = &[
    "ls",
    "cd",
    "pwd",
    "cat",
    "less",
    "head",
    "tail",
    "grep",
    "find",
    "sort",
    "uniq",
    "wc",
    "cut",
    "diff",
    "cp",
    "mv",
    "rm",
    "mkdir",
    "touch",
    "chmod",
    "chown",
    "ln",
    "tar",
    "gzip",
    "gunzip",
    "zip",
    "unzip",
    "du",
    "df",
    "free",
    "ps",
    "top",
    "htop",
    "btop",
    "kill",
    "pkill",
    "pgrep",
    "ss",
    "ip",
    "ping",
    "curl",
    "wget",
    "ssh",
    "scp",
    "rsync",
    "git",
    "docker",
    "cargo",
    "rustc",
    "python",
    "python3",
    "pacman",
    "yay",
    "apt",
    "dnf",
    "systemctl",
    "journalctl",
    "sudo",
    "man",
    "which",
    "echo",
    "whoami",
    "uname",
    "uptime",
    "lsblk",
    "lscpu",
    "lsusb",
    "mount",
    "umount",
    "tmux",
    "vim",
    "nvim",
    "nano",
    "ollama",
    "tailscale",
    "virsh",
    "omarchy",
    "jq",
    "rg",
    "nmap",
    "dosbox",
    "flatpak",
    "mpv",
    "vlc",
    "ffmpeg",
    "xdg-open",
    "wine",
    "gio",
];

#[cfg(test)]
fn system_facts(os_release: &str, shell: &str) -> String {
    let name =
        os_release.lines().find_map(|line| line.strip_prefix("NAME=")).map_or("Linux", |name| name.trim_matches('"'));
    let platforms = crate::platform::expand(&crate::platform::ids_from(os_release));
    let mut facts = format!("{name}, shell {}.", shell.rsplit('/').next().unwrap_or_default());
    if let Some(install) = crate::platform::fact(&platforms, "pkg_install") {
        facts.push_str(&format!(" Packages are installed with `{}`.", crate::platform::fill(&install, "<package>")));
    }
    facts
}

/// Kept for prompt/context formatting helpers and tests.
#[cfg(test)]
fn system_line() -> String {
    system_facts(&crate::platform::read_os_release(), &std::env::var("SHELL").unwrap_or_default())
}

#[cfg(test)]
fn question(last: &Last) -> String {
    let exit = last.exit.map_or("unknown".to_string(), |code| match exit_meaning(code) {
        Some(meaning) => format!("{code} ({meaning})"),
        None => code.to_string(),
    });
    let command = if last.command.is_empty() { "(unknown)" } else { &last.command };
    format!(
        "System: {}\nCommand: {command}\nExit code: {exit}\nLast lines of output:\n{}",
        system_line(),
        if last.output.trim().is_empty() { "(the command printed nothing)" } else { &last.output }
    )
}

/// The rest of a line after `Try:`, allowing for a list dash in front of it.
fn try_line(line: &str) -> Option<&str> {
    line.trim().trim_start_matches(['-', '*', ' ']).strip_prefix("Try:").map(str::trim)
}

#[cfg(test)]
fn complete_at(text: &str) -> Option<usize> {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        offset += line.len();
        if line.ends_with('\n') && try_line(line).is_some_and(|rest| !rest.is_empty()) {
            return Some(offset - 1);
        }
    }
    None
}

pub struct Answer {
    pub text: String,
    pub total: Duration,
}

/// Diagnoses the failed (or interrupted/succeeded) command deterministically and hands
/// the explanation text to `show`.
pub fn explain(last: &Last, show: &mut dyn FnMut(&str)) -> Result<Answer> {
    let started = Instant::now();
    let text = diagnose(last);
    show(&text);
    Ok(Answer { text, total: started.elapsed() })
}

/// The command on the answer's `Try:` line, if one was given.
pub fn suggested(answer: &str) -> Option<&str> {
    let command = answer.lines().rev().find_map(try_line)?.trim_matches('`');
    (!command.is_empty() && !command.eq_ignore_ascii_case("nothing")).then_some(command)
}

// ---------------------------------------------------------------------------
// Deterministic Error Doctor
// ---------------------------------------------------------------------------

fn diagnose(last: &Last) -> String {
    // curl's progress meter ends without a newline, so its error lands mid-line ("…0curl: (6) …").
    let unglued;
    let last = match unglue_curl(&last.output) {
        Some(output) => {
            unglued = Last { command: last.command.clone(), exit: last.exit, output };
            &unglued
        }
        None => last,
    };
    let tar_resolved;
    let last = match resolve_tar_directory(last) {
        Some(resolved) => {
            tar_resolved = resolved;
            &tar_resolved
        }
        None => last,
    };
    if last.exit == Some(130) {
        return format_answer(
            &format!(
                "The command{} was interrupted with Ctrl+C (exit code 130: {}).",
                cmd_label(&last.command),
                exit_meaning(130).unwrap_or("interrupted")
            ),
            "nothing",
        );
    }
    if last.exit == Some(0) {
        return diagnose_exit_zero(last);
    }

    if let Some(ans) = diagnose_round4(last) {
        return ans;
    }
    if let Some(ans) = diagnose_round3(last) {
        return ans;
    }
    if let Some(ans) = diagnose_measured(last) {
        return ans;
    }

    // Without an exit code, only output with the shape of an error message is treated as a
    // failure; a sentence that merely contains "error" or "failed" is not one.
    if last.exit.is_none() && !last.output.trim().is_empty() && !has_error_shaped_line(&last.output) {
        return diagnose_unrecognised(last);
    }

    if let Some(ans) = diagnose_docker(last) {
        return ans;
    }
    if let Some(ans) = diagnose_npm_global_permissions(last) {
        return ans;
    }
    if let Some(ans) = diagnose_service_daemons(last) {
        return ans;
    }
    if let Some(ans) = diagnose_appimage_and_exec_format(last) {
        return ans;
    }
    if let Some(ans) = diagnose_not_executable(last) {
        return ans;
    }
    if let Some(ans) = diagnose_command_not_found(last) {
        return ans;
    }
    if let Some(ans) = diagnose_pacman(last) {
        return ans;
    }
    if let Some(ans) = diagnose_git(last) {
        return ans;
    }
    if let Some(ans) = diagnose_network(last) {
        return ans;
    }
    if let Some(ans) = diagnose_several_files(last) {
        return ans;
    }
    if let Some(ans) = diagnose_filesystem(last) {
        return ans;
    }
    if let Some(ans) = diagnose_permissions(last) {
        return ans;
    }
    if let Some(ans) = diagnose_tools_and_code(last) {
        return ans;
    }
    if let Some(ans) = diagnose_crashes(last) {
        return ans;
    }
    diagnose_fallback(last)
}

/// The explanation, then a `Try:` line with the command to run. "nothing" means there is no
/// command to suggest, and then there is no `Try:` line either.
/// Puts a `curl: (N)` error that follows a progress meter on a line of its own.
fn unglue_curl(output: &str) -> Option<String> {
    let glued = output.lines().any(|l| l.find("curl: (").is_some_and(|at| at > 0));
    glued.then(|| {
        output
            .lines()
            .map(|l| match l.find("curl: (") {
                Some(at) if at > 0 => format!("{}\n{}", &l[..at], &l[at..]),
                _ => l.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// `npm install -g` into a system folder fails with EACCES. The common advice, `sudo npm
/// install -g`, runs every package's install scripts as root and leaves root-owned files in
/// the npm cache, so point at a per-user prefix instead.
fn diagnose_npm_global_permissions(last: &Last) -> Option<String> {
    let lines = || last.output.lines().filter(|l| l.to_lowercase().contains("eacces"));
    let line = lines().find(|l| l.to_lowercase().contains("permission denied")).or_else(|| lines().next())?;
    let lower = last.output.to_lowercase();
    let is_npm =
        lower.contains("npm err") || lower.contains("npm error") || last.command.trim_start().starts_with("npm");
    if !is_npm || !lower.contains("node_modules") || !(lower.contains("/usr/") || lower.contains("/opt/")) {
        return None;
    }
    Some(format_answer(
        &format!(
            "npm tried to install a global package into a system folder that only root can write to (`{}`). \
             Do not rerun it with sudo: that runs the package's install scripts as root. \
             Give npm a folder of your own instead, add `~/.local/bin` to your PATH, then install again \
             (or run the tool once with `npx`).",
            line.trim()
        ),
        "npm config set prefix ~/.local",
    ))
}

fn format_answer(explanation: &str, try_cmd: &str) -> String {
    if try_cmd.trim().is_empty() || try_cmd.trim().eq_ignore_ascii_case("nothing") {
        explanation.to_string()
    } else {
        format!("{explanation}\nTry: {try_cmd}{}", risk_notes(try_cmd))
    }
}

/// `\nNote (category): ...` lines for a `Try:` command that is itself risky, so the person knows
/// what they are about to run. Uses the same analysis and categories as `huh explain`; plain
/// package installs, `sudo` itself and long-running commands are left out, since the Try line is
/// what the person asked to fix. At most two notes, one per category.
fn risk_notes(try_cmd: &str) -> String {
    use crate::risk_kinds::Risk;
    let opts = crate::breakdown::Options { man: false, path: false };
    let explained = crate::breakdown::explain_line(&[], try_cmd, opts);
    let mut seen: Vec<Risk> = Vec::new();
    let mut out = String::new();
    for w in &explained.warnings {
        if matches!(w.risk, Risk::HeadsUp | Risk::BigSystemChange | Risk::RunsUnreadCode) || seen.contains(&w.risk) {
            continue;
        }
        seen.push(w.risk);
        let mut chars = w.text.chars();
        let text = chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default();
        out.push_str(&format!("\nNote ({}): {text}", w.risk.name()));
        if seen.len() == 2 {
            break;
        }
    }
    out
}

fn cmd_label(command: &str) -> String {
    let trimmed = command.trim();
    if trimmed.is_empty() { String::new() } else { format!(" `{trimmed}`") }
}

fn diagnose_exit_zero(last: &Last) -> String {
    let msg = if last.output.trim().is_empty() {
        format!("The command{} succeeded (exit code 0) and produced no output.", cmd_label(&last.command))
    } else {
        format!("The command{} succeeded (exit code 0) and did not report a failure.", cmd_label(&last.command))
    };
    format_answer(&msg, "nothing")
}

fn diagnose_docker(last: &Last) -> Option<String> {
    let lower = last.output.to_lowercase();
    if lower.contains("docker.sock") && lower.contains("permission denied") {
        // What the output says is one thing; what this computer's group file says is another,
        // so the second is labelled as a check made here (and only claimed when it ran).
        let checked = user_in_group_checked("docker");
        let base = "Permission to connect to the Docker daemon socket (/var/run/docker.sock: permission denied) was denied: only root and members of the docker group may use it.";
        let (explanation, fix) = match checked {
            Some(true) => (
                format!(
                    "{base} On this computer, your user is in the docker group, so this shell probably started before you were added: log in again, or open a shell with the group using `newgrp docker`."
                ),
                "newgrp docker",
            ),
            Some(false) => (
                format!(
                    "{base} On this computer, your user is not in the docker group; adding it fixes this (log in again afterwards). Know what that grants: members of the docker group can start a container that mounts the whole disk, which is the same as having root (rootless Docker avoids this)."
                ),
                "sudo usermod -aG docker $USER",
            ),
            None => (
                format!(
                    "{base} huh could not check which groups your user is in; if docker is not among them, adding your user to the docker group fixes this (log in again afterwards). Know what that grants: members of the docker group can start a container that mounts the whole disk, which is the same as having root (rootless Docker avoids this)."
                ),
                "sudo usermod -aG docker $USER",
            ),
        };
        return Some(format_answer(&explanation, fix));
    }
    if lower.contains("pull access denied") || lower.contains("repository does not exist") {
        let image = last
            .output
            .split("pull access denied for ")
            .nth(1)
            .and_then(|r| r.split([',', ' ']).next())
            .map(str::to_string)
            .or_else(|| {
                last.output
                    .split("Unable to find image '")
                    .nth(1)
                    .and_then(|r| r.split('\'').next())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "the image".to_string());
        let name = image.split(':').next().unwrap_or(&image).rsplit('/').next().unwrap_or(&image).to_string();
        return Some(format_answer(
            &format!(
                "Docker could not pull `{image}` (`pull access denied ... repository does not exist or may require 'docker login'`): either no image has that name or it is private. Check the spelling of the name and tag, or run `docker login` if the image is private."
            ),
            &format!("docker search {name}"),
        ));
    }
    if lower.contains("manifest unknown") || (lower.contains("manifest for") && lower.contains("not found")) {
        return Some(format_answer(
            "Docker found the repository but not that tag (`manifest unknown`): the image exists, but the tag you asked for does not. Check the tag spelling on the image's page.",
            "nothing",
        ));
    }
    if lower.contains("cannot connect to the docker daemon") {
        let sock_note = if Path::new("/var/run/docker.sock").exists() {
            "The socket /var/run/docker.sock exists, but the service is not answering."
        } else {
            "The Docker daemon service does not appear to be running."
        };
        return Some(format_answer(
            &format!("Docker failed with `Cannot connect to the Docker daemon`. {sock_note}"),
            "sudo systemctl start docker",
        ));
    }
    None
}

fn diagnose_service_daemons(last: &Last) -> Option<String> {
    let lower = last.output.to_lowercase();
    if lower.contains("could not connect to ollama server") {
        return Some(format_answer(
            "The Ollama client failed with `could not connect to ollama server` because the Ollama background server is not running.",
            "ollama serve",
        ));
    }
    if lower.contains("failed to connect to local tailscaled") {
        return Some(format_answer(
            "Tailscale could not connect to the local `tailscaled` daemon (`failed to connect to local tailscaled`), which means the service is not running.",
            "sudo systemctl start tailscaled",
        ));
    }
    if let Some(service) = extract_systemd_failed_service(&last.output) {
        return Some(format_answer(
            &format!(
                "Systemd failed to start `{service}` (`Job for {service} failed`). Check the service status and logs for the underlying cause."
            ),
            &format!("systemctl status {service}"),
        ));
    }
    if let Some(unit) = extract_systemd_missing_unit(&last.output) {
        let stem = unit.rsplit_once('.').map_or(unit.as_str(), |(s, _)| s);
        let user = if last.command.split_whitespace().any(|w| w == "--user") { " --user" } else { "" };
        let try_cmd = if stem.is_empty() {
            format!("systemctl{user} list-unit-files")
        } else {
            format!("systemctl{user} list-unit-files '*{stem}*'")
        };
        let said =
            if last.output.to_lowercase().contains("could not be found") { "could not be found" } else { "not found" };
        let scope = if user.is_empty() { "" } else { " among your user units" };
        return Some(format_answer(
            &format!(
                "Systemd could not find the unit `{unit}`{scope} (`Unit {unit} {said}`). The service may not be installed or may use a different unit name."
            ),
            &try_cmd,
        ));
    }
    None
}

fn is_context_noise(t: &str) -> bool {
    t.is_empty()
        || is_long_listing_line(t)
        || is_table_row(t)
        || is_prose(t)
        || is_code_line(t)
        || is_commit_line(t)
        || is_log_line(t)
}

fn is_non_error_context_line(line: &str) -> bool {
    let t = line.trim();
    is_context_noise(t) || t.starts_with("- ") || t.starts_with("* ") || t.starts_with("# ") || t.starts_with('•')
}

fn quote_shell_arg(arg: &str) -> String {
    if arg.contains(char::is_whitespace) && !arg.starts_with(['\'', '"']) {
        format!("'{}'", arg.replace('\'', r"'\''"))
    } else {
        arg.to_string()
    }
}

/// First non-env/sudo wrapper command token in `command`, ignoring generic shells.
fn first_command_target(command: &str) -> Option<String> {
    let mut iter = command.split_whitespace().peekable();
    while let Some(tok) = iter.next() {
        if matches!(tok, "sudo" | "doas" | "env" | "command" | "time" | "nohup" | "exec") {
            if tok == "env" {
                while let Some(&next) = iter.peek() {
                    if matches!(next, "-u" | "--unset") {
                        iter.next();
                        iter.next();
                    } else if next.starts_with('-') {
                        iter.next();
                    } else {
                        break;
                    }
                }
            }
            continue;
        }
        if tok.contains('=') && !tok.starts_with('-') {
            continue;
        }
        let cleaned = tok.trim_matches(['\'', '"']);
        if cleaned.is_empty() || cleaned.starts_with('-') {
            continue;
        }
        if matches!(cleaned, "bash" | "-bash" | "sh" | "zsh" | "dash" | "fish") {
            return None;
        }
        return Some(cleaned.to_string());
    }
    None
}

fn has_appimage_ext(tok: &str) -> bool {
    let low = tok.to_lowercase();
    low.ends_with(".appimage") && low.len() > ".appimage".len() && !low.ends_with("/.appimage")
}

fn extract_appimage_target(last: &Last) -> String {
    let cmd = last.command.trim();
    if let Some(quoted) = extract_quoted(cmd, &['\'', '"'])
        && has_appimage_ext(&quoted)
    {
        return quoted;
    }
    for tok in cmd.split_whitespace() {
        let cleaned = tok.trim_matches(['\'', '"', '`']);
        if !cleaned.starts_with('-')
            && !(cleaned.contains('=') && !cleaned.starts_with(['.', '/']))
            && has_appimage_ext(cleaned)
        {
            return cleaned.to_string();
        }
    }
    for tok in last.output.split_whitespace() {
        let cleaned = tok.trim_matches(['\'', '"', '`', ':', ',', ';', '(', ')', '[', ']']);
        if !cleaned.starts_with('-') && !cleaned.contains("://") && has_appimage_ext(cleaned) {
            return cleaned.to_string();
        }
    }
    first_command_target(cmd).unwrap_or_else(|| "./app.AppImage".to_string())
}

fn extract_exec_format_target(last: &Last) -> Option<String> {
    for line in last.output.lines().filter(|l| !is_non_error_context_line(l)) {
        let t = line.trim();
        let low = t.to_lowercase();
        if low.starts_with("fish:")
            && let Some(path) = extract_quoted(t, &['\'']).filter(|p| !p.is_empty())
        {
            return Some(path);
        }
        if !low.contains("exec format error") && !low.contains("cannot execute binary file") {
            continue;
        }
        if low.starts_with("zsh:")
            && let Some(pos) = low.find("exec format error:")
        {
            let rest = t[pos + "exec format error:".len()..].trim().trim_matches(['\'', '"', '`']);
            if !rest.is_empty() && !rest.to_lowercase().contains("cannot execute") {
                return Some(rest.to_string());
            }
        }
        for part in t.split(':').map(str::trim) {
            let p_low = part.to_lowercase();
            let is_shell = matches!(
                p_low.as_str(),
                "bash"
                    | "-bash"
                    | "sh"
                    | "-sh"
                    | "zsh"
                    | "-zsh"
                    | "dash"
                    | "fish"
                    | "ash"
                    | "/bin/sh"
                    | "/bin/bash"
                    | "/usr/bin/bash"
                    | "/usr/bin/sh"
                    | "exec"
            );
            let is_err_phrase = p_low.contains("cannot execute")
                || p_low.contains("exec format error")
                || p_low.contains("binary file")
                || p_low.contains("permission denied");
            if !part.is_empty()
                && !part.chars().all(|c| c.is_ascii_digit())
                && !is_line_prefix(part)
                && !is_shell
                && !is_err_phrase
            {
                return Some(part.trim_matches(['\'', '"', '`']).to_string());
            }
        }
    }
    first_command_target(&last.command)
}

/// Diagnoses AppImage FUSE 2 loader failures and `Exec format error` (Windows/DOS binaries or
/// foreign-architecture/malformed executables) before `diagnose_not_executable` treats exit 126
/// as a missing `chmod +x`.
fn diagnose_appimage_and_exec_format(last: &Last) -> Option<String> {
    let relevant_lower = last
        .output
        .lines()
        .filter(|l| !is_non_error_context_line(l))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();

    if relevant_lower.contains("libfuse.so.2")
        || relevant_lower.contains("appimages require fuse")
        || relevant_lower.contains("cannot mount appimage")
        || (relevant_lower.contains("fusermount") && relevant_lower.contains("appimage"))
    {
        let target = extract_appimage_target(last);
        let quoted = quote_shell_arg(&target);
        let try_cmd = if quoted.contains("--appimage-extract-and-run") {
            quoted
        } else {
            format!("{quoted} --appimage-extract-and-run")
        };
        let which = if target == "./app.AppImage" { "this AppImage".to_string() } else { format!("`{target}`") };
        return Some(format_answer(
            &format!(
                "An AppImage bundles an app into one self-contained file and normally mounts itself using FUSE 2 (`libfuse.so.2` / `libfuse2`), which is not installed or available here, so {which} could not mount. You can extract and run it right away with `--appimage-extract-and-run`, or install `fuse2` / `libfuse2` via your package manager so AppImages launch directly next time."
            ),
            &try_cmd,
        ));
    }

    if relevant_lower.contains("exec format error") {
        let target = extract_exec_format_target(last);
        let subject = target.as_deref().map_or_else(|| "This file".to_string(), |t| format!("`{t}`"));
        let is_dos_or_windows = target.as_deref().is_some_and(|t| {
            let low = t.to_lowercase();
            low.ends_with(".exe") || low.ends_with(".com") || low.ends_with(".bat")
        });
        if is_dos_or_windows {
            let try_cmd = match &target {
                Some(t) if !t.is_empty() => format!("dosbox {}", quote_shell_arg(t)),
                _ => "dosbox".to_string(),
            };
            return Some(format_answer(
                &format!(
                    "{subject} is a Windows or classic MS-DOS executable, which Linux cannot run directly in bash (`Exec format error`). Classic DOS games and programs run inside `dosbox`, while modern Windows programs and games use `wine` or Steam Proton."
                ),
                &try_cmd,
            ));
        }
        let try_target = target.as_deref().map(quote_shell_arg).unwrap_or_else(|| "<file>".to_string());
        return Some(format_answer(
            &format!(
                "{subject} is not runnable on this CPU or OS (`Exec format error`) — for example, it may be an ARM binary on an x86_64 PC, or a damaged or non-Linux binary without a `#!` header."
            ),
            &format!("file {try_target}"),
        ));
    }

    None
}

fn diagnose_not_executable(last: &Last) -> Option<String> {
    let lower = last.output.to_lowercase();
    let trimmed_cmd = last.command.trim_start();
    let shell_exec_denied = lower.contains("permission denied")
        && last.output.lines().any(|l| {
            let t = l.trim().to_lowercase();
            (t.starts_with("bash:") || t.starts_with("sh:") || t.starts_with("zsh:"))
                && t.contains("permission denied")
                && (t.contains("./") || t.contains(": /"))
        });
    // Only the program being run counts: `cat /etc/x` failing to read a file is not an execute problem.
    let (_, prog, _) = extract_command_parts(&last.command);
    let runs_a_path = prog.contains('/')
        && last.output.lines().any(|l| {
            let low = l.to_lowercase();
            low.contains("permission denied") && low.contains(&prog.to_lowercase())
        });
    let is_exec_denied = last.exit == Some(126)
        || (lower.contains("permission denied")
            && if trimmed_cmd.is_empty() { shell_exec_denied } else { runs_a_path });
    if !is_exec_denied {
        return None;
    }

    let target = extract_exec_target(last);
    // No command to go by: `bash: /etc/foo: Permission denied` is what both a refused write
    // (`echo x > /etc/foo`) and a refused run print, and `chmod +x` on a system file is wrong
    // for the first. Say so instead of guessing.
    if trimmed_cmd.is_empty() && last.exit != Some(126) && target.starts_with('/') && under_system_dir(&target) {
        return Some(format_answer(
            &format!(
                "The shell was refused access to `{target}` (`Permission denied`). The output does not say what the command was doing to it (replacing it, appending to it, reading it or starting it) and huh was not given the command, so it cannot name an exact fix. \
                 A path under /etc, /var, /usr and the like can only be changed by an administrator: run the same command again with sudo (when it is a `>` or `>>` redirect, the redirect itself is not covered by sudo; pipe the text into `sudo tee -a <file>` to append, or `sudo tee <file>`, which replaces the whole file)."
            ),
            "nothing",
        ));
    }
    let stat_note = if let Ok(meta) = Path::new(&target).metadata() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o111 == 0 {
                format!(" The file `{target}` exists on disk (mode {mode:04o}) but lacks executable permission.")
            } else {
                format!(" The file `{target}` has mode {mode:04o} on disk, though execution was still denied.")
            }
        }
        #[cfg(not(unix))]
        {
            let _ = meta;
            String::new()
        }
    } else {
        String::new()
    };

    let detail = if last.exit == Some(126) { "Permission denied, exit code 126" } else { "Permission denied" };
    Some(format_answer(
        &format!("The file `{target}` was found but is not executable ({detail}).{stat_note}"),
        &format!("chmod +x {target}"),
    ))
}

/// The command line with `missing` replaced by `correction`: the whole word in command
/// position (start, or after `sudo`, `&&`, `;`, `|`), else the first whole word.
fn fix_command(command: &str, missing: &str, correction: &str) -> String {
    let boundary = |c: Option<char>| c.is_none_or(|c| c.is_whitespace() || ";&|()".contains(c));
    let whole_words: Vec<usize> = command
        .match_indices(missing)
        .map(|(at, _)| at)
        .filter(|&at| {
            boundary(command[..at].chars().next_back()) && boundary(command[at + missing.len()..].chars().next())
        })
        .collect();
    let in_command_position = |at: &usize| {
        let before = command[..*at].trim_end();
        before.is_empty()
            || before.ends_with(['&', ';', '|', '('])
            || before.rsplit(char::is_whitespace).next().is_some_and(|word| matches!(word, "sudo" | "env" | "time"))
    };
    match whole_words.iter().find(|at| in_command_position(at)).or(whole_words.first()) {
        Some(&at) => format!("{}{correction}{}", &command[..at], &command[at + missing.len()..]),
        None => correction.to_string(),
    }
}

fn diagnose_command_not_found(last: &Last) -> Option<String> {
    use crate::platform::{self, Found};
    let lower = last.output.to_lowercase();
    let from_output = missing_from_output(&last.output);
    if last.exit != Some(127) && from_output.is_none() && !lower.contains("fish: unknown command:") {
        return None;
    }
    // Only say "exit code 127" when it was given (a piped `command not found` has no code).
    let code = if last.exit == Some(127) { ", exit code 127" } else { "" };
    let code_paren = if last.exit == Some(127) { " (exit code 127)" } else { "" };

    let (_, cmd, _) = extract_command_parts(&last.command);
    let missing = from_output.unwrap_or_else(|| cmd.to_string());
    if missing.is_empty() {
        return Some(format_answer(&format!("The command was not found{code_paren}."), "nothing"));
    }
    let intro = format!("The command `{missing}` is not installed (`command not found`{code})");

    // The program is here after all: do not tell the user to install it.
    match platform::locate(&missing) {
        Found::OnPath(file) => {
            return Some(format_answer(
                &format!(
                    "The shell reported `{missing}` as not found, but it is installed at `{}` on this machine, \
                     so the failure is elsewhere: the shell that ran it probably had a different PATH, or a script \
                     it ran looks for another program or has a typo in its arguments.",
                    file.display()
                ),
                "nothing",
            ));
        }
        Found::SbinOnly(file) => {
            let dir = file.parent().map_or(String::new(), |d| d.display().to_string());
            let run = fix_command(&last.command, &missing, &file.display().to_string());
            let run = if last.command.trim().is_empty() { format!("sudo {missing}") } else { run };
            return Some(format_answer(
                &format!("`{missing}` is installed in `{dir}`, which is not on your PATH (`command not found`{code})."),
                &run,
            ));
        }
        Found::Missing => {}
    }

    // A file of that name in the current folder: the shell does not look there.
    if !missing.contains('/') && Path::new(&missing).is_file() {
        let runnable = platform::is_executable_file(Path::new(&missing));
        let fix = if runnable { format!("./{missing}") } else { format!("chmod +x {missing} && ./{missing}") };
        let note = if runnable { "" } else { " It is not marked executable yet." };
        return Some(format_answer(
            &format!(
                "The shell does not look in the current folder for programs: `{missing}` is a file in this folder, so run it by its path (`command not found`{code}).{note}"
            ),
            &fix,
        ));
    }

    let platforms = platform::detect();
    let advice = platform::install_advice(&missing, None, &platforms);
    // A name that is a known program, a package manager tool or a common command is a missing
    // program, never a typo; so is a file name such as `run.sh` (not a command at all).
    let known = platform::is_known_program(&missing)
        || platform::is_system_tool(&missing)
        || COMMON_COMMANDS.contains(&missing.as_str())
        || common_package_command(&missing).is_some();
    let looks_like_file = missing.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty() && (1..=4).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric())
    });
    // Only an installed command can be what was meant.
    let typo = |max_dist: usize| {
        if known || looks_like_file || missing.chars().count() < 2 || (max_dist > 1 && missing.chars().count() < 3) {
            return None;
        }
        closest_typo(&missing, &scan_path_executables(), max_dist)
    };
    let typo_answer = |correction: String| {
        format_answer(
            &format!("The command `{missing}` was not found{code_paren}, which looks like a typo for `{correction}`."),
            fix_command(&last.command, &missing, &correction).trim(),
        )
    };

    if let Some(correction) = typo(1) {
        return Some(typo_answer(correction));
    }
    // One edit from a very common command that is not installed here: say so rather than
    // suggest running something that would fail the same way.
    if !known && !looks_like_file && missing.chars().count() >= 3 {
        let common: Vec<String> = COMMON_COMMANDS.iter().map(|c| (*c).to_string()).collect();
        if let Some(correction) = closest_typo(&missing, &common, 1) {
            let install = install_command(&correction).or_else(|| common_package_command(&correction));
            return Some(format_answer(
                &format!(
                    "The command `{missing}` was not found{code_paren}, which looks like a typo for `{correction}`, but `{correction}` is not installed on this machine either."
                ),
                install.as_deref().unwrap_or("nothing"),
            ));
        }
    }
    let advice = match (advice, common_package_command(&missing)) {
        (Some(a), _) if a.installs => Some(a),
        (_, Some(command)) => Some(platform::Advice { command, installs: true, package: None, note: None }),
        (other, None) => other,
    };
    if let Some(advice) = advice.as_ref().filter(|a| a.installs) {
        let package = advice.package.as_deref().unwrap_or(&missing);
        let mut note = if package == missing {
            format!("{intro}.")
        } else {
            format!("{intro}; it is provided by the `{package}` package.")
        };
        if let Some(remark) = &advice.note {
            note.push(' ');
            note.push_str(remark);
        }
        return Some(format_answer(&note, &advice.command));
    }
    if let Some(correction) = typo(2) {
        return Some(typo_answer(correction));
    }
    if let Some(advice) = advice {
        return Some(format_answer(
            &format!(
                "The command `{missing}` is not installed or not in your PATH (`command not found`{code}). \
                 This system's package manager can tell you which package provides it."
            ),
            &advice.command,
        ));
    }
    Some(format_answer(&format!("The command `{missing}` was not found{code_paren}."), "nothing"))
}

fn diagnose_pacman(last: &Last) -> Option<String> {
    let lower = last.output.to_lowercase();
    if lower.contains("you cannot perform this operation unless you are root") {
        let command = last.command.trim();
        if command.is_empty() {
            return Some(format_answer(
                "Pacman changes the system's packages, so it must run as root (`you cannot perform this operation unless you are root`); run the same pacman command with sudo in front of it.",
                "nothing",
            ));
        }
        let command = command.strip_prefix("sudo ").unwrap_or(command);
        return Some(format_answer(
            "Pacman changes the system's packages, so it must run as root (`you cannot perform this operation unless you are root`); run it with sudo.",
            &format!("sudo {command}"),
        ));
    }
    if lower.contains("unable to lock database") || lower.contains("db.lck") {
        let lock_note = if Path::new("/var/lib/pacman/db.lck").exists() {
            " The lock file `/var/lib/pacman/db.lck` currently exists on disk."
        } else {
            ""
        };
        return Some(format_answer(
            &format!(
                "Pacman could not lock the package database because `/var/lib/pacman/db.lck` already exists (`unable to lock database`).{lock_note} Check whether another package manager is running before removing the lock file."
            ),
            "pgrep -l pacman",
        ));
    }
    if lower.contains("failed retrieving file") && (lower.contains("404") || lower.contains("not found")) {
        return Some(format_answer(
            "Pacman failed to download the package because the mirror returned 404, meaning your local package database is out of date.",
            "sudo pacman -Syu",
        ));
    }
    if let Some(pkg) = extract_pacman_missing_target(&last.output) {
        return Some(format_answer(
            &format!(
                "Pacman could not find package `{pkg}` (`target not found: {pkg}`) in the official repositories. Check the package spelling with `pacman -Ss {pkg}` or search the AUR with `yay -Ss {pkg}`."
            ),
            &format!("pacman -Ss {pkg}"),
        ));
    }
    if lower.contains("exists in filesystem") || lower.contains("conflicting files") {
        if let Some(conflict_path) = extract_pacman_conflict_path(&last.output) {
            return Some(format_answer(
                &format!(
                    "Pacman aborted because `{conflict_path}` already exists in the filesystem (`exists in filesystem` / `conflicting files`). Check which package owns it with `pacman -Qo {conflict_path}`, or update the system with `sudo pacman -Syu`."
                ),
                &format!("pacman -Qo {conflict_path}"),
            ));
        }
        return Some(format_answer(
            "Pacman aborted due to conflicting files (`exists in filesystem` / `conflicting files`). Update your system first or inspect file ownership with `pacman -Qo <path>`.",
            "sudo pacman -Syu",
        ));
    }
    None
}

fn diagnose_git(last: &Last) -> Option<String> {
    let lower = last.output.to_lowercase();
    if lower.contains("[rejected]") && (lower.contains("fetch first") || lower.contains("non-fast-forward")) {
        let why = if lower.contains("non-fast-forward") { "non-fast-forward" } else { "fetch first" };
        return Some(format_answer(
            &format!(
                "Git rejected the push (`[rejected] ... ({why})`) because the remote branch has commits you do not have locally yet. Pull them in (and fix any conflict) before pushing again; force-pushing instead would throw those commits away."
            ),
            "git pull",
        ));
    }
    if lower.contains("has no upstream branch") {
        let branch = extract_git_branch(&last.output).unwrap_or_else(|| "HEAD".to_string());
        return Some(format_answer(
            &format!(
                "Git cannot push because the current branch `{branch}` has no upstream tracking branch configured on the remote."
            ),
            &format!("git push --set-upstream origin {branch}"),
        ));
    }
    if lower.contains("author identity unknown") {
        return Some(format_answer(
            "Git cannot commit because your author identity (name and email) is not configured yet (`Author identity unknown`).",
            "git config --global user.email \"you@example.com\"",
        ));
    }
    if lower.contains("local changes") && lower.contains("would be overwritten by") {
        return Some(format_answer(
            "Git aborted switching branches because your uncommitted local changes would be overwritten by checkout.",
            "git stash",
        ));
    }
    diagnose_git_more(last)
}

fn diagnose_network(last: &Last) -> Option<String> {
    let lower = last.output.to_lowercase();
    if let Some((host, said)) = unresolved_host(&last.output) {
        let temporary = said.to_lowercase().contains("temporary failure");
        return Some(match host {
            Some(host) if temporary => format_answer(
                &format!(
                    "The DNS lookup for `{host}` failed (`{said}`): the DNS server could not be reached, so the network connection or DNS settings are the likely problem rather than the name."
                ),
                "ping -c 4 1.1.1.1",
            ),
            Some(host) => format_answer(
                &format!(
                    "The host name `{host}` could not be looked up (`{said}`): either the name is misspelled, or DNS or the network connection is down."
                ),
                &format!("getent hosts {host}"),
            ),
            None if temporary => format_answer(
                "A host name could not be looked up (`Temporary failure in name resolution`): DNS or the network connection is down.",
                "ping -c 4 1.1.1.1",
            ),
            None => format_answer(
                &format!(
                    "A host name could not be looked up (`{said}`): the name is misspelled, or DNS or the network is down."
                ),
                "nothing",
            ),
        });
    }
    if lower.contains("permission denied (publickey)") {
        let host = extract_ssh_target(&last.command, &last.output).unwrap_or_else(|| "<host>".to_string());
        if is_git_ssh_target(&host, &last.command) {
            return Some(format_answer(
                &format!(
                    "SSH key authentication to the Git host `{host}` failed (`Permission denied (publickey)`). Git hosts require an SSH public key registered on your account (do not use `ssh-copy-id`); verify your key and SSH agent with `ssh -T {host}`."
                ),
                &format!("ssh -T {host}"),
            ));
        }
        return Some(format_answer(
            &format!(
                "SSH key authentication failed (`Permission denied (publickey)`) because the remote host `{host}` did not accept your SSH key."
            ),
            &format!("ssh-copy-id {host}"),
        ));
    }
    if (lower.contains("connection refused")
        || lower.contains("econnrefused")
        || lower.contains("couldn't connect to server"))
        && let Some((host, port)) = refused_target(&last.command, &last.output)
    {
        let host_text = host.clone().unwrap_or_else(|| "that host".to_string());
        let on_port = port.as_ref().map_or(String::new(), |p| format!(" on port {p}"));
        let local = host
            .as_deref()
            .is_none_or(|h| h == "localhost" || h == "::1" || h == "[::1]" || h == "0.0.0.0" || h.starts_with("127."));
        let is_ssh = last.command.trim_start().starts_with("ssh") || lower.contains("ssh:");
        let who = if is_ssh { "an SSH server" } else { "a server" };
        let cause = if local {
            format!("nothing is listening{on_port} on this machine, so {who} is not running there or uses another port")
        } else {
            format!(
                "nothing is listening{on_port} at `{host_text}`: {who} is not running there, uses another port, or a firewall rejects the connection"
            )
        };
        let fix = match (&port, local) {
            (Some(p), true) => format!("ss -ltn 'sport = :{p}'"),
            (Some(p), false) => format!("nc -zv {host_text} {p}"),
            (None, _) => "ss -ltn".to_string(),
        };
        return Some(format_answer(
            &format!("The connection to `{host_text}`{on_port} was refused (`Connection refused`): {cause}."),
            &fix,
        ));
    }
    if lower.contains("ssl certificate problem") && lower.contains("self-signed") {
        let url = last
            .command
            .split_whitespace()
            .find(|tok| tok.starts_with("http://") || tok.starts_with("https://"))
            .unwrap_or("<url>");
        return Some(format_answer(
            "Curl could not verify the server's SSL certificate because it is self-signed and not trusted by the system.",
            &format!("curl -k {url}"),
        ));
    }
    None
}

fn diagnose_filesystem(last: &Last) -> Option<String> {
    let lower = last.output.to_lowercase();
    if lower.contains("mount point does not exist") {
        let mount_path = extract_mountpoint(&last.output).unwrap_or_else(|| "<mountpoint>".to_string());
        return Some(format_answer(
            &format!("The mount failed because the mount point folder `{mount_path}` does not exist yet."),
            &format!("sudo mkdir -p {mount_path}"),
        ));
    }
    if lower.contains("target is busy") {
        let mount_path = extract_mountpoint(&last.output).unwrap_or_else(|| "<mountpoint>".to_string());
        return Some(format_answer(
            &format!(
                "Cannot unmount `{mount_path}` because the target is busy: a process or shell still has files open there."
            ),
            &format!("lsof {mount_path}"),
        ));
    }
    if lower.contains("no space left on device") {
        return Some(format_answer(
            "Writing failed with `No space left on device` because the destination disk or partition is full.",
            "df -h",
        ));
    }
    if lower.contains("read-only file system") {
        return Some(format_answer(
            "The operation failed with `Read-only file system` because the filesystem is currently mounted read-only.",
            "findmnt",
        ));
    }
    if lower.contains("npm error code enoent") || (lower.contains("enoent") && lower.contains("package.json")) {
        return Some(format_answer(
            "npm failed with `ENOENT` because `package.json` does not exist in this folder.",
            "ls",
        ));
    }
    if lower.contains("could not find `cargo.toml`") || lower.contains("could not find cargo.toml") {
        return Some(format_answer(
            "Cargo could not find `Cargo.toml` in this directory or any parent directory, so this is not a Rust project folder.",
            "ls",
        ));
    }
    if lower.contains("no such file or directory") || lower.contains("cannot find or open") {
        let (_, prog, _) = extract_command_parts(&last.command);
        let is_cd = prog == "cd" || lower.contains("cd:");
        let missing_target = extract_missing_path(last);
        let cwd_hint = if !missing_target.is_empty() && !missing_target.contains('/') {
            let entries = scan_cwd_entries(is_cd);
            let max_dist = if missing_target.chars().count() <= 3 { 1 } else { 2 };
            closest_typo(&missing_target, &entries, max_dist)
                .map(|m| format!(" Closest match in this folder: `{m}`."))
                .unwrap_or_default()
        } else {
            String::new()
        };
        let subject = if missing_target.is_empty() {
            "The target file or directory".to_string()
        } else {
            format!("`{missing_target}`")
        };
        return Some(format_answer(
            &format!(
                "{subject} does not exist or cannot be opened (`No such file or directory` / `cannot find or open`).{cwd_hint}"
            ),
            "ls",
        ));
    }
    None
}

fn diagnose_permissions(last: &Last) -> Option<String> {
    let lower = last.output.to_lowercase();
    if lower.contains("cannot write to") && lower.contains("permission denied") {
        let file = extract_quoted(&last.output, &['\'', '"']).unwrap_or_else(|| "the destination file".to_string());
        return Some(format_answer(
            &format!(
                "Cannot write to `{file}` (`Permission denied`): the current folder or target file is not writable by your user."
            ),
            "ls -l",
        ));
    }
    if lower.contains("permission denied")
        || lower.contains("access denied")
        || lower.contains("operation not permitted")
        || lower.contains("must be root")
        || lower.contains("interactive authentication required")
    {
        let trimmed = last.command.trim();
        let evidence_line = last.output.lines().map(str::trim).find(|l| {
            let low = l.to_lowercase();
            ["permission denied", "access denied", "not permitted", "must be root", "authentication required"]
                .iter()
                .any(|p| low.contains(p))
        });
        let evidence = evidence_line.map(str::to_string).or_else(|| error_summary(&last.output)).unwrap_or_default();
        if trimmed.is_empty()
            && lower.contains("interactive authentication required")
            && let Some(systemctl_cmd) = extract_systemd_failed_action(&last.output)
        {
            return Some(format_answer(
                &format!("The command failed because root or PolicyKit authentication is required (`{evidence}`)."),
                &format!("sudo {systemctl_cmd}"),
            ));
        }
        if trimmed.starts_with("sudo ") {
            return Some(format_answer(
                &format!(
                    "Access was still denied even with `sudo` (`{evidence}`). Check file ownership, permissions, or mount options."
                ),
                "ls -l",
            ));
        }
        let denied = file_failures(&last.output).into_iter().find(|f| f.problem == FileProblem::Denied);
        if let Some(failure) = denied {
            let (sentence, fix) =
                permission_advice(&failure.path, failure.writing || writes_to_file(trimmed, &last.output), trimmed);
            return Some(format_answer(&format!("{sentence} (`{evidence}`)"), &fix));
        }
        let needs_root = lower.contains("operation not permitted")
            || lower.contains("must be root")
            || lower.contains("access denied")
            || lower.contains("authentication required");
        if !needs_root {
            return Some(format_answer(
                &format!(
                    "The command was refused with `{evidence}`, but the output does not say which file or resource. Check the owner and mode of what it touches; only if that is a system file does it need root."
                ),
                "nothing",
            ));
        }
        if trimmed.is_empty() || is_shell_builtin(trimmed) {
            return Some(format_answer(
                &format!("The command failed because it needs administrator privileges (`{evidence}`)."),
                "nothing",
            ));
        }
        return Some(format_answer(
            &format!(
                "The operation failed due to insufficient permissions (`{evidence}`); it probably needs administrator rights."
            ),
            &format!("sudo {trimmed}"),
        ));
    }
    None
}

/// Commands the shell runs itself, which `sudo` cannot run.
fn is_shell_builtin(command: &str) -> bool {
    let (_, prog, _) = extract_command_parts(command);
    matches!(prog, "cd" | "source" | "." | "export" | "alias" | "ulimit" | "umask" | "exec")
}

/// A shell redirect (`> file`) or a command that changes files, so a refused access was a write.
fn writes_to_file(command: &str, output: &str) -> bool {
    let (_, prog, _) = extract_command_parts(command);
    let first = output.lines().next().unwrap_or("").trim_start().to_lowercase();
    (command.contains('>') && (first.starts_with("bash:") || first.starts_with("sh:") || first.starts_with("zsh:")))
        || matches!(prog, "touch" | "mkdir" | "rm" | "rmdir" | "mv" | "tee" | "ln" | "truncate")
        || command.split('|').any(|stage| extract_command_parts(stage).1 == "tee")
}

fn diagnose_tools_and_code(last: &Last) -> Option<String> {
    let lower = last.output.to_lowercase();
    if lower.contains("address already in use") || lower.contains("failed to bind host port") {
        let port = extract_port(last);
        let port_msg = port.as_deref().map_or("The requested port".to_string(), |p| format!("Port {p}"));
        let try_cmd = port.as_deref().map_or("ss -tulpn".to_string(), |p| format!("ss -tulpn 'sport = :{p}'"));
        return Some(format_answer(
            &format!("{port_msg} is already in use by another running process (`Address already in use`)."),
            &try_cmd,
        ));
    }
    if let Some(ans) = diagnose_archives(last) {
        return Some(ans);
    }
    if lower.contains("externally-managed-environment") {
        return Some(format_answer(
            "pip refused to install into the system Python because it is externally managed (`externally-managed-environment`, PEP 668): the distribution's package manager owns those files. Use a virtual environment (`python3 -m venv .venv`, then `.venv/bin/pip install <package>`), `pipx install <app>` for command-line tools, or the distribution's own python package.",
            "python3 -m venv .venv",
        ));
    }
    if let Some(module) = extract_python_module(&last.output) {
        return Some(diagnose_python_module(&module));
    }
    if let Some(name) = extract_node_module(&last.output) {
        return Some(diagnose_node_module(&name));
    }
    if let Some(script) = last.output.split("Missing script: \"").nth(1).and_then(|r| r.split('"').next()) {
        return Some(format_answer(
            &format!(
                "npm has no script called `{script}` in this package.json (`Missing script: \"{script}\"`); `npm run` lists the scripts it does have."
            ),
            "npm run",
        ));
    }
    if let Some(ans) = diagnose_makefile(last) {
        return Some(ans);
    }
    if lower.contains("no such process") {
        return Some(format_answer(
            "That process ID does not exist (`No such process`); the process may have already exited or the PID was mistyped.",
            "ps aux",
        ));
    }
    if let Some(ans) = diagnose_python(last) {
        return Some(ans);
    }
    if let Some(err_line) = last.output.lines().find(|l| l.trim_start().starts_with("error[E")) {
        let detail = err_line.trim();
        let place = last
            .output
            .lines()
            .skip_while(|l| *l != err_line)
            .find_map(|l| l.trim_start().strip_prefix("--> "))
            .map(|p| format!(" at `{}`", p.trim()))
            .unwrap_or_default();
        return Some(format_answer(
            &format!(
                "The Rust compiler rejected the source code{place} (`{detail}`), so the code itself must be adjusted before building again. `rustc --explain` describes the error code."
            ),
            "nothing",
        ));
    }
    None
}

fn diagnose_fallback(last: &Last) -> String {
    let lower = last.output.to_lowercase();
    let (_, prog, _) = extract_command_parts(&last.command);
    if last.exit == Some(143) {
        return format_answer(
            &format!(
                "The process{} was terminated (exit code 143: {}).",
                cmd_label(&last.command),
                exit_meaning(143).unwrap_or("terminated")
            ),
            "nothing",
        );
    }
    if last.exit == Some(2)
        || lower.contains("unrecognized option")
        || lower.contains("invalid option")
        || lower.contains("unknown option")
        || lower.contains("unexpected argument")
        || lower.contains("missing operand")
    {
        let evidence = last
            .output
            .lines()
            .map(str::trim)
            .find(|l| {
                let low = l.to_lowercase();
                low.contains("option")
                    || low.contains("argument")
                    || low.contains("operand")
                    || low.contains("invalid")
                    || low.contains("unrecognized")
                    || low.contains("unknown")
            })
            .or_else(|| last_meaningful_line(&last.output))
            .unwrap_or("incorrect command usage or syntax");
        // Only offer `--help` for a program known to have failed: the first word of a pipeline
        // or a `&&` chain may be a different one.
        let failed_prog = last
            .output
            .lines()
            .find_map(|l| l.split_once(':').map(|(p, _)| p.trim()))
            .filter(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)));
        let single = !last.command.contains(['|', '&', ';']);
        let help_cmd = match failed_prog {
            Some(p) => format!("{p} --help"),
            None if single && !prog.is_empty() => format!("{prog} --help"),
            None => "nothing".to_string(),
        };
        return format_answer(
            &format!(
                "The command failed due to incorrect usage, flags, or syntax (exit code {}: `{evidence}`).",
                last.exit.unwrap_or(2)
            ),
            &help_cmd,
        );
    }

    let exit_desc = last.exit.map_or_else(
        || "an error".to_string(),
        |c| match exit_meaning(c) {
            Some(m) => format!("exit code {c} ({m})"),
            None => format!("exit code {c}"),
        },
    );
    if let Some(line) = error_summary(&last.output) {
        format_answer(
            &format!("The command{} failed with {exit_desc}: {}.", cmd_label(&last.command), quote_line(&line)),
            "nothing",
        )
    } else {
        format_answer(
            &format!("The command{} failed with {exit_desc} and printed no output.", cmd_label(&last.command)),
            "nothing",
        )
    }
}

// ---------------------------------------------------------------------------
// Which lines are errors, and what does successful output look like
// ---------------------------------------------------------------------------

/// A row of a table (`docker images`, `free`, `ps`): at least three cells separated by runs of
/// spaces. Words such as "failed" in a cell are data, not an error message.
fn is_table_row(line: &str) -> bool {
    let t = line.trim();
    let lower = t.to_lowercase();
    if lower.starts_with("error") || lower.starts_with("fatal") || lower.contains("traceback") {
        return false;
    }
    let mut gaps = 0;
    let mut spaces = 0;
    for c in t.chars() {
        if c == ' ' || c == '\t' {
            spaces += 1;
        } else {
            if spaces >= 2 {
                gaps += 1;
            }
            spaces = 0;
        }
    }
    gaps >= 3
}

/// A sentence that talks about errors (`Permission denied responses should be logged`) rather
/// than reporting one.
fn is_prose(line: &str) -> bool {
    let lower = line.to_lowercase();
    [" should ", " responses ", " would ", " will be ", " can be ", " per hour", " per day", " must be logged"]
        .iter()
        .any(|w| lower.contains(w))
        && !lower.starts_with("error")
        && !lower.starts_with("fatal")
        && !lower.contains(": ")
}

/// True for a line of `ls -l` output (`-rw-r--r-- 1 user group ...`).
fn is_long_listing_line(line: &str) -> bool {
    let Some(mode) = line.split_whitespace().next() else { return false };
    let chars: Vec<char> = mode.chars().collect();
    chars.len() >= 10
        && "-dlbcpsDC".contains(chars[0])
        && chars[1..10].iter().all(|c| "rwxsStT-".contains(*c))
        && chars[10..].iter().all(|c| matches!(c, '.' | '+' | '@'))
}

/// Does this line report a problem? Deliberately generous, because a wrong "this is not an
/// error" is worse than a wrong "this might be".
fn is_error_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() || is_long_listing_line(t) || is_table_row(t) || is_prose(t) {
        return false;
    }
    let mut lower = t.to_lowercase();
    // Test runners and build tools say "ok" or "0 failed" on success.
    if lower.ends_with("... ok") || lower.contains("a complete log of this run") {
        return false;
    }
    for benign in
        ["0 failed", "failed: 0", "0 failures", "0 errors", "no errors", "errors: 0", "0 error", "without errors"]
    {
        lower = lower.replace(benign, "");
    }
    const PATTERNS: &[&str] = &[
        "error:",
        "error[",
        "err!",
        "fatal",
        "traceback",
        "exception",
        "no such",
        "not found",
        "denied",
        "refused",
        "failed",
        "failure",
        "cannot",
        "can't ",
        "unable to",
        "undefined reference",
        "could not",
        "couldn't",
        "not permitted",
        "not allowed",
        "unrecognized",
        "unknown",
        "unexpected",
        "invalid",
        "illegal",
        "segmentation fault",
        "panicked",
        "killed",
        "abort",
        "conflict (",
        "merge conflict",
        "unmerged",
        "no stash",
        "did not match",
        "missing",
        "not in gzip",
        "not a zip",
        "corrupt",
        "already in use",
        "no space",
        "read-only file system",
        "busy",
        "timed out",
        "timeout",
        "unreachable",
        "unauthorized",
        "forbidden",
        "must be root",
        "authentication required",
        "not exist",
        "does not match",
        "rejected",
        "mismatch",
    ];
    lower.starts_with("error")
        || lower.contains("error ")
        || lower.ends_with("error")
        || PATTERNS.iter().any(|pattern| lower.contains(pattern))
}

/// The line that says what went wrong: the exception line of a Python traceback, else the
/// first line that looks like an error.
fn error_summary(output: &str) -> Option<String> {
    let lines: Vec<&str> = output.lines().collect();
    if lines.iter().any(|l| l.starts_with("Traceback (most recent call last)")) {
        let last = lines.iter().rev().map(|l| l.trim_end()).find(|l| !l.is_empty())?;
        if !last.starts_with(char::is_whitespace) {
            return Some(last.trim().to_string());
        }
    }
    let explicit = |l: &str| {
        let low = l.to_lowercase();
        low.starts_with("error") || low.starts_with("fatal") || low.starts_with("==> error") || low.starts_with("e: ")
    };
    let first = lines
        .iter()
        .map(|l| l.trim())
        .find(|l| explicit(l) && is_error_line(l))
        .or_else(|| {
            lines.iter().map(|l| l.trim()).find(|l| is_error_line(l) && !l.to_lowercase().starts_with("warning"))
        })
        .or_else(|| lines.iter().map(|l| l.trim()).find(|l| is_error_line(l)));
    // npm prints `npm error code EJSONPARSE` first and says what is wrong on a later line.
    if let Some(first) = first
        && first.starts_with("npm error code ")
        && let Some(detail) = lines.iter().map(|l| l.trim()).skip_while(|l| *l != first).skip(1).find(|l| {
            l.strip_prefix("npm error ").is_some_and(|rest| {
                !rest.is_empty()
                    && !["code ", "syscall ", "path ", "errno ", "dest ", "A complete log"]
                        .iter()
                        .any(|p| rest.starts_with(p))
            })
        })
    {
        return Some(format!("{first}: {}", detail.trim_start_matches("npm error ")));
    }
    first.or_else(|| last_meaningful_line(output)).map(str::to_string)
}

/// A line wrapped in backticks, or in quotes when it has backticks of its own.
fn quote_line(line: &str) -> String {
    let line: String =
        if line.chars().count() > 200 { line.chars().take(200).collect::<String>() + "..." } else { line.to_string() };
    if line.contains('`') { format!("\"{line}\"") } else { format!("`{line}`") }
}

/// What a piece of output without an error in it seems to be.
fn describe_output(output: &str) -> &'static str {
    let lower = output.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    if has(&["make: nothing to be done", "is up to date."]) && lower.starts_with("make:") {
        "make saying there is nothing to rebuild (the targets are already up to date)"
    } else if has(&["nothing to commit", "working tree clean", "on branch "]) {
        "the result of `git status`"
    } else if has(&["set up to track", "everything up-to-date", "everything up to date"])
        || (lower.contains("\nto ") && lower.contains(" -> "))
    {
        "a successful `git push`"
    } else if lower.starts_with("container id") {
        "the table printed by `docker ps`"
    } else if has(&["test result: ok"]) {
        "a passing test run"
    } else if has(&["finished `", "finished dev", "finished release"]) {
        "a finished `cargo` build"
    } else if has(&["installing ", "total installed size", "proceed with installation"]) {
        "the log of a package installation"
    } else if lower.starts_with("repository ") && lower.contains("image id") {
        "the table printed by `docker images`"
    } else if lower.contains("\nmem:") && lower.contains("swap:") {
        "the memory table printed by `free`"
    } else if output.lines().any(is_long_listing_line) || lower.trim_start().starts_with("total ") {
        "a directory listing"
    } else {
        "ordinary program output"
    }
}

fn diagnose_not_an_error(last: &Last) -> String {
    format_answer(
        &format!(
            "This output does not look like an error: it seems to be {}, and no line in it reports a failure. If the command did go wrong, run `huh why` right after it, or pipe in the lines that show the error.",
            describe_output(&last.output)
        ),
        "nothing",
    )
}

// ---------------------------------------------------------------------------
// Files that could not be read, written or found
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FileProblem {
    Denied,
    Missing,
}

struct FileFailure {
    path: String,
    problem: FileProblem,
    /// The line said the program was writing, creating or removing.
    writing: bool,
}

/// The path a tool's error line is about: the quoted part, or the last `:`-separated part
/// with any "cannot open" lead-in removed.
fn path_in(before: &str) -> Option<String> {
    for (open, close) in [('\'', '\''), ('‘', '’'), ('`', '\''), ('"', '"')] {
        if let Some(start) = before.find(open) {
            let rest = &before[start + open.len_utf8()..];
            if let Some(end) = rest.rfind(close) {
                let path = rest[..end].trim();
                if !path.is_empty() {
                    return Some(path.to_string());
                }
            }
        }
    }
    let mut segments = before.rsplit(": ");
    let mut piece = segments.next().unwrap_or(before).trim();
    // `tar: x.tar: Cannot open`: the lead-in is the last part and the file the one before it.
    if matches!(
        piece.to_lowercase().as_str(),
        "cannot open"
            | "cannot read"
            | "cannot stat"
            | "cannot access"
            | "cannot find"
            | "unable to open"
            | "could not open"
            | "can't open"
            | "can't read"
    ) && let Some(previous) = segments.next()
    {
        piece = previous.trim();
    }
    let piece = piece;
    let lower = piece.to_lowercase();
    let mut piece = piece;
    for lead in [
        "can't read ",
        "can't open ",
        "could not open file ",
        "could not open ",
        "cannot open file ",
        "cannot open ",
        "can't open file ",
        "cannot access ",
        "cannot stat ",
        "cannot find or open ",
        "cannot read ",
        "unable to open ",
    ] {
        if lower.starts_with(lead) {
            piece = piece[lead.len()..].trim();
            break;
        }
    }
    let piece = piece.trim_matches(['\'', '"', '`']);
    (!piece.is_empty()
        && !matches!(piece, "bash" | "sh" | "zsh" | "fish" | "dash")
        && !piece.contains(char::is_whitespace))
    .then(|| piece.to_string())
}

/// Every file named in a "Permission denied" or "No such file or directory" line, in order,
/// without repeats.
fn file_failures(output: &str) -> Vec<FileFailure> {
    let mut found: Vec<FileFailure> = Vec::new();
    for line in output.lines() {
        let t = line.trim();
        let lower = t.to_lowercase();
        let mut hit: Option<(FileProblem, String)> = None;
        if lower.starts_with("permissionerror") || lower.contains("[errno 13]") || lower.contains("[errno 2]") {
            let problem = if lower.contains("[errno 13]") || lower.starts_with("permissionerror") {
                FileProblem::Denied
            } else {
                FileProblem::Missing
            };
            if let Some(start) = t.find(": '").or_else(|| t.find(": \"")) {
                hit = path_in(&t[start + 1..]).map(|path| (problem, path));
            }
        } else if let Some(at) = lower.find("eacces: permission denied, ") {
            // Node: `EACCES: permission denied, mkdir '/usr/lib/node_modules/foo'`
            hit = path_in(&t[at..]).map(|path| (FileProblem::Denied, path));
        } else {
            for (needle, problem) in
                [(": permission denied", FileProblem::Denied), (": no such file or directory", FileProblem::Missing)]
            {
                if let Some(at) = lower.find(needle) {
                    let rest = lower[at + needle.len()..].trim().trim_matches('.');
                    if rest.is_empty() {
                        hit = path_in(&t[..at]).map(|path| (problem, path));
                        break;
                    }
                }
            }
        }
        // `useradd: Permission denied.` names the program that was refused, not a file.
        if let Some((_, path)) = &hit
            && t.split(':').next().is_some_and(|prefix| prefix.trim() == path)
            && !path.contains('/')
            && !Path::new(path).exists()
        {
            hit = None;
        }
        if let Some((problem, path)) = hit
            && !found.iter().any(|f| f.path == path)
        {
            let writing = [
                "cannot create",
                "cannot touch",
                "cannot remove",
                "cannot write",
                "cannot delete",
                "cannot move",
                "for writing",
                "cannot make",
                "cannot unlink",
            ]
            .iter()
            .any(|w| lower.contains(w))
                || lower.starts_with("tee:");
            found.push(FileFailure { path, problem, writing });
        }
    }
    found
}

#[cfg(unix)]
fn my_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").ok().map(|m| m.uid())
}

#[cfg(not(unix))]
fn my_uid() -> Option<u32> {
    None
}

fn under_system_dir(path: &str) -> bool {
    let absolute = if path.starts_with('/') {
        path.to_string()
    } else {
        match std::env::current_dir() {
            Ok(dir) => dir.join(path).display().to_string(),
            Err(_) => path.to_string(),
        }
    };
    ["/etc", "/var", "/usr", "/root", "/sys", "/proc", "/dev", "/boot"]
        .iter()
        .any(|dir| absolute == *dir || absolute.starts_with(&format!("{dir}/")))
}

/// Says whether a refused write appends to the file or replaces all of it, when the command
/// shows which. `>` and plain `tee` replace the whole file, so the fix does too.
fn write_mode_note(command: &str, path: &str) -> &'static str {
    let tee_stage = command.split('|').map(str::trim).find(|s| s.starts_with("tee ") || s.starts_with("sudo tee "));
    if let Some(stage) = tee_stage {
        return if stage.split_whitespace().any(|w| w == "-a" || w == "--append") {
            " `tee -a` appends, so the existing content is kept."
        } else {
            " `tee` without `-a` replaces the whole file, so the existing content is lost."
        };
    }
    let squeezed: String = command.split_whitespace().collect::<Vec<_>>().join("");
    if squeezed.contains(&format!(">>{path}")) {
        " The command appends (`>>`), so the fix below uses `tee -a` and keeps the existing content."
    } else if squeezed.contains(&format!(">{path}")) {
        " The command replaces the whole file (`>` overwrites it), and the fix below does too; to add a line to it, use `>>` instead."
    } else {
        ""
    }
}

/// What a refused access to `path` most likely means, from the file itself (read-only checks),
/// and the command that fixes it.
#[cfg(unix)]
fn permission_advice(path: &str, writing: bool, command: &str) -> (String, String) {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let sudo = {
        let trimmed = command.trim();
        if trimmed.is_empty() || trimmed.starts_with("sudo ") || is_shell_builtin(trimmed) {
            format!("ls -ld {path}")
        } else {
            sudo_form(trimmed, path)
        }
    };
    let me = my_uid();
    let action = if writing { "write to" } else { "read" };
    let system = under_system_dir(path);
    let (target, meta) = match std::fs::metadata(path) {
        Ok(meta) => (path.to_string(), Some(meta)),
        Err(_) => {
            // Not there (or not reachable): the folder it would live in decides.
            let parent = Path::new(path).parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
            (parent.display().to_string(), std::fs::metadata(parent).ok())
        }
    };
    let creating = target != path;
    let Some(meta) = meta else {
        return (
            format!(
                "Permission was denied for `{path}`; look at its owner and mode, and at those of the folders above it, to see why."
            ),
            format!("ls -ld {path}"),
        );
    };
    let what = if creating || meta.is_dir() { "folder" } else { "file" };
    if system {
        let trimmed = command.trim();
        let note = write_mode_note(trimmed, path);
        let fix = if trimmed.is_empty() { "nothing".to_string() } else { sudo };
        return (
            format!(
                "`{path}` is a system path (under /etc, /var, /usr and the like), which ordinary users cannot {action}, so administrator rights are needed.{note}"
            ),
            fix,
        );
    }
    if me.is_some() && meta.uid() != me.unwrap_or(0) && me != Some(0) {
        return (
            format!(
                "The {what} `{target}` belongs to another user (uid {}), so your user cannot {action} `{path}` there.",
                meta.uid()
            ),
            sudo,
        );
    }
    let mode = meta.permissions().mode() & 0o777;
    let wants_write = writing || creating;
    let (bit, letter, need) = if wants_write { (0o200, "u+w", "write") } else { (0o400, "u+r", "read") };
    if meta.is_dir() && mode & 0o100 == 0 {
        return (
            format!("`{target}` is your own folder, but its mode ({mode:04o}) does not let you enter it."),
            format!("chmod u+x {target}"),
        );
    }
    if mode & bit == 0 {
        let owner =
            if creating { format!("The folder `{target}` is yours") } else { format!("`{target}` is your own file") };
        let sentence = if creating {
            format!(
                "{owner}, but its mode ({mode:04o}) does not let you {need} in it, so `{path}` cannot be created or changed."
            )
        } else {
            format!("{owner}, but its mode ({mode:04o}) gives you no {need} permission.")
        };
        return (sentence, format!("chmod {letter} {target}"));
    }
    (
        format!(
            "`{target}` is yours and its mode ({mode:04o}) looks fine, so the refusal comes from somewhere else: a folder above it, an ACL, an immutable flag or a read-only mount."
        ),
        format!("ls -ld {path}"),
    )
}

#[cfg(not(unix))]
fn permission_advice(path: &str, _writing: bool, _command: &str) -> (String, String) {
    (format!("Permission was denied for `{path}`."), format!("ls -ld {path}"))
}

/// Several different files failed in one command: say what is wrong with each (up to three).
fn diagnose_several_files(last: &Last) -> Option<String> {
    let failures = file_failures(&last.output);
    if failures.len() < 2 || last.command.trim_start().starts_with("cd ") {
        return None;
    }
    let mut parts = Vec::new();
    let mut fix: Option<String> = None;
    for failure in failures.iter().take(3) {
        match failure.problem {
            FileProblem::Missing => {
                parts.push(format!("`{}` does not exist (`No such file or directory`).", failure.path))
            }
            FileProblem::Denied => {
                let (sentence, command) = permission_advice(&failure.path, failure.writing, &last.command);
                parts.push(format!("`{}`: permission denied. {sentence}", failure.path));
                fix.get_or_insert(command);
            }
        }
    }
    let more =
        if failures.len() > 3 { format!(" ({} more files failed too.)", failures.len() - 3) } else { String::new() };
    let fix = fix.unwrap_or_else(|| "ls".to_string());
    Some(format_answer(&format!("{} files failed in this command. {}{more}", failures.len(), parts.join(" ")), &fix))
}

// ---------------------------------------------------------------------------
// Python and node modules
// ---------------------------------------------------------------------------

/// Python import names whose pip package has another name.
const PIP_NAMES: &[(&str, &str)] = &[
    ("yaml", "pyyaml"),
    ("cv2", "opencv-python"),
    ("PIL", "pillow"),
    ("bs4", "beautifulsoup4"),
    ("sklearn", "scikit-learn"),
    ("skimage", "scikit-image"),
    ("dateutil", "python-dateutil"),
    ("dotenv", "python-dotenv"),
    ("jwt", "pyjwt"),
    ("serial", "pyserial"),
    ("usb", "pyusb"),
    ("OpenSSL", "pyopenssl"),
    ("Crypto", "pycryptodome"),
    ("attr", "attrs"),
    ("git", "GitPython"),
    ("magic", "python-magic"),
    ("docx", "python-docx"),
    ("MySQLdb", "mysqlclient"),
    ("zmq", "pyzmq"),
    ("fitz", "pymupdf"),
    ("google", "protobuf"),
    ("psycopg2", "psycopg2-binary"),
    ("lxml", "lxml"),
    ("wx", "wxPython"),
    ("gi", "PyGObject"),
];

/// Modules whose names people mistype: a missing module close to one of these is a typo.
const COMMON_PY_MODULES: &[&str] = &[
    "requests",
    "numpy",
    "pandas",
    "scipy",
    "matplotlib",
    "seaborn",
    "flask",
    "django",
    "fastapi",
    "pytest",
    "yaml",
    "torch",
    "tensorflow",
    "keras",
    "sklearn",
    "cv2",
    "PIL",
    "bs4",
    "lxml",
    "sqlalchemy",
    "aiohttp",
    "httpx",
    "click",
    "pydantic",
    "jinja2",
    "boto3",
    "paramiko",
    "psutil",
    "tqdm",
    "rich",
    "typer",
    "uvicorn",
    "selenium",
    "scrapy",
    "networkx",
    "sympy",
    "plotly",
    "dash",
    "streamlit",
    "openpyxl",
    "redis",
    "pymongo",
    "psycopg2",
    "setuptools",
    "wheel",
    "cryptography",
    "dotenv",
    "dateutil",
    "pytz",
    "markdown",
    "docker",
    "toml",
    "tomli",
    "attrs",
    "numba",
    "transformers",
    "openai",
];

const COMMON_NPM_PACKAGES: &[&str] = &[
    "express",
    "react",
    "react-dom",
    "lodash",
    "axios",
    "moment",
    "left-pad",
    "chalk",
    "commander",
    "dotenv",
    "webpack",
    "typescript",
    "jest",
    "mocha",
    "request",
    "underscore",
    "async",
    "debug",
    "uuid",
    "yargs",
    "body-parser",
    "cors",
    "mongoose",
    "socket.io",
    "vue",
    "next",
    "eslint",
    "prettier",
    "rimraf",
    "glob",
    "minimist",
    "inquirer",
    "bluebird",
    "cheerio",
    "puppeteer",
    "nodemon",
    "ws",
    "mysql",
    "pg",
    "sequelize",
    "passport",
    "jsonwebtoken",
    "bcrypt",
    "morgan",
    "redux",
    "rxjs",
    "classnames",
    "date-fns",
    "fs-extra",
    "semver",
    "colors",
    "ora",
    "node-fetch",
    "cross-env",
];

fn pip_name(module: &str) -> String {
    let top = module.split('.').next().unwrap_or(module);
    PIP_NAMES.iter().find(|(import, _)| *import == top).map_or_else(|| top.to_string(), |(_, pip)| (*pip).to_string())
}

/// Python packages installed here or sitting next to the failing script.
fn nearby_python_modules() -> Vec<String> {
    let mut names = BTreeSet::new();
    let mut add = |dir: &Path| {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
            let stem = name.strip_suffix(".py").unwrap_or(&name);
            if !stem.is_empty() && !stem.starts_with(['_', '.']) && !stem.contains(['-', '.']) {
                names.insert(stem.to_string());
            }
        }
    };
    add(Path::new("."));
    let mut roots = vec![PathBuf::from("/usr/lib"), PathBuf::from("/usr/local/lib")];
    if let Ok(home) = std::env::var("HOME") {
        roots.push(Path::new(&home).join(".local/lib"));
    }
    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else { continue };
        for entry in entries.flatten() {
            if entry.file_name().to_str().is_some_and(|n| n.starts_with("python3")) {
                add(&entry.path().join("site-packages"));
                add(&entry.path().join("dist-packages"));
            }
        }
    }
    names.into_iter().collect()
}

fn externally_managed() -> bool {
    let platforms = crate::platform::detect();
    if crate::platform::manager(&platforms).is_some_and(|family| {
        matches!(family.as_str(), "arch" | "omarchy" | "debian" | "fedora" | "suse" | "tumbleweed")
    }) {
        return true;
    }
    let Ok(entries) = std::fs::read_dir("/usr/lib") else { return false };
    entries.flatten().any(|e| {
        e.file_name().to_str().is_some_and(|n| n.starts_with("python3")) && e.path().join("EXTERNALLY-MANAGED").exists()
    })
}

fn diagnose_python_module(module: &str) -> String {
    let top = module.split('.').next().unwrap_or(module);
    let evidence = format!("`No module named '{module}'`");
    let known = PIP_NAMES.iter().any(|(import, _)| *import == top) || COMMON_PY_MODULES.contains(&top);
    if !known && top.chars().count() >= 4 {
        let mut candidates: Vec<String> = COMMON_PY_MODULES.iter().map(|m| (*m).to_string()).collect();
        candidates.extend(nearby_python_modules());
        if let Some(close) = closest_typo(top, &candidates, 1) {
            return format_answer(
                &format!(
                    "Python could not import `{top}` ({evidence}). That looks like a typo for `{close}`: fix the spelling in the code rather than installing a package named `{top}`."
                ),
                &format!("grep -rn '{top}' --include=*.py ."),
            );
        }
    }
    let pip = pip_name(module);
    let renamed = if pip == top {
        String::new()
    } else {
        format!(" The import name `{top}` is installed from the pip package `{pip}`.")
    };
    if externally_managed() {
        format_answer(
            &format!(
                "Python could not import `{top}` because it is not installed ({evidence}).{renamed} This system's Python is externally managed (PEP 668), so a bare `pip install {pip}` is refused: install it in a virtual environment and run your script with `.venv/bin/python`, or use your distribution's own python package."
            ),
            &format!("python3 -m venv .venv && .venv/bin/pip install {pip}"),
        )
    } else {
        format_answer(
            &format!("Python could not import `{top}` because it is not installed ({evidence}).{renamed}"),
            &format!("pip install {pip}"),
        )
    }
}

fn extract_node_module(output: &str) -> Option<String> {
    for line in output.lines() {
        if let Some(pos) = line.find("Cannot find module ") {
            let name = line[pos + "Cannot find module ".len()..].split_whitespace().next()?;
            let name = name.trim_matches(['\'', '"', '`']);
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

fn diagnose_node_module(name: &str) -> String {
    let evidence = format!("`Cannot find module '{name}'`");
    if name.starts_with('.') || name.starts_with('/') {
        return format_answer(
            &format!(
                "Node could not load the file `{name}` ({evidence}): that path does not exist, or has no index file or the wrong extension."
            ),
            "ls",
        );
    }
    // `lodash/fp` is the package `lodash`; `@scope/pkg/sub` is `@scope/pkg`.
    let package = if name.starts_with('@') {
        name.splitn(3, '/').take(2).collect::<Vec<_>>().join("/")
    } else {
        name.split('/').next().unwrap_or(name).to_string()
    };
    if !COMMON_NPM_PACKAGES.contains(&package.as_str()) && package.chars().count() >= 4 {
        let mut candidates: Vec<String> = COMMON_NPM_PACKAGES.iter().map(|m| (*m).to_string()).collect();
        if let Ok(entries) = std::fs::read_dir("node_modules") {
            candidates.extend(
                entries
                    .flatten()
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|n| !n.starts_with(['.', '@'])),
            );
        }
        if let Some(close) = closest_typo(&package, &candidates, 1) {
            return format_answer(
                &format!(
                    "Node could not find the module `{name}` ({evidence}). That looks like a typo for `{close}`: fix the spelling in the code rather than installing a package named `{package}`."
                ),
                &format!("grep -rn --exclude-dir=node_modules '{package}' ."),
            );
        }
    }
    format_answer(
        &format!(
            "Node could not find the module `{name}` ({evidence}) because the package is not installed in this project."
        ),
        &format!("npm install {package}"),
    )
}

// ---------------------------------------------------------------------------
// Python tracebacks, makefiles, crashes
// ---------------------------------------------------------------------------

/// `File "x.py", line 3` frames of a Python error, in order: (file, line, the source line after it).
fn python_frames(output: &str) -> Vec<(String, u32, String)> {
    let lines: Vec<&str> = output.lines().collect();
    let mut frames = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        let Some(rest) = t.strip_prefix("File \"") else { continue };
        let Some((file, after)) = rest.split_once("\", line ") else { continue };
        let number: String = after.chars().take_while(char::is_ascii_digit).collect();
        let Ok(number) = number.parse::<u32>() else { continue };
        let code = lines
            .get(i + 1)
            .map(|l| l.trim().to_string())
            .filter(|l| !l.starts_with("File \"") && !l.starts_with('^'))
            .unwrap_or_default();
        frames.push((file.to_string(), number, code));
    }
    frames
}

fn diagnose_python(last: &Last) -> Option<String> {
    let exception = last.output.lines().rev().map(str::trim_end).find(|l| !l.is_empty())?;
    if exception.starts_with(char::is_whitespace) {
        return None;
    }
    let (name, message) = exception.split_once(':').map_or((exception, ""), |(n, m)| (n, m.trim()));
    let short = name.rsplit('.').next().unwrap_or(name);
    if name.contains(char::is_whitespace) || !(short.ends_with("Error") || short.ends_with("Exception")) {
        return None;
    }
    let frames = python_frames(&last.output);
    // The user's own code: not the standard library or installed packages. `<string>` (from
    // `python -c`) and `<stdin>` are the user's.
    let own = |file: &str| {
        !file.contains("/lib/python")
            && !file.contains("site-packages")
            && !file.contains("dist-packages")
            && !file.starts_with("<frozen")
    };
    let (file, line, code) = frames.iter().rev().find(|(f, ..)| own(f)).or_else(|| frames.last())?.clone();
    let name_of_file = Path::new(&file).file_name().and_then(|n| n.to_str()).unwrap_or(&file).to_string();
    let spot =
        if code.is_empty() { format!("`{file}`, line {line}") } else { format!("`{file}`, line {line} (`{code}`)") };
    let sed = if file.starts_with('<') {
        "nothing".to_string()
    } else {
        format!("sed -n '{},{}p' {file}", line.saturating_sub(1).max(1), line + 1)
    };
    let evidence = format!("`{exception}`");
    let explanation = match short {
        "SyntaxError" | "IndentationError" | "TabError" => format!(
            "Python could not parse {spot}: {evidence}. The mistake is on that line or just before it (a missing bracket, colon or quote, or inconsistent indentation)."
        ),
        "KeyError" => format!(
            "The dictionary has no key {message} ({evidence}), raised in {spot}. Check that the key exists, or read it with `.get()`."
        ),
        "AttributeError" => format!(
            "An object does not have the attribute you used ({evidence}), raised in {spot}. Often the value is `None` or a different type than expected."
        ),
        "JSONDecodeError" => {
            let place = message.rsplit_once(": ").map_or(message, |(_, place)| place);
            format!(
                "The text given to the JSON parser is not valid JSON ({evidence}); the problem is at {}. The parse was called in {spot}.",
                place.split(" (char").next().unwrap_or(place)
            )
        }
        _ => format!("Python raised {evidence} in {spot}."),
    };
    let _ = name_of_file;
    Some(format_answer(&explanation, &sed))
}

fn diagnose_makefile(last: &Last) -> Option<String> {
    for line in last.output.lines() {
        let t = line.trim();
        if !t.contains("missing separator") {
            continue;
        }
        let mut parts = t.splitn(3, ':');
        let (file, number) = (parts.next()?, parts.next()?.trim());
        if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
            return Some(format_answer(
                "Make found a line it cannot read (`missing separator`): recipe lines must start with a tab character, not spaces.",
                "nothing",
            ));
        }
        return Some(format_answer(
            &format!(
                "`{file}` line {number} has `missing separator`: make needs the lines under a target (the commands) to start with a tab character, and this line starts with spaces (or a typo)."
            ),
            &format!("sed -n '{number}p' {file} | cat -A"),
        ));
    }
    None
}

fn diagnose_crashes(last: &Last) -> Option<String> {
    let lower = last.output.to_lowercase();
    if lower.contains("segmentation fault") {
        return Some(format_answer(
            "The program crashed with `Segmentation fault`: it touched memory it must not use, which is a bug in the program itself (or in a library or plugin it loads), not a mistake in how you ran it. systemd keeps a record of the crash.",
            "coredumpctl list",
        ));
    }
    let killed_line = last.output.lines().any(|l| l.split_whitespace().any(|w| w == "Killed"));
    if killed_line || last.exit == Some(137) {
        let code = if last.exit == Some(137) { ", exit code 137" } else { "" };
        return Some(format_answer(
            &format!(
                "The process was killed with SIGKILL (`Killed`{code}). The usual cause is the kernel's out-of-memory killer stopping a program that used too much memory; a manual `kill -9` looks the same. The kernel log says which."
            ),
            "journalctl -k -g 'out of memory|killed process'",
        ));
    }
    None
}

// ---------------------------------------------------------------------------
// Network
// ---------------------------------------------------------------------------

/// The host that could not be looked up, and the exact words of the error, from curl, git, ssh,
/// ping and wget output.
fn unresolved_host(output: &str) -> Option<(Option<String>, String)> {
    for line in output.lines() {
        let t = line.trim();
        let lower = t.to_lowercase();
        let clean = |s: &str| s.trim().trim_matches(['\'', '"', '`', ':', '.', ',']).to_string();
        if let Some(at) = lower.find("could not resolve hostname ") {
            let host = t[at + "could not resolve hostname ".len()..].split([':', ' ']).next().map(clean);
            let host_part = host.clone().unwrap_or_default();
            return Some((host, format!("Could not resolve hostname {host_part}")));
        }
        if let Some(at) = lower.find("could not resolve host: ") {
            let host = t[at + "could not resolve host: ".len()..].split_whitespace().next().map(clean);
            let host_part = host.clone().unwrap_or_default();
            return Some((host, format!("Could not resolve host: {host_part}")));
        }
        if let Some(at) = lower.find("unable to resolve host address ") {
            let host = t[at + "unable to resolve host address ".len()..].split_whitespace().next().map(clean);
            return Some((host, "unable to resolve host address".to_string()));
        }
        for phrase in
            ["name or service not known", "temporary failure in name resolution", "nodename nor servname provided"]
        {
            if let Some(at) = lower.find(phrase) {
                // `ping: host: Name or service not known`
                let host = t[..at]
                    .trim_end()
                    .trim_end_matches(':')
                    .rsplit([' ', ':'])
                    .next()
                    .map(clean)
                    .filter(|h| !h.is_empty() && h.contains('.') || h == "localhost");
                return Some((host, t[at..at + phrase.len()].to_string()));
            }
        }
    }
    None
}

/// Where a refused connection was going: (host, port).
fn refused_target(command: &str, output: &str) -> Option<(Option<String>, Option<String>)> {
    for line in output.lines() {
        let t = line.trim();
        let lower = t.to_lowercase();
        for lead in ["connect to host ", "failed to connect to ", "connect to ", "econnrefused ", "dial tcp "] {
            if let Some(at) = lower.find(lead) {
                let rest = &t[at + lead.len()..];
                let target = rest.split_whitespace().next().unwrap_or("").trim_end_matches(':');
                if let Some((host, port)) =
                    target.rsplit_once(':').filter(|(_, p)| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
                {
                    return Some((Some(host.to_string()), Some(port.to_string())));
                }
                let port = rest
                    .split(" port ")
                    .nth(1)
                    .map(|p| p.chars().take_while(char::is_ascii_digit).collect::<String>())
                    .filter(|p| !p.is_empty());
                if !target.is_empty() {
                    return Some((Some(target.to_string()), port));
                }
            }
        }
    }
    if command.trim_start().starts_with("ssh") {
        return Some((extract_ssh_target(command, output), extract_ssh_port(command, output)));
    }
    None
}

// ---------------------------------------------------------------------------
// Git, pip, npm, archives
// ---------------------------------------------------------------------------

fn conflicted_files(output: &str) -> Vec<String> {
    let mut files = Vec::new();
    for line in output.lines() {
        let t = line.trim();
        if t.starts_with("CONFLICT") {
            let file = if let Some((_, f)) = t.split_once("Merge conflict in ") {
                f.trim().to_string()
            } else if let Some((_, rest)) = t.split_once("): ") {
                rest.split_whitespace().next().unwrap_or("").to_string()
            } else {
                String::new()
            };
            if !file.is_empty() && !files.contains(&file) {
                files.push(file);
            }
        } else if let Some(file) = t.strip_prefix("U\t").or_else(|| t.strip_prefix("U ")) {
            let file = file.trim().to_string();
            if !file.is_empty() && !files.contains(&file) {
                files.push(file);
            }
        }
    }
    files
}

fn diagnose_git_more(last: &Last) -> Option<String> {
    let lower = last.output.to_lowercase();
    if lower.contains("not a git repository") {
        return Some(format_answer(
            "This folder is not inside a git repository (`fatal: not a git repository`). Either you are in the wrong folder (`cd` into the project), or the project has never been put under git, in which case `git init` creates a repository here.",
            "pwd",
        ));
    }
    if lower.contains("src refspec") && lower.contains("does not match any") {
        let name =
            last.output.split("src refspec ").nth(1).and_then(|r| r.split_whitespace().next()).unwrap_or("that name");
        return Some(format_answer(
            &format!(
                "Git has nothing to push as `{name}` (`src refspec {name} does not match any`): no local branch has that name. Usually the repository has no commits yet, or the branch is called something else."
            ),
            "git branch -a",
        ));
    }
    if lower.contains("did not match any file(s) known to git")
        || lower.contains("invalid reference:")
        || lower.contains("unknown revision or path not in the working tree")
    {
        let name = last
            .output
            .split('\'')
            .nth(1)
            .or_else(|| last.output.split("invalid reference:").nth(1).and_then(|r| r.split_whitespace().next()))
            .unwrap_or("that name");
        return Some(format_answer(
            &format!(
                "`{name}` is neither a branch, tag or commit nor a file that git knows (`{}`). Check the spelling against the existing branches.",
                last.output
                    .lines()
                    .find(|l| l.to_lowercase().contains("did not match")
                        || l.to_lowercase().contains("invalid reference")
                        || l.to_lowercase().contains("unknown revision"))
                    .unwrap_or("did not match")
                    .trim()
            ),
            "git branch -a",
        ));
    }
    if lower.contains("conflict (")
        || lower.contains("automatic merge failed")
        || lower.contains("unmerged paths")
        || lower.contains("unmerged files")
        || lower.contains("unresolved conflict")
    {
        let files = conflicted_files(&last.output);
        let which = if files.is_empty() {
            "the conflicted files (listed under \"Unmerged paths\" by `git status`)".to_string()
        } else {
            files.iter().take(3).map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ")
        };
        return Some(format_answer(
            &format!(
                "Git stopped halfway through a merge with unresolved conflicts, because the same lines changed on both sides: {which} must be fixed by hand. Open each file, resolve the `<<<<<<<` / `=======` / `>>>>>>>` markers, then run `git add <file>` for each and `git commit` to finish (or `git merge --abort` to undo the merge)."
            ),
            "git status",
        ));
    }
    if lower.contains("no stash entries found") {
        return Some(format_answer(
            "Git has no stash to apply (`No stash entries found`): nothing was stashed here, or it was already popped or dropped.",
            "git stash list",
        ));
    }
    None
}

fn archive_in_output(output: &str) -> Option<String> {
    for line in output.lines() {
        if let Some(rest) = line.trim().strip_prefix("Archive:") {
            let name = rest.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

fn diagnose_archives(last: &Last) -> Option<String> {
    let lower = last.output.to_lowercase();
    if lower.contains("end-of-central-directory signature not found") || lower.contains("cannot find zipfile directory")
    {
        let name = archive_in_output(&last.output)
            .or_else(|| {
                last.command.split_whitespace().skip(1).filter(|t| !t.starts_with('-')).last().map(str::to_string)
            })
            .unwrap_or_else(|| "<name>".to_string());
        return Some(format_answer(
            &format!(
                "`unzip` says `{name}` is not a zip archive (`End-of-central-directory signature not found`): the file is damaged, was only partly downloaded, or is some other format (a .tar.gz or an HTML error page, for example). `file` shows what it really is."
            ),
            &format!("file {name}"),
        ));
    }
    if lower.contains("not in gzip format") {
        // `gzip -d f.gz` and `gunzip f.gz` name the file; `tar xzf` reports `gzip: stdin`.
        let named = last.output.lines().find_map(|line| {
            let name = line.trim().strip_prefix("gzip: ")?.strip_suffix(": not in gzip format")?;
            (name != "stdin").then(|| name.to_string())
        });
        if let Some(name) = named {
            return Some(format_answer(
                &format!(
                    "`{name}` is not in gzip format (`gzip: {name}: not in gzip format`), so gzip cannot unpack it. The file is probably not compressed at all, or compressed another way (xz, bzip2, zstd, zip), or it is not an archive, as when a download saved an error page. `file {name}` shows what it really is."
                ),
                &format!("file {name}"),
            ));
        }
        let archive = extract_tar_archive(last);
        let known = archive.is_some();
        let name = archive.unwrap_or_else(|| "<name>".to_string());
        let which = if known { format!("`{name}`") } else { "the file".to_string() };
        return Some(format_answer(
            &format!(
                "`tar` was told to decompress {which} as gzip (the `z` flag), but it is not gzip-compressed (`gzip: stdin: not in gzip format`). It may be a plain or differently compressed archive (then drop the `z`, since plain `tar xf` detects the format), or not an archive at all, as when a download saved an error page. `file` shows what it really is."
            ),
            &format!("file {name}"),
        ));
    }
    None
}

// ---------------------------------------------------------------------------
// Second measurement round: recognisers for failures that used to fall through, and the rule
// for output that cannot be classified
// ---------------------------------------------------------------------------

/// The `sudo` form of a refused command. A redirect (`echo x > /etc/foo`) is done by the shell
/// before `sudo` starts, so `sudo echo x > /etc/foo` fails the same way; `tee` does the write.
fn sudo_form(command: &str, path: &str) -> String {
    if let Some(at) = command.find("| tee ") {
        let (head, tail) = command.split_at(at + 2);
        return format!("{head}sudo {tail}");
    }
    for (op, flag) in [(">>", "-a "), (">", "")] {
        for gap in [" ", ""] {
            let needle = format!("{op}{gap}{path}");
            if let Some(at) = command.find(&needle) {
                let before = &command[..at];
                let left = before.trim_end();
                if left.is_empty() || (op == ">" && before.ends_with('>')) {
                    continue;
                }
                return format!("{left} | sudo tee {flag}{path}");
            }
        }
    }
    format!("sudo {command}")
}

/// How much a line looks like an error message.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    No,
    /// A known reason (`No such file or directory`) somewhere in the line: could be prose.
    Weak,
    /// A severity marker or program prefix at the start (`error:`, `cat: x: reason`).
    Strong,
}

/// A log line from another program: a timestamp or a `[level]` tag at the start.
fn is_log_line(line: &str) -> bool {
    let t = line.trim_start_matches('[').trim();
    let b: Vec<char> = t.chars().take(24).collect();
    let digits = |from: usize, n: usize| b.len() >= from + n && b[from..from + n].iter().all(char::is_ascii_digit);
    let date = digits(0, 4) && matches!(b.get(4), Some('-' | '/')) && digits(5, 2);
    let clock = (0..b.len().saturating_sub(7))
        .any(|i| digits(i, 2) && b[i + 2] == ':' && digits(i + 3, 2) && b[i + 5] == ':' && digits(i + 6, 2));
    let lower = line.trim().to_lowercase();
    let level = ["[error]", "[warn]", "[warning]", "[info]", "[crit]", "[notice]", "[debug]"]
        .iter()
        .any(|l| lower.starts_with(l));
    date || clock || level
}

/// A line of source code or a grep hit in source code (`src/io.py:41:    except OSError:`).
fn is_code_line(line: &str) -> bool {
    let mut t = line.trim();
    // `path:12:` or `path:12:5:` in front of the code.
    let mut parts = t.splitn(3, ':');
    if let (Some(path), Some(num), Some(rest)) = (parts.next(), parts.next(), parts.next())
        && !path.contains(char::is_whitespace)
        && !num.is_empty()
        && num.chars().all(|c| c.is_ascii_digit())
    {
        t = rest.trim();
    }
    ["except", "catch", "} catch", "raise ", "throw ", "if err", "return err", "err != nil", "//", "def ", "fn "]
        .iter()
        .any(|k| t.starts_with(k))
}

/// `git log --oneline` and similar: an abbreviated commit id, then the subject.
fn is_commit_line(line: &str) -> bool {
    let Some((id, rest)) = line.trim().split_once(' ') else { return false };
    (7..=40).contains(&id.len())
        && id.chars().all(|c| c.is_ascii_hexdigit())
        && id.chars().any(|c| c.is_ascii_digit())
        && !rest.trim().is_empty()
}

/// Words that say an operation went fine.
fn is_success_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    if ["fail", "error", "not ", "refused", "denied", "cannot", "unable"].iter().any(|w| lower.contains(w)) {
        return false;
    }
    lower
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| matches!(w, "connected" | "done" | "ok" | "passed" | "success" | "successful" | "succeeded" | "ready"))
}

/// The shape of one line, ignoring the lines around it.
fn line_shape(line: &str) -> Shape {
    let t = line.trim();
    if is_context_noise(t) {
        return Shape::No;
    }
    let lower = t.to_lowercase();
    // Success messages that start with a tool name.
    if ["make: nothing to be done", "make: entering directory", "make: leaving directory"]
        .iter()
        .any(|s| lower.starts_with(s))
        || (lower.starts_with("make: '") && lower.ends_with("is up to date."))
    {
        return Shape::No;
    }
    const STRONG: &[&str] = &[
        "error:",
        "error[",
        "error response",
        "err!",
        "npm err",
        "npm error",
        "e: ",
        "fatal",
        "traceback (most recent call last)",
        "panic",
        "thread '",
        "exception in thread",
        "job for ",
        "make:",
        "make[",
        "***",
        "-bash:",
        "bash:",
        "sh:",
        "zsh:",
        "fish:",
        "==> error",
        "conflict (",
        "automatic merge failed",
        "the previous cherry-pick is now empty",
        "the following paths are ignored",
        "segmentation fault",
        "bus error",
        "dlopen():",
    ];
    const WEAK_OPENERS: &[&str] = &[
        "failed ",
        "failed:",
        "cannot ",
        "can't ",
        "could not ",
        "couldn't ",
        "unable to ",
        "permission denied",
        "no such ",
        "killed",
        "aborted",
        "warning: unable",
    ];
    const CONTAINS: &[&str] = &[
        ": command not found",
        "no such file or directory",
        "permission denied",
        "out of memory",
        "not in gzip format",
        "address already in use",
        "connection refused",
        "connection timed out",
        "name or service not known",
        "temporary failure in name resolution",
        "could not resolve",
        "could not be found",
        "is a directory",
        "not a directory",
        "operation not permitted",
        "no space left",
        "read-only file system",
        "unrecognized option",
        "invalid option",
        "unknown option",
        "undefined reference",
        "unterminated",
        "[rejected]",
        "host key verification failed",
        "no module named",
    ];
    const REASONS: &[&str] = &[
        "omitting directory",
        "not something",
        "no files found",
        "no rule",
        "try '",
        "syntax",
        "unexpected",
        "unknown",
        "invalid",
        "missing",
        "illegal",
        "exists",
        "not a ",
        "not supported",
        "must ",
    ];
    if STRONG.iter().any(|o| lower.starts_with(o))
        || (t.starts_with("ERROR") && !t[5..].starts_with(char::is_alphabetic))
    {
        return Shape::Strong;
    }
    // `ReferenceError: x is not defined`, `json.decoder.JSONDecodeError: ...`
    if let Some((head, _)) = t.split_once(": ")
        && (head.ends_with("Error") || head.ends_with("Exception"))
        && !head.contains(char::is_whitespace)
    {
        return Shape::Strong;
    }
    // `program: something: reason`: a lowercase program name with no spaces, then a reason.
    if let Some((head, rest)) = t.split_once(": ") {
        let head_ok = head.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c == '/' || c == '.')
            && head.chars().all(|c| c.is_ascii_alphanumeric() || "-_./+".contains(c));
        let rest_lower = rest.to_lowercase();
        if head_ok && (is_error_line(rest) || REASONS.iter().any(|r| rest_lower.contains(r))) {
            return Shape::Strong;
        }
    }
    let words = t.split_whitespace().count();
    // A sentence ending in a period, a list item or a heading is documentation, not a message.
    if t.starts_with(['-', '*', '#', '•']) {
        return Shape::No;
    }
    if lower.starts_with("cannot mount appimage") || lower.trim_end_matches('.') == "exec format error" {
        return Shape::Weak;
    }
    if t.ends_with('.') && words >= 5 && !t.contains(": ") {
        return Shape::No;
    }
    if WEAK_OPENERS.iter().any(|o| lower.starts_with(o)) {
        return Shape::Weak;
    }
    for keyword in CONTAINS {
        if let Some(at) = lower.find(keyword) {
            // Several words in front of the reason, and no `prefix:` in front of them: prose.
            let before = &lower[..at];
            if before.split_whitespace().count() >= 5 && !before.contains(": ") {
                return Shape::No;
            }
            return Shape::Weak;
        }
    }
    Shape::No
}

/// Does the output hold a line shaped like an error that nothing later contradicts? A weak
/// line followed by a success line ("connected", "done") was a hiccup.
pub(crate) fn has_error_shaped_line(output: &str) -> bool {
    let lines: Vec<&str> = output.lines().collect();
    lines.iter().enumerate().any(|(i, line)| match line_shape(line) {
        Shape::No => false,
        Shape::Strong => true,
        Shape::Weak => !lines[i + 1..].iter().any(|later| is_success_line(later)),
    })
}

/// The first line of an output that is a log line from another program reporting an error.
fn foreign_log_error(output: &str) -> Option<&str> {
    output.lines().map(str::trim).find(|l| {
        is_log_line(l) && {
            let lower = l.to_lowercase();
            lower.contains("error") || lower.contains("failed") || lower.contains("crit")
        }
    })
}

/// Output without an exit code and without any line shaped like an error.
fn diagnose_unrecognised(last: &Last) -> String {
    let tail = " If the command did go wrong, run `huh why` right after it, or pipe in the lines that show the error.";
    if describe_output(&last.output) != "ordinary program output" {
        return diagnose_not_an_error(last);
    }
    if let Some(line) = foreign_log_error(&last.output) {
        let program =
            if line.to_lowercase().contains("[error]") && last.command.contains("nginx") { " (nginx)" } else { "" };
        return format_answer(
            &format!(
                "This is a log line{program} written by another program, not an error from your command: {}. It shows what that program logged earlier; if the command itself failed, run `huh why` right after it.",
                quote_line(line)
            ),
            "nothing",
        );
    }
    if let Some(line) = last.output.lines().map(str::trim).find(|l| is_error_line(l)) {
        return format_answer(
            &format!(
                "huh does not recognise this output as a failure. The closest line is {}, but it reads like ordinary text, not an error message (those look like `error: ...`, `fatal: ...` or `program: file: reason`), and no exit code was given.{tail}",
                quote_line(line)
            ),
            "nothing",
        );
    }
    format_answer(
        &format!(
            "huh does not recognise an error in this output: it looks like ordinary program output, but without an exit code huh cannot be sure the command succeeded.{tail}"
        ),
        "nothing",
    )
}

/// Text between the first quote after `marker` and its closing quote (`'x'`, `` `x' ``, `‘x’`).
fn quoted_after(line: &str, marker: &str) -> Option<String> {
    let rest = &line[line.find(marker)? + marker.len()..];
    let start = rest.find(['\'', '`', '‘'])?;
    let rest = &rest[start + rest[start..].chars().next()?.len_utf8()..];
    let end = rest.find(['\'', '’', '`'])?;
    (end > 0).then(|| rest[..end].to_string())
}

/// The first line of `output` containing `needle` (compared in lower case).
fn line_with<'a>(output: &'a str, needle: &str) -> Option<&'a str> {
    output.lines().map(str::trim).find(|l| l.to_lowercase().contains(needle))
}

/// The word after `marker` in `text`.
fn word_after<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    text.split_once(marker)?.1.split_whitespace().next()
}

fn diagnose_measured(last: &Last) -> Option<String> {
    let lower = last.output.to_lowercase();
    measured_docker(last, &lower)
        .or_else(|| measured_git(last, &lower))
        .or_else(|| measured_files(last, &lower))
        .or_else(|| measured_libraries(last, &lower))
        .or_else(|| measured_net(last, &lower))
        .or_else(|| measured_build(last, &lower))
        .or_else(|| measured_runtime(last, &lower))
}

fn measured_docker(last: &Last, lower: &str) -> Option<String> {
    let out = last.output.as_str();
    if let Some(line) = line_with(out, "is not a docker command") {
        let name = quoted_after(line, "docker: ").unwrap_or_default();
        return Some(format_answer(
            &format!(
                "Docker has no command called `{name}` (`{line}`). `docker --help` lists the commands it does have."
            ),
            "docker --help",
        ));
    }
    if let Some(line) = line_with(out, "unknown command:") {
        let at = line.to_lowercase().find("unknown command:")? + "unknown command:".len();
        let words: Vec<&str> = line[at..].split_whitespace().collect();
        if words.first() == Some(&"docker") && words.len() > 1 {
            let name = words[1..].join(" ");
            let help = if words.len() > 2 {
                format!("docker {} --help", words[1..words.len() - 1].join(" "))
            } else {
                "docker --help".to_string()
            };
            return Some(format_answer(
                &format!("Docker has no command called `{name}` (`{line}`). `{help}` lists the commands it does have."),
                &help,
            ));
        }
    }
    if lower.contains("no such container")
        && let Some(name) =
            word_after(&last.output, "No such container: ").or_else(|| word_after(&last.output, "no such container: "))
    {
        return Some(format_answer(
            &format!(
                "Docker has no container called `{name}` (`Error response from daemon: No such container: {name}`): it was never created, was already removed, or the name is misspelled. `docker ps -a` lists the containers that exist, running or not."
            ),
            "docker ps -a",
        ));
    }
    None
}

fn measured_git(last: &Last, lower: &str) -> Option<String> {
    let out = last.output.as_str();
    let cmd = last.command.trim();
    let committing = cmd.contains("git commit") || (cmd.is_empty() && last.exit.is_some());
    if committing
        && (lower.contains("nothing to commit")
            || lower.contains("nothing added to commit")
            || lower.contains("no changes added to commit"))
    {
        if lower.contains("working tree clean") {
            return Some(format_answer(
                "`git commit` made no commit because there was nothing to commit: the working tree is clean, so no file has changed since the last commit. That is not a problem with the repository. Change a file first, then `git add` it; `git status` shows the state.",
                "nothing",
            ));
        }
        if lower.contains("untracked files present") {
            return Some(format_answer(
                "`git commit` had nothing to commit: the only new files are untracked, and a commit takes only what is staged. `git add` stages them (`git status` lists them first).",
                "git add .",
            ));
        }
        if lower.contains("no changes added to commit") {
            return Some(format_answer(
                "`git commit` had nothing to commit: files are modified but none is staged (`no changes added to commit`). `git add` stages the changes; `git add -u` stages every modified tracked file.",
                "git add -u",
            ));
        }
        return Some(format_answer(
            "`git commit` had nothing to commit (`nothing to commit (create/copy files and use \"git add\" to track)`): the repository has no files that git could record yet. Create or copy files in, then `git add` them and commit; `git status` shows what git sees.",
            "git status",
        ));
    }
    if let Some(line) = line_with(out, "not something we can merge") {
        let name = line.strip_prefix("merge: ").and_then(|r| r.split(" - ").next()).unwrap_or("that name");
        return Some(format_answer(
            &format!(
                "`git merge` cannot merge `{name}` (`{line}`): it is not a branch, tag or commit in this repository. Check the spelling against the branches that exist."
            ),
            "git branch -a",
        ));
    }
    if let Some(line) = line_with(out, "is not a git command") {
        let bad = quoted_after(line, "git: ").unwrap_or_default();
        let suggestion = out
            .lines()
            .skip_while(|l| !l.starts_with("The most similar command"))
            .nth(1)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        return Some(match suggestion {
            Some(sug) => {
                let fixed = if cmd.is_empty() { format!("git {sug}") } else { fix_command(cmd, &bad, sug) };
                format_answer(
                    &format!(
                        "`{bad}` is not a git command (`{line}`); git suggests `{sug}`, so it is probably a typo."
                    ),
                    &fixed,
                )
            }
            None => format_answer(
                &format!("`{bad}` is not a git command (`{line}`). `git help -a` lists the commands there are."),
                "nothing",
            ),
        });
    }
    if let Some(line) = line_with(out, "pathspec")
        && lower.contains("did not match any files")
        && !lower.contains("known to git")
    {
        let name = quoted_after(line, "pathspec").unwrap_or_default();
        return Some(format_answer(
            &format!(
                "`git add` found no file matching `{name}` (`{line}`): the name is misspelled, or you are in a different folder from the file. `git status` shows the files git knows about."
            ),
            "git status",
        ));
    }
    if lower.contains("no configured push destination") {
        return Some(format_answer(
            "`git push` has nowhere to push to (`No configured push destination`): this repository has no remote. Add one with `git remote add origin <url>` (the address of the repository on GitHub or your server), then push again; `git remote -v` shows the remotes there are.",
            "git remote -v",
        ));
    }
    if lower.contains("is not fully merged") {
        let branch = quoted_after(out, "The branch ").unwrap_or_else(|| "the branch".to_string());
        return Some(format_answer(
            &format!(
                "Git will not delete `{branch}` with `-d` because it holds commits that no other branch has, so deleting it would lose them (`error: The branch '{branch}' is not fully merged.`). Look at what is on it first; only if you are sure you do not need those commits, `git branch -D {branch}` deletes it anyway."
            ),
            &format!("git log --oneline HEAD..{branch}"),
        ));
    }
    if (lower.contains("fatal: repository") && (lower.contains("does not exist") || lower.contains("not found")))
        || lower.contains("repository not found")
    {
        let repo = line_with(out, "fatal: repository").and_then(|l| quoted_after(l, "repository")).unwrap_or_default();
        let (what, fix) = if repo.is_empty() {
            ("the repository".to_string(), "nothing".to_string())
        } else {
            (format!("`{repo}`"), format!("git ls-remote {repo}"))
        };
        return Some(format_answer(
            &format!(
                "Git could not find {what} (`fatal: repository ... does not exist`/`not found`): the address or path is misspelled, the repository was deleted or renamed, or it is private and your account has no access. Check the address character by character."
            ),
            &fix,
        ));
    }
    if let Some(line) = line_with(out, "already exists")
        && lower.contains("remote ")
        && lower.contains("error: remote")
    {
        let name = word_after(line, "remote ").unwrap_or("origin");
        return Some(format_answer(
            &format!(
                "This repository already has a remote called `{name}` (`{line}`). `git remote -v` shows where it points; `git remote set-url {name} <url>` changes the address."
            ),
            "git remote -v",
        ));
    }
    None
}

fn measured_files(last: &Last, lower: &str) -> Option<String> {
    let out = last.output.as_str();
    let cmd = last.command.trim();
    if let Some(line) =
        out.lines().map(str::trim).find(|l| l.to_lowercase().trim_end_matches('.').ends_with(": is a directory"))
    {
        let prog = line.split(':').next().unwrap_or_default();
        let before = &line[..line.len() - ": Is a directory".len()];
        let path = path_in(before)?;
        if prog == "rm" {
            return Some(format_answer(
                &format!(
                    "`rm` does not delete folders unless told to (`{line}`): `{path}` is a folder. If it is empty, `rmdir {path}` removes it. `rm -r {path}` removes the folder and everything inside it, with no way back, so look at `ls {path}` before using it."
                ),
                &format!("rmdir {path}"),
            ));
        }
        return Some(format_answer(
            &format!(
                "`{path}` is a folder, and `{prog}` works on files, not folders (`{line}`). `ls {path}` shows what is inside it."
            ),
            &format!("ls {path}"),
        ));
    }
    if let Some(line) = line_with(out, "-r not specified; omitting directory") {
        let dir = quoted_after(line, "directory").unwrap_or_default();
        let fix = if cmd.starts_with("cp ") { cmd.replacen("cp ", "cp -r ", 1) } else { "nothing".to_string() };
        return Some(format_answer(
            &format!(
                "`cp` skipped the folder `{dir}` (`{line}`): it copies a folder only with `-r`, which copies everything inside it too."
            ),
            &fix,
        ));
    }
    if lower.contains("cannot create directory") && lower.trim_end().ends_with("file exists") {
        let line = line_with(out, "cannot create directory")?;
        let path = quoted_after(line, "directory")?;
        return Some(format_answer(
            &format!(
                "`{path}` already exists (`{line}`), as a folder or as a file, so `mkdir` has nothing to create. `mkdir -p {path}` would accept an existing folder silently."
            ),
            &format!("ls -ld {path}"),
        ));
    }
    if let Some(line) = line_with(out, "unknown predicate") {
        let name = quoted_after(line, "predicate").unwrap_or_default();
        return Some(format_answer(
            &format!(
                "`find` does not have a test or option called `{name}` (`{line}`): it is misspelled or not a find option. `man find` lists what it accepts."
            ),
            "man find",
        ));
    }
    if let Some(line) = out.lines().map(str::trim).find(|l| l.starts_with("No files found for ")) {
        let unit = line.trim_start_matches("No files found for ").trim_end_matches('.');
        let user = if cmd.split_whitespace().any(|w| w == "--user") { " --user" } else { "" };
        let stem = unit.rsplit_once('.').map_or(unit, |(s, _)| s);
        return Some(format_answer(
            &format!(
                "systemd has no unit file called `{unit}` (`{line}`): the service is not installed, or it has another name. `list-unit-files` searches the names."
            ),
            &format!("systemctl{user} list-unit-files '*{stem}*'"),
        ));
    }
    if lower.contains("unterminated") && lower.contains("sed:") {
        let line = line_with(out, "unterminated")?;
        return Some(format_answer(
            &format!(
                "`sed` could not read its expression (`{line}`): a `s` command needs three slashes, as in `s/old/new/`, and this one is missing the closing `/` (or a quote is unbalanced)."
            ),
            "nothing",
        ));
    }
    None
}

/// Development packages for the libraries compilers most often cannot find:
/// (library names, header prefixes, arch, debian, fedora, suse, alpine).
const DEV_PACKAGES: &[(&[&str], &[&str], [&str; 5])] = &[
    (&["ssl", "crypto"], &["openssl/"], ["openssl", "libssl-dev", "openssl-devel", "libopenssl-devel", "openssl-dev"]),
    (&["z"], &["zlib.h", "zconf.h"], ["zlib", "zlib1g-dev", "zlib-devel", "zlib-devel", "zlib-dev"]),
    (&["curl"], &["curl/"], ["curl", "libcurl4-openssl-dev", "libcurl-devel", "libcurl-devel", "curl-dev"]),
    (&["ffi"], &["ffi.h"], ["libffi", "libffi-dev", "libffi-devel", "libffi-devel", "libffi-dev"]),
    (&["sqlite3"], &["sqlite3.h"], ["sqlite", "libsqlite3-dev", "sqlite-devel", "sqlite3-devel", "sqlite-dev"]),
    (&["xml2"], &["libxml/"], ["libxml2", "libxml2-dev", "libxml2-devel", "libxml2-devel", "libxml2-dev"]),
    (&["yaml"], &["yaml.h"], ["libyaml", "libyaml-dev", "libyaml-devel", "libyaml-devel", "yaml-dev"]),
    (&["png"], &["png.h"], ["libpng", "libpng-dev", "libpng-devel", "libpng16-devel", "libpng-dev"]),
    (&["bz2"], &["bzlib.h"], ["bzip2", "libbz2-dev", "bzip2-devel", "libbz2-devel", "bzip2-dev"]),
    (
        &["readline"],
        &["readline/"],
        ["readline", "libreadline-dev", "readline-devel", "readline-devel", "readline-dev"],
    ),
    (
        &["ncurses", "curses"],
        &["ncurses.h", "curses.h"],
        ["ncurses", "libncurses-dev", "ncurses-devel", "ncurses-devel", "ncurses-dev"],
    ),
    (&[], &["Python.h"], ["python", "python3-dev", "python3-devel", "python3-devel", "python3-dev"]),
];

/// A missing library (`ld: cannot find -lssl`) or header (`fatal error: openssl/ssl.h: No such
/// file or directory`): the development package is not installed.
fn measured_libraries(last: &Last, lower: &str) -> Option<String> {
    use crate::platform;
    let out = last.output.as_str();
    let (what, evidence, library, header): (String, String, Option<String>, Option<String>) =
        if let Some(line) = line_with(out, "cannot find -l") {
            let lib =
                line.split("cannot find -l").nth(1)?.split(|c: char| c == ':' || c.is_whitespace()).next()?.to_string();
            (format!("the library `-l{lib}`"), line.to_string(), Some(lib), None)
        } else if lower.contains("fatal error:") && lower.contains(".h: no such file or directory") {
            let line = line_with(out, ".h: no such file or directory")?;
            let header = line.split("fatal error:").nth(1)?.split(':').next()?.trim().to_string();
            (format!("the header `{header}`"), line.to_string(), None, Some(header))
        } else {
            return None;
        };
    let platforms = platform::detect();
    let column = match platform::manager(&platforms).as_deref() {
        Some("arch" | "omarchy") => Some(0),
        Some("debian") => Some(1),
        Some("fedora") => Some(2),
        Some("suse" | "tumbleweed") => Some(3),
        Some("alpine") => Some(4),
        _ => None,
    };
    let entry = DEV_PACKAGES.iter().find(|(libs, headers, _)| {
        library.as_ref().is_some_and(|l| libs.contains(&l.as_str()))
            || header.as_ref().is_some_and(|h| headers.iter().any(|p| h.starts_with(p)))
    });
    let known = entry.and_then(|(_, _, names)| column.map(|c| names[c]));
    if let (Some(package), Some(install)) = (known, platform::fact(&platforms, "pkg_install")) {
        return Some(format_answer(
            &format!(
                "The compiler could not use {what} (`{evidence}`) because the library's development files (headers and link library) are not installed; the plain runtime package is not enough. Install the development package, `{package}` on this system, then build again."
            ),
            &platform::fill(&install, package),
        ));
    }
    let search_for = header.clone().or_else(|| library.as_ref().map(|l| format!("lib{l}.so")))?;
    let fix =
        platform::fact(&platforms, "pkg_provides").map_or("nothing".to_string(), |t| platform::fill(&t, &search_for));
    Some(format_answer(
        &format!(
            "The compiler could not use {what} (`{evidence}`). If it belongs to a library, that library's development package (headers and link library) is not installed; if it is your own file, check its path. The package manager can tell you which package provides it."
        ),
        &fix,
    ))
}

/// The host a curl or ssh command talks to.
fn url_host(command: &str) -> Option<String> {
    let url = command
        .split_whitespace()
        .find(|t| t.starts_with("http://") || t.starts_with("https://") || t.starts_with("ftp://"))?;
    let rest = url.split("://").nth(1)?;
    let host = rest.split(['/', '?']).next()?;
    let host = host.rsplit('@').next()?;
    let host =
        if host.starts_with('[') { host.split(']').next()?.trim_start_matches('[') } else { host.split(':').next()? };
    (!host.is_empty()).then(|| host.to_string())
}

fn measured_net(last: &Last, lower: &str) -> Option<String> {
    let out = last.output.as_str();
    if lower.contains("curl: (28)") {
        let line = line_with(out, "curl: (28)")?;
        let host = url_host(&last.command);
        let (target, fix) = match &host {
            Some(h) => (format!("`{h}`"), format!("ping -c 3 {h}")),
            None => ("the server".to_string(), "nothing".to_string()),
        };
        return Some(format_answer(
            &format!(
                "curl gave up waiting for {target} (`{line}`): the machine is switched off or unreachable, a firewall silently drops the traffic, or the network is down. Unlike `Connection refused`, nothing answered at all."
            ),
            &fix,
        ));
    }
    if lower.contains("host key verification failed") || lower.contains("remote host identification has changed") {
        let host = line_with(out, "host key for ")
            .and_then(|l| word_after(l, "Host key for ").or_else(|| word_after(l, "host key for ")))
            .map(str::to_string)
            .or_else(|| extract_ssh_target(&last.command, out).map(|t| t.rsplit('@').next().unwrap_or(&t).to_string()));
        let changed = lower.contains("has changed") || lower.contains("offending");
        if changed {
            let file = line_with(out, "offending ").and_then(|l| word_after(l, " in ")).unwrap_or("~/.ssh/known_hosts");
            let fix = host.as_ref().map_or("nothing".to_string(), |h| format!("ssh-keygen -R {h}"));
            return Some(format_answer(
                &format!(
                    "SSH refused to connect because the host key the server presented is not the one saved for it in your known_hosts file (`Host key verification failed`; the entry is in `{file}`). This is normal after the server was reinstalled or its address was given to another machine, but it is also exactly what a man-in-the-middle attack looks like, so confirm with whoever runs the server that the key really changed. Only then remove the old entry with `ssh-keygen -R` and connect again."
                ),
                &fix,
            ));
        }
        return Some(format_answer(
            "SSH could not verify the server's host key (`Host key verification failed`): the host is not in your known_hosts file, and ssh could not ask you to trust it (a script, or a non-interactive session). Connect once by hand and compare the fingerprint it shows with the server's before accepting it.",
            "nothing",
        ));
    }
    None
}

fn measured_build(last: &Last, lower: &str) -> Option<String> {
    let out = last.output.as_str();
    let cmd = last.command.trim();
    if let Some(line) = line_with(out, "error: no such command:") {
        let bad = quoted_after(line, "command:").unwrap_or_default();
        return Some(
            match out
                .lines()
                .find_map(|l| l.contains("Did you mean").then(|| quoted_after(l, "Did you mean")).flatten())
            {
                Some(sug) => {
                    let fixed = if cmd.is_empty() { format!("cargo {sug}") } else { fix_command(cmd, &bad, &sug) };
                    format_answer(
                        &format!(
                            "Cargo has no command `{bad}` (`{line}`); it suggests `{sug}`, so this is probably a typo."
                        ),
                        &fixed,
                    )
                }
                None => format_answer(
                    &format!("Cargo has no command `{bad}` (`{line}`). `cargo --list` shows the commands that exist."),
                    "cargo --list",
                ),
            },
        );
    }
    if let Some(line) = out.lines().map(str::trim).find(|l| l.contains("undefined reference to")) {
        let name = quoted_after(line, "undefined reference to").unwrap_or_default();
        return Some(format_answer(
            &format!(
                "The program compiled but could not be linked: nothing defines `{name}` (`{line}`). The source file or library that defines it is missing from the build (for a library, a `-l` flag), or the name is misspelled."
            ),
            "nothing",
        ));
    }
    if let Some((place, message)) = out.lines().map(str::trim).find_map(|t| {
        let (place, rest) = t.split_once(": ")?;
        let mut parts = place.split(':');
        let (_, line, _) = (parts.next()?, parts.next()?, parts.next());
        (line.chars().all(|c| c.is_ascii_digit())
            && !line.is_empty()
            && (rest.starts_with("error: ") || rest.starts_with("fatal error: ")))
        .then(|| (place.to_string(), rest.to_string()))
    }) {
        return Some(format_answer(
            &format!(
                "The compiler rejected the code at `{place}`: `{message}`. Fix that line (or the one just before it); later errors are often just follow-ons of the first."
            ),
            "nothing",
        ));
    }
    if let Some(line) = out
        .lines()
        .map(str::trim)
        .find(|l| ["ReferenceError: ", "SyntaxError: ", "TypeError: ", "RangeError: "].iter().any(|k| l.starts_with(k)))
        && !out.contains("Traceback")
        && !out.contains("File \"")
        && (out.contains("Node.js v") || out.contains("node:internal") || out.contains("\n    at "))
    {
        let place = out
            .lines()
            .map(str::trim)
            .take_while(|l| *l != line)
            .find(|l| {
                !l.contains(char::is_whitespace)
                    && l.rsplit_once(':').is_some_and(|(_, n)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            })
            .map(|p| format!(" at `{p}`"))
            .unwrap_or_default();
        let explanation = if line.starts_with("ReferenceError") {
            format!(
                "Node stopped{place}: `{line}`. The code uses a name that was never declared or imported where it is used: check the spelling, or add the missing `require`/`import` or declaration."
            )
        } else if line.starts_with("SyntaxError") {
            format!(
                "Node could not parse the code{place}: `{line}`. There is a typo in the syntax on that line or just before it (a missing bracket, quote or operand)."
            )
        } else {
            format!(
                "Node stopped{place} with `{line}`; the line it points to used a value of the wrong type or out of range."
            )
        };
        return Some(format_answer(&explanation, "nothing"));
    }
    if (lower.contains("npm err! code e404") || lower.contains("npm error code e404"))
        && let Some(name) = out
            .lines()
            .find_map(|l| l.split("registry.npmjs.org/").nth(1).and_then(|r| r.split_whitespace().next()))
            .map(|n| n.replace("%2f", "/").replace("%2F", "/"))
            .or_else(|| out.lines().find_map(|l| quoted_after(l, "404").map(|q| q.trim_end_matches("@*").to_string())))
    {
        return Some(format_answer(
            &format!(
                "npm could not find a package called `{name}` in the registry (`404 Not Found`): the name is misspelled, or the package is private or was removed (then you need to log in or set the right registry)."
            ),
            &format!("npm search {name}"),
        ));
    }
    if let Some(line) = line_with(out, "no matching distribution found for") {
        let name = word_after(line, "for ").unwrap_or("the package");
        return Some(format_answer(
            &format!(
                "pip found no package called `{name}` that it can install (`{line}`): the name is misspelled, the package is not on PyPI, or no version of it supports this Python. Check the spelling of the name."
            ),
            &format!("pip index versions {name}"),
        ));
    }
    if let Some(line) = line_with(out, "make: *** no rule to make target") {
        let target = quoted_after(line, "target").unwrap_or_default();
        let file = cmd.split_whitespace().skip_while(|t| *t != "-f").nth(1).unwrap_or("Makefile");
        let fix = format!("grep -nE '^[A-Za-z0-9_.-]+:' {file}");
        return Some(match quoted_after(line, "needed by") {
            Some(from) => format_answer(
                &format!(
                    "`make` needs `{target}` to build `{from}` (`{line}`), but no such file exists and the Makefile has no rule that creates it. The name in the Makefile may be misspelled, or the file is missing."
                ),
                &fix,
            ),
            None => format_answer(
                &format!(
                    "The Makefile has no target called `{target}`, and no file of that name exists (`{line}`). Check the spelling against the targets the Makefile defines."
                ),
                &fix,
            ),
        });
    }
    if lower.contains("no targets specified and no makefile found") {
        return Some(format_answer(
            "`make` found no Makefile in this folder (`No targets specified and no makefile found`): you are probably in the wrong folder, or the project builds some other way.",
            "ls",
        ));
    }
    if lower.contains("jq: error") && lower.contains("compile error") {
        let line = line_with(out, "jq: error")?;
        return Some(format_answer(
            &format!(
                "jq could not compile its filter (`{line}`): the filter has a syntax error, usually an unclosed `[`, `(` or quote. Keep the filter in single quotes so the shell does not change it."
            ),
            "nothing",
        ));
    }
    if (lower.starts_with("awk:") || lower.starts_with("gawk:") || lower.starts_with("mawk:"))
        && (lower.contains("syntax error") || lower.contains("unexpected") || lower.contains("line:"))
    {
        let line = out.lines().map(str::trim).rfind(|l| !l.is_empty())?;
        return Some(format_answer(
            &format!(
                "awk could not parse its program (`{line}`): there is a syntax error, usually a missing closing `}}`, `)` or quote. Keep the program in single quotes so the shell does not change it."
            ),
            "nothing",
        ));
    }
    if let Some(line) = out.lines().map(str::trim).find(|l| l.starts_with("curl: try 'curl --help'")) {
        return Some(format_answer(
            &format!(
                "curl was started without a URL, or with arguments it could not use (`{line}`). It needs an address to fetch, as in `curl https://example.com`."
            ),
            "curl --help",
        ));
    }
    None
}

fn measured_runtime(last: &Last, lower: &str) -> Option<String> {
    let out = last.output.as_str();
    let cmd = last.command.trim();
    if let Some(line) = line_with(out, "panicked at") {
        let rest = line.split("panicked at ").nth(1)?.trim();
        let (place, message) = if rest.starts_with(['\'', '"']) {
            let (message, place) = rest[1..].rsplit_once(", ").map_or((rest, ""), |(m, p)| (m, p));
            (place.trim().to_string(), message.trim_end_matches(['\'', '"']).to_string())
        } else {
            let message =
                out.lines().skip_while(|l| !l.contains("panicked at")).nth(1).unwrap_or("").trim().to_string();
            (rest.trim_end_matches(':').to_string(), message)
        };
        let fix = if cmd.is_empty() || cmd.contains("RUST_BACKTRACE") {
            "nothing".to_string()
        } else {
            format!("RUST_BACKTRACE=1 {cmd}")
        };
        return Some(format_answer(
            &format!(
                "The Rust program panicked at `{place}`: `{message}`. That is a bug in the program (or input it did not expect), not a build problem. Running it again with `RUST_BACKTRACE=1` prints the chain of calls that led there."
            ),
            &fix,
        ));
    }
    if lower.contains("javascript heap out of memory") || lower.contains("reached heap limit") {
        let fix = if extract_command_parts(cmd).1 == "node" {
            cmd.replacen("node ", "node --max-old-space-size=4096 ", 1)
        } else if cmd.is_empty() {
            "nothing".to_string()
        } else {
            format!("NODE_OPTIONS=--max-old-space-size=4096 {cmd}")
        };
        return Some(format_answer(
            "Node ran out of memory (`JavaScript heap out of memory`): the program tried to hold more than node's default heap limit allows. Either raise the limit with `--max-old-space-size` (in megabytes), or find what keeps so much in memory (a leak, or a huge file read whole).",
            &fix,
        ));
    }
    if lower.contains("fatal error: runtime: out of memory") || lower.contains("fatal error: out of memory") {
        return Some(format_answer(
            "The Go program ran out of memory (`fatal error: runtime: out of memory`): the machine, a container or a ulimit had no memory left to give it. Reduce what the program holds in memory at once, or free memory; `free -h` shows what is available.",
            "free -h",
        ));
    }
    None
}

// ---------------------------------------------------------------------------
// Extraction & Live Machine Inspection Helpers
// ---------------------------------------------------------------------------

fn extract_command_parts(command: &str) -> (&str, &str, &str) {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return ("", "", "");
    }
    let mut offset = 0;
    let rest = trimmed;
    let tokens = rest.split_whitespace();
    let mut prefix_end = 0;
    for tok in tokens {
        let tok_start = trimmed[offset..].find(tok).map_or(offset, |i| offset + i);
        let tok_end = tok_start + tok.len();
        if tok == "sudo" || (tok.contains('=') && !tok.starts_with('-')) {
            prefix_end = tok_end;
            offset = tok_end;
            continue;
        }
        let prefix = if prefix_end > 0 { &trimmed[..tok_start] } else { "" };
        let suffix = &trimmed[tok_end..];
        return (prefix, tok, suffix);
    }
    ("", "", "")
}

fn is_line_prefix(part: &str) -> bool {
    part.strip_prefix("line ").is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// The program a shell could not find, from its error text. Handles bash
/// (`bash: line 1: x: command not found`), dash and busybox (`sh: 1: x: not found`,
/// `sh: x: not found`), a bare `x: command not found`, zsh and fish.
fn missing_from_output(output: &str) -> Option<String> {
    for line in output.lines() {
        let t = line.trim();
        let lower = t.to_lowercase();
        if let Some(rest) = lower.strip_prefix("zsh: command not found:") {
            let at = t.len() - rest.len();
            return t[at..].split_whitespace().next().map(str::to_string);
        }
        if let Some(rest) = lower.strip_prefix("fish: unknown command:") {
            let at = t.len() - rest.len();
            return t[at..].split_whitespace().next().map(str::to_string);
        }
        let end = [": command not found", ": not found"]
            .iter()
            .find(|suffix| lower.trim_end_matches('.').ends_with(*suffix))
            .map(|suffix| t.trim_end_matches('.').len() - suffix.len());
        let Some(end) = end else { continue };
        // The program is the last `:`-separated part: "bash: line 1: htop" -> "htop".
        let program = t[..end].rsplit(':').next().unwrap_or_default().trim();
        if !program.is_empty() && !program.contains(char::is_whitespace) && !program.chars().all(|c| c.is_ascii_digit())
        {
            return Some(program.to_string());
        }
    }
    None
}

fn extract_exec_target(last: &Last) -> String {
    for line in last.output.lines() {
        let t = line.trim();
        if let Some(after_shell) = t.strip_prefix("bash: ").or_else(|| t.strip_prefix("sh: ")) {
            for part in after_shell.split(':').map(str::trim) {
                if !part.is_empty() && !part.chars().all(|c| c.is_ascii_digit()) && !is_line_prefix(part) {
                    return part.to_string();
                }
            }
        }
        if let Some(after_zsh) = t.strip_prefix("zsh: permission denied:") {
            return after_zsh.trim().to_string();
        }
    }
    let (_, prog, _) = extract_command_parts(&last.command);
    if prog.is_empty() { "<file>".to_string() } else { prog.to_string() }
}

fn extract_pacman_missing_target(output: &str) -> Option<String> {
    for line in output.lines() {
        let low = line.to_lowercase();
        if let Some(pos) = low.find("target not found:") {
            let pkg = line[pos + "target not found:".len()..].split_whitespace().next().unwrap_or("").trim();
            if !pkg.is_empty() {
                return Some(pkg.to_string());
            }
        }
    }
    None
}

fn extract_pacman_conflict_path(output: &str) -> Option<String> {
    for line in output.lines() {
        let low = line.to_lowercase();
        if let Some(pos) = low.find("exists in filesystem") {
            let before = line[..pos].trim();
            if let Some(tok) = before.split_whitespace().rfind(|w| w.starts_with('/')) {
                return Some(tok.trim_matches(['\'', '"', ':']).to_string());
            }
        }
    }
    None
}

fn extract_systemd_failed_action(output: &str) -> Option<String> {
    for line in output.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("Failed to ")
            && let Some((action_and_unit, _)) = rest.split_once(':')
        {
            let mut parts = action_and_unit.split_whitespace();
            let verb = parts.next()?;
            let unit = parts.next()?;
            if matches!(verb, "start" | "stop" | "restart" | "reload" | "enable" | "disable") {
                return Some(format!("systemctl {verb} {unit}"));
            }
        }
    }
    None
}

fn extract_tar_archive(last: &Last) -> Option<String> {
    let tokens: Vec<&str> = last.command.split_whitespace().collect();
    for (i, tok) in tokens.iter().enumerate() {
        if (*tok == "-f" || (tok.starts_with('-') && !tok.starts_with("--") && tok.ends_with('f')))
            && let Some(file) = tokens.get(i + 1)
            && !file.starts_with('-')
        {
            return Some((*file).to_string());
        }
    }
    for tok in tokens.iter().skip(1) {
        if !tok.starts_with('-')
            && (tok.ends_with(".tar")
                || tok.ends_with(".tar.gz")
                || tok.ends_with(".tgz")
                || tok.ends_with(".tar.bz2")
                || tok.ends_with(".tar.xz")
                || tok.ends_with(".tar.zst"))
        {
            return Some((*tok).to_string());
        }
    }
    if let Some(tok) = tokens.iter().skip(1).rfind(|t| !t.starts_with('-')) {
        return Some((*tok).to_string());
    }
    for tok in last.output.split_whitespace() {
        let cleaned = tok.trim_matches(['\'', '"', ':', ',', '`']);
        if cleaned.ends_with(".tar")
            || cleaned.ends_with(".tar.gz")
            || cleaned.ends_with(".tgz")
            || cleaned.ends_with(".tar.bz2")
            || cleaned.ends_with(".tar.xz")
            || cleaned.ends_with(".tar.zst")
        {
            return Some(cleaned.to_string());
        }
    }
    None
}

fn is_git_ssh_target(host: &str, command: &str) -> bool {
    let h = host.to_lowercase();
    let c = command.trim_start().to_lowercase();
    h.starts_with("git@")
        || h.contains("github.com")
        || h.contains("gitlab.com")
        || h.contains("bitbucket.org")
        || h.contains("codeberg.org")
        || h.contains("sr.ht")
        || c.starts_with("git ")
}

fn extract_ssh_port(command: &str, output: &str) -> Option<String> {
    for line in output.lines() {
        let low = line.to_lowercase();
        if let Some(pos) = low.find(" port ") {
            let rest = &line[pos + " port ".len()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                return Some(digits);
            }
        }
    }
    let mut iter = command.split_whitespace();
    while let Some(tok) = iter.next() {
        if tok == "-p" {
            if let Some(p) = iter.next()
                && p.chars().all(|c| c.is_ascii_digit())
            {
                return Some(p.to_string());
            }
        } else if let Some(p) = tok.strip_prefix("-p")
            && !p.is_empty()
            && p.chars().all(|c| c.is_ascii_digit())
        {
            return Some(p.to_string());
        }
    }
    None
}

fn extract_systemd_failed_service(output: &str) -> Option<String> {
    for line in output.lines() {
        if let Some(rest) = line.trim().strip_prefix("Job for ")
            && let Some((svc, _)) = rest.split_once(" failed")
        {
            return Some(svc.trim().to_string());
        }
    }
    None
}

fn extract_systemd_missing_unit(output: &str) -> Option<String> {
    for line in output.lines() {
        let low = line.to_lowercase();
        if let Some(pos) = low.find("unit ") {
            let rest = &line[pos + "unit ".len()..];
            if let Some(end_pos) =
                rest.to_lowercase().find(" not found").or_else(|| rest.to_lowercase().find(" could not be found"))
            {
                let unit = rest[..end_pos].trim().trim_matches(['\'', '"', '.']);
                if !unit.is_empty() && !unit.contains(' ') {
                    return Some(unit.to_string());
                }
            }
        }
    }
    None
}

fn extract_git_branch(output: &str) -> Option<String> {
    for line in output.lines() {
        if let Some(rest) = line.trim().strip_prefix("fatal: The current branch ")
            && let Some((branch, _)) = rest.split_once(" has no upstream branch")
        {
            return Some(branch.trim().to_string());
        }
        if let Some(pos) = line.find("--set-upstream origin ") {
            let branch = line[pos + "--set-upstream origin ".len()..].split_whitespace().next()?;
            if !branch.is_empty() {
                return Some(branch.to_string());
            }
        }
    }
    None
}

fn extract_ssh_target(command: &str, output: &str) -> Option<String> {
    for line in output.lines() {
        let t = line.trim();
        if let Some((host, _rest)) = t.split_once(": Permission denied")
            && !host.is_empty()
            && !host.contains(' ')
        {
            return Some(host.to_string());
        }
        if let Some(rest) = t.strip_prefix("ssh: connect to host ")
            && let Some(host) = rest.split_whitespace().next()
        {
            return Some(host.to_string());
        }
    }
    let mut skip_next = false;
    for tok in command.split_whitespace().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }
        if matches!(tok, "-p" | "-i" | "-l" | "-o" | "-F" | "-J") {
            skip_next = true;
            continue;
        }
        if !tok.starts_with('-') {
            return Some(tok.to_string());
        }
    }
    None
}

fn extract_mountpoint(output: &str) -> Option<String> {
    for line in output.lines() {
        let t = line.trim();
        for prefix in ["mount: ", "umount: "] {
            if let Some(rest) = t.strip_prefix(prefix)
                && let Some((path, _)) = rest.split_once(':')
            {
                let path = path.trim();
                if !path.is_empty() {
                    return Some(path.to_string());
                }
            }
        }
    }
    None
}

fn extract_missing_path(last: &Last) -> String {
    if let Some(failure) = file_failures(&last.output).iter().find(|f| f.problem == FileProblem::Missing) {
        return failure.path.clone();
    }
    for line in last.output.lines() {
        let t = line.trim();
        if let Some(pos) = t.find("cannot find or open ") {
            let rest = &t[pos + "cannot find or open ".len()..];
            let first = rest.split([',', ':', '\n']).next().unwrap_or("").trim();
            if !first.is_empty() {
                return first.to_string();
            }
        }
        if let Some(pos) = t.find(": No such file or directory") {
            let before = &t[..pos];
            if let Some((_, target)) = before.rsplit_once(':') {
                let target = target.trim().trim_matches(['\'', '"']);
                if !target.is_empty() {
                    return target.to_string();
                }
            }
        }
    }
    last.command.split_whitespace().skip(1).find(|tok| !tok.starts_with('-')).unwrap_or("").to_string()
}

fn extract_quoted(text: &str, quotes: &[char]) -> Option<String> {
    for &q in quotes {
        if let Some(start) = text.find(q) {
            let rest = &text[start + q.len_utf8()..];
            if let Some(end) = rest.find(q) {
                return Some(rest[..end].to_string());
            }
        }
    }
    None
}

fn extract_port(last: &Last) -> Option<String> {
    // 0. Node: `listen EADDRINUSE: address already in use :::8099` (other lines have `node:events:495`)
    if let Some(line) = last.output.lines().find(|l| l.contains("EADDRINUSE") || l.contains("already in use"))
        && let Some(tail) = line.rsplit(':').next()
    {
        let digits: String = tail.trim().chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            return Some(digits);
        }
    }
    // 1. Check output for `:<port>` or `0.0.0.0:<port>/tcp`
    for tok in last.output.split_whitespace() {
        if let Some((_, after)) = tok.rsplit_once(':') {
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(port) = digits.parse::<u16>()
                && port > 0
            {
                return Some(digits);
            }
        }
    }
    // 2. Check command for `-p <host>:<container>` or standalone port numbers like `8000`
    let tokens: Vec<&str> = last.command.split_whitespace().collect();
    for (i, tok) in tokens.iter().enumerate() {
        if (*tok == "-p" || *tok == "--publish")
            && let Some(mapping) = tokens.get(i + 1)
        {
            let host_port = mapping.split(':').next().unwrap_or("");
            if let Ok(port) = host_port.parse::<u16>()
                && port > 0
            {
                return Some(host_port.to_string());
            }
        }
        if let Ok(port) = tok.parse::<u16>()
            && port >= 80
        {
            return Some((*tok).to_string());
        }
    }
    None
}

fn extract_python_module(output: &str) -> Option<String> {
    for line in output.lines() {
        if let Some(pos) = line.find("No module named ") {
            let rest = line[pos + "No module named ".len()..].trim();
            let mod_name = rest.trim_matches(['\'', '"', '.', ' ']);
            if !mod_name.is_empty() {
                return Some(mod_name.to_string());
            }
        }
    }
    None
}

fn last_meaningful_line(output: &str) -> Option<&str> {
    let lines: Vec<&str> = output.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if let Some(err) = lines.iter().rev().find(|l| {
        let low = l.to_lowercase();
        low.contains("error") || low.contains("fatal") || low.contains("failed") || low.contains("cannot")
    }) {
        return Some(*err);
    }
    lines.last().copied()
}

/// Is the user in `group`? `None` when that could not be looked up.
fn user_in_group_checked(group: &str) -> Option<bool> {
    let user = std::env::var("USER").ok()?;
    let groups = std::fs::read_to_string("/etc/group").ok()?;
    Some(groups.lines().any(|line| {
        let mut cols = line.split(':');
        let Some(name) = cols.next() else { return false };
        if name != group {
            return false;
        }
        cols.nth(2).is_some_and(|members| members.split(',').any(|m| m.trim() == user))
    }))
}

fn scan_path_executables() -> Vec<String> {
    let mut seen = BTreeSet::new();
    for dir in crate::platform::path_dirs() {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            if !crate::platform::is_executable_file(&entry.path()) {
                continue;
            }
            if let Some(name) = entry.file_name().to_str() {
                seen.insert(name.to_string());
            }
        }
    }
    seen.into_iter().collect()
}

fn scan_cwd_entries(dirs_only: bool) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(".") else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| !dirs_only || e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .collect()
}

/// Optimal String Alignment (restricted Damerau-Levenshtein) distance: counts adjacent transpositions as 1.
fn damerau_levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (m, n) = (a.len(), b.len());
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }
    let mut d = vec![vec![0usize; n + 1]; m + 1];
    for (i, row) in d.iter_mut().enumerate().take(m + 1) {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate().take(n + 1) {
        *cell = j;
    }
    for i in 1..=m {
        for j in 1..=n {
            let cost = usize::from(!a[i - 1].eq_ignore_ascii_case(&b[j - 1]));
            let mut val = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1].eq_ignore_ascii_case(&b[j - 2]) && a[i - 2].eq_ignore_ascii_case(&b[j - 1]) {
                val = val.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = val;
        }
    }
    d[m][n]
}

fn closest_typo(target: &str, candidates: &[String], max_dist: usize) -> Option<String> {
    let target_len = target.chars().count();
    let target_first = target.chars().next().map(|c| c.to_ascii_lowercase());
    let mut best: Option<(usize, bool, bool, bool, usize, String)> = None;

    for cand in candidates {
        let cand_len = cand.chars().count();
        if cand_len.abs_diff(target_len) > max_dist {
            continue;
        }
        let dist = damerau_levenshtein(target, cand);
        if dist == 0 || dist > max_dist {
            continue;
        }
        let same_first = cand.chars().next().map(|c| c.to_ascii_lowercase()) == target_first;
        let len_diff = cand_len.abs_diff(target_len);
        let popular = COMMON_COMMANDS.contains(&cand.as_str());
        let swapped = dist == 1 && cand_len == target_len && {
            let sorted = |word: &str| {
                let mut chars: Vec<char> = word.to_lowercase().chars().collect();
                chars.sort_unstable();
                chars
            };
            sorted(target) == sorted(cand)
        };
        let key = (dist, !swapped, !popular, !same_first, len_diff, cand.clone());
        if best.as_ref().is_none_or(|b| key < *b) {
            best = Some(key);
        }
    }
    best.map(|(.., cand)| cand)
}

// ---------------------------------------------------------------------------
// Round 3: causes that used to be mislabelled or missed ("never mislead")
// ---------------------------------------------------------------------------

/// The first line of `output` whose lower-case form contains `needle`, trimmed.
fn first_line_with<'a>(output: &'a str, needle: &str) -> Option<&'a str> {
    output.lines().map(str::trim).find(|l| l.to_lowercase().contains(needle))
}

/// Specific, unmistakable messages: each rule needs a line only that failure prints.
fn diagnose_round3(last: &Last) -> Option<String> {
    diagnose_python_setup(last)
        .or_else(|| diagnose_script_line_endings(last))
        .or_else(|| diagnose_git_round3(last))
        .or_else(|| diagnose_admin(last))
        .or_else(|| diagnose_tests_and_builds(last))
        .or_else(|| diagnose_system_errors(last))
        .or_else(|| diagnose_remote_errors(last))
}

/// A package-manager command for `package` on this system, if the platform is known.
fn install_command(program: &str) -> Option<String> {
    let platforms = crate::platform::detect();
    crate::platform::install_advice(program, None, &platforms).filter(|a| a.installs).map(|a| a.command)
}

fn diagnose_python_setup(last: &Last) -> Option<String> {
    let out = last.output.as_str();
    let lower = out.to_lowercase();
    if lower.split_whitespace().collect::<Vec<_>>().join(" ").contains("ensurepip is not available") {
        let words: Vec<&str> = out
            .split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-'))
            .collect();
        let is_venv_package = |w: &&str| w.starts_with("python3") && w.ends_with("-venv");
        // The package in the command the message prints (`apt install python3.11-venv`) is the one
        // that matches this Python's version.
        let package = words
            .windows(2)
            .find(|pair| pair[0] == "install" && is_venv_package(&pair[1]))
            .map(|pair| pair[1])
            .or_else(|| words.iter().copied().find(is_venv_package))
            .unwrap_or("python3-venv");
        return Some(format_answer(
            &format!(
                "Python could not create the virtual environment because its `ensurepip` module is missing: Debian and Ubuntu ship it in a separate package, `{package}`. Install that package, then run the `venv` command again."
            ),
            &format!("sudo apt install {package}"),
        ));
    }
    let pip_missing = out.lines().map(str::trim).any(|l| {
        let low = l.to_lowercase();
        low.contains("python") && low.trim_end_matches('.').ends_with(": no module named pip")
    });
    if pip_missing {
        let fix = install_command("pip").unwrap_or_else(|| "python3 -m ensurepip --user".to_string());
        return Some(format_answer(
            "This Python has no `pip` module (`No module named pip`), so `python3 -m pip` cannot run. The distribution's pip package adds it; where Python ships `ensurepip`, `python3 -m ensurepip --user` also does. A virtual environment cannot help: it needs pip from one of those sources too.",
            &fix,
        ));
    }
    None
}

/// The script named in a shell's exec error: `bash: line 1: ./x.sh: ...` gives `./x.sh`.
fn script_in_exec_error(line: &str) -> Option<String> {
    line.split(": ")
        .map(str::trim)
        .find(|part| {
            !part.is_empty()
                && !matches!(*part, "bash" | "-bash" | "sh" | "zsh" | "dash" | "fish")
                && !is_line_prefix(part)
                && !part.contains(char::is_whitespace)
        })
        .map(str::to_string)
}

fn diagnose_script_line_endings(last: &Last) -> Option<String> {
    let line = last.output.lines().map(str::trim).find(|l| {
        let low = l.to_lowercase();
        (low.contains("bad interpreter") && (l.contains("^M") || l.contains('\r')))
            || low.contains("cannot execute: required file not found")
    })?;
    let script = script_in_exec_error(line)?;
    let fix = format!("sed -i 's/\\r$//' {script}");
    if line.contains("^M") || line.contains('\r') {
        return Some(format_answer(
            &format!(
                "`{script}` has Windows line endings (CRLF): the carriage return at the end of its `#!` line became part of the interpreter's name, so the system looks for a program called `...^M`, which is not a real program. Converting the file to Unix line endings fixes it (`dos2unix {script}` does the same)."
            ),
            &fix,
        ));
    }
    // "required file not found": either the interpreter named on the `#!` line is missing, or
    // that line ends in a carriage return. A script on disk tells which.
    let first_line_has_cr = std::fs::read(&script).ok().is_some_and(|bytes| {
        let head = &bytes[..bytes.len().min(256)];
        head.iter().position(|b| *b == b'\n').is_some_and(|end| end > 0 && head[end - 1] == b'\r')
    });
    if first_line_has_cr {
        return Some(format_answer(
            &format!(
                "`{script}` has Windows line endings (CRLF): its `#!` line ends in a carriage return, so the system cannot find an interpreter with that name (`required file not found`). `dos2unix {script}` does the same as the command below."
            ),
            &fix,
        ));
    }
    Some(format_answer(
        &format!(
            "The system could not run `{script}` (`required file not found`): the file exists, but the interpreter named on its first (`#!`) line does not, or that line ends in a Windows carriage return (CRLF). The first line shown with `cat -A` tells which: `^M` at its end means CRLF."
        ),
        &format!("head -n1 {script} | cat -A"),
    ))
}

fn diagnose_git_round3(last: &Last) -> Option<String> {
    let out = last.output.as_str();
    let lower = out.to_lowercase();
    if lower.contains("could not apply") && (lower.contains("git rebase --continue") || lower.contains("rebasing")) {
        let files = conflicted_files(out);
        let which = match files.as_slice() {
            [] => "the files git lists as conflicted".to_string(),
            [one] => format!("`{one}`"),
            many => many.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", "),
        };
        return Some(format_answer(
            &format!(
                "A `git rebase` stopped because replaying one of your commits conflicts with the new base. This is a rebase, not a merge: fix the conflict markers (`<<<<<<<` ... `>>>>>>>`) in {which}, run `git add` on each file, then `git rebase --continue`. `git rebase --skip` drops that commit and `git rebase --abort` returns to how things were before the rebase."
            ),
            "git status",
        ));
    }
    if lower.contains("no tracking information for the current branch") {
        let branch = out
            .lines()
            .find(|l| l.contains("--set-upstream-to="))
            .and_then(|l| l.split_whitespace().last())
            .filter(|b| !b.contains(['<', '=']))
            .map(str::to_string);
        return Some(match branch {
            Some(b) => format_answer(
                &format!(
                    "`git pull` does not know which remote branch `{b}` follows (`There is no tracking information for the current branch`). `git branch --set-upstream-to=origin/{b} {b}` records it if `origin/{b}` exists; for a branch that exists only here, `git push -u origin {b}` creates it on the remote and sets the link."
                ),
                &format!("git branch --set-upstream-to=origin/{b} {b}"),
            ),
            None => format_answer(
                "`git pull` does not know which remote branch this branch follows (`There is no tracking information for the current branch`). `git branch --set-upstream-to=origin/<branch>` records it, or name the remote branch: `git pull origin <branch>`.",
                "nothing",
            ),
        });
    }
    None
}

const ADMIN_TOOLS: &[&str] = &[
    "useradd",
    "userdel",
    "usermod",
    "groupadd",
    "groupdel",
    "groupmod",
    "passwd",
    "chpasswd",
    "mount",
    "umount",
    "fdisk",
    "parted",
    "modprobe",
    "iptables",
    "ufw",
    "apt",
    "apt-get",
    "dpkg",
    "dnf",
    "yum",
    "zypper",
    "apk",
    "systemctl",
    "hostnamectl",
    "timedatectl",
    "visudo",
];

fn is_admin_tool(name: &str) -> bool {
    ADMIN_TOOLS.contains(&name) || name.starts_with("mkfs")
}

/// System-administration commands refused for lack of privileges need `sudo`: the filename
/// logic for ordinary files does not apply to them.
fn diagnose_admin(last: &Last) -> Option<String> {
    let out = last.output.as_str();
    let lower = out.to_lowercase();
    let trimmed = last.command.trim();
    if trimmed.starts_with("sudo ") || trimmed.starts_with("doas ") {
        return None;
    }
    // sed -i writes a temporary file next to the one it edits.
    if let Some(line) = first_line_with(out, "couldn't open temporary file")
        && lower.contains("permission denied")
    {
        let path = line.split("file ").nth(1)?.split(':').next()?.trim();
        let dir = Path::new(path).parent()?.display().to_string();
        if under_system_dir(&dir) {
            let (_, prog, _) = extract_command_parts(trimmed);
            let fix = if matches!(prog, "sed" | "gsed") { format!("sudo {trimmed}") } else { "nothing".to_string() };
            return Some(format_answer(
                &format!(
                    "`sed -i` edits a file by writing a temporary file next to it, in `{dir}`, and only an administrator (root) can create files there (`{}`). Run the same edit with sudo.",
                    line.trim()
                ),
                &fix,
            ));
        }
        return None;
    }
    let denial_words = [
        "permission denied",
        "are you root",
        "only root",
        "must be root",
        "must be superuser",
        "unless you are root",
        "you may not view or modify",
        "interactive authentication required",
        "operation not permitted",
    ];
    let denial = out.lines().map(str::trim).find(|l| {
        let low = l.to_lowercase();
        denial_words.iter().any(|w| low.contains(w))
    })?;
    let (_, prog, _) = extract_command_parts(trimmed);
    let prefix = denial.split(':').next().unwrap_or("").trim();
    let apt_lock = lower.contains("/var/lib/dpkg/lock")
        || lower.contains("/var/lib/apt/lists/lock")
        || lower.contains("/var/cache/apt/archives/lock");
    let from_output = if apt_lock { "apt" } else { prefix };
    let tool = if is_admin_tool(prog) { prog } else { from_output };
    if !is_admin_tool(tool) || (tool == "systemctl" && trimmed.split_whitespace().any(|w| w == "--user")) {
        return None;
    }
    let fix = if prog == tool { format!("sudo {trimmed}") } else { "nothing".to_string() };
    let rerun = if fix == "nothing" { "Run the same command again with sudo in front of it." } else { "" };
    Some(format_answer(
        format!(
            "`{tool}` changes the system itself (accounts, packages, mounts or services), so only an administrator (root) may run it, and your user is refused (`{denial}`). {rerun}"
        )
        .trim_end(),
        &fix,
    ))
}

/// The number written before `word` on a line (`1 failed, 2 passed` gives 1 for "failed").
fn count_before(line: &str, word: &str) -> Option<u32> {
    let at = line.find(word)?;
    line[..at].split_whitespace().next_back()?.trim_matches(',').parse().ok()
}

fn diagnose_tests_and_builds(last: &Last) -> Option<String> {
    let out = last.output.as_str();
    let lower = out.to_lowercase();
    let lines: Vec<&str> = out.lines().map(str::trim).collect();

    if let Some(answer) = diagnose_makefile(last) {
        return Some(answer);
    }
    // Go: `FAIL example.com/app [build failed]`
    if let Some(fail) = lines.iter().find(|l| l.contains("[build failed]") || l.contains("[setup failed]")) {
        let package = fail.split_whitespace().nth(1).unwrap_or("a package");
        let first = lines.iter().find(|l| {
            l.split(':').nth(1).is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty())
                && l.split(':').next().is_some_and(|f| f.ends_with(".go"))
        });
        let detail = first.map_or(String::new(), |l| format!(" The first compiler error is {}.", quote_line(l)));
        return Some(format_answer(
            &format!(
                "Go could not compile `{package}` (`[build failed]`), so none of its tests ran.{detail} The code has to change; the compiler lines above the `FAIL` line say where."
            ),
            "nothing",
        ));
    }
    // Jest and friends: `Tests:       1 failed, 1 total`
    let jest_line =
        lines.iter().find(|l| l.starts_with("Tests:") && count_before(l, "failed").is_some_and(|n| n > 0)).or_else(
            || lines.iter().find(|l| l.starts_with("Test Suites:") && count_before(l, "failed").is_some_and(|n| n > 0)),
        );
    if let Some(summary) = jest_line {
        let files: Vec<&str> = lines
            .iter()
            .filter_map(|l| l.strip_prefix("FAIL "))
            .map(|f| f.split_whitespace().next().unwrap_or(f))
            .collect();
        let named = if files.is_empty() {
            String::new()
        } else {
            format!(" Failing: {}.", files.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", "))
        };
        return Some(format_answer(
            &format!(
                "Some tests failed (`{summary}`).{named} The summary only counts them: the expected and received values are printed above it, next to the line of each assertion."
            ),
            "nothing",
        ));
    }
    // pytest: `FAILED test_a.py::test_a - assert 1 == 2`, `1 failed in 0.01s`
    let pytest_summary = lines.iter().find(|l| {
        let l = l.trim_matches('=').trim();
        l.contains(" in ")
            && l.ends_with('s')
            && (count_before(l, "failed").is_some_and(|n| n > 0) || l.contains("error"))
    });
    let pytest_failed: Vec<&str> =
        lines.iter().filter_map(|l| l.strip_prefix("FAILED ").or_else(|| l.strip_prefix("ERROR "))).collect();
    if lower.contains("short test summary info") || (pytest_summary.is_some() && !pytest_failed.is_empty()) {
        let summary = pytest_summary.map(|s| s.trim_matches('=').trim().to_string()).unwrap_or_default();
        let names: Vec<String> =
            pytest_failed.iter().map(|f| format!("`{}`", f.split(" - ").next().unwrap_or(f).trim())).take(5).collect();
        let named = if names.is_empty() { String::new() } else { format!(" Failing: {}.", names.join(", ")) };
        let counted = if summary.is_empty() { String::new() } else { format!(" (`{summary}`)") };
        return Some(format_answer(
            &format!(
                "pytest ran the tests and some failed{counted}.{named} The assertion that did not hold is shown above the summary, with the values that were compared."
            ),
            "nothing",
        ));
    }
    if lower.contains("npm err! test failed") || lower.contains("npm error test failed") {
        return Some(format_answer(
            "`npm test` ran the package's test script and that script exited with an error. npm only reports it (`Test failed. See above for more details`): what failed is printed above those npm lines.",
            "nothing",
        ));
    }
    if let Some(line) = first_line_with(out, "pkgbuild does not exist") {
        return Some(format_answer(
            &format!(
                "`makepkg` builds the package described by a file named `PKGBUILD` in the current folder, and there is none here (`{line}`). Change into the folder that holds it first."
            ),
            "nothing",
        ));
    }
    // make: `make: *** [Makefile:2: all] Error 1` (a recipe command failed)
    if !lower.contains("missing separator")
        && let Some(at) =
            lines.iter().position(|l| l.starts_with("make") && l.contains("*** [") && l.contains("Error "))
    {
        let make_line = lines[at];
        let inner = make_line.split("*** [").nth(1).and_then(|r| r.split(']').next()).unwrap_or("");
        let (place, target) = match inner.split_once(": ") {
            Some((loc, target)) => (Some(loc), target),
            None => (None, inner),
        };
        let place = place
            .and_then(|loc| loc.rsplit_once(':'))
            .map_or(String::new(), |(file, line)| format!(" (`{file}` line {line})"));
        // The recipe command printed its own error above the make line: explain that first.
        let recipe_output: Vec<&str> = lines[..at].iter().copied().filter(|l| !l.starts_with("make")).collect();
        if !recipe_output.is_empty() {
            let inner = diagnose(&Last { command: String::new(), exit: None, output: recipe_output.join("\n") });
            let generic =
                inner.starts_with("huh does not recognise") || inner.starts_with("The command failed with an error");
            if !generic {
                let note = format!(" This stopped make in the recipe for the target `{target}`{place}.");
                return Some(match inner.split_once("\nTry: ") {
                    Some((text, try_cmd)) => format!("{text}{note}\nTry: {try_cmd}"),
                    None => format!("{inner}{note}"),
                });
            }
        }
        let above = lines[..at].iter().rev().find(|l| !l.is_empty() && !l.starts_with("make"));
        let said = above.map_or(String::new(), |l| format!(" The output just above it is {}.", quote_line(l)));
        let status = make_line.rsplit("Error ").next().unwrap_or("").trim();
        return Some(format_answer(
            &format!(
                "A command in the recipe for the target `{target}`{place} failed, so make stopped. `Error {status}` is only that command's exit status: what went wrong is printed by the command itself, above the make line.{said}"
            ),
            "nothing",
        ));
    }
    if let Some(line) = lines.iter().find(|l| l.to_lowercase().starts_with("grep: unmatched")) {
        let open = lower.contains("unmatched ( or");
        let hint = if open {
            "an unescaped `(` starts a group that is never closed; write `\\(` to search for a literal parenthesis, or use `grep -F` for plain text"
        } else {
            "a bracket or brace in the pattern is never closed; escape it with a backslash to search for the character itself, or use `grep -F` for plain text"
        };
        return Some(format_answer(
            &format!("`grep` could not read its pattern as a regular expression (`{line}`): {hint}."),
            "nothing",
        ));
    }
    if let Some(line) = lines.iter().find(|l| {
        let low = l.to_lowercase();
        low.contains("file format not recognized")
            && (low.starts_with("xz:") || low.starts_with("unxz:") || low.starts_with("lzma:"))
    }) {
        let file = line.split(": ").nth(1).unwrap_or("the file");
        return Some(format_answer(
            &format!(
                "`{file}` is not in the xz format (`{line}`): it is not an xz archive, or the download or copy is damaged. `file` says what it really is."
            ),
            &format!("file {file}"),
        ));
    }
    if let Some(line) = lines.iter().find(|l| l.to_lowercase().contains("requires an argument")) {
        let option = if let Some(at) = line.find("-- '") {
            line[at + 4..].chars().next().map(|c| format!("-{c}"))
        } else {
            quoted_after(line, "option").map(|c| format!("-{c}"))
        };
        let which = option.map_or(String::new(), |o| format!(" `{o}`"));
        return Some(format_answer(
            &format!(
                "An option{which} was given without the value it needs (`{line}`): put the value right after it. A value that starts with a dash or a missing shell variable is a common reason it looks absent."
            ),
            "nothing",
        ));
    }
    if let Some(line) = lines.iter().find(|l| l.starts_with("parse error:") && l.contains(" at line ")) {
        return Some(format_answer(
            &format!(
                "`jq` could not parse its input as JSON (`{line}`): what was piped into it is not valid JSON at that line and column. Typical causes are a trailing comma, single quotes, or an error page or message instead of the data. Look at the input itself, just before jq."
            ),
            "nothing",
        ));
    }
    if lower.contains("multiple definition of") {
        let mut symbols: Vec<String> = Vec::new();
        for line in out.lines().filter(|l| l.contains("multiple definition of")) {
            if let Some(s) = quoted_after(line, "multiple definition of")
                && !symbols.contains(&s)
            {
                symbols.push(s);
            }
        }
        let names = symbols.iter().map(|s| format!("`{s}`")).collect::<Vec<_>>().join(", ");
        return Some(format_answer(
            &format!(
                "The linker found {names} defined more than once (`multiple definition`) in the object files being joined. Usual causes: a variable or function defined (not just declared) in a header included by several .c files, or the same source file listed twice."
            ),
            "nothing",
        ));
    }
    None
}

fn diagnose_system_errors(last: &Last) -> Option<String> {
    let out = last.output.as_str();
    let lower = out.to_lowercase();
    if let Some(line) = first_line_with(out, "is not in the sudoers file") {
        return Some(format_answer(
            &format!(
                "Your account is not allowed to use sudo (`{line}`). Only an administrator of this machine can grant that: ask them to give your account sudo rights. Nothing you run as yourself can do it."
            ),
            "nothing",
        ));
    }
    if lower.contains("system has not been booted with systemd") {
        return Some(format_answer(
            "systemd is not running here (`System has not been booted with systemd as init system`), so `systemctl` has nothing to talk to. That is normal inside a Docker container and in WSL without systemd enabled. Run the service's own program directly, or in WSL turn systemd on in `/etc/wsl.conf` (`[boot]` then `systemd=true`) and restart WSL.",
            "nothing",
        ));
    }
    if let Some(line) = first_line_with(out, "too many levels of symbolic links") {
        let at = line.to_lowercase().find(": too many levels")?;
        let path = path_in(&line[..at]);
        let (shown, fix) = match path {
            Some(p) => (format!("`{p}`"), format!("ls -l {p}")),
            None => ("A path in the command".to_string(), "nothing".to_string()),
        };
        return Some(format_answer(
            &format!(
                "{shown} runs into a symbolic link loop (`Too many levels of symbolic links`): following the links leads back to where it started, or is chained too deep. `ls -l` shows where each link points."
            ),
            &fix,
        ));
    }
    if let Some(line) = out.lines().map(str::trim).find(|l| {
        let low = l.to_lowercase();
        low.contains("already exists") && low.contains("not overwritten")
    }) {
        let prog = line.split(':').next().unwrap_or("the program");
        let file = line.split(": ").nth(1).and_then(|r| r.split(" already exists").next()).unwrap_or("the output file");
        return Some(format_answer(
            &format!(
                "`{prog}` refused to overwrite `{file}`, which already exists (`already exists; not overwritten`). Move or rename the old file if you want to keep it; `-f` makes {prog} overwrite it, and the old content is lost."
            ),
            "nothing",
        ));
    }
    if lower.contains("invalid or corrupted package (pgp signature)")
        || (lower.contains("signature from") && (lower.contains("unknown trust") || lower.contains("is invalid")))
    {
        let line = first_line_with(out, "signature").unwrap_or("signature");
        return Some(format_answer(
            &format!(
                "pacman could not verify a package's signature (`{line}`). This usually means the keyring of packager keys is out of date. Update the keyring first, then the system; do not turn signature checking off."
            ),
            "sudo pacman -Sy archlinux-keyring && sudo pacman -Syu",
        ));
    }
    // glibc's own heap messages only: the word "corrupted" alone is used by many programs.
    let memory_error = [
        "double free",
        "free(): invalid",
        "corrupted size vs",
        "corrupted double-linked list",
        "corrupted top size",
        "munmap_chunk",
        "malloc(): ",
        "malloc_consolidate",
    ]
    .iter()
    .find(|m| lower.contains(*m));
    if let Some(m) = memory_error {
        let line = first_line_with(out, m).unwrap_or(m);
        return Some(format_answer(
            &format!(
                "The program stopped itself after the C library found a memory error (`{line}`): it freed memory twice or damaged the heap. That is a bug in the program, not in your system. Running it under `valgrind`, or building with `-fsanitize=address`, points at the line."
            ),
            "nothing",
        ));
    }
    let other_cause = lower.contains("out of memory") || lower.contains("fatal error") || lower.contains("assert");
    if !other_cause
        && (lower.contains("aborted") && lower.contains("(core dumped)")
            || (last.exit == Some(134) && lower.contains("abort")))
    {
        return Some(format_answer(
            "The program aborted itself (SIGABRT, `Aborted (core dumped)`), usually after a failed assertion or an unrecoverable error; it is a crash of the program, not of the system. Any message printed just before the line `Aborted` says why.",
            "nothing",
        ));
    }
    if lower.contains("too many open files") && !lower.contains("in system") {
        return Some(format_answer(
            "The program hit the limit on files it may have open at once (`Too many open files`). `ulimit -n` shows the limit for this shell; either raise it (`ulimit -n 4096`, for this shell and what it starts) or fix the program so it closes files and sockets it no longer needs.",
            "ulimit -n",
        ));
    }
    None
}

fn diagnose_remote_errors(last: &Last) -> Option<String> {
    let out = last.output.as_str();
    let lower = out.to_lowercase();
    if let Some(line) = first_line_with(out, " are too open") {
        let low = line.to_lowercase();
        if low.starts_with("permissions ") {
            let path = quoted_after(line, "for")?;
            return Some(format_answer(
                &format!(
                    "ssh refuses to use the private key `{path}` because other users can read it (`{line}`), and it then falls back to other ways to log in. A private key must be readable by its owner only."
                ),
                &format!("chmod 600 {path}"),
            ));
        }
    }
    if lower.contains("curl: (60)") && lower.contains("expired") {
        return Some(format_answer(
            "curl refused the connection because the server's TLS certificate has expired (`curl: (60)`). Either the site's owner has not renewed it, or this computer's clock is wrong, which makes valid certificates look expired: `date` shows the clock. Turning certificate checks off would hide the problem, not fix it.",
            "nothing",
        ));
    }
    if lower.contains("curl: (56)") {
        let line = first_line_with(out, "curl: (56)").unwrap_or("curl: (56)");
        return Some(format_answer(
            &format!(
                "The connection was cut while curl was receiving the answer (`{line}`): the server, or a proxy or firewall in between, closed it abruptly. It may be temporary; if it repeats, the server is overloaded or rejecting this kind of request."
            ),
            "nothing",
        ));
    }
    if lower.contains("protected branch") || lower.contains("gh006") {
        return Some(format_answer(
            "The server refused the push because the branch is protected (`Protected branch update failed`): direct pushes to it are blocked. Push your work to a new branch and open a pull request, as the message says.",
            "nothing",
        ));
    }
    if lower.contains("requested url returned error: 403")
        || (lower.contains("permission to ") && lower.contains(" denied to "))
    {
        let line = first_line_with(out, " denied to ").or_else(|| first_line_with(out, "error: 403")).unwrap_or("403");
        return Some(format_answer(
            &format!(
                "The server answered 403: it knows who you are but that account has no write access to this repository (`{line}`). Check which account the credentials belong to (a saved token or SSH key of another user is the usual cause) and ask the owner for access."
            ),
            "nothing",
        ));
    }
    if lower.contains("could not get lock") && lower.contains("dpkg") {
        let pid = lower.split("held by process ").nth(1).and_then(|r| r.split_whitespace().next());
        let (held, fix) = match pid {
            Some(p) => (format!(" (process {p})"), format!("ps -p {p}")),
            None => (String::new(), "nothing".to_string()),
        };
        return Some(format_answer(
            &format!(
                "Another package manager is running{held} and holds the dpkg lock, so apt cannot start (`Could not get lock`). Often it is an automatic update: wait for it to finish, then run the command again. Do not remove the lock files by hand; that can corrupt the package database."
            ),
            &fix,
        ));
    }
    if let Some(line) = first_line_with(out, "unable to locate package") {
        let name = line.rsplit("package ").next().unwrap_or("it");
        let update = if last.command.contains("apt-get") { "sudo apt-get update" } else { "sudo apt update" };
        return Some(format_answer(
            &format!(
                "apt has no package called `{name}` in its lists (`{line}`): the name is misspelled, the package list is out of date, or the package lives in a repository that is not enabled. Refreshing the lists is the first thing to try; `apt-cache search` finds the right name."
            ),
            update,
        ));
    }
    if let Some(line) = first_line_with(out, "the connection to the server")
        && lower.contains("was refused")
    {
        return Some(format_answer(
            &format!(
                "kubectl could not reach a cluster (`{line}`). With `localhost:8080` it usually has no kubeconfig at all, so it fell back to its default address; otherwise the cluster is down. `kubectl config current-context` shows which cluster kubectl is set to use, and `echo $KUBECONFIG` where it looks for the settings."
            ),
            "kubectl config current-context",
        ));
    }
    None
}

/// Programs people expect on any system whose package name differs from the command, as
/// (command, Arch, Debian, Fedora).
const COMMON_PACKAGES: &[(&str, &str, &str, &str)] = &[
    ("ping", "iputils", "iputils-ping", "iputils"),
    ("ip", "iproute2", "iproute2", "iproute"),
    ("ss", "iproute2", "iproute2", "iproute"),
    ("ifconfig", "net-tools", "net-tools", "net-tools"),
    ("netstat", "net-tools", "net-tools", "net-tools"),
    ("sudo", "sudo", "sudo", "sudo"),
    ("traceroute", "traceroute", "traceroute", "traceroute"),
    ("nc", "openbsd-netcat", "netcat-openbsd", "nmap-ncat"),
    ("lsof", "lsof", "lsof", "lsof"),
    ("dig", "bind", "dnsutils", "bind-utils"),
    ("nslookup", "bind", "dnsutils", "bind-utils"),
];

/// How to install a very common command that the platform data does not describe.
fn common_package_command(program: &str) -> Option<String> {
    let (_, arch, debian, fedora) = COMMON_PACKAGES.iter().find(|(name, ..)| *name == program)?;
    let platforms = crate::platform::detect();
    let family = crate::platform::manager(&platforms)?;
    let package = match family.as_str() {
        "arch" | "omarchy" => arch,
        "debian" => debian,
        "fedora" => fedora,
        _ => return None,
    };
    Some(crate::platform::fill(&crate::platform::fact(&platforms, "pkg_install")?, package))
}

// ---------------------------------------------------------------------------
// Round 4: git states that are not "errors", Python name clashes, and honest misses
// ---------------------------------------------------------------------------

/// The words of a command that are not options, after the program name.
fn plain_args(command: &str) -> Vec<&str> {
    command.split_whitespace().skip(1).filter(|w| !w.starts_with('-')).collect()
}

/// Is `path` a symlink whose target is missing?
fn is_dangling_symlink(path: &str) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) && !Path::new(path).exists()
}

/// `tar -xf a.tar -C d` names its members relative to `d`: puts `d/` in front of the relative
/// paths of the `tar: member: reason` lines so they are checked where tar looked.
fn resolve_tar_directory(last: &Last) -> Option<Last> {
    let tokens: Vec<&str> = last.command.split_whitespace().collect();
    if tokens.first() != Some(&"tar") {
        return None;
    }
    let dir = tokens.iter().enumerate().find_map(|(i, t)| {
        if let Some(d) = t.strip_prefix("--directory=") {
            Some(d)
        } else if *t == "-C" || *t == "--directory" {
            tokens.get(i + 1).copied()
        } else {
            t.strip_prefix("-C").filter(|d| !d.is_empty())
        }
    })?;
    let dir = dir.trim_matches(['\'', '"']).trim_end_matches('/');
    let mut changed = false;
    let output: Vec<String> = last
        .output
        .lines()
        .map(|line| match line.strip_prefix("tar: ").and_then(|r| r.split_once(": ")) {
            Some((member, reason))
                if !member.starts_with('/')
                    && !member.contains(char::is_whitespace)
                    && (reason.starts_with("Cannot ") || reason.starts_with("Could not")) =>
            {
                changed = true;
                format!("tar: {dir}/{member}: {reason}")
            }
            _ => line.to_string(),
        })
        .collect();
    changed.then(|| Last { command: last.command.clone(), exit: last.exit, output: output.join("\n") })
}

fn diagnose_round4(last: &Last) -> Option<String> {
    let out = last.output.as_str();
    let lower = out.to_lowercase();
    let command = last.command.trim();
    let first = command.split_whitespace().next().unwrap_or("");

    // --- git states that read like "nothing" but need an action ---
    if lower.contains("the stash entry is kept") || (command.contains("stash") && lower.contains("conflict (")) {
        let files: Vec<String> = out
            .lines()
            .filter_map(|l| {
                l.trim().strip_prefix("CONFLICT (").and_then(|r| r.split_once("in ")).map(|(_, f)| f.trim().to_string())
            })
            .collect();
        let files =
            if files.is_empty() { "the conflicting files".to_string() } else { format!("`{}`", files.join("`, `")) };
        return Some(format_answer(
            &format!(
                "`git stash pop` applied your stash, but it conflicted with changes already in the working tree, so the pop stopped halfway and the stash entry is kept (nothing is lost). Open {files}, resolve the `<<<<<<<` / `=======` / `>>>>>>>` markers, then `git add` the files and run `git stash drop` to remove the kept entry."
            ),
            "git status",
        ));
    }
    if lower.contains("cherry-pick is now empty") {
        return Some(format_answer(
            "The cherry-pick produced no change: the commit's changes are already in your branch (or conflict resolution cancelled them out), so there is nothing to commit. Skip the commit with `git cherry-pick --skip`, or keep it as an empty commit with `git commit --allow-empty`; `git cherry-pick --abort` cancels the whole operation.",
            "git cherry-pick --skip",
        ));
    }
    if lower.contains("need to specify how to reconcile divergent branches") {
        return Some(format_answer(
            "Your branch and the remote branch each have commits the other lacks (they diverged), and git no longer guesses how to combine them. `git pull --rebase` replays your commits on top of the remote ones (a straight history); `git pull --no-rebase` merges the two (adds a merge commit). Run `git config pull.rebase true` (or `false`) once to make the choice permanent.",
            "git pull --rebase",
        ));
    }
    if lower.contains("index.lock") && lower.contains("file exists") {
        let lock =
            out.lines().find_map(|l| quoted_after(l, "Unable to create")).unwrap_or_else(|| ".git/index.lock".into());
        return Some(format_answer(
            &format!(
                "Git left a lock file (`{lock}`) that stops other git commands. It is normally removed when the git command that made it ends, so another git process (an editor opened by `git commit`, an IDE, a second terminal) may still be running. Remove the file only if no git process is running: check first, and only then delete it."
            ),
            "pgrep -a git",
        ));
    }
    if lower.contains("dubious ownership") {
        let fix = out.lines().map(str::trim).find(|l| l.starts_with("git config") && l.contains("safe.directory"));
        let explanation = "Git refuses to work in a repository owned by another user (its \"dubious ownership\" check, a safety check against running hooks planted by someone else). If you trust this folder, tell git so with the command printed in its output.";
        return Some(format_answer(explanation, fix.unwrap_or("nothing")));
    }
    if lower.contains("the following paths are ignored by one of your .gitignore files") {
        let paths: Vec<&str> = out
            .lines()
            .map(str::trim)
            .skip_while(|l| !l.starts_with("The following paths are ignored"))
            .skip(1)
            .take_while(|l| !l.starts_with("hint:") && !l.is_empty())
            .collect();
        let shown = paths.join(" ");
        return Some(format_answer(
            &format!(
                "`git add` skipped `{shown}` because a .gitignore rule ignores it: that is on purpose for build output, logs and secrets. Add it anyway with `git add -f {shown}` only if you really want it tracked; otherwise leave it out. To see which rule matches, use `git check-ignore -v`."
            ),
            &format!("git check-ignore -v {shown}"),
        ));
    }

    // --- Python: a local file named like a module shadows it ---
    if let Some(at) = lower.find("partially initialized module '")
        && (lower.contains("circular import"))
    {
        let module = out[at + "partially initialized module '".len()..].split('\'').next().unwrap_or("").to_string();
        let file = format!("{module}.py");
        let frame = python_frames(out)
            .into_iter()
            .map(|(path, _, _)| path)
            .find(|p| Path::new(p).file_name().is_some_and(|n| n.to_string_lossy() == file));
        let place = frame.as_deref().unwrap_or(&file);
        let local = std::env::current_dir()
            .ok()
            .is_some_and(|d| frame.as_ref().is_none_or(|f| Path::new(f).parent() == Some(d.as_path())));
        let shown = if local { file.clone() } else { place.to_string() };
        let renamed = shown.replace(".py", "_local.py");
        if frame.is_some() || Path::new(&file).exists() {
            return Some(format_answer(
                &format!(
                    "A file of yours named `{shown}` shadows Python's own `{module}` module: `import {module}` loaded your file instead, which is half-loaded at that point, hence `partially initialized module` and the \"circular import\" hint. Rename `{shown}` (and delete the `__pycache__` folder beside it), then run the script again."
                ),
                &format!("mv {shown} {renamed}"),
            ));
        }
        return Some(format_answer(
            &format!(
                "Module `{module}` imports itself through a cycle (A imports B, B imports A), so one of them is used before it has finished loading (`partially initialized module`). Move the shared code into a third module, or import inside the function that needs it. If you have a file of your own named `{file}`, rename it: it shadows the standard module."
            ),
            "nothing",
        ));
    }
    if lower.contains("cannot import name") && lower.contains("circular import") {
        return Some(format_answer(
            "Python could not import that name because the module is only half-loaded: two modules import each other, or a file of yours has the same name as an installed module and shadows it. Rename such a file, or move the shared code into a third module.",
            "nothing",
        ));
    }

    // --- Node ---
    if lower.contains("eaddrinuse")
        && let Some(port) = out.lines().find(|l| l.contains("EADDRINUSE")).and_then(|l| {
            let tail = l.rsplit(':').next()?.trim();
            let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
            (!digits.is_empty()).then_some(digits)
        })
    {
        return Some(format_answer(
            &format!(
                "Port {port} is already in use (`EADDRINUSE`): another process (often an earlier copy of this server) is listening on it. Find it, stop it, or start this program on another port."
            ),
            &format!("ss -tulpn 'sport = :{port}'"),
        ));
    }
    if lower.contains("syntaxerror")
        && (lower.contains("json.parse")
            || lower.contains("in json at position")
            || lower.contains("is not valid json"))
    {
        return Some(format_answer(
            "`JSON.parse` stopped with a SyntaxError: the text it was given is not valid JSON. The problem is in the data (a missing or extra quote, comma or bracket, single quotes instead of double, an empty string, or an HTML error page instead of JSON), not in the code. Print the string before parsing it to see what it holds.",
            "nothing",
        ));
    }

    // --- shell and tool usage ---
    if (lower.contains("sudo: command not found") || lower.contains("sudo: not found"))
        && (first == "sudo" || command.contains("sudo "))
    {
        let install = common_package_command("sudo")
            .map(|c| c.strip_prefix("sudo ").unwrap_or(&c).to_string())
            .unwrap_or_else(|| "apt install sudo".to_string());
        return Some(format_answer(
            "`sudo` is not installed here (common in minimal containers and fresh Debian installs), and installing it needs administrator rights itself: do not put `sudo` in front of the install. Log in as root (`su -`) and install it, ask an administrator, or run the command as root directly.",
            &format!("su -c '{install}'"),
        ));
    }
    if lower.contains("superuser privileges") && !command.starts_with("sudo ") && !command.is_empty() {
        return Some(format_answer(
            "This command changes the system and needs administrator (root) rights. Run it with `sudo`.",
            &format!("sudo {command}"),
        ));
    }
    if lower.contains("running makepkg as root is not allowed") {
        return Some(format_answer(
            "`makepkg` refuses to run as root: a build script run as root could damage the whole system. Run it as a normal user (it asks for the root password itself when it installs with `-i`).",
            "nothing",
        ));
    }
    if lower.contains("sed:") && lower.contains("unknown option to `s'") {
        return Some(format_answer(
            "`sed` split the `s` command at an extra `/`: the pattern or the replacement contains a `/` (a path or URL), which ends it early. Use another delimiter for the whole command, for example `s|a|b|` or `s#a#b#`, or escape each slash as `\\/`.",
            "nothing",
        ));
    }
    if lower.contains("sed:") && lower.contains("unmatched ( or \\(") {
        return Some(format_answer(
            "`sed` found an opening parenthesis without its closing one. In a basic regular expression a group is written `\\( ... \\)` and a bare `(` is a literal character; with `sed -E` it is the other way around. Match every `\\(` with a `\\)` (or escape a literal parenthesis).",
            "nothing",
        ));
    }
    if lower.contains("xargs: unmatched") && lower.contains("quote") {
        let fixed = if command.contains("xargs ") {
            command.replacen("xargs ", "xargs -d '\\n' ", 1)
        } else {
            "nothing".to_string()
        };
        return Some(format_answer(
            "`xargs` treats quotes and backslashes in its input as quoting, so a single `'` or `\"` in a file name or line breaks it. Tell it to take each line literally with `-d '\\n'` (GNU xargs), or use `find ... -print0 | xargs -0` so names are separated by NUL bytes.",
            &fixed,
        ));
    }
    if lower.contains("zip error: nothing to do") {
        return Some(format_answer(
            "`zip` was given an archive name but no files to put in it (or every file was excluded): `zip archive.zip file1 file2`, and add `-r` for folders.",
            "nothing",
        ));
    }
    if lower.contains("find: paths must precede expression") || lower.contains("find: possible unquoted pattern") {
        let mut fixed = false;
        let words: Vec<String> = command
            .split_whitespace()
            .scan(false, |after_test, w| {
                let quote = *after_test && w.contains(['*', '?', '[']) && !w.starts_with(['\'', '"']);
                *after_test = matches!(w, "-name" | "-iname" | "-path" | "-ipath" | "-regex" | "-wholename");
                fixed |= quote;
                Some(if quote { format!("'{w}'") } else { w.to_string() })
            })
            .collect();
        return Some(format_answer(
            "The shell expanded the unquoted pattern (`*.txt`) into the matching file names in the current folder before `find` ran, so `find` got extra words after `-name`. Quote the pattern so `find` sees it unchanged.",
            &if fixed { words.join(" ") } else { "nothing".to_string() },
        ));
    }
    if lower.contains("could not open a connection to your authentication agent") {
        return Some(format_answer(
            "`ssh-add` needs the `ssh-agent` program running in this shell, and none is (the `SSH_AUTH_SOCK` variable is not set). Start an agent for this shell, then add the key again.",
            "eval \"$(ssh-agent)\"",
        ));
    }
    if lower.contains("unary operator expected") || (lower.contains("[: ") && lower.contains("unexpected operator")) {
        return Some(format_answer(
            "A `[ ... ]` test lost an operand: the variable on the left of the operator is empty or unset, so the shell saw `[ == 1 ]`. Put double quotes around the variable (`[ \"$x\" == 1 ]`) or use `[[ $x == 1 ]]` in bash, and check why the variable is empty.",
            "nothing",
        ));
    }
    if lower.contains("value too great for base") {
        let token = quoted_after(out, "error token is \"")
            .or_else(|| out.split("error token is \"").nth(1).map(|r| r.split('"').next().unwrap_or("").to_string()))
            .unwrap_or_default();
        return Some(format_answer(
            &format!(
                "Bash reads a number with a leading zero (`{token}`) as octal, where 8 and 9 are not digits. Force base 10 with `10#`: `$((10#$x))`, or strip the zeros first (`${{x#0}}`)."
            ),
            "nothing",
        ));
    }
    if lower.contains("no journal files were found") {
        let try_cmd = if command.starts_with("journalctl") { format!("sudo {command}") } else { "nothing".to_string() };
        return Some(format_answer(
            "`journalctl` found no journal it may read: as a normal user you only see your own logs and only if the system keeps a persistent journal. Run it with `sudo`, or add yourself to the `systemd-journal` group (then log in again). In a container there is no journal at all.",
            &try_cmd,
        ));
    }
    if lower.contains("unable to find a match") || lower.contains("no match for argument") {
        let name = out
            .lines()
            .find_map(|l| l.trim().strip_prefix("Error: Unable to find a match:"))
            .map(|n| n.split_whitespace().next().unwrap_or("").to_string())
            .or_else(|| {
                out.lines().find_map(|l| l.trim().strip_prefix("No match for argument:")).map(|n| n.trim().to_string())
            })
            .unwrap_or_default();
        return Some(format_answer(
            &format!(
                "dnf has no package called `{name}` in the repositories that are enabled: the name is misspelled, it has another name on this distribution, or its repository (EPEL, RPM Fusion, a vendor repo) is not enabled. Search by part of the name."
            ),
            &format!("dnf search {name}"),
        ));
    }
    if lower.contains("gpg check failed") {
        return Some(format_answer(
            "dnf could not verify the package's signature: the signing key is not imported, the file is damaged or was not signed by that key. Import the vendor's public key (`sudo rpm --import <key-url>`) and try again; use `--nogpgcheck` only for a package you built or fully trust.",
            "nothing",
        ));
    }
    if lower.contains("curl: (35)") {
        let url = last.command.split_whitespace().find(|w| w.contains("://")).unwrap_or("<url>");
        return Some(format_answer(
            "curl could not complete the TLS handshake (`(35)`): the server dropped the connection while setting up HTTPS. Typical causes are a port that does not speak TLS (plain HTTP on it), a proxy or firewall in between, a very old or very new TLS version on one side, or a wrong clock. `curl -v` shows how far the handshake got.",
            &format!("curl -v {url}"),
        ));
    }
    if lower.contains("eresolve") && (lower.contains("--legacy-peer-deps") || lower.contains("--force")) {
        let try_cmd = if command.starts_with("npm ") && !command.contains("--legacy-peer-deps") {
            format!("{command} --legacy-peer-deps")
        } else {
            "npm install --legacy-peer-deps".to_string()
        };
        return Some(format_answer(
            "npm found two packages whose version requirements cannot both be met (a peer dependency conflict, `ERESOLVE`). `--legacy-peer-deps` installs anyway, the way npm 6 did; it ignores the conflict, so the program may break at runtime. The clean fix is to upgrade or downgrade the package named in the message so the versions agree.",
            &try_cmd,
        ));
    }
    if lower.contains("npm err! test failed") || lower.contains("npm error test failed") {
        let names: Vec<&str> = out.lines().map(str::trim).filter_map(|l| l.strip_prefix("FAIL ")).collect();
        if !names.is_empty() {
            return Some(format_answer(
                &format!(
                    "`npm test` ran the test suite and it failed: {} failed. The details (expected and received values) are in the test output above npm's own lines; `npm ERR! Test failed` is only npm reporting the exit status.",
                    names.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", ")
                ),
                "nothing",
            ));
        }
    }
    if let Some(name) = out.lines().map(str::trim).find_map(|l| l.strip_prefix("FAIL: ").map(|r| r.trim().to_string()))
        && !name.is_empty()
        && !name.contains(' ')
    {
        let summary =
            out.lines().map(str::trim).rev().find(|l| l.contains("passed") && l.contains("failed")).unwrap_or("");
        let more = if summary.is_empty() { String::new() } else { format!(" (`{summary}`)") };
        return Some(format_answer(
            &format!(
                "A test failed: `{name}`{more}. The other tests passed; make's `Error 1` only reports that one command failed. Look at the output printed by `{name}` for what it expected."
            ),
            "nothing",
        ));
    }
    if lower.starts_with("package ") && lower.contains(" is not installed") && first == "rpm" {
        let name = out.split_whitespace().nth(1).unwrap_or("it");
        return Some(format_answer(
            &format!(
                "The rpm database has no package called `{name}`: it is not installed, or it is installed under another name. List installed packages that match part of the name."
            ),
            &format!("rpm -qa | grep -i {name}"),
        ));
    }
    if lower.trim() == "bus error" || lower.contains("\nbus error") || last.exit == Some(135) {
        return Some(format_answer(
            "The program was killed by `SIGBUS` (`Bus error`, exit code 135): it touched memory the system could not give it. Usual causes are a memory-mapped file that was truncated or sits on a disk that filled up or was unplugged, or a misaligned access in native code. It is a bug or an environment problem in the program, not a mistake in the command line; check free disk space and whether the files it maps still exist.",
            "nothing",
        ));
    }

    // --- cp / mv with a space in a name ---
    if matches!(first, "cp" | "mv")
        && let Some(target) = out.lines().find_map(|l| quoted_after(l, "target "))
        && lower.contains("no such file or directory")
    {
        let args = plain_args(command);
        if args.len() >= 3 && args.last() == Some(&target.as_str()) {
            let sources = args[..args.len() - 1].join(" ");
            let many = args.len() > 3;
            let explanation = format!(
                "`{first}` was given {} words, so the shell split the name at the space: it treated `{}` as separate files and `{target}` as the destination, and `{target}` does not exist.{} Quote a name that contains a space.",
                args.len(),
                args[..args.len() - 1].join("`, `"),
                if many { " (If you really meant several files, create the destination folder first.)" } else { "" }
            );
            let try_cmd = if many { "nothing".to_string() } else { format!("{first} \"{sources}\" {target}") };
            return Some(format_answer(&explanation, &try_cmd));
        }
    }

    // --- a symlink whose target is gone ---
    if lower.contains("no such file or directory") {
        for failure in file_failures(out) {
            if failure.problem == FileProblem::Missing && is_dangling_symlink(&failure.path) {
                let target = std::fs::read_link(&failure.path).map(|t| t.display().to_string()).unwrap_or_default();
                return Some(format_answer(
                    &format!(
                        "`{}` exists, but it is a symbolic link pointing to `{target}`, which does not exist (a dangling link), so opening it fails with \"No such file or directory\". Recreate the target, or point the link somewhere valid with `ln -sfn`.",
                        failure.path
                    ),
                    &format!("ls -l {}", failure.path),
                ));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A risky `Try:` command gets a `Note (category): ...` line with the same category
    /// `huh explain` shows; harmless ones stay as they were.
    #[test]
    fn a_risky_try_line_names_its_category() {
        let a = format_answer("The Docker socket is not reachable.", "sudo usermod -aG docker $USER");
        assert!(a.contains("\nTry: sudo usermod -aG docker $USER\nNote (gives away access): "), "{a}");
        let a = format_answer("Git does not trust this folder.", "git config --global --add safe.directory /srv/app");
        assert!(a.contains("\nNote (weakens security): "), "{a}");
        let a = format_answer("Not executable.", "chmod 777 run.sh");
        assert!(a.contains("\nNote (gives away access): "), "{a}");
        assert_eq!(suggested(&a), Some("chmod 777 run.sh"));
        assert_eq!(format_answer("It broke.", "ls -la"), "It broke.\nTry: ls -la");
        assert_eq!(format_answer("Missing.", "sudo pacman -S jq"), "Missing.\nTry: sudo pacman -S jq");
        assert_eq!(format_answer("Hung.", "tail -f app.log"), "Hung.\nTry: tail -f app.log");
    }

    #[test]
    fn finds_the_suggested_command() {
        assert_eq!(suggested("The file is missing.\nTry: `ls -la`"), Some("ls -la"));
        assert_eq!(suggested("Nothing to do.\nTry: nothing"), None);
        assert_eq!(suggested("- It is busy.\n- Try: `git stash`"), Some("git stash"));
        assert_eq!(suggested("No final line here."), None);
    }

    #[test]
    fn names_the_package_manager() {
        let facts = system_facts("NAME=\"Omarchy\"\nID=omarchy\nID_LIKE=arch\n", "/usr/bin/bash");
        assert_eq!(facts, "Omarchy, shell bash. Packages are installed with `omarchy pkg add <package>`.");
        assert_eq!(
            system_facts("NAME=Debian\nID=debian\n", "/bin/sh"),
            "Debian, shell sh. Packages are installed with `sudo apt install <package>`."
        );
        assert_eq!(
            system_facts("NAME=Void\nID=void\n", "/bin/zsh"),
            "Void, shell zsh. Packages are installed with `sudo xbps-install -S <package>`."
        );
        assert_eq!(system_facts("NAME=Mystery\nID=mystery\n", "/bin/zsh"), "Mystery, shell zsh.");
    }

    #[test]
    fn stops_after_the_try_line() {
        assert_eq!(complete_at("It failed.\nTry: ls\nBut also"), Some("It failed.\nTry: ls".len()));
        assert_eq!(complete_at("It failed.\nTry: ls"), None);
        assert_eq!(complete_at("It failed.\nTry:\n"), None);
    }

    #[test]
    fn question_shows_the_command_and_its_output() {
        let last = Last { command: "cat nope".into(), exit: Some(1), output: "cat: nope: No such file".into() };
        let text = question(&last);
        assert!(
            text.contains("Command: cat nope")
                && text.contains("Exit code: 1 (general error)")
                && text.contains("No such file")
        );
    }

    #[test]
    fn exit_code_table_covers_standard_codes() {
        assert_eq!(exit_meaning(0), Some("succeeded"));
        assert_eq!(exit_meaning(1), Some("general error"));
        assert_eq!(exit_meaning(2), Some("incorrect command usage or syntax"));
        assert_eq!(exit_meaning(126), Some("command found but not executable"));
        assert_eq!(exit_meaning(127), Some("command not found"));
        assert_eq!(exit_meaning(128), Some("fatal error or signal"));
        assert_eq!(exit_meaning(130), Some("interrupted with Ctrl+C"));
        assert_eq!(exit_meaning(137), Some("killed"));
        assert_eq!(exit_meaning(143), Some("terminated"));
        assert_eq!(exit_meaning(255), Some("connection or remote command error"));
    }

    #[test]
    fn damerau_levenshtein_handles_transpositions() {
        assert_eq!(damerau_levenshtein("htpo", "htop"), 1);
        assert_eq!(damerau_levenshtein("htpo", "top"), 2);
        assert_eq!(damerau_levenshtein("gti", "git"), 1);
        assert_eq!(damerau_levenshtein("same", "same"), 0);
    }

    #[test]
    fn explain_returns_answer_and_streams_to_callback() {
        let last = Last { command: "htpo".into(), exit: Some(127), output: "bash: htpo: command not found".into() };
        let mut streamed = String::new();
        let answer = explain(&last, &mut |piece| streamed.push_str(piece)).unwrap();
        assert_eq!(answer.text, streamed);
        assert!(answer.text.contains("htop"));
        assert!(suggested(&answer.text).is_some_and(|c| c.contains("htop")));
    }

    #[test]
    fn diagnoses_exit_130_and_exit_0_and_exit_2() {
        let interrupted = Last { command: "ping 1.1.1.1".into(), exit: Some(130), output: String::new() };
        let ans = diagnose(&interrupted);
        assert!(ans.to_lowercase().contains("ctrl+c"));
        assert_eq!(suggested(&ans), None);

        let ok = Last { command: "mkdir -p out".into(), exit: Some(0), output: String::new() };
        let ans = diagnose(&ok);
        assert!(ans.to_lowercase().contains("succeeded"));
        assert_eq!(suggested(&ans), None);

        let bad_flag = Last {
            command: "ls --bogus".into(),
            exit: Some(2),
            output: "ls: unrecognized option '--bogus'\nTry 'ls --help' for more information.".into(),
        };
        let ans = diagnose(&bad_flag);
        assert!(ans.contains("unrecognized option '--bogus'"));
        assert_eq!(suggested(&ans), Some("ls --help"));
    }

    #[test]
    fn diagnoses_missing_packages_and_common_failures() {
        crate::platform::pin_for_eval(&["arch"]);
        let rg = Last { command: "rg foo src".into(), exit: Some(127), output: "bash: rg: command not found".into() };
        let ans = diagnose(&rg);
        assert!(ans.contains("ripgrep"));
        assert!(suggested(&ans).unwrap().contains("ripgrep"));

        let port = Last {
            command: "docker run -p 8080:80 nginx".into(),
            exit: Some(125),
            output: "failed to bind host port 0.0.0.0:8080/tcp: address already in use".into(),
        };
        let ans = diagnose(&port);
        assert!(ans.contains("8080"));
        assert!(suggested(&ans).unwrap().contains("8080"));

        let piped =
            Last { command: String::new(), exit: None, output: "Error: could not connect to ollama server".into() };
        assert_eq!(suggested(&diagnose(&piped)), Some("ollama serve"));
    }

    #[test]
    fn live_machine_checks_stat_and_cwd_typo() {
        // Cargo.toml exists in repo root and is not executable -> stat reports mode without +x.
        let non_exec = Last {
            command: "./Cargo.toml".into(),
            exit: Some(126),
            output: "bash: ./Cargo.toml: Permission denied".into(),
        };
        let ans = diagnose(&non_exec);
        assert!(ans.contains("lacks executable permission"));
        assert_eq!(suggested(&ans), Some("chmod +x ./Cargo.toml"));

        // Typo for `catalog` directory in repo root -> mentions closest match `catalog`.
        let cd_typo = Last {
            command: "cd catalgo".into(),
            exit: Some(1),
            output: "bash: cd: catalgo: No such file or directory".into(),
        };
        let ans = diagnose(&cd_typo);
        assert!(ans.contains("catalog"));
        assert_eq!(suggested(&ans), Some("ls"));
    }

    #[test]
    fn handles_shell_line_prefix_fish_polkit_pacman_ssh_and_tar() {
        crate::platform::pin_for_eval(&["arch"]);
        // bash: line 1: htpo: command not found (piped, exit None)
        let bash_line_typo =
            Last { command: String::new(), exit: None, output: "bash: line 1: htpo: command not found".into() };
        let ans = diagnose(&bash_line_typo);
        assert!(ans.contains("htop"), "got: {ans}");
        assert!(suggested(&ans).is_some_and(|c| c.contains("htop")));

        // bash: line 1: ./Cargo.toml: Permission denied (piped, exit None -> checks metadata +x)
        let bash_line_noexec =
            Last { command: String::new(), exit: None, output: "bash: line 1: ./Cargo.toml: Permission denied".into() };
        let ans = diagnose(&bash_line_noexec);
        assert!(ans.contains("lacks executable permission"), "got: {ans}");
        assert_eq!(suggested(&ans), Some("chmod +x ./Cargo.toml"));

        // fish: Unknown command: rg
        let fish_missing = Last { command: String::new(), exit: None, output: "fish: Unknown command: rg".into() };
        let ans = diagnose(&fish_missing);
        assert!(ans.contains("ripgrep"), "got: {ans}");
        assert!(suggested(&ans).unwrap().contains("ripgrep"));

        // PolicyKit / Interactive authentication required
        let polkit = Last {
            command: String::new(),
            exit: None,
            output: "Failed to restart bluetooth.service: Interactive authentication required.".into(),
        };
        assert_eq!(suggested(&diagnose(&polkit)), Some("sudo systemctl restart bluetooth.service"));

        // Pacman target not found & conflicting files
        let pac_missing = Last {
            command: "sudo pacman -S firefox-esr".into(),
            exit: Some(1),
            output: "error: target not found: firefox-esr".into(),
        };
        let ans = diagnose(&pac_missing);
        assert!(ans.contains("yay -Ss firefox-esr"));
        assert_eq!(suggested(&ans), Some("pacman -Ss firefox-esr"));

        let pac_conflict = Last {
            command: "sudo pacman -S foo".into(),
            exit: Some(1),
            output: "foo: /usr/bin/foo exists in filesystem\nErrors occurred, no packages were upgraded.".into(),
        };
        assert_eq!(suggested(&diagnose(&pac_conflict)), Some("pacman -Qo /usr/bin/foo"));

        // SSH non-default port + Git publickey
        let ssh_port = Last {
            command: "ssh -p 22222 127.0.0.1".into(),
            exit: Some(255),
            output: "ssh: connect to host 127.0.0.1 port 22222: Connection refused".into(),
        };
        let ans = diagnose(&ssh_port);
        assert!(ans.contains("port 22222"), "got: {ans}");
        assert!(!ans.contains("port 22 "), "got: {ans}");

        let git_pubkey = Last {
            command: "git clone git@github.com:org/repo.git".into(),
            exit: Some(128),
            output: "git@github.com: Permission denied (publickey).".into(),
        };
        let ans = diagnose(&git_pubkey);
        assert_eq!(suggested(&ans), Some("ssh -T git@github.com"));

        // tar -xzf fake.tar.gz -> Try: tar -xf fake.tar.gz
        let tar_bad = Last {
            command: "tar -xzf fake.tar.gz".into(),
            exit: Some(2),
            output: "gzip: stdin: not in gzip format".into(),
        };
        assert_eq!(suggested(&diagnose(&tar_bad)), Some("file fake.tar.gz"));

        let missing_unit = Last {
            command: "sudo systemctl restart postgresql".into(),
            exit: Some(5),
            output: "Failed to restart postgresql.service: Unit postgresql.service not found.".into(),
        };
        let ans = diagnose(&missing_unit);
        assert!(ans.contains("postgresql.service"), "got: {ans}");
        assert_eq!(suggested(&ans), Some("systemctl list-unit-files '*postgresql*'"));
    }

    fn failed(command: &str, output: &str) -> Last {
        Last { command: command.into(), exit: if command.is_empty() { None } else { Some(127) }, output: output.into() }
    }

    #[test]
    fn finds_the_missing_program_in_every_shell_format() {
        let missing = |output: &str| missing_from_output(output);
        assert_eq!(missing("bash: htop: command not found").as_deref(), Some("htop"));
        assert_eq!(missing("bash: line 1: htop: command not found").as_deref(), Some("htop"));
        assert_eq!(missing("-bash: htop: command not found").as_deref(), Some("htop"));
        assert_eq!(missing("./run.sh: line 3: htop: command not found").as_deref(), Some("htop"));
        assert_eq!(missing("sh: 1: ip: not found").as_deref(), Some("ip"));
        assert_eq!(missing("/bin/sh: 1: ip: not found").as_deref(), Some("ip"));
        assert_eq!(missing("sh: htop: not found").as_deref(), Some("htop"));
        assert_eq!(missing("/bin/sh: htop: not found").as_deref(), Some("htop"));
        assert_eq!(missing("ash: htop: not found").as_deref(), Some("htop"));
        assert_eq!(missing("htop: command not found").as_deref(), Some("htop"));
        assert_eq!(missing("zsh: command not found: htop").as_deref(), Some("htop"));
        assert_eq!(missing("fish: Unknown command: htop").as_deref(), Some("htop"));
        assert_eq!(missing("some noise\nbash: htop: command not found\nmore").as_deref(), Some("htop"));
        // Things that merely contain "not found".
        assert_eq!(missing("Failed to restart x.service: Unit x.service not found."), None);
        assert_eq!(missing("error: target not found: firefox"), None);
        assert_eq!(missing("bash: 127: command not found"), None);
    }

    #[test]
    fn install_hints_use_this_systems_package_name() {
        crate::platform::pin_for_eval(&["debian"]);
        let hint = |command: &str, output: &str| suggested(&diagnose(&failed(command, output))).map(str::to_string);
        assert_eq!(hint("", "bash: dig: command not found").as_deref(), Some("sudo apt install bind9-dnsutils"));
        assert_eq!(hint("", "sh: 1: ip: not found").as_deref(), Some("sudo apt install iproute2"));
        assert_eq!(hint("", "sh: htop: not found").as_deref(), Some("sudo apt install htop"));
        assert_eq!(hint("", "htop: command not found").as_deref(), Some("sudo apt install htop"));
        assert_eq!(
            hint("nmcli device", "bash: nmcli: command not found").as_deref(),
            Some("sudo apt install network-manager")
        );
        crate::platform::pin_for_eval(&["fedora"]);
        assert_eq!(hint("", "bash: dig: command not found").as_deref(), Some("sudo dnf install bind-utils"));
        crate::platform::pin_for_eval(&["omarchy", "arch"]);
        assert_eq!(hint("", "bash: dig: command not found").as_deref(), Some("omarchy pkg add bind"));
        // Not a program anyone has listed: say how to find the package, never guess one.
        crate::platform::pin_for_eval(&["alpine"]);
        assert_eq!(hint("", "sh: frobnicate: not found").as_deref(), Some("apk search -e cmd:frobnicate"));
        // A system with no known package manager gives no install command.
        crate::platform::pin_for_eval(&["mystery"]);
        assert_eq!(hint("", "bash: dig: command not found"), None);
    }

    #[test]
    fn a_typo_of_an_installed_command_beats_installing_a_package() {
        crate::platform::pin_for_eval(&["debian"]);
        let hint = |command: &str, output: &str| suggested(&diagnose(&failed(command, output))).map(str::to_string);
        assert_eq!(hint("sl -la", "bash: sl: command not found").as_deref(), Some("ls -la"));
        assert_eq!(hint("gti status", "bash: gti: command not found").as_deref(), Some("git status"));
        assert_eq!(
            hint("sudo gti push origin main", "sudo: gti: command not found").as_deref(),
            Some("sudo git push origin main")
        );
        assert_eq!(
            hint("cd /tmp && gti log | head", "bash: gti: command not found").as_deref(),
            Some("cd /tmp && git log | head")
        );
        // A real tool that is one edit from another command is still installed, not "fixed".
        assert_eq!(hint("rg foo", "bash: rg: command not found").as_deref(), Some("sudo apt install ripgrep"));
    }

    #[test]
    fn an_installed_program_is_never_suggested_for_installation() {
        let ans = diagnose(&failed("sh -c 'sh: x'", "bash: sh: command not found"));
        assert!(ans.contains("installed at"), "{ans}");
        assert_eq!(suggested(&ans), None);
        let ans = diagnose(&failed("", "sh: 1: ls: not found"));
        assert_eq!(suggested(&ans), None, "{ans}");
    }

    #[test]
    fn nothing_to_try_means_no_try_line() {
        let ans = diagnose(&failed("sh -c 'sh: x'", "bash: sh: command not found"));
        assert!(!ans.contains("Try:"), "{ans}");
        assert!(ans.contains("installed at") && ans.contains("failure is elsewhere"), "{ans}");
        assert_eq!(format_answer("It is fine.", "nothing"), "It is fine.");
        assert_eq!(format_answer("It broke.", "ls -la"), "It broke.\nTry: ls -la");
    }

    #[test]
    fn install_hints_use_the_package_not_the_program() {
        crate::platform::pin_for_eval(&["debian"]);
        let ans = diagnose(&failed("dig example.com", "bash: dig: command not found"));
        assert_eq!(suggested(&ans), Some("sudo apt install bind9-dnsutils"), "{ans}");
        assert!(ans.contains("`bind9-dnsutils` package"), "{ans}");
        let ans = diagnose(&failed("eza", "bash: eza: command not found"));
        assert!(ans.contains("Ubuntu before 24.04"), "{ans}");
    }

    #[test]
    fn fixes_keep_the_arguments() {
        assert_eq!(fix_command("gti status", "gti", "git"), "git status");
        assert_eq!(fix_command("sudo gti -v", "gti", "git"), "sudo git -v");
        assert_eq!(fix_command("echo gti && gti x", "gti", "git"), "echo gti && git x");
        assert_eq!(fix_command("", "gti", "git"), "git");
        assert_eq!(fix_command("mygti x", "gti", "git"), "git");
    }

    #[test]
    fn success_lines_are_not_errors_and_real_ones_are() {
        for ok in [
            "test result: ok. 12 passed; 0 failed; 0 ignored",
            "test parser::rejects_denied_input ... ok",
            "-rw-r--r--  1 radu radu 1024 Nov 18 06:00 error.log",
            "npm error A complete log of this run can be found in: /x.log",
            "    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.41s",
        ] {
            assert!(!is_error_line(ok), "{ok}");
        }
        for bad in ["cat: x: Permission denied", "fatal: not a git repository", "npm error code EJSONPARSE", "Killed"] {
            assert!(is_error_line(bad), "{bad}");
        }
    }

    #[test]
    fn paths_come_out_of_error_lines_cleanly() {
        let path = |output: &str| file_failures(output).first().map(|f| f.path.clone());
        assert_eq!(
            path("ls: cannot access '/nonexistent/path': No such file or directory").as_deref(),
            Some("/nonexistent/path")
        );
        assert_eq!(
            path("jq: error: Could not open file missing.json: No such file or directory").as_deref(),
            Some("missing.json")
        );
        assert_eq!(path("bash: cd: projcts: No such file or directory").as_deref(), Some("projcts"));
        assert_eq!(path("cat: a b.txt: Permission denied"), None);
        assert_eq!(path("user@host: Permission denied (publickey)."), None);
        assert_eq!(path("Permission denied, please try again."), None);
    }

    #[test]
    fn python_import_names_map_to_pip_names() {
        assert_eq!(pip_name("yaml"), "pyyaml");
        assert_eq!(pip_name("PIL.Image"), "pillow");
        assert_eq!(pip_name("requests"), "requests");
    }

    #[test]
    fn no_try_line_means_the_word_nothing_never_shows() {
        for case in ["", " ", "nothing", "Nothing"] {
            assert!(!format_answer("It broke.", case).contains("Try:"), "{case:?}");
        }
    }

    #[test]
    fn appimage_missing_fuse2_suggests_extract_and_run() {
        let last = Last {
            command: "./RetroGame.AppImage".into(),
            exit: Some(127),
            output: "dlopen(): error loading libfuse.so.2\nAppImages require FUSE to run.".into(),
        };
        let ans = diagnose(&last);
        let low = ans.to_lowercase();
        assert!(low.contains("fuse") && low.contains("libfuse2") && low.contains("fuse2"), "got: {ans}");
        assert_eq!(suggested(&ans), Some("./RetroGame.AppImage --appimage-extract-and-run"), "got: {ans}");

        let already_flagged = Last {
            command: "  ./RetroGame.AppImage --appimage-extract-and-run  ".into(),
            exit: Some(1),
            output: "dlopen(): error loading libfuse.so.2\nAppImages require FUSE to run.".into(),
        };
        assert_eq!(suggested(&diagnose(&already_flagged)), Some("./RetroGame.AppImage --appimage-extract-and-run"));

        let fusermount_err = Last {
            command: String::new(),
            exit: None,
            output: "fusermount: mount failed: Operation not permitted\nCannot mount AppImage ./Obsidian.AppImage, please check your FUSE setup.".into(),
        };
        assert_eq!(suggested(&diagnose(&fusermount_err)), Some("./Obsidian.AppImage --appimage-extract-and-run"));
    }

    #[test]
    fn appimage_piped_without_command_falls_back_cleanly() {
        let piped = Last {
            command: String::new(),
            exit: None,
            output: "dlopen(): error loading libfuse.so.2\nAppImages require FUSE to run.".into(),
        };
        let ans = diagnose(&piped);
        let low = ans.to_lowercase();
        assert!(low.contains("fuse") && low.contains("appimage"), "got: {ans}");
        assert_eq!(suggested(&ans), Some("./app.AppImage --appimage-extract-and-run"), "got: {ans}");
    }

    #[test]
    fn exec_format_error_handles_dos_windows_and_foreign_cpu() {
        let dos_exe = Last {
            command: "./DOOM.EXE".into(),
            exit: Some(126),
            output: "bash: ./DOOM.EXE: cannot execute binary file: Exec format error".into(),
        };
        let ans = diagnose(&dos_exe);
        let low = ans.to_lowercase();
        assert!(!low.contains("chmod +x"), "must not suggest chmod +x: {ans}");
        assert!(low.contains("windows") && low.contains("dosbox") && low.contains("wine"), "got: {ans}");
        assert_eq!(suggested(&ans), Some("dosbox ./DOOM.EXE"), "got: {ans}");

        let dos_bat = Last {
            command: String::new(),
            exit: None,
            output: "bash: line 1: ./INSTALL.BAT: cannot execute binary file: Exec format error".into(),
        };
        assert_eq!(suggested(&diagnose(&dos_bat)), Some("dosbox ./INSTALL.BAT"));

        let arm_bin = Last {
            command: "./arm-binary".into(),
            exit: Some(126),
            output: "bash: ./arm-binary: cannot execute binary file: Exec format error".into(),
        };
        let ans = diagnose(&arm_bin);
        let low = ans.to_lowercase();
        assert!(!low.contains("chmod +x"), "must not suggest chmod +x: {ans}");
        assert!(low.contains("arm") || low.contains("cpu"), "got: {ans}");
        assert_eq!(suggested(&ans), Some("file ./arm-binary"), "got: {ans}");

        let no_target = Last { command: String::new(), exit: None, output: "Exec format error".into() };
        assert_eq!(suggested(&diagnose(&no_target)), Some("file <file>"));
    }

    #[test]
    fn appimage_prose_with_no_exit_does_not_trigger_fuse_diagnosis() {
        for line in [
            "I downloaded a new AppImage from their releases page.",
            "Some AppImages require FUSE 2 (libfuse.so.2) to run on older systems.",
            "- Fixed an issue where AppImages require FUSE to mount.",
        ] {
            let last = Last { command: String::new(), exit: None, output: line.into() };
            let ans = diagnose(&last);
            assert!(
                !ans.contains("--appimage-extract-and-run") && !ans.to_lowercase().contains("libfuse2"),
                "false positive for {line:?}: {ans}"
            );
        }
    }

    #[test]
    fn common_commands_includes_everyday_media_and_gaming_tools() {
        for tool in ["dosbox", "flatpak", "mpv", "vlc", "ffmpeg", "xdg-open", "wine", "gio"] {
            assert!(COMMON_COMMANDS.contains(&tool), "missing {tool} from COMMON_COMMANDS");
        }
    }

    #[test]
    fn tumbleweed_uses_suse_dev_packages_and_externally_managed() {
        crate::platform::pin_for_eval(&["tumbleweed"]);
        assert!(externally_managed());
        let missing_ssl =
            Last { command: "cc main.c -lssl".into(), exit: Some(1), output: "/usr/bin/ld: cannot find -lssl".into() };
        let ans = diagnose(&missing_ssl);
        assert!(ans.contains("libopenssl-devel"), "got: {ans}");
        assert_eq!(suggested(&ans), Some("sudo zypper install libopenssl-devel"), "got: {ans}");
    }
}
