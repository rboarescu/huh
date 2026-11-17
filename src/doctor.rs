//! Deterministic Error Doctor (`huh why`): explains why a command failed.
//!
//! No network calls are made here: errors are diagnosed with live
//! machine checks and structured diagnostic rules.

use crate::session::Last;
use anyhow::Result;
use std::collections::BTreeSet;
use std::path::Path;
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

    if let Some(ans) = diagnose_docker(last) {
        return ans;
    }
    if let Some(ans) = diagnose_service_daemons(last) {
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
    if let Some(ans) = diagnose_filesystem(last) {
        return ans;
    }
    if let Some(ans) = diagnose_permissions(last) {
        return ans;
    }
    if let Some(ans) = diagnose_tools_and_code(last) {
        return ans;
    }
    diagnose_fallback(last)
}

fn format_answer(explanation: &str, try_cmd: &str) -> String {
    format!("{explanation}\nTry: {try_cmd}")
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
        let in_docker_group = user_in_group("docker");
        let explanation = if in_docker_group {
            "Permission to connect to the Docker daemon socket (/var/run/docker.sock: permission denied) was denied. Your user is listed in the docker group, so refresh your session groups or re-login."
        } else {
            "Permission to connect to the Docker daemon socket (/var/run/docker.sock: permission denied) was denied because your user does not have docker group permission."
        };
        let fix = if in_docker_group { "newgrp docker" } else { "sudo usermod -aG docker $USER" };
        return Some(format_answer(explanation, fix));
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
        let try_cmd = if stem.is_empty() {
            "systemctl list-unit-files".to_string()
        } else {
            format!("systemctl list-unit-files '*{stem}*'")
        };
        return Some(format_answer(
            &format!(
                "Systemd could not find the unit `{unit}` (`Unit {unit} not found`). The service may not be installed or may use a different unit name."
            ),
            &try_cmd,
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
    let is_exec_denied = last.exit == Some(126)
        || (lower.contains("permission denied")
            && (trimmed_cmd.starts_with("./") || trimmed_cmd.starts_with('/') || shell_exec_denied));
    if !is_exec_denied {
        return None;
    }

    let target = extract_exec_target(last);
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
    if last.exit != Some(127) && from_output.is_none() && !lower.contains("unknown command:") {
        return None;
    }

    let (_, cmd, _) = extract_command_parts(&last.command);
    let missing = from_output.unwrap_or_else(|| cmd.to_string());
    if missing.is_empty() {
        return Some(format_answer("The command was not found (exit code 127).", "nothing"));
    }
    let intro = format!("The command `{missing}` is not installed (`command not found`, exit code 127)");

    // The program is here after all: do not tell the user to install it.
    match platform::locate(&missing) {
        Found::OnPath(file) => {
            return Some(format_answer(
                &format!(
                    "The shell reported `{missing}` as not found, but it is installed at `{}` on this machine. \
                     The shell that ran it probably had a different PATH, or a script it ran looks for another program.",
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
                &format!(
                    "`{missing}` is installed in `{dir}`, which is not on your PATH (`command not found`, exit code 127)."
                ),
                &run,
            ));
        }
        Found::Missing => {}
    }

    let platforms = platform::detect();
    let advice = platform::install_advice(&missing, None, &platforms);
    let typo = |max_dist: usize, with_common: bool| {
        if missing.chars().count() < 2 || (max_dist > 1 && missing.chars().count() < 3) {
            return None;
        }
        let mut candidates = scan_path_executables();
        if with_common {
            for &common in COMMON_COMMANDS {
                if !candidates.iter().any(|c| c == common) {
                    candidates.push(common.to_string());
                }
            }
        }
        closest_typo(&missing, &candidates, max_dist)
    };
    let typo_answer = |correction: String| {
        format_answer(
            &format!(
                "The command `{missing}` was not found (exit code 127), which looks like a typo for `{correction}`."
            ),
            fix_command(&last.command, &missing, &correction).trim(),
        )
    };

    // A name that is not a known program and is one edit from an installed command is a typo
    // (`sl` for `ls`), even if some package happens to ship a program of that name.
    let known = platform::is_known_program(&missing);
    if !known && let Some(correction) = typo(1, false) {
        return Some(typo_answer(correction));
    }
    if let Some(advice) = advice.as_ref().filter(|a| a.installs) {
        let package = advice.package.as_deref().unwrap_or(&missing);
        let note = if package == missing {
            format!("{intro}.")
        } else {
            format!("{intro}; it is provided by the `{package}` package.")
        };
        return Some(format_answer(&note, &advice.command));
    }
    if !known && let Some(correction) = typo(2, true) {
        return Some(typo_answer(correction));
    }
    if let Some(advice) = advice {
        return Some(format_answer(
            &format!(
                "The command `{missing}` is not installed or not in your PATH (`command not found`, exit code 127). \
                 This system's package manager can tell you which package provides it."
            ),
            &advice.command,
        ));
    }
    Some(format_answer(&format!("The command `{missing}` was not found (exit code 127)."), "nothing"))
}

fn diagnose_pacman(last: &Last) -> Option<String> {
    let lower = last.output.to_lowercase();
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
    if lower.contains("[rejected]") && lower.contains("fetch first") {
        return Some(format_answer(
            "Git rejected the push (`[rejected] ... (fetch first)`) because the remote branch has commits you do not have locally yet.",
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
    None
}

fn diagnose_network(last: &Last) -> Option<String> {
    let lower = last.output.to_lowercase();
    if lower.contains("temporary failure in name resolution") || lower.contains("could not resolve host") {
        return Some(format_answer(
            "DNS name resolution failed (`Temporary failure in name resolution`), so the hostname could not be looked up.",
            "ping -c 4 1.1.1.1",
        ));
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
    if lower.contains("connection refused") {
        let port = extract_ssh_port(&last.command, &last.output);
        if last.command.trim_start().starts_with("ssh") || lower.contains("ssh:") || port.is_some() {
            let host = extract_ssh_target(&last.command, &last.output).unwrap_or_else(|| "<host>".to_string());
            let port_str = port.unwrap_or_else(|| "22".to_string());
            return Some(format_answer(
                &format!(
                    "SSH could not connect to `{host}` because the host refused the connection on port {port_str} (the SSH server may not be running, listening on port {port_str}, or reachable)."
                ),
                &format!("ping -c 4 {host}"),
            ));
        }
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
        let evidence = last_meaningful_line(&last.output).unwrap_or("Permission denied");
        if trimmed.is_empty() {
            if lower.contains("interactive authentication required")
                && let Some(systemctl_cmd) = extract_systemd_failed_action(&last.output)
            {
                return Some(format_answer(
                    &format!("The command failed because root or PolicyKit authentication is required (`{evidence}`)."),
                    &format!("sudo {systemctl_cmd}"),
                ));
            }
            return Some(format_answer(
                &format!("The command failed because root or administrator privileges are required (`{evidence}`)."),
                "nothing",
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
        return Some(format_answer(
            &format!(
                "The operation failed due to insufficient permissions (`{evidence}`); root privileges are required."
            ),
            &format!("sudo {trimmed}"),
        ));
    }
    None
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
    if lower.contains("not in gzip format") {
        let archive = extract_tar_archive(last).unwrap_or_else(|| "<archive>".to_string());
        return Some(format_answer(
            &format!(
                "`tar` failed because `{archive}` is not compressed with gzip (`not in gzip format`), so the `-z` flag does not match the file."
            ),
            &format!("tar -xf {archive}"),
        ));
    }
    if let Some(module) = extract_python_module(&last.output) {
        return Some(format_answer(
            &format!(
                "Python could not import `{module}` because that module is not installed (`ModuleNotFoundError: No module named '{module}'`)."
            ),
            &format!("pip install {module}"),
        ));
    }
    if lower.contains("no such process") {
        return Some(format_answer(
            "That process ID does not exist (`No such process`); the process may have already exited or the PID was mistyped.",
            "ps aux",
        ));
    }
    if let Some(err_line) = last.output.lines().find(|l| l.trim_start().starts_with("error[E")) {
        let detail = err_line.trim();
        return Some(format_answer(
            &format!(
                "The Rust compiler rejected the source code (`{detail}`), so the code itself must be adjusted before building again."
            ),
            "nothing",
        ));
    }
    None
}

fn diagnose_fallback(last: &Last) -> String {
    let lower = last.output.to_lowercase();
    let (_, prog, _) = extract_command_parts(&last.command);
    if last.exit == Some(137) {
        return format_answer(
            &format!(
                "The process{} was forcefully killed (exit code 137: {}).",
                cmd_label(&last.command),
                exit_meaning(137).unwrap_or("killed")
            ),
            "nothing",
        );
    }
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
        let help_cmd = if prog.is_empty() { "nothing".to_string() } else { format!("{prog} --help") };
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
    if let Some(line) = last_meaningful_line(&last.output) {
        format_answer(&format!("The command{} failed with {exit_desc}: `{line}`.", cmd_label(&last.command)), "nothing")
    } else {
        format_answer(
            &format!("The command{} failed with {exit_desc} and printed no output.", cmd_label(&last.command)),
            "nothing",
        )
    }
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
            if let Some(end_pos) = rest.to_lowercase().find(" not found") {
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

fn user_in_group(group: &str) -> bool {
    let Ok(user) = std::env::var("USER") else { return false };
    let Ok(groups) = std::fs::read_to_string("/etc/group") else { return false };
    groups.lines().any(|line| {
        let mut cols = line.split(':');
        let Some(name) = cols.next() else { return false };
        if name != group {
            return false;
        }
        cols.nth(2).is_some_and(|members| members.split(',').any(|m| m.trim() == user))
    })
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(suggested(&answer.text), Some("htop"));
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
        assert_eq!(suggested(&ans), Some("htop"));

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
        assert_eq!(suggested(&diagnose(&tar_bad)), Some("tar -xf fake.tar.gz"));

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
    fn fixes_keep_the_arguments() {
        assert_eq!(fix_command("gti status", "gti", "git"), "git status");
        assert_eq!(fix_command("sudo gti -v", "gti", "git"), "sudo git -v");
        assert_eq!(fix_command("echo gti && gti x", "gti", "git"), "echo gti && git x");
        assert_eq!(fix_command("", "gti", "git"), "git");
        assert_eq!(fix_command("mygti x", "gti", "git"), "git");
    }
}
