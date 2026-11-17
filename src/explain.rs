//! Deterministic Error Doctor (`why`) and shell command explainer (`explain_command`).
//!
//! No network calls are made here: errors are diagnosed with live
//! machine checks and structured diagnostic rules, and shell commands are
//! broken down directly against the command catalog.

use crate::catalog::{Command, SlotKind};
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

/// How packages are installed on each family of systems the catalog knows.
const INSTALL: &[(&str, &str)] = &[
    ("arch", "sudo pacman -S <package>"),
    ("debian", "sudo apt install <package>"),
    ("fedora", "sudo dnf install <package>"),
];

/// Binary-to-package mappings where the package name differs or is commonly missing.
const KNOWN_PACKAGES: &[(&str, &str)] = &[
    ("rg", "ripgrep"),
    ("jq", "jq"),
    ("zbarimg", "zbar"),
    ("virsh", "libvirt"),
    ("smartctl", "smartmontools"),
    ("nmap", "nmap"),
    ("ss", "iproute2"),
    ("ip", "iproute2"),
    ("upower", "upower"),
    ("fd", "fd"),
    ("bat", "bat"),
    ("eza", "eza"),
    ("fzf", "fzf"),
    ("htop", "htop"),
    ("btop", "btop"),
    ("tmux", "tmux"),
    ("nvim", "neovim"),
    ("vim", "vim"),
    ("tree", "tree"),
    ("unzip", "unzip"),
    ("unrar", "unrar"),
    ("zip", "zip"),
    ("7z", "7zip"),
    ("rsync", "rsync"),
    ("wget", "wget"),
    ("curl", "curl"),
    ("ffmpeg", "ffmpeg"),
    ("convert", "imagemagick"),
    ("magick", "imagemagick"),
    ("lsof", "lsof"),
    ("strace", "strace"),
    ("iperf3", "iperf3"),
    ("ethtool", "ethtool"),
    ("lsusb", "usbutils"),
    ("lspci", "pciutils"),
    ("dig", "bind"),
    ("nc", "openbsd-netcat"),
    ("socat", "socat"),
    ("sqlite3", "sqlite"),
    ("git", "git"),
    ("docker", "docker"),
    ("tailscale", "tailscale"),
    ("ollama", "ollama"),
    ("python", "python"),
    ("python3", "python"),
    ("pip", "python-pip"),
    ("node", "nodejs"),
    ("npm", "npm"),
    ("cargo", "rust"),
    ("rustc", "rust"),
];

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
    let platforms = crate::catalog::platforms_from(os_release);
    let mut facts = format!("{name}, shell {}.", shell.rsplit('/').next().unwrap_or_default());
    if let Some((_, install)) = INSTALL.iter().find(|(family, _)| platforms.iter().any(|p| p == family)) {
        facts.push_str(&format!(" Packages are installed with `{install}`."));
    }
    facts
}

fn read_os_release() -> String {
    std::fs::read_to_string("/etc/os-release").unwrap_or_default()
}

fn package_install_command(package: &str, os_release: &str) -> Option<String> {
    let platforms = crate::catalog::platforms_from(os_release);
    if let Some((_, tmpl)) = INSTALL.iter().find(|(family, _)| platforms.iter().any(|p| p == family)) {
        return Some(tmpl.replace("<package>", package));
    }
    if platforms.iter().any(|p| p == "omarchy") {
        return Some(format!("omarchy pkg add {package}"));
    }
    None
}

/// Kept for prompt/context formatting helpers and tests.
#[cfg(test)]
fn system_line() -> String {
    system_facts(&read_os_release(), &std::env::var("SHELL").unwrap_or_default())
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

fn omarchy_install_prose(pkg: &str, os_release: &str) -> String {
    if crate::catalog::platforms_from(os_release).iter().any(|p| p == "omarchy") {
        format!(" You can also install it with `omarchy pkg add {pkg}`.")
    } else {
        String::new()
    }
}

fn diagnose_command_not_found(last: &Last) -> Option<String> {
    let lower = last.output.to_lowercase();
    if last.exit != Some(127) && !lower.contains("command not found") && !lower.contains("unknown command:") {
        return None;
    }

    let (prefix, cmd, suffix) = extract_command_parts(&last.command);
    let missing = if !cmd.is_empty() { cmd.to_string() } else { extract_missing_cmd_from_output(&last.output) };
    if missing.is_empty() {
        return Some(format_answer("The command was not found (exit code 127).", "nothing"));
    }

    // Known binary -> package mapping takes precedence over short fuzzy typo guesses (e.g. `rg`, `jq`).
    if let Some((_, pkg)) = KNOWN_PACKAGES.iter().find(|(bin, _)| *bin == missing) {
        let os_release = read_os_release();
        let omarchy_hint = omarchy_install_prose(pkg, &os_release);
        let pkg_note = if *pkg == missing {
            format!("The command `{missing}` is not installed (`command not found`, exit code 127).{omarchy_hint}")
        } else {
            format!(
                "The command `{missing}` is not installed (`command not found`, exit code 127); it is provided by the `{pkg}` package.{omarchy_hint}"
            )
        };
        if let Some(install_cmd) = package_install_command(pkg, &os_release) {
            return Some(format_answer(&pkg_note, &install_cmd));
        }
        return Some(format_answer(&pkg_note, "nothing"));
    }

    // Live check: scan $PATH executables + common commands for a close typo (Damerau-Levenshtein <= 2).
    if missing.chars().count() >= 3 {
        let mut candidates = scan_path_executables();
        for &common in COMMON_COMMANDS {
            if !candidates.iter().any(|c| c == common) {
                candidates.push(common.to_string());
            }
        }
        let max_dist = if missing.chars().count() == 3 { 1 } else { 2 };
        if let Some(correction) = closest_typo(&missing, &candidates, max_dist) {
            let fixed = format!("{prefix}{correction}{suffix}");
            return Some(format_answer(
                &format!(
                    "The command `{missing}` was not found (exit code 127), which looks like a typo for `{correction}`."
                ),
                fixed.trim(),
            ));
        }
    }

    let os_release = read_os_release();
    if let Some(install_cmd) = package_install_command(&missing, &os_release) {
        let omarchy_hint = omarchy_install_prose(&missing, &os_release);
        return Some(format_answer(
            &format!(
                "The command `{missing}` is not installed or not in your PATH (`command not found`, exit code 127).{omarchy_hint}"
            ),
            &install_cmd,
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

fn extract_missing_cmd_from_output(output: &str) -> String {
    for line in output.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("bash: ").or_else(|| t.strip_prefix("sh: ")) {
            // e.g. "bash: htpo: command not found", "bash: line 1: htpo: command not found", "sh: 1: htpo: not found"
            let parts: Vec<&str> = rest.split(':').map(str::trim).collect();
            for p in &parts {
                if !p.is_empty()
                    && !p.chars().all(|c| c.is_ascii_digit())
                    && !is_line_prefix(p)
                    && !p.contains("not found")
                {
                    return (*p).to_string();
                }
            }
        }
        if let Some(rest) = t.strip_prefix("zsh: command not found:") {
            return rest.trim().to_string();
        }
        if let Some(rest) =
            t.strip_prefix("fish: Unknown command:").or_else(|| t.strip_prefix("fish: unknown command:"))
        {
            return rest.trim().to_string();
        }
    }
    String::new()
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
    let Some(path_os) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    for dir in std::env::split_paths(&path_os) {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let Ok(meta) = entry.metadata() else { continue };
                if meta.permissions().mode() & 0o111 == 0 {
                    continue;
                }
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
    let mut best: Option<(usize, bool, usize, String)> = None;

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
        let key = (dist, !same_first, len_diff, cand.clone());
        if best.as_ref().is_none_or(|b| key < *b) {
            best = Some(key);
        }
    }
    best.map(|(_, _, _, cand)| cand)
}

// ---------------------------------------------------------------------------
// Shell Command Explainer (`huh explain "<command line>"`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum ShellToken {
    Word { raw: String, unquoted: String, quoted: bool },
    Op(String),
}

impl ShellToken {
    fn as_word(&self) -> Option<(&str, &str, bool)> {
        match self {
            ShellToken::Word { raw, unquoted, quoted } => Some((raw, unquoted, *quoted)),
            ShellToken::Op(_) => None,
        }
    }
}

fn tokenize_shell(line: &str) -> Vec<ShellToken> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }

        // Check multi-character operators first (outside quotes).
        let rest: String = chars[i..].iter().take(4).collect();
        let matched_op = ["2>&1", "&>>", "&&", "||", ">>", "2>>", "2>", "&>", "|", ";", ">", "<"]
            .into_iter()
            .find(|op| rest.starts_with(op));
        if let Some(op) = matched_op {
            tokens.push(ShellToken::Op(op.to_string()));
            i += op.chars().count();
            continue;
        }

        let mut raw = String::new();
        let mut unquoted = String::new();
        let mut was_quoted = false;
        let mut quote: Option<char> = None;

        while i < chars.len() {
            let c = chars[i];
            match quote {
                Some('\'') => {
                    raw.push(c);
                    if c == '\'' {
                        quote = None;
                    } else {
                        unquoted.push(c);
                    }
                    i += 1;
                }
                Some('"') => {
                    raw.push(c);
                    if c == '\\' && i + 1 < chars.len() {
                        i += 1;
                        raw.push(chars[i]);
                        unquoted.push(chars[i]);
                        i += 1;
                    } else if c == '"' {
                        quote = None;
                        i += 1;
                    } else {
                        unquoted.push(c);
                        i += 1;
                    }
                }
                Some(_) => unreachable!(),
                None => {
                    if c.is_whitespace() {
                        break;
                    }
                    let lookahead: String = chars[i..].iter().take(4).collect();
                    if ["2>&1", "&>>", "&&", "||", ">>", "2>>", "2>", "&>", "|", ";", ">", "<"]
                        .iter()
                        .any(|op| lookahead.starts_with(op))
                    {
                        break;
                    }
                    if c == '\\' && i + 1 < chars.len() {
                        raw.push(c);
                        i += 1;
                        raw.push(chars[i]);
                        unquoted.push(chars[i]);
                        i += 1;
                    } else if c == '\'' || c == '"' {
                        was_quoted = true;
                        raw.push(c);
                        quote = Some(c);
                        i += 1;
                    } else {
                        raw.push(c);
                        unquoted.push(c);
                        i += 1;
                    }
                }
            }
        }
        tokens.push(ShellToken::Word { raw, unquoted, quoted: was_quoted });
    }
    tokens
}

fn split_pipeline_segments(tokens: &[ShellToken]) -> Vec<(Vec<ShellToken>, Option<String>)> {
    let mut segments = Vec::new();
    let mut current = Vec::new();
    for tok in tokens {
        match tok {
            ShellToken::Op(op) if matches!(op.as_str(), "|" | "&&" | "||" | ";") => {
                segments.push((std::mem::take(&mut current), Some(op.clone())));
            }
            other => current.push(other.clone()),
        }
    }
    if !current.is_empty() || segments.is_empty() {
        segments.push((current, None));
    }
    segments
}

fn explain_operator(op: &str) -> &'static str {
    match op {
        "|" => "pipes standard output from the previous command into the next command",
        "&&" => "runs the next command only if the previous command succeeded",
        "||" => "runs the next command only if the previous command failed",
        ";" => "runs the next command unconditionally after the previous command finishes",
        ">" => "overwrites the target file with standard output",
        ">>" => "appends standard output to the end of the target file",
        "<" => "reads standard input from the target file",
        "2>&1" => "merges standard error (stderr) into standard output (stdout)",
        "2>" => "overwrites the target file with standard error",
        "2>>" => "appends standard error to the target file",
        "&>" | "&>>" => "redirects both standard output and standard error to a file",
        _ => "shell operator",
    }
}

/// Converts a catalog flag's yes/no question into a crisp second-person/imperative statement.
fn question_to_statement(question: &str) -> String {
    let q = question.trim().trim_end_matches('?').trim();
    if let Some(rest) = q.strip_prefix("Does the user want to ") {
        return capitalize(&second_person_pronouns(rest));
    }
    if let Some(rest) = q.strip_prefix("Does the user only want ") {
        return format!("Only show {}", second_person_pronouns(rest));
    }
    if let Some(rest) = q.strip_prefix("Does the user want ") {
        let rest = second_person_pronouns(rest);
        if let Some(before) = rest.strip_suffix(" included") {
            return format!("Include {before}");
        }
        if let Some((before, after)) = rest.split_once(" included in ") {
            return format!("Include {before} in {after}");
        }
        if let Some(before) = rest.strip_suffix(" shown") {
            return format!("Show {before}");
        }
        if let Some(before) = rest.strip_suffix(" turned off") {
            return format!("Turn {before} off");
        }
        if let Some(before) = rest.strip_suffix(" created") {
            return format!("Create {before}");
        }
        if let Some(before) = rest.strip_suffix(" searched") {
            return format!("Search {before}");
        }
        if let Some(before) = rest.strip_suffix(" listed first") {
            return format!("List {before} first");
        }
        if let Some(before) = rest.strip_suffix(" saved as a file") {
            return format!("Save {before} as a file");
        }
        if let Some((x, y)) = rest.split_once(" applied to ") {
            return format!("Apply {x} to {y}");
        }
        if let Some((x, y)) = rest.split_once(" ordered as ") {
            return format!("Order {x} as {y}");
        }
        if let Some(after) = rest.strip_prefix("the order reversed, ") {
            return format!("Reverse the order, {after}");
        }
        if let Some((x, y)) = rest.split_once(" treated as ") {
            return format!("Treat {x} as {y}");
        }
        if let Some((x, y)) = rest.split_once(" created inside ") {
            return format!("Create {x} inside {y}");
        }
        if let Some(subject) = rest.strip_suffix(" to keep running in the background") {
            return format!("Keep {subject} running in the background");
        }
        return format!("Include {rest}");
    }
    if let Some(rest) = q.strip_prefix("Does the user say ") {
        return format!("Used when {}", second_person_pronouns(rest));
    }
    if let Some(rest) = q.strip_prefix("Is the user asking about ") {
        return format!("Focus on {}", second_person_pronouns(rest));
    }
    if q == "Is the thing being deleted a folder" {
        return "Delete folders and everything inside them recursively".to_string();
    }
    if q == "Is the thing being copied a folder" {
        return "Copy folders and everything inside them recursively".to_string();
    }
    if q.starts_with("Is the thing being ") {
        return q.to_string();
    }
    capitalize(&second_person_pronouns(&q.replace("Does the user", "Do you").replace("Is the user", "Are you")))
}

fn second_person_pronouns(text: &str) -> String {
    text.replace("the user's", "your").replace("their", "your").replace("the user", "you")
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

/// Literal subcommand prefix words extracted from a catalog template's relevant pipeline stage.
/// Returns `(prefix_tokens, stage_text, is_primary_stage)`.
fn template_stage_prefix<'a>(template: &'a str, stage_program: &str) -> Option<(Vec<String>, &'a str, bool)> {
    for (stage_idx, stage) in template.split('|').enumerate() {
        let words: Vec<&str> = stage.split_whitespace().filter(|w| *w != "sudo").collect();
        let Some(&prog) = words.first() else { continue };
        if prog != stage_program {
            continue;
        }
        let mut prefix = Vec::new();
        for w in words {
            if w.starts_with('{') || w.contains('{') {
                break;
            }
            // Keep subcommand words (non-flags) and flag-discriminators for pacman/yay/systemctl.
            let is_subcommand_flag = matches!(prog, "pacman" | "yay") && w.starts_with('-')
                || (prog == "systemctl" && w == "--user")
                || (prog == "ip" && w == "-br");
            if w.starts_with('-') && !is_subcommand_flag {
                break;
            }
            // Stop before literal fixed options on single-command tools (e.g. `ping -c 4`, `virsh list --all`).
            if !prefix.is_empty() && w.chars().all(|c| c.is_ascii_digit()) {
                break;
            }
            prefix.push(w.to_string());
        }
        return Some((prefix, stage, stage_idx == 0));
    }
    None
}

fn match_echo_command<'a>(
    catalog: &'a [Command],
    words: &[&ShellToken],
) -> Option<(&'a Command, usize, String, &'a str)> {
    let arg = words.get(1).and_then(|t| t.as_word()).map(|(_, u, _)| u).unwrap_or("");
    let target_name = if arg.contains("export PATH=") {
        "add to PATH"
    } else if arg.starts_with("alias ") {
        "add shell alias"
    } else if arg.starts_with("export ") {
        "set shell variable"
    } else {
        return None;
    };
    catalog.iter().find(|c| c.name == target_name).map(|c| (c, 1, "echo".to_string(), c.template.as_str()))
}

fn match_catalog_command<'a>(
    catalog: &'a [Command],
    words: &[&ShellToken],
) -> Option<(&'a Command, usize, String, &'a str)> {
    let (_, prog, _) = words.first()?.as_word()?;
    if prog == "echo" {
        return match_echo_command(catalog, words);
    }

    let word_strings: Vec<&str> = words.iter().filter_map(|t| t.as_word().map(|(_, u, _)| u)).collect();

    let mut best: Option<(&Command, usize, String, &str, bool)> = None;
    let mut primary_multiword_only = false;
    let mut primary_found = false;

    for cmd in catalog {
        let Some((prefix, stage, is_primary)) = template_stage_prefix(&cmd.template, prog) else {
            continue;
        };
        if is_primary {
            primary_found = true;
            if prefix.len() > 1 {
                primary_multiword_only = true;
            }
        }
        if word_strings.len() >= prefix.len() && prefix.iter().enumerate().all(|(i, p)| word_strings[i] == p) {
            let label = prefix.join(" ");
            let better = best.as_ref().is_none_or(|(_, len, _, _, best_primary)| {
                (is_primary && !*best_primary) || (is_primary == *best_primary && prefix.len() > *len)
            });
            if better {
                best = Some((cmd, prefix.len(), label, stage, is_primary));
            }
        }
    }

    if let Some((cmd, len, label, stage, _)) = best {
        return Some((cmd, len, label, stage));
    }
    if primary_found && !primary_multiword_only {
        return catalog.iter().find_map(|c| {
            let (prefix, stage, is_primary) = template_stage_prefix(&c.template, prog)?;
            (is_primary && prefix.first().is_some_and(|p| p == prog)).then(|| (c, 1, prog.to_string(), stage))
        });
    }
    None
}

fn explain_common_flag(program: &str, flag: &str) -> Option<&'static str> {
    // Program-specific well-known flags
    match (program, flag) {
        ("du" | "df" | "ls" | "free", "-h") => {
            return Some("human-readable: print sizes in easy-to-read units (e.g. 1K, 234M, 2G)");
        }
        ("sort", "-h") => return Some("human-numeric-sort: compare human-readable numbers such as 2K or 1G"),
        ("du", "-s") => return Some("summarize: display only a total for each argument"),
        ("ln", "-s") => return Some("create a symbolic (soft) link instead of a hard link"),
        ("curl", "-L") => return Some("follow HTTP redirects automatically"),
        ("curl", "-s") => return Some("silent mode: hide progress meter and error messages"),
        ("curl", "-S") => return Some("show errors even when silent mode (-s) is enabled"),
        ("curl", "-k") | ("curl", "--insecure") => {
            return Some("allow insecure TLS/SSL connections without verifying certificates");
        }
        ("curl", "-I") => return Some("fetch only the HTTP response headers"),
        ("rsync", "-a") => {
            return Some("archive mode: preserve permissions, timestamps, symlinks, and recurse into folders");
        }
        ("rsync", "-z") => return Some("compress file data during transfer"),
        ("tar", "-x") => return Some("extract files from an archive"),
        ("tar", "-c") => return Some("create a new archive"),
        ("tar", "-t") => return Some("list the contents of an archive"),
        ("tar", "-z") => return Some("filter the archive through gzip compression"),
        ("tar", "-j") => return Some("filter the archive through bzip2 compression"),
        ("tar", "-J") => return Some("filter the archive through xz compression"),
        ("tar", "-f") => return Some("specify the archive filename"),
        ("ss", "-t") => return Some("show TCP sockets"),
        ("ss", "-u") => return Some("show UDP sockets"),
        ("ss", "-l") => return Some("show only listening sockets"),
        ("ss", "-p") => return Some("show the process using each socket"),
        ("ss", "-n") => return Some("show numeric ports and addresses instead of resolving names"),
        ("ping", "-c") => return Some("stop after sending the specified number of packets"),
        ("head" | "tail" | "journalctl", "-n") => return Some("specify the number of lines to display"),
        ("tracepath", "-n") => return Some("print IP addresses numerically without resolving hostnames"),
        ("pgrep", "-a") => return Some("list both process IDs and full command lines"),
        ("pgrep", "-l") => return Some("list process IDs along with process names"),
        ("uname", "-a") => return Some("print all system and kernel information"),
        ("git", "-m") => return Some("provide the commit message directly on the command line"),
        ("docker", "-d") => return Some("detached mode: run containers in the background"),
        ("docker", "-i") => return Some("keep standard input open even if not attached"),
        ("docker", "-t") => return Some("allocate a pseudo-TTY terminal"),
        ("docker", "-p") => return Some("publish a container port to the host"),
        ("mkdir", "-p") => return Some("create parent directories as needed without error if they exist"),
        ("tee", "-a") => return Some("append to the file instead of overwriting it"),
        ("grep", "-E") => return Some("use extended regular expressions"),
        ("grep", "-F") => return Some("treat the pattern as a fixed literal string"),
        ("grep", "-v") => return Some("invert match: select lines that do not match"),
        ("grep", "-w") => return Some("match whole words only"),
        ("grep", "-c") => return Some("print only a count of matching lines"),
        ("cat", "-n") => return Some("number all output lines"),
        ("ps", "aux") => return Some("show all running processes for all users with user, CPU, and memory details"),
        ("tmux", "-s") => return Some("name the new tmux session"),
        ("tmux", "-t") => return Some("target a specific tmux session"),
        ("smartctl", "-a") => return Some("print all SMART health and diagnostic information for the device"),
        ("usermod", "-aG") => return Some("append the user to supplementary groups without removing existing ones"),
        ("usermod", "-a") => return Some("append the user to supplementary groups (used with -G)"),
        ("usermod", "-G") => return Some("specify supplementary group(s) for the user"),
        _ => {}
    }
    // General Unix flags
    match flag {
        "-h" | "--help" => Some("display help and usage information"),
        "-V" | "--version" => Some("display the program version"),
        "-v" | "--verbose" => Some("verbose output: show more detail about what is happening"),
        "-q" | "--quiet" | "--silent" => Some("quiet mode: suppress normal output"),
        "-f" | "--force" => Some("force the operation without prompting or failing on missing targets"),
        "-r" | "-R" | "--recursive" => Some("operate recursively on folders and everything inside them"),
        "-a" | "--all" => Some("include all entries, including hidden or inactive ones"),
        "-y" | "--yes" | "--noconfirm" => Some("answer yes automatically to confirmation prompts"),
        "--dry-run" => Some("preview what would happen without making changes"),
        "-i" | "--interactive" => Some("prompt for confirmation before modifying targets"),
        _ => None,
    }
}

fn explain_flag_in_command(cmd: &Command, program: &str, flag: &str) -> Option<String> {
    if let Some(f) = cmd.flags.iter().find(|f| f.value == flag) {
        return Some(question_to_statement(&f.question));
    }
    if let Some(desc) = explain_common_flag(program, flag) {
        return Some(desc.to_string());
    }
    None
}

fn split_segment_redirections(tokens: &[ShellToken]) -> (Vec<ShellToken>, Vec<(String, Option<String>)>) {
    let mut cmd_tokens = Vec::new();
    let mut redirs = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if let ShellToken::Op(op) = &tokens[i] {
            if matches!(op.as_str(), "2>&1") {
                redirs.push((op.clone(), None));
                i += 1;
                continue;
            }
            if matches!(op.as_str(), ">" | ">>" | "<" | "2>" | "2>>" | "&>" | "&>>") {
                let target = tokens.get(i + 1).and_then(|t| t.as_word()).map(|(raw, _, _)| raw.to_string());
                if target.is_some() {
                    i += 2;
                } else {
                    i += 1;
                }
                redirs.push((op.clone(), target));
                continue;
            }
        }
        cmd_tokens.push(tokens[i].clone());
        i += 1;
    }
    (cmd_tokens, redirs)
}

pub fn explain_command(catalog: &[Command], line: &str) -> Vec<(String, String)> {
    let tokens = tokenize_shell(line);
    let segments = split_pipeline_segments(&tokens);
    let mut out = Vec::new();
    let mut after_pipe = false;

    for (seg_tokens, next_op) in segments {
        explain_segment(catalog, &seg_tokens, after_pipe, &mut out);
        if let Some(op) = &next_op {
            out.push((op.clone(), explain_operator(op).to_string()));
        }
        after_pipe = next_op.as_deref() == Some("|");
    }
    out
}

fn explain_segment(catalog: &[Command], tokens: &[ShellToken], after_pipe: bool, out: &mut Vec<(String, String)>) {
    let (cmd_tokens, redirs) = split_segment_redirections(tokens);
    let mut idx = 0;
    while idx < cmd_tokens.len() {
        if let Some((_, unquoted, _)) = cmd_tokens[idx].as_word() {
            if unquoted == "sudo" {
                out.push(("sudo".into(), "run this command with administrator (root) privileges".into()));
                idx += 1;
                continue;
            }
            if unquoted.contains('=') && !unquoted.starts_with('-') && idx + 1 < cmd_tokens.len() {
                out.push((unquoted.to_string(), "set an environment variable for this command".into()));
                idx += 1;
                continue;
            }
        }
        break;
    }

    let words: Vec<&ShellToken> = cmd_tokens[idx..].iter().collect();
    if words.is_empty() {
        emit_redirs(&redirs, out);
        return;
    }

    let matched = match_catalog_command(catalog, &words);
    if let Some((cmd, consumed, label, stage)) = matched {
        let mut desc = if after_pipe && cmd.name == "grep" {
            "Filter matching lines from the piped output".to_string()
        } else {
            cmd.description.clone()
        };
        if cmd.destructive {
            desc.push_str(" — WARNING: deletes or overwrites data");
        }
        out.push((label, desc));

        let program = words[0].as_word().map_or("", |(_, u, _)| u);
        let pos_slots: Vec<&crate::catalog::Slot> = cmd
            .slots
            .iter()
            .filter(|s| {
                matches!(s.kind, SlotKind::Path | SlotKind::Text)
                    && s.format.is_none()
                    && stage.contains(&format!("{{{}}}", s.id))
                    && !stage.contains(&format!("-f{{{}}}", s.id))
                    && !stage.contains(&format!("-n {{{}}}", s.id))
            })
            .collect();
        let mut pos_idx = 0usize;
        let mut wi = consumed;

        while wi < words.len() {
            let Some((raw, unquoted, quoted)) = words[wi].as_word() else {
                wi += 1;
                continue;
            };

            // 1. Check 2-token choice option values like `-type f`, `-size +10M`.
            if !quoted
                && wi + 1 < words.len()
                && let Some((_, next_u, _)) = words[wi + 1].as_word()
            {
                let pair = format!("{unquoted} {next_u}");
                if let Some(opt) = cmd
                    .slots
                    .iter()
                    .filter(|s| s.kind == SlotKind::Choice)
                    .flat_map(|s| &s.options)
                    .find(|o| o.value == pair)
                {
                    out.push((pair, opt.description.clone()));
                    wi += 2;
                    continue;
                }
            }

            // 2. Check formatted slots like `-name {}`, `-mtime -{}`, `-u {}`, `-n {} --no-pager`.
            if !quoted
                && let Some(slot) = cmd.slots.iter().find(|s| {
                    s.format.as_deref().is_some_and(|fmt| {
                        let prefix = fmt.split_whitespace().next().unwrap_or("");
                        prefix == unquoted
                    })
                })
                && wi + 1 < words.len()
                && let Some((arg_raw, arg_u, _)) = words[wi + 1].as_word()
            {
                let what = slot.what.as_deref().unwrap_or(&slot.id);
                let val_clean = if slot.format.as_deref().is_some_and(|f| f.contains("-{}")) {
                    arg_u.trim_start_matches('-')
                } else {
                    arg_u
                };
                out.push((format!("{unquoted} {arg_raw}"), format!("{what} ({val_clean})")));
                wi += 2;
                // Consume trailing `--no-pager` if part of the slot's format.
                if slot.format.as_deref().is_some_and(|f| f.ends_with("--no-pager"))
                    && words.get(wi).and_then(|t| t.as_word()).is_some_and(|(_, u, _)| u == "--no-pager")
                {
                    out.push(("--no-pager".into(), "print output directly without opening a pager".into()));
                    wi += 1;
                }
                continue;
            }

            // 3. Check template inline `-f{field}` (e.g. `cut -f3` or `cut -f 3`).
            if !quoted
                && (unquoted == "-f" || (unquoted.starts_with("-f") && unquoted.len() > 2))
                && cmd.template.contains("-f{")
                && let Some(slot) = cmd.slots.iter().find(|s| s.id == "field")
            {
                let what = slot.what.as_deref().unwrap_or("field number");
                if unquoted == "-f"
                    && wi + 1 < words.len()
                    && let Some((_, next_u, _)) = words[wi + 1].as_word()
                {
                    out.push((format!("-f {next_u}"), format!("{what} ({next_u})")));
                    wi += 2;
                } else {
                    out.push((unquoted.to_string(), format!("{what} ({})", &unquoted[2..])));
                    wi += 1;
                }
                continue;
            }

            // 4. Check single-token choice option values (like `+x`, `600`, `-xzf`, `-l`, `-d,`, `start`, `status`).
            if !quoted
                && let Some(opt) = cmd
                    .slots
                    .iter()
                    .filter(|s| s.kind == SlotKind::Choice)
                    .flat_map(|s| &s.options)
                    .find(|o| !o.value.is_empty() && (o.value == unquoted || o.value == raw))
            {
                out.push((raw.to_string(), opt.description.clone()));
                wi += 1;
                continue;
            }

            // 5. Check exact flag in `cmd.flags` or `explain_common_flag`.
            if !quoted && (unquoted.starts_with('-') || (program == "ps" && unquoted == "aux")) {
                if let Some(desc) = explain_flag_in_command(cmd, program, unquoted) {
                    out.push((unquoted.to_string(), desc));
                    // Consume numeric value right after `-n` or `-c` if the template expects a count/arg.
                    if matches!(unquoted, "-n" | "-c")
                        && wi + 1 < words.len()
                        && let Some((_, next_u, _)) = words[wi + 1].as_word()
                        && next_u.chars().all(|c| c.is_ascii_digit())
                    {
                        if let Some(slot) = cmd.slots.iter().find(|s| s.id == "count") {
                            let what = slot.what.as_deref().unwrap_or("count");
                            out.push((next_u.to_string(), what.to_string()));
                        } else {
                            out.push((next_u.to_string(), "number / count argument".to_string()));
                        }
                        wi += 2;
                        continue;
                    }
                    wi += 1;
                    continue;
                }

                // 6. Combined short flags like `-la`, `-sh`, `-rf`, `-xzf`.
                if unquoted.starts_with('-')
                    && !unquoted.starts_with("--")
                    && unquoted.len() > 2
                    && !unquoted[1..].chars().all(|c| c.is_ascii_digit())
                {
                    let chars: Vec<char> = unquoted[1..].chars().collect();
                    let parts: Vec<(String, Option<String>)> = chars
                        .iter()
                        .map(|c| {
                            let single = format!("-{c}");
                            let desc = explain_flag_in_command(cmd, program, &single);
                            (single, desc)
                        })
                        .collect();
                    if parts.iter().any(|(_, d)| d.is_some()) {
                        for (single, desc) in parts {
                            out.push((single, desc.unwrap_or_else(|| "not in the catalog".to_string())));
                        }
                        wi += 1;
                        continue;
                    }
                }

                out.push((raw.to_string(), "not in the catalog".to_string()));
                wi += 1;
                continue;
            }

            // 7. Literal `$USER` in template (e.g. `sudo usermod -aG {group} $USER`).
            if unquoted == "$USER" && stage.split_whitespace().any(|w| w == "$USER") {
                out.push(("$USER".to_string(), "your current username (the user account)".to_string()));
                wi += 1;
                continue;
            }

            // 8. Positional argument -> match against remaining Path/Text slots.
            if pos_idx < pos_slots.len() {
                let slot = pos_slots[pos_idx];
                let what = slot.what.as_deref().unwrap_or(&slot.id);
                let detail = if unquoted == "." && slot.kind == SlotKind::Path {
                    format!("{what} (current folder)")
                } else if unquoted == "*" && slot.kind == SlotKind::Path {
                    format!("{what} (wildcard matching all items in the current folder)")
                } else if let Some((dir, "*")) = unquoted.rsplit_once('/')
                    && slot.kind == SlotKind::Path
                {
                    format!("{what} (wildcard matching all items inside {dir})")
                } else if unquoted.contains('*') && slot.kind == SlotKind::Path {
                    format!("{what} (wildcard pattern matching multiple items)")
                } else {
                    what.to_string()
                };
                out.push((unquoted.to_string(), detail));
                pos_idx += 1;
            } else if !pos_slots.is_empty() {
                // Additional file/target argument for the last slot (e.g., `rm a.txt b.txt`).
                let last_slot = pos_slots[pos_slots.len() - 1];
                let what = last_slot.what.as_deref().unwrap_or(&last_slot.id);
                out.push((unquoted.to_string(), what.to_string()));
            } else {
                out.push((unquoted.to_string(), "not in the catalog".to_string()));
            }
            wi += 1;
        }
    } else {
        // Command not in catalog (or builtin like `echo`)
        let program = words[0].as_word().map_or("", |(_, u, _)| u);
        if program == "echo" {
            out.push(("echo".into(), "print text to standard output (shell builtin)".into()));
            for tok in &words[1..] {
                if let Some((_, u, _)) = tok.as_word() {
                    out.push((u.to_string(), "text to print".into()));
                }
            }
        } else {
            out.push((program.to_string(), "not in the catalog".to_string()));
            for tok in &words[1..] {
                if let Some((raw, u, quoted)) = tok.as_word() {
                    if !quoted && u.starts_with('-') {
                        let desc = explain_common_flag(program, u).unwrap_or("not in the catalog");
                        out.push((raw.to_string(), desc.to_string()));
                    } else {
                        out.push((u.to_string(), "not in the catalog".to_string()));
                    }
                }
            }
        }
    }

    emit_redirs(&redirs, out);
}

fn emit_redirs(redirs: &[(String, Option<String>)], out: &mut Vec<(String, String)>) {
    for (op, target) in redirs {
        out.push((op.clone(), explain_operator(op).to_string()));
        if let Some(file) = target {
            let role = match op.as_str() {
                "<" => "input file to read from",
                ">>" | "2>>" | "&>>" => "output file to append to",
                _ => "output file to write to",
            };
            out.push((file.clone(), role.to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_test_catalog() -> Vec<Command> {
        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/catalog"));
        crate::catalog::load(dir).unwrap()
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
        assert_eq!(facts, "Omarchy, shell bash. Packages are installed with `sudo pacman -S <package>`.");
        assert_eq!(system_facts("NAME=Void\nID=void\n", "/bin/zsh"), "Void, shell zsh.");
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

        assert_eq!(
            package_install_command("ripgrep", "ID=omarchy\nID_LIKE=arch\n").as_deref(),
            Some("sudo pacman -S ripgrep")
        );
        assert_eq!(
            package_install_command("jq", "ID=ubuntu\nID_LIKE=debian\n").as_deref(),
            Some("sudo apt install jq")
        );
        assert_eq!(package_install_command("jq", "ID=fedora\n").as_deref(), Some("sudo dnf install jq"));
    }

    #[test]
    fn diagnoses_missing_packages_and_common_failures() {
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
    fn explains_ten_plus_command_lines() {
        let catalog = load_test_catalog();

        // 1. Combined flags on ls + directory
        let r1 = explain_command(&catalog, "ls -la src/");
        assert!(r1.iter().any(|(p, d)| p == "ls" && d.contains("List the files")));
        assert!(r1.iter().any(|(p, d)| p == "-l" && d.contains("details")));
        assert!(r1.iter().any(|(p, d)| p == "-a" && d.contains("hidden")));
        assert!(r1.iter().any(|(p, d)| p == "src/" && d.contains("folder")));

        // 2. Pipeline: du -sh . | sort -h
        let r2 = explain_command(&catalog, "du -sh . | sort -h");
        assert!(r2.iter().any(|(p, _)| p == "du"));
        assert!(r2.iter().any(|(p, d)| p == "-s" && d.contains("summarize")));
        assert!(r2.iter().any(|(p, d)| p == "-h" && d.contains("human-readable")));
        assert!(r2.iter().any(|(p, d)| p == "|" && d.contains("pipes")));
        assert!(r2.iter().any(|(p, _)| p == "sort"));

        // 3. sudo + pacman -S
        let r3 = explain_command(&catalog, "sudo pacman -S ripgrep");
        assert!(r3.iter().any(|(p, d)| p == "sudo" && d.contains("administrator")));
        assert!(r3.iter().any(|(p, d)| p == "pacman -S" && d.contains("Install")));
        assert!(r3.iter().any(|(p, d)| p == "ripgrep" && d.contains("program to install")));

        // 4. tar -xzf
        let r4 = explain_command(&catalog, "tar -xzf photos.tar.gz");
        assert!(r4.iter().any(|(p, _)| p == "tar"));
        assert!(r4.iter().any(|(p, d)| p == "-xzf" && d.contains("Unpack")));
        assert!(r4.iter().any(|(p, d)| p == "photos.tar.gz" && d.contains("archive")));

        // 5. git commit with quoted message containing pipe
        let r5 = explain_command(&catalog, r#"git commit -m "fix | pipe handling""#);
        assert_eq!(r5.iter().filter(|(p, _)| p == "|").count(), 0);
        assert!(r5.iter().any(|(p, _)| p == "git commit"));
        assert!(r5.iter().any(|(p, d)| p == "fix | pipe handling" && d.contains("commit message")));

        // 6. docker compose up -d
        let r6 = explain_command(&catalog, "docker compose up -d");
        assert!(r6.iter().any(|(p, d)| p == "docker compose up" && d.contains("Docker Compose")));
        assert!(r6.iter().any(|(p, d)| p == "-d" && d.contains("background")));

        // 7. Destructive command: rm -rf build/
        let r7 = explain_command(&catalog, "rm -rf build/");
        assert!(r7.iter().any(|(p, d)| p == "rm" && d.contains("WARNING: deletes or overwrites data")));
        assert!(r7.iter().any(|(p, _)| p == "-r"));
        assert!(r7.iter().any(|(p, d)| p == "-f" && d.contains("force")));

        // 8. Chained &&, ||, ; and redirections >, >>, 2>&1
        let r8 =
            explain_command(&catalog, "cargo test && echo ok > out.txt 2>&1 || echo fail >> err.log; ls --not-real");
        assert!(r8.iter().any(|(p, d)| p == "&&" && d.contains("succeeded")));
        assert!(r8.iter().any(|(p, d)| p == ">" && d.contains("overwrites")));
        assert!(r8.iter().any(|(p, d)| p == "2>&1" && d.contains("stderr")));
        assert!(r8.iter().any(|(p, d)| p == "||" && d.contains("failed")));
        assert!(r8.iter().any(|(p, d)| p == ">>" && d.contains("appends")));
        assert!(r8.iter().any(|(p, d)| p == ";" && d.contains("unconditionally")));
        assert!(r8.iter().any(|(p, d)| p == "--not-real" && d == "not in the catalog"));

        // 9. Unknown command
        let r9 = explain_command(&catalog, "frobnicate --mystery foo");
        assert!(r9.iter().any(|(p, d)| p == "frobnicate" && d == "not in the catalog"));
        assert!(r9.iter().any(|(p, d)| p == "--mystery" && d == "not in the catalog"));

        // 10. find with multi-token choice & formatted slots
        let r10 = explain_command(&catalog, "find . -type f -name '*.rs' -mtime -7 -size +50M");
        assert!(r10.iter().any(|(p, _)| p == "find"));
        assert!(r10.iter().any(|(p, d)| p == "-type f" && d == "Only files"));
        assert!(r10.iter().any(|(p, d)| p.starts_with("-name") && d.contains("pattern")));
        assert!(r10.iter().any(|(p, d)| p.starts_with("-mtime") && d.contains("days")));
        assert!(r10.iter().any(|(p, d)| p == "-size +50M" && d.contains("50M")));

        // 11. cut -d, -f3 users.csv
        let r11 = explain_command(&catalog, "cut -d, -f3 users.csv");
        assert!(r11.iter().any(|(p, d)| p == "-d," && d.contains("Commas")));
        assert!(r11.iter().any(|(p, d)| p == "-f3" && d.contains("column")));
        assert!(r11.iter().any(|(p, d)| p == "users.csv" && d.contains("file")));

        // 12. chmod -R +x deploy.sh
        let r12 = explain_command(&catalog, "chmod -R +x deploy.sh");
        assert!(r12.iter().any(|(p, d)| p == "-R" && d.contains("Apply the change")));
        assert!(r12.iter().any(|(p, d)| p == "+x" && d.contains("runnable")));
        assert!(r12.iter().any(|(p, d)| p == "deploy.sh" && d.contains("permissions")));

        // 13. head with and without -n 20
        let r13a = explain_command(&catalog, "head notes.txt");
        assert!(r13a.iter().any(|(p, d)| p == "notes.txt" && d.contains("file to show")));
        let r13b = explain_command(&catalog, "head -n 20 notes.txt");
        assert!(r13b.iter().any(|(p, d)| p == "-n" && d.contains("lines")));
        assert!(r13b.iter().any(|(p, d)| p == "20" && d.contains("number of lines")));
        assert!(r13b.iter().any(|(p, d)| p == "notes.txt" && d.contains("file to show")));

        // 14. omarchy theme set + echo | sudo tee -a
        let r14a = explain_command(&catalog, r#"omarchy theme set "Tokyo Night""#);
        assert!(r14a.iter().any(|(p, d)| p == "omarchy theme set" && d.contains("theme")));
        assert!(r14a.iter().any(|(p, d)| p == "Tokyo Night" && d.contains("theme to apply")));
        let r14b = explain_command(&catalog, "echo hello | sudo tee -a /etc/hosts");
        assert!(r14b.iter().any(|(p, d)| p == "tee" && d.contains("configuration file")));
        assert!(r14b.iter().any(|(p, d)| p == "-a" && d.contains("append")));
        assert!(r14b.iter().any(|(p, d)| p == "/etc/hosts" && d.contains("system file")));
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

    #[test]
    fn explains_usermod_rsync_z_wildcards_and_piped_grep() {
        let catalog = load_test_catalog();

        let u = explain_command(&catalog, "sudo usermod -aG libvirt $USER");
        assert!(u.iter().any(|(p, d)| p == "libvirt" && d.contains("group")));
        assert!(u.iter().any(|(p, d)| p == "$USER" && d.contains("username")));

        let r = explain_command(&catalog, "rsync -avz ./dist/ user@host:/var/www/");
        assert!(r.iter().any(|(p, d)| p == "-z" && d.contains("compress")));

        let rm = explain_command(&catalog, "rm -rf folder");
        assert!(rm.iter().any(|(p, d)| p == "-r" && d.contains("Delete folders")));

        let glob = explain_command(&catalog, "du -sh * | sort -h");
        assert!(glob.iter().any(|(p, d)| p == "*" && d.contains("wildcard")));

        let piped = explain_command(&catalog, "ps aux | grep chrome");
        assert!(piped.iter().any(|(p, d)| p == "grep" && d.contains("piped output")));
    }
}
