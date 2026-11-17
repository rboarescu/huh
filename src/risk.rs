//! Risk analysis: what could go wrong if this command were run. Pure text analysis, nothing runs.

use crate::decode;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq)]
pub struct Warning {
    /// The exact part of the line the warning is about.
    pub part: String,
    /// The command it belongs to (shown when different from `part`).
    pub context: String,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct PosArg {
    pub text: String,
    pub vars: Vec<(String, bool)>,
    pub has_sub: bool,
    pub glob: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Inv {
    pub prog: String,
    /// The program and its subcommands (`git`, `reset`).
    pub path: Vec<String>,
    /// Every spelling of every option used (`-r` and `--recursive`).
    pub flags: Vec<String>,
    /// Option raw text as written, for naming the exact part.
    pub raw_flags: Vec<(String, String)>,
    pub values: Vec<(String, String)>,
    pub pos: Vec<PosArg>,
    pub raw: String,
    pub root: bool,
    pub remote: Option<String>,
    pub piped_in: bool,
    /// The file (or text) standard input is redirected from, when the line says so.
    pub stdin_from: Option<String>,
    pub inner_raw: Option<String>,
    pub inner_prog: Option<String>,
}

impl Inv {
    pub fn has(&self, names: &[&str]) -> bool {
        self.flags.iter().any(|f| names.contains(&f.as_str()))
    }

    /// The text as written for the first of `names` that was used (to name the exact part).
    fn part_for(&self, names: &[&str]) -> String {
        self.raw_flags
            .iter()
            .find(|(canon, _)| names.contains(&canon.as_str()))
            .map(|(_, raw)| raw.clone())
            .unwrap_or_else(|| names[0].to_string())
    }

    pub fn value(&self, names: &[&str]) -> Option<&str> {
        self.values.iter().find(|(f, _)| names.contains(&f.as_str())).map(|(_, v)| v.as_str())
    }

    /// The subcommand: `git`'s `reset`, or `npm`'s first word.
    fn sub(&self) -> Option<&str> {
        self.path.get(1).map(String::as_str).or_else(|| self.pos.first().map(|p| p.text.as_str()))
    }

    fn sub_is(&self, names: &[&str]) -> bool {
        self.sub().is_some_and(|s| names.contains(&s))
    }

    fn pos_texts(&self) -> Vec<&str> {
        let skip = usize::from(
            self.path.len() <= 1
                && !self.pos.is_empty()
                && self.sub().is_some()
                && NO_SUBSECTION_TOOLS.contains(&self.prog.as_str()),
        );
        self.pos.iter().skip(skip).map(|p| p.text.as_str()).collect()
    }
}

/// Programs whose first positional word is a subcommand that has no table section.
const NO_SUBSECTION_TOOLS: [&str; 9] = ["npm", "pnpm", "yarn", "apt", "apt-get", "dnf", "systemctl", "make", "pip3"];

const SYSTEM_PATHS: [&str; 16] = [
    "/", "/etc", "/usr", "/bin", "/sbin", "/lib", "/lib64", "/boot", "/var", "/home", "/root", "/opt", "/dev", "/sys",
    "/proc", "/srv",
];

const KNOWN_VARS: [&str; 14] = [
    "HOME",
    "PWD",
    "USER",
    "OLDPWD",
    "TMPDIR",
    "SHELL",
    "PATH",
    "LOGNAME",
    "HOSTNAME",
    "UID",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_RUNTIME_DIR",
];

pub const SHELLS: [&str; 6] = ["sh", "bash", "zsh", "dash", "ksh", "fish"];

pub const INTERPRETERS: [&str; 9] = ["sh", "bash", "zsh", "dash", "ksh", "python", "python3", "perl", "ruby"];

fn strip_slash(path: &str) -> &str {
    if path.len() > 1 { path.trim_end_matches('/') } else { path }
}

/// Is `path` a system folder, or everything under one (`/etc/*`)?
fn is_system_path(path: &str) -> bool {
    let p = path.trim_end_matches('*');
    let p = strip_slash(p);
    p == "~" || p == "$HOME" || p == "${HOME}" || SYSTEM_PATHS.contains(&p)
}

fn expand_empty(arg: &PosArg, assigned: &HashSet<String>) -> Option<(String, String)> {
    for (name, guarded) in &arg.vars {
        if *guarded
            || KNOWN_VARS.contains(&name.as_str())
            || assigned.contains(name)
            || name.chars().all(|c| c.is_ascii_digit()) && name != "0" && false
        {
            continue;
        }
        let expanded = arg.text.replace(&format!("${{{name}}}"), "").replace(&format!("${name}"), "");
        let dangerous = matches!(expanded.as_str(), "/" | "/*" | "*" | "~" | "~/" | "~/*" | ".." | "../*")
            || (expanded.starts_with('/')
                && !arg.text.starts_with('/')
                && SYSTEM_PATHS.contains(&strip_slash(expanded.trim_end_matches('*'))));
        if dangerous {
            return Some((name.clone(), expanded));
        }
    }
    None
}

/// Warns about the things `inv` could do.
pub fn check(inv: &Inv, assigned: &HashSet<String>, out: &mut Vec<Warning>) {
    let mut warn = |part: &str, text: String| {
        let w = Warning { part: part.to_string(), context: inv.raw.clone(), text };
        if !out.contains(&w) {
            out.push(w);
        }
    };
    let prog = inv.prog.as_str();
    let on = match &inv.remote {
        Some(host) => format!(" on {host}"),
        None => String::new(),
    };
    let recursive = inv.has(&["-r", "-R", "--recursive"]);
    secret_warnings(inv, &mut warn);

    match prog {
        "sudo" | "doas" | "su" => check_priv(inv, &on, &mut warn),
        "rm" => {
            let kind = if recursive { "deletes folders and everything inside them" } else { "deletes files" };
            let force = if inv.has(&["-f", "--force"]) { ", without asking and ignoring missing files" } else { "" };
            warn(prog, format!("{kind} permanently{force}{on}: there is no trash and no undo."));
            for arg in &inv.pos {
                if recursive && is_system_path(&arg.text) {
                    warn(
                        &arg.text,
                        format!("recursively deletes a system or home folder{on}: this would wreck the machine."),
                    );
                }
                if let Some((var, expanded)) = expand_empty(arg, assigned) {
                    warn(&arg.text, unset_text(&var, &expanded, "deletes"));
                }
                if arg.glob && arg.text == "*" && recursive {
                    warn(
                        &arg.text,
                        "the wildcard * matches everything in the current folder; all of it is deleted.".to_string(),
                    );
                }
            }
            if inv.has(&["--no-preserve-root"]) {
                warn(
                    &inv.part_for(&["--no-preserve-root"]),
                    "switches off the safety check that stops `rm -r /`.".to_string(),
                );
            }
        }
        "find" => {
            if inv.has(&["-delete"]) {
                warn("-delete", "deletes every match permanently, with no confirmation (no trash). Run the same command without -delete first to see what would go.".to_string());
            }
        }
        "xargs" => {
            if let Some(inner) = &inv.inner_prog
                && matches!(
                    inner.as_str(),
                    "rm" | "kill" | "mv" | "chmod" | "chown" | "shred" | "dd" | "sed" | "unlink" | "rmdir" | "truncate"
                )
            {
                let source = match (&inv.stdin_from, inv.piped_in) {
                    (Some(f), _) => format!("the lines of {f}"),
                    (None, true) => "whatever the earlier command prints".to_string(),
                    (None, false) => "whatever arrives on standard input".to_string(),
                };
                warn(
                    "xargs",
                    format!(
                        "feeds {source} straight into `{inner}`. Check that input first (put `echo` in front of `{inner}` to preview the commands)."
                    ),
                );
            }
        }
        "kill" | "killall" | "pkill" => {
            let killing = inv.raw_flags.iter().any(|(c, _)| c == "-KILL" || c == "-9")
                || inv.value(&["-s", "--signal"]).is_some_and(decode::is_kill_signal);
            if killing {
                let part = inv
                    .raw_flags
                    .iter()
                    .find(|(c, _)| c == "-KILL" || c == "-9")
                    .map(|(_, r)| r.clone())
                    .unwrap_or_else(|| prog.to_string());
                warn(&part, "signal 9 (KILL) stops the process immediately and it cannot clean up: unsaved data can be lost and lock or temp files left behind. Try plain `kill` (signal 15) first.".to_string());
            }
            if inv.pos.iter().any(|p| p.has_sub) || inv.piped_in {
                warn(prog, "the process IDs are worked out when the line runs, not written here; if the lookup matches more than you expect, unrelated programs are stopped.".to_string());
            }
            if prog != "kill" {
                warn(
                    prog,
                    "stops every process whose name matches, including ones you did not start on purpose.".to_string(),
                );
            }
            if inv.pos.iter().any(|p| p.text == "-1" || p.text == "1" || p.text == "0") && prog == "kill" {
                warn("kill", "a process ID of 1, 0 or -1 targets init or every process you may signal.".to_string());
            }
        }
        "git" => check_git(inv, &mut warn),
        "docker" | "docker-compose" => check_docker(inv, &mut warn),
        "dd" => {
            for arg in &inv.pos {
                if let Some(target) = arg.text.strip_prefix("of=") {
                    if target.starts_with("/dev/")
                        && !target.starts_with("/dev/null")
                        && !target.starts_with("/dev/std")
                    {
                        warn(
                            &arg.text,
                            format!(
                                "writes raw data straight onto the device {target}, destroying everything on it (files and partition table). Check the device name with `lsblk` first: the wrong letter wipes the wrong disk."
                            ),
                        );
                    } else if target != "/dev/null" {
                        warn(&arg.text, format!("overwrites {target} if it exists."));
                    }
                }
            }
        }
        "mkfs" | "mkswap" | "wipefs" | "shred" | "blkdiscard" | "sgdisk" | "fdisk" | "parted" | "cfdisk" => {
            if prog.starts_with("mk") || prog == "wipefs" || prog == "shred" || prog == "blkdiscard" {
                warn(
                    prog,
                    format!(
                        "destroys the existing data on {}.",
                        inv.pos_texts().first().copied().unwrap_or("its target")
                    ),
                );
            } else if !inv.has(&["-l", "--list"]) {
                warn(prog, "edits the partition table: a wrong change can make a disk unreadable.".to_string());
            }
        }
        _ if prog.starts_with("mkfs.") => {
            warn(
                prog,
                format!("destroys the existing data on {}.", inv.pos_texts().first().copied().unwrap_or("its target")),
            );
        }
        "truncate" => {
            warn(prog, "cuts or extends the file to the given size: data beyond the new size is lost.".to_string())
        }
        "tee" => {
            let append = inv.has(&["-a", "--append"]);
            for arg in &inv.pos {
                if arg.text == "/dev/null" || arg.text.starts_with("/dev/std") {
                    continue;
                }
                if let Some(text) =
                    persistence_text(&arg.text, if append { Write::Append } else { Write::Replace }, on.as_str())
                    && (append || !arg.text.starts_with("/etc/"))
                {
                    warn("tee", text);
                } else if !append {
                    warn(
                        "tee",
                        format!("replaces the current contents of {}{on} (use `tee -a` to append instead).", arg.text),
                    );
                }
            }
        }
        "sed" if inv.has(&["-i", "--in-place"]) => {
            let part = inv.part_for(&["-i", "--in-place"]);
            let suffix = part
                .strip_prefix("--in-place=")
                .or_else(|| part.strip_prefix("-i"))
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let mut text = match suffix {
                Some(s) => format!("edits the files themselves; the original is kept as a backup with the suffix `{s}`."),
                None => "edits the files themselves with no backup: a wrong script cannot be undone (add a suffix, like -i.bak, to keep a copy).".to_string(),
            };
            if inv.pos.iter().skip(1).any(|p| p.has_sub) {
                text.push_str(
                    " The list of files comes from a command substitution: check what it matches before running.",
                );
            }
            warn(&part, text);
            for arg in &inv.pos {
                if let Some(text) = persistence_text(&arg.text, Write::Edit, on.as_str()) {
                    warn("-i", text);
                }
            }
        }
        "ln" if inv.has(&["-f", "--force"]) && inv.pos.len() >= 2 => {
            let name = inv.pos.last().map(|p| p.text.as_str()).unwrap_or("");
            warn(
                &inv.part_for(&["-f", "--force"]),
                format!("replaces an existing file or link named `{name}` without asking."),
            );
        }
        "cp" | "mv"
            if !inv.has(&["-n", "--no-clobber", "-i", "--interactive", "-b", "--backup"]) && inv.pos.len() >= 2 =>
        {
            let dest = inv.pos.last().map(|p| p.text.as_str()).unwrap_or("");
            warn(prog, format!("if `{dest}` (or a file inside it) already exists, it is replaced without asking."));
            if let Some(text) = persistence_text(dest, Write::Replace, on.as_str()) {
                warn(prog, text);
            }
            if prog == "mv" && inv.pos.iter().any(|p| is_system_path(&p.text) && p.text != "/") {
                warn(prog, "moves a system folder, which can break the machine.".to_string());
            }
            for arg in &inv.pos {
                if let Some((var, expanded)) = expand_empty(arg, assigned) {
                    warn(&arg.text, unset_text(&var, &expanded, "moves"));
                }
            }
        }
        "chmod" | "chown" | "chgrp" => {
            if prog == "chmod"
                && let Some(mode) = inv.pos.first()
            {
                if decode::chmod_world_writable(&mode.text) {
                    warn(&mode.text, "lets every user on the machine modify these files; anything running on the machine, or any compromised account, can change or replace them. 755 (folders, programs) or 644 (plain files) is usually what is wanted.".to_string());
                }
                if mode.text.contains('s') && mode.text.contains('+')
                    || mode.text.len() == 4 && mode.text.starts_with(['4', '2', '6'])
                {
                    warn(&mode.text, "sets the setuid/setgid bit: the program then runs with its owner's rights, a classic privilege-escalation risk.".to_string());
                }
            }
            if recursive {
                let skip = 1;
                for arg in inv.pos.iter().skip(skip) {
                    if is_system_path(&arg.text) {
                        warn(
                            &inv.part_for(&["-R", "-r", "--recursive"]),
                            format!(
                                "changes {} on everything under `{}`{on}: on system folders this can break the machine.",
                                if prog == "chmod" { "permissions" } else { "ownership" },
                                arg.text
                            ),
                        );
                    }
                    if let Some((var, expanded)) = expand_empty(arg, assigned) {
                        warn(&arg.text, unset_text(&var, &expanded, "changes"));
                    }
                }
                if prog != "chmod" || inv.pos.len() > 1 {
                    // Recursive ownership or permission changes are hard to undo.
                    let target = inv.pos.last().map(|p| p.text.as_str()).unwrap_or("");
                    if !target.is_empty() && !is_system_path(target) {
                        warn(
                            &inv.part_for(&["-R", "-r", "--recursive"]),
                            format!(
                                "applies to everything under `{target}`, not just the folder itself; the previous {} are not recorded anywhere, so it is hard to undo.",
                                if prog == "chmod" { "permissions" } else { "owners" }
                            ),
                        );
                    }
                }
            }
        }
        "curl" | "wget" => {
            if inv.has(&["-k", "--insecure", "--no-check-certificate"]) {
                warn(
                    &inv.part_for(&["-k", "--insecure", "--no-check-certificate"]),
                    "turns off the check that the server is who it claims to be: the download can be tampered with."
                        .to_string(),
                );
            }
        }
        "eval" => warn("eval", "runs the given text as shell code; whatever it contains will be executed.".to_string()),
        "ssh" => {
            if inv.values.iter().any(|(f, v)| {
                f == "-o" && v.replace(' ', "").to_ascii_lowercase().starts_with("stricthostkeychecking=no")
            }) {
                warn(
                    "StrictHostKeyChecking=no",
                    "skips the check of the server's identity: you cannot tell if someone is impersonating it."
                        .to_string(),
                );
            }
        }
        "rsync" if inv.has(&["--delete"]) => {
            warn("--delete", "deletes files at the destination that do not exist in the source. Add -n (dry run) first to see what would go.".to_string());
        }
        "tar"
            if inv.has(&["-x", "--extract", "--get"]) && !inv.has(&["-k", "--keep-old-files", "--skip-old-files"]) =>
        {
            warn(
                &inv.part_for(&["-x", "--extract", "--get"]),
                format!("files in the archive replace files of the same name already in the target folder{on}."),
            );
        }
        "crontab" => {
            if inv.has(&["-r"]) {
                warn("-r", "deletes ALL your scheduled jobs without asking.".to_string());
            } else if !inv.has(&["-l", "-e"])
                && let Some(src) = inv.pos.first()
            {
                let from =
                    if src.text == "-" { "what is piped in".to_string() } else { format!("the file {}", src.text) };
                warn(
                    "crontab",
                    format!(
                        "replaces your WHOLE list of scheduled jobs{on} with {from}: any job that is not in it is lost, and the new jobs then run on their own schedule without you being there."
                    ),
                );
            }
        }
        "reboot" | "poweroff" | "halt" => warn(
            prog,
            format!(
                "{} the machine{on}: running programs are stopped and connections are lost.",
                if prog == "reboot" { "restarts" } else { "shuts down" }
            ),
        ),
        "shutdown" => warn(
            prog,
            format!("shuts down or restarts the machine{on}: running programs are stopped and connections are lost."),
        ),
        "systemctl" => {
            if let Some(sub) = inv.sub() {
                let units = inv.pos_texts().join(" ");
                match sub {
                    "stop" | "restart" | "kill" | "disable" | "mask" | "reload" | "try-restart" => {
                        warn(
                            sub,
                            format!(
                                "{} the service {units}{on}; anything using it is interrupted.",
                                match sub {
                                    "stop" | "kill" => "stops",
                                    "restart" | "try-restart" => "restarts",
                                    "reload" => "reloads",
                                    "disable" => "stops starting at boot",
                                    _ => "blocks from starting at all",
                                }
                            ),
                        );
                    }
                    "enable" => warn(
                        sub,
                        format!(
                            "makes {units}{on} start automatically at every boot{}; it keeps coming back until it is disabled.",
                            if inv.has(&["--now"]) { " and starts it right now" } else { "" }
                        ),
                    ),
                    "poweroff" | "reboot" | "halt" | "suspend" | "isolate" => {
                        warn(sub, format!("changes the power state of the machine{on}."))
                    }
                    _ => {}
                }
            }
        }
        "usermod" | "useradd" | "userdel" | "passwd" | "groupadd" | "gpasswd" => {
            warn(prog, "changes user accounts or their permissions on this machine.".to_string());
        }
        "ufw" | "iptables" | "nft" | "ip6tables" => check_firewall(inv, &mut warn),
        "pip" | "pip3" | "python" | "python3" | "uv" | "poetry" => check_python(inv, &mut warn),
        "perl" | "ruby" | "node" => check_inline_code(inv, &mut warn),
        "npm" | "pnpm" | "yarn" => check_node(inv, &mut warn),
        "make" => check_make(inv, &mut warn),
        "cmake" => {
            let installs = inv.has(&["--install"])
                || inv.values.iter().any(|(f, v)| f == "--target" && v == "install")
                || inv.pos.iter().any(|p| p.text == "install");
            if installs {
                warn(
                    if inv.has(&["--install"]) { "--install" } else { "--build" },
                    "also installs: copies the build results into the install folder (system folders such as /usr/local need root)."
                        .to_string(),
                );
            }
        }
        "cargo" => match inv.sub() {
            Some("install") => {
                warn("install", "downloads, compiles and installs a program into ~/.cargo/bin.".to_string())
            }
            Some("clean") => warn(
                "clean",
                "deletes the whole `target/` build folder; the next build starts from scratch.".to_string(),
            ),
            _ => {}
        },
        "kubectl" => check_kubectl(inv, &mut warn),
        "helm" => match inv.sub() {
            Some("install" | "upgrade") => warn(
                inv.sub().unwrap_or("install"),
                "changes what runs on the cluster: it creates or updates real resources.".to_string(),
            ),
            Some("uninstall" | "delete") => {
                warn("uninstall", "removes the release and the resources it created on the cluster.".to_string())
            }
            _ => {}
        },
        "terraform" => {
            if let Some(sub @ ("apply" | "destroy")) = inv.sub() {
                let what = if sub == "destroy" {
                    "DELETES real infrastructure"
                } else {
                    "creates, changes or deletes real infrastructure"
                };
                if inv.has(&["-auto-approve"]) {
                    warn(
                        "-auto-approve",
                        format!(
                            "{what} without asking you to confirm the plan first. Run `terraform plan` and read it before this."
                        ),
                    );
                } else {
                    warn(sub, format!("{what} (it shows the plan and asks before doing it)."));
                }
            }
        }
        "gh" if inv.path.get(1).is_some_and(|p| p == "pr") && inv.path.get(2).is_some_and(|p| p == "merge") => {
            if inv.has(&["--admin"]) {
                warn(
                    "--admin",
                    "merges even when required checks or reviews have not passed, bypassing the branch protection rules.".to_string(),
                );
            } else {
                warn("merge", "merges the pull request into its target branch on GitHub.".to_string());
            }
        }
        "psql" | "mysql" | "mariadb" | "sqlite3" => check_sql(inv, &mut warn),
        "dpkg" | "rpm" => check_low_level_pkg(inv, &mut warn),
        "apk" | "xbps-install" | "xbps-remove" | "emerge" | "nix-env" => {
            let changes = match prog {
                "apk" => inv.sub_is(&["add", "del", "upgrade"]),
                "emerge" => !inv.has(&["-p", "--pretend", "-s", "--search"]),
                _ => true,
            };
            if changes {
                warn(prog, "changes the software installed on the whole system.".to_string());
            }
        }
        "apt" | "apt-get" | "dnf" | "yum" | "zypper" => {
            if let Some(sub) = inv.sub()
                && matches!(
                    sub,
                    "install"
                        | "remove"
                        | "purge"
                        | "autoremove"
                        | "upgrade"
                        | "dist-upgrade"
                        | "full-upgrade"
                        | "erase"
                        | "in"
                        | "rm"
                )
            {
                warn(
                    sub,
                    format!(
                        "changes the software installed on {}the whole system.",
                        if inv.root { "" } else { "(needs root) " }
                    ),
                );
            }
        }
        "pacman" | "yay" | "paru" if inv.has(&["-S", "-R", "-U", "--sync", "--remove", "--upgrade"]) => {
            let upgrade = inv.has(&["-u", "--sysupgrade"]);
            let remove = inv.has(&["-R", "--remove"]);
            warn(
                &inv.part_for(&["-S", "-R", "-U", "--sync", "--remove", "--upgrade"]),
                if remove {
                    "removes packages from the whole system.".to_string()
                } else if upgrade {
                    "upgrades every installed package: a big change that can need a restart afterwards.".to_string()
                } else {
                    "installs packages system-wide.".to_string()
                },
            );
        }
        "tail" | "journalctl" if inv.has(&["-f", "-F", "--follow"]) => {
            warn(
                &inv.part_for(&["-f", "-F", "--follow"]),
                "keeps running and printing new lines until you press Ctrl+C.".to_string(),
            );
        }
        "ping" if !inv.has(&["-c", "-w"]) => {
            warn("ping", "keeps sending until you press Ctrl+C (add -c N to stop after N).".to_string())
        }
        "watch" => warn("watch", "re-runs the command every couple of seconds until you press Ctrl+C.".to_string()),
        "top" | "htop" | "btop" => warn(prog, "stays open and updates live until you press q.".to_string()),
        "yes" => warn("yes", "prints forever until interrupted.".to_string()),
        _ => {}
    }
    if prog == "umount" && inv.has(&["-f", "--force", "-l", "--lazy"]) {
        warn("umount", "forces a disk to be detached even if programs still use it.".to_string());
    }
}

fn check_kubectl(inv: &Inv, warn: &mut dyn FnMut(&str, String)) {
    let Some(sub) = inv.sub() else { return };
    let all = inv.has(&["--all", "-A", "--all-namespaces"]);
    match sub {
        "delete" | "drain" | "replace" => {
            let sweeping = if inv.has(&["--all"]) {
                " With --all it removes EVERY resource of that kind in the namespace."
            } else {
                ""
            };
            warn(sub, format!("removes cluster resources, which may take running applications down.{sweeping}"));
        }
        "apply" | "create" | "scale" | "rollout" => {
            let rollout_view = sub == "rollout" && inv.pos.get(1).is_some_and(|p| p.text == "status");
            if !rollout_view && !inv.has(&["--dry-run"]) {
                warn(sub, "changes the live cluster (the current kubectl context decides which one).".to_string());
            }
        }
        "exec" => warn(sub, "runs a command inside a running container of the cluster.".to_string()),
        "get" | "logs" | "describe" if all && sub == "logs" => {
            warn("--all", "reads logs from every matching pod, which can be a lot of output.".to_string());
        }
        _ => {}
    }
}

fn check_low_level_pkg(inv: &Inv, warn: &mut dyn FnMut(&str, String)) {
    let prog = inv.prog.as_str();
    let install = if prog == "dpkg" {
        inv.has(&["-i", "--install"])
    } else {
        inv.has(&["-i", "--install", "-U", "--upgrade", "-F", "--freshen"])
    };
    let remove =
        if prog == "dpkg" { inv.has(&["-r", "--remove", "-P", "--purge"]) } else { inv.has(&["-e", "--erase"]) };
    if install {
        warn(
            &inv.part_for(&["-i", "--install", "-U", "--upgrade", "-F", "--freshen"]),
            "installs a package as root and runs its maintainer scripts, which are arbitrary commands that execute on this machine before and after the files are copied.".to_string(),
        );
    }
    if remove {
        warn(
            &inv.part_for(&["-r", "--remove", "-P", "--purge", "-e", "--erase"]),
            "removes the package from the whole system (its removal scripts run as root).".to_string(),
        );
    }
}

fn check_firewall(inv: &Inv, warn: &mut dyn FnMut(&str, String)) {
    let prog = inv.prog.as_str();
    if prog == "ufw" {
        match inv.sub() {
            Some("enable") => {
                let forced = if inv.has(&["--force"]) {
                    " --force skips the confirmation prompt that would warn you about this."
                } else {
                    ""
                };
                warn(
                    "enable",
                    format!(
                        "turns the firewall on and keeps it on at every boot. Over SSH this can lock you out unless ssh is already allowed (`ufw allow ssh` first).{forced}"
                    ),
                );
            }
            Some("disable" | "reset") => warn(
                inv.sub().unwrap_or("disable"),
                "turns the firewall off or deletes its rules: services become reachable from the network.".to_string(),
            ),
            Some("allow" | "deny" | "delete" | "limit" | "reject") => warn(
                inv.sub().unwrap_or("allow"),
                "changes the firewall rules, which can lock you out or expose services.".to_string(),
            ),
            _ => {}
        }
        return;
    }
    if inv.has(&["-F", "--flush"]) {
        warn(
            &inv.part_for(&["-F", "--flush"]),
            "flushes (deletes) every firewall rule: the protection they gave is gone, and if the default policy is DROP all traffic, your ssh included, is cut off.".to_string(),
        );
    }
    if inv.has(&["-X", "--delete-chain"]) {
        warn(
            &inv.part_for(&["-X", "--delete-chain"]),
            "deletes user-made firewall chains and the rules in them.".to_string(),
        );
    }
    if inv.has(&["-P", "--policy"]) {
        let chain = inv.value(&["-P", "--policy"]).unwrap_or("a chain");
        let target = inv.pos.first().map(|p| p.text.as_str()).unwrap_or("");
        let text = match target {
            "ACCEPT" => format!(
                "sets the default for {chain} to ACCEPT: everything that no rule blocks is let through, which opens the firewall."
            ),
            "DROP" | "REJECT" => format!(
                "sets the default for {chain} to {target}: everything that no rule allows is blocked, which can cut off your ssh session."
            ),
            _ => {
                format!("changes the default policy of {chain}, which decides what happens to traffic no rule matches.")
            }
        };
        warn(&inv.part_for(&["-P", "--policy"]), text);
    }
    if !inv.has(&["-F", "--flush", "-X", "--delete-chain", "-P", "--policy"])
        && (inv.has(&["-A", "--append", "-I", "--insert", "-D", "--delete"])
            || inv.sub().is_some_and(|s| matches!(s, "flush" | "delete" | "add")))
    {
        warn(prog, "changes the firewall rules, which can lock you out or expose services.".to_string());
    }
}

/// SQL given on the command line that destroys or rewrites data.
fn check_sql(inv: &Inv, warn: &mut dyn FnMut(&str, String)) {
    let (flags, from_pos): (&[&str], bool) = match inv.prog.as_str() {
        "psql" => (&["-c", "--command"], false),
        "mysql" | "mariadb" => (&["-e", "--execute"], false),
        _ => (&["-cmd"], true),
    };
    let mut texts: Vec<(String, String)> = Vec::new();
    for (f, v) in &inv.values {
        if flags.contains(&f.as_str()) {
            texts.push((inv.part_for(flags), v.clone()));
        }
    }
    if from_pos {
        for p in inv.pos.iter().skip(1) {
            texts.push((p.text.clone(), p.text.clone()));
        }
    }
    for (part, sql) in texts {
        for stmt in sql.split(';') {
            let stmt = stmt.trim();
            let upper = stmt.to_ascii_uppercase();
            let mut words = upper.split_whitespace();
            let Some(first) = words.next() else { continue };
            let has_where = upper.split_whitespace().any(|w| w == "WHERE");
            let named = stmt.split_whitespace().take(4).collect::<Vec<_>>().join(" ");
            let text = match first {
                "DROP" => Some("permanently removes it and all the data in it".to_string()),
                "TRUNCATE" => Some("empties the table: every row is deleted at once".to_string()),
                "ALTER" => Some(
                    "changes the structure of a table or database, which can lose data or break programs using it"
                        .to_string(),
                ),
                "DELETE" if !has_where => Some("has no WHERE, so it deletes EVERY row of the table".to_string()),
                "UPDATE" if !has_where => Some("has no WHERE, so it changes EVERY row of the table".to_string()),
                _ => None,
            };
            if let Some(what) = text {
                warn(
                    &part,
                    format!(
                        "runs the SQL statement `{named}` straight away: this {what}. There is no confirmation and often no undo."
                    ),
                );
            }
        }
    }
}

/// Is `path` a shell startup file, a system settings file or a git hook? Returns a warning for
/// writing to it.
#[derive(Clone, Copy, PartialEq)]
enum Write {
    Append,
    Replace,
    Edit,
}

fn persistence_text(path: &str, how: Write, on: &str) -> Option<String> {
    let verb = match how {
        Write::Append => "adds to",
        Write::Replace => "replaces the current contents of",
        Write::Edit => "edits in place",
    };
    let p = path.trim_end_matches('/');
    let startup = [
        ".bashrc",
        ".zshrc",
        ".profile",
        ".bash_profile",
        ".bash_login",
        ".zprofile",
        ".zshenv",
        ".zlogin",
        ".config/fish/config.fish",
    ]
    .iter()
    .any(|f| p == *f || p.ends_with(&format!("/{f}")));
    if startup {
        return Some(format!(
            "{verb} the shell startup file {path}{on}: whatever is in it runs every time a new shell starts, which is a common way to make code or changes persist.{}",
            match how {
                Write::Append => " Existing settings in it are kept.",
                Write::Replace => " Existing settings in it are lost.",
                Write::Edit => "",
            }
        ));
    }
    if p.contains(".git/hooks/") || p.ends_with(".git/hooks") {
        return Some(format!(
            "writes into the git hooks folder ({path}): hook scripts run automatically on later git commands (commit, push, checkout, ...)."
        ));
    }
    if p.starts_with("/etc/") {
        return Some(format!(
            "{verb} the system file {path}{on}: it changes how the whole machine behaves{}.",
            if how == Write::Replace { " (what was there is lost)" } else { "" }
        ));
    }
    None
}

/// A secret passed as a plain value on the command line or in an assignment.
pub fn secret_assignment(name: &str, value: &str) -> Option<String> {
    let upper = name.to_ascii_uppercase();
    let secret_name = matches!(upper.as_str(), "PGPASSWORD" | "MYSQL_PWD" | "PASSWORD" | "TOKEN" | "SECRET")
        || [
            "_PASSWORD",
            "_PASSWD",
            "_TOKEN",
            "_SECRET",
            "_SECRET_KEY",
            "_SECRET_ACCESS_KEY",
            "_API_KEY",
            "_ACCESS_KEY",
        ]
        .iter()
        .any(|suffix| upper.ends_with(suffix));
    if !secret_name || value.is_empty() || value.starts_with('$') {
        return None;
    }
    Some(format!(
        "{name} is set to a secret value written right in the command: it is saved in your shell history and can show up in the process list. Read it from a file or a prompt instead."
    ))
}

pub const SECRET_OPTIONS: [&str; 9] = [
    "--password",
    "--passwd",
    "--pass",
    "--token",
    "--secret",
    "--api-key",
    "--apikey",
    "--auth-token",
    "--access-token",
];

const SECRET_PATTERNS: [&str; 9] = [
    "authorization: bearer ",
    "authorization: basic ",
    "token=",
    "password=",
    "passwd=",
    "secret=",
    "api_key=",
    "apikey=",
    "access_token=",
];

fn secret_warnings(inv: &Inv, warn: &mut dyn FnMut(&str, String)) {
    const TEXT: &str = "puts a password or token right on the command line: it is saved in your shell history and any user on the machine can read it in the process list while this runs. Leave the value out to be prompted, or read it from a file or an environment variable set elsewhere.";
    let mut seen: Vec<String> = Vec::new();
    let mut hit = |part: &str, warn: &mut dyn FnMut(&str, String)| {
        if !seen.iter().any(|s| s == part) {
            seen.push(part.to_string());
            warn(part, TEXT.to_string());
        }
    };
    let mysql_like = matches!(inv.prog.as_str(), "mysql" | "mariadb" | "mysqldump" | "mysqladmin");
    for (flag, value) in &inv.values {
        let lower = flag.to_ascii_lowercase();
        let secret_flag = SECRET_OPTIONS.contains(&lower.as_str())
            || (matches!(flag.as_str(), "-p" | "--password")
                && (mysql_like || matches!(inv.prog.as_str(), "sshpass" | "docker")))
            || (matches!(flag.as_str(), "-u" | "--user") && inv.prog == "curl" && value.contains(':'));
        if secret_flag && !value.is_empty() {
            let names = [flag.as_str()];
            hit(&inv.part_for(&names), warn);
        }
    }
    let texts = inv.values.iter().map(|(_, v)| v.as_str()).chain(inv.pos.iter().map(|p| p.text.as_str()));
    for text in texts {
        let low = text.to_ascii_lowercase();
        let found =
            SECRET_PATTERNS.iter().any(|pat| low.find(pat).is_some_and(|at| !low[at + pat.len()..].trim().is_empty()))
                || low.split("://").nth(1).is_some_and(|rest| {
                    let authority = rest.split('/').next().unwrap_or("");
                    authority.contains('@') && authority.split('@').next().is_some_and(|u| u.contains(':'))
                });
        if found {
            let shown: String = text.chars().take(40).collect();
            hit(&shown, warn);
        }
    }
}

fn check_priv(inv: &Inv, on: &str, warn: &mut dyn FnMut(&str, String)) {
    let prog = inv.prog.as_str();
    let su = prog == "su";
    let login_marker = inv.pos.iter().any(|p| p.text == "-");
    let named: Option<&str> =
        if su { inv.pos.iter().map(|p| p.text.as_str()).find(|t| *t != "-") } else { inv.value(&["-u", "--user"]) };
    let group = inv.value(&["-g", "--group"]);
    let root = named.is_none_or(|u| u == "root");
    let mut phrase = if root {
        "with administrator (root) rights".to_string()
    } else {
        format!("as the user {}", named.unwrap_or(""))
    };
    if let Some(g) = group {
        phrase.push_str(&format!(" and the group {g}"));
    }
    let reach = if root {
        format!("it can read, change or delete any file{on}")
    } else {
        format!("it can read, change or delete any file that user can{on}")
    };
    let command = if su { inv.value(&["-c", "--command"]).map(str::to_string) } else { inv.inner_raw.clone() };
    let text = match command {
        Some(c) => format!("runs `{c}`{on} {phrase}: {reach}."),
        None => {
            let login = login_marker || inv.has(&["-i", "--login", "-l"]) || (su && inv.has(&["-l", "--login"]));
            let shell_kind = if login {
                "a login shell (a fresh environment, as if that user had just logged in)"
            } else {
                "a shell"
            };
            if su || inv.has(&["-i", "--login", "-s", "--shell"]) {
                format!(
                    "opens {shell_kind}{on} {phrase}: every command typed into it, not just one, runs with those rights; {reach}."
                )
            } else {
                format!("runs a command{on} {phrase}: {reach}.")
            }
        }
    };
    warn(prog, text);
}

fn unset_text(var: &str, expanded: &str, verb: &str) -> String {
    format!(
        "if `${var}` is empty or unset, this becomes `{}`{}. Nothing on this line sets {var}. Use \"${{{var}:?}}\" so the shell stops instead of {verb} the wrong thing.",
        if expanded.is_empty() { "(nothing)" } else { expanded },
        if expanded.starts_with('/') { ", which is the top of the file system" } else { "" }
    )
}

fn check_git(inv: &Inv, warn: &mut dyn FnMut(&str, String)) {
    let Some(sub) = inv.path.get(1).map(String::as_str) else { return };
    let pos: Vec<&str> = inv.pos.iter().map(|p| p.text.as_str()).collect();
    match sub {
        "reset" if inv.has(&["--hard"]) => {
            warn("--hard", "throws away every uncommitted change in tracked files and moves the branch to the given commit. Work that was never committed cannot be recovered.".to_string());
            warn("reset", "rewrites where the branch points: commits after the target disappear from this branch (they stay reachable through `git reflog` for a while).".to_string());
        }
        "clean" if inv.has(&["-f", "--force"]) && !inv.has(&["-n", "--dry-run"]) => {
            let mut what = "deletes files git does not track".to_string();
            if inv.has(&["-d"]) {
                what.push_str(" and untracked folders");
            }
            if inv.has(&["-x"]) {
                what.push_str(", including ignored files such as .env, build output and local settings");
            }
            warn(
                "clean",
                format!("{what}: they are gone for good (not in trash, not in git). Use -n first to list them."),
            );
        }
        "push" => {
            if inv.has(&["-f", "--force"]) {
                warn(&inv.part_for(&["-f", "--force"]), "overwrites the remote branch with yours, discarding commits others may have pushed. Prefer --force-with-lease.".to_string());
            }
            if inv.has(&["--force-with-lease"]) {
                warn(
                    "--force-with-lease",
                    "rewrites the remote branch (but refuses if someone pushed since you last fetched).".to_string(),
                );
            }
            if let Some(spec) = pos.iter().skip(1).find(|p| p.starts_with('+')) {
                warn(
                    spec,
                    "a refspec starting with + is a force push: it overwrites the remote branch with yours, discarding commits others may have pushed. Prefer --force-with-lease.".to_string(),
                );
            }
            if inv.has(&["--delete", "-d"]) {
                warn(
                    &inv.part_for(&["--delete", "-d"]),
                    "deletes a branch or tag on the remote for everyone.".to_string(),
                );
            } else if let Some(spec) = pos.iter().find(|p| p.starts_with(':') && p.len() > 1) {
                warn(
                    spec,
                    format!(
                        "pushing an empty source to `{}` deletes that branch or tag on the remote for everyone.",
                        &spec[1..]
                    ),
                );
            }
            if inv.has(&["--mirror"]) {
                warn(
                    "--mirror",
                    "makes the remote an exact copy of this repository, deleting anything it has that you do not."
                        .to_string(),
                );
            }
        }
        "config" => {
            let key = pos.first().map(|k| k.to_ascii_lowercase()).unwrap_or_default();
            if key == "core.hookspath" && pos.len() > 1 && !inv.has(&["--unset"]) {
                warn(
                    "core.hooksPath",
                    "points git at another folder of hook scripts: the programs in it run automatically on later git commands (commit, push, checkout, ...), in this repository or every one if --global.".to_string(),
                );
            }
            if matches!(key.as_str(), "core.fsmonitor" | "core.sshcommand" | "core.pager" | "core.editor")
                && pos.len() > 1
            {
                warn(pos[0], "sets a program that git itself starts later during ordinary git commands.".to_string());
            }
        }
        "rebase" => {
            warn("rebase", "rewrites history: your commits are replaced by new copies on a new base. Never do this to commits others already have.".to_string());
        }
        "commit" if inv.has(&["--amend"]) => {
            warn("--amend", "replaces the previous commit instead of adding a new one (rewrites history; matters if it was already pushed).".to_string());
        }
        "checkout" | "restore" | "switch" => {
            let discards = pos.iter().any(|p| matches!(*p, "." | "*") || p.ends_with("/."))
                || (sub == "checkout" && inv.has(&["-f", "--force"]))
                || (sub == "switch" && inv.has(&["-f", "--force", "--discard-changes"]));
            if discards && !(sub == "restore" && inv.has(&["--staged", "-S"]) && !inv.has(&["--worktree", "-W"])) {
                warn(
                    sub,
                    "throws away uncommitted changes in the working folder; they cannot be recovered.".to_string(),
                );
            }
        }
        "branch" if inv.has(&["-D"]) => {
            warn("-D", "deletes the branch even if its commits were never merged anywhere.".to_string())
        }
        "stash" if pos.first().is_some_and(|p| matches!(*p, "drop" | "clear")) => {
            warn(pos[0], "permanently drops stashed changes.".to_string());
        }
        "filter-branch" | "filter-repo" => warn(sub, "rewrites the entire history of the repository.".to_string()),
        "reflog" if pos.first().is_some_and(|p| matches!(*p, "expire" | "delete")) => {
            warn(pos[0], "removes the safety net that lets you recover lost commits.".to_string());
        }
        "gc" if inv.value(&["--prune"]).is_some() => {
            warn("--prune", "permanently deletes unreferenced objects (lost commits).".to_string())
        }
        "rm" if !inv.has(&["--cached"]) => warn("rm", "deletes the files from disk and from git.".to_string()),
        "pull" if inv.has(&["--rebase"]) => {
            warn("--rebase", "rewrites your local commits on top of what was downloaded.".to_string())
        }
        "clone" | "submodule" => {}
        _ => {}
    }
}

fn check_docker(inv: &Inv, warn: &mut dyn FnMut(&str, String)) {
    let path: Vec<&str> = inv.path.iter().map(String::as_str).collect();
    let last = path.last().copied().unwrap_or("");
    let pos: Vec<&str> = inv.pos.iter().map(|p| p.text.as_str()).collect();
    if matches!(last, "rm" | "rmi") || (path.len() > 2 && matches!(path.get(2), Some(&"rm"))) {
        let what = if last == "rmi" || path.get(1) == Some(&"image") {
            "images"
        } else if path.get(1) == Some(&"volume") {
            "volumes (and the data in them)"
        } else {
            "containers"
        };
        warn(
            last,
            format!(
                "deletes the {what} named here{}.",
                if inv.has(&["-f", "--force"]) { ", forcing removal even of running ones" } else { "" }
            ),
        );
    }
    if path.contains(&"prune") {
        let extra = if inv.has(&["-a", "--all"]) { " every unused image," } else { "" };
        let vol = if inv.has(&["--volumes"]) || path.get(1) == Some(&"volume") {
            " and unused volumes (their data is lost)"
        } else {
            ""
        };
        warn("prune", format!("deletes all unused docker data:{extra} stopped containers, unused networks{vol}."));
    }
    if path.get(1) == Some(&"compose") && last == "down" && inv.has(&["-v", "--volumes"]) {
        warn(
            &inv.part_for(&["-v", "--volumes"]),
            "also deletes the named volumes: the data stored in them is lost.".to_string(),
        );
    }
    if path.get(1) == Some(&"compose") && last == "down" {
        warn("down", "stops and removes the containers and networks of this project.".to_string());
    }
    if last == "run" || path.get(1) == Some(&"container") && path.get(2) == Some(&"run") {
        if inv.has(&["--privileged"]) {
            warn(
                "--privileged",
                "gives the container nearly full access to the host: it can act as root on this machine.".to_string(),
            );
        }
        let mut seen_values: Vec<&String> = Vec::new();
        for (flag, v) in &inv.values {
            if seen_values.contains(&v) {
                continue;
            }
            seen_values.push(v);
            if matches!(flag.as_str(), "-v" | "--volume") {
                let src = v.split(':').next().unwrap_or("");
                if src == "/"
                    || src == "/var/run/docker.sock"
                    || src == "/run/docker.sock"
                    || src == "/etc"
                    || src == "/root"
                {
                    warn(
                        &format!("{flag} {v}"),
                        format!(
                            "shares {src} with the container, which gives it the power to read or change the host (the docker socket is the same as root access)."
                        ),
                    );
                }
            }
            if matches!(flag.as_str(), "-p" | "--publish")
                && !v.starts_with("127.0.0.1")
                && !v.starts_with("[::1]")
                && v.contains(':')
            {
                warn(
                    &format!("{flag} {v}"),
                    format!(
                        "publishes the port on every network interface, so other computers can reach it, not just this one. Use 127.0.0.1:{v} to keep it local."
                    ),
                );
            }
        }
        if inv.has(&["-d", "--detach"]) {
            warn(
                "-d",
                "starts a container that keeps running in the background until you stop it (`docker stop`)."
                    .to_string(),
            );
        } else {
            warn("run", "starts a container, which keeps running until the program inside it ends.".to_string());
        }
    }
    if last == "build" {
        warn("build", "starts building an image (can take a while and downloads data).".to_string());
    }
    if path.get(1) == Some(&"compose") && last == "up" && !inv.has(&["-d", "--detach"]) {
        warn("up", "starts the services and stays attached to their output until you press Ctrl+C.".to_string());
    }
    if path.get(1) == Some(&"compose") && last == "up" && inv.has(&["--build"]) {
        warn("--build", "builds the images first (can take a while).".to_string());
    }
    if matches!(last, "logs") && inv.has(&["-f", "--follow"]) {
        warn("-f", "keeps running and printing new lines until you press Ctrl+C.".to_string());
    }
    if matches!(last, "stop" | "kill" | "restart") && !pos.is_empty() {
        warn(last, format!("interrupts the container(s) {}.", pos.join(", ")));
    }
}

fn check_python(inv: &Inv, warn: &mut dyn FnMut(&str, String)) {
    check_inline_code(inv, warn);
    let prog = inv.prog.as_str();
    let pip_like = matches!(prog, "pip" | "pip3")
        || (inv.has(&["-m"]) && inv.value(&["-m"]) == Some("pip"))
        || (prog == "uv" && inv.pos.first().is_some_and(|p| p.text == "pip"));
    let tool_install = (prog == "uv" && inv.sub_is(&["add", "sync"]))
        || (prog == "poetry" && inv.sub_is(&["install", "add", "update"]));
    if !pip_like && !tool_install {
        return;
    }
    let install = tool_install || inv.sub_is(&["install"]) || inv.pos.iter().any(|p| p.text == "install");
    if inv.has(&["--break-system-packages"]) {
        warn("--break-system-packages", "tells pip to ignore your operating system's protection and install into the system-wide Python. That can break system tools that depend on specific versions. A virtualenv (`python -m venv`) or `pipx` is the safe way.".to_string());
    }
    if install {
        warn("install", "installs packages (can run their setup code on this machine).".to_string());
    }
    if inv.sub_is(&["uninstall"]) || inv.pos.iter().any(|p| p.text == "uninstall") {
        warn("uninstall", "removes the packages.".to_string());
    }
}

/// Code given on the command line (`python3 -c`, `perl -e`, `node -e`) that shells out.
fn check_inline_code(inv: &Inv, warn: &mut dyn FnMut(&str, String)) {
    let flags: &[&str] = match inv.prog.as_str() {
        "python" | "python3" => &["-c"],
        "perl" | "ruby" => &["-e", "-E"],
        "node" => &["-e", "--eval", "-p", "--print"],
        _ => return,
    };
    let Some(code) = inv.value(flags) else { return };
    let low = code.to_ascii_lowercase();
    let mut needles = vec![
        "os.system",
        "subprocess",
        "os.popen",
        "popen(",
        "pty.spawn",
        "child_process",
        "execsync",
        "spawnsync",
        "system(",
        "exec(",
        "qx{",
        "qx(",
        "%x(",
    ];
    if matches!(inv.prog.as_str(), "perl" | "ruby") {
        needles.push("`");
        needles.push("system ");
    }
    if let Some(hit) = needles.iter().find(|n| low.contains(**n)) {
        warn(
            &inv.part_for(flags),
            format!(
                "the code given can run shell commands (it uses `{}`), and huh does not look inside it: read it before running.",
                hit.trim()
            ),
        );
    }
}

fn check_node(inv: &Inv, warn: &mut dyn FnMut(&str, String)) {
    let sub = inv.sub().unwrap_or("");
    let is_install = matches!(sub, "install" | "i" | "add" | "ci" | "update" | "up" | "upgrade");
    let global = inv.has(&["-g", "--global"]);
    if global {
        warn(&inv.part_for(&["-g", "--global"]), "installs or removes packages system-wide (shared by every project, often needs root) instead of in this project.".to_string());
    }
    if is_install || (inv.prog == "yarn" && inv.pos.is_empty()) {
        warn(sub, "installs packages (can run their install scripts on this machine).".to_string());
    } else if matches!(sub, "run" | "start" | "test" | "build" | "dev" | "exec" | "dlx" | "x") {
        warn(sub, "runs a script that package.json defines (huh does not read that file).".to_string());
    }
    if matches!(sub, "uninstall" | "remove" | "rm") {
        warn(sub, "removes packages.".to_string());
    }
    if sub == "publish" {
        warn(sub, "uploads this package to a public registry.".to_string());
    }
}

fn check_make(inv: &Inv, warn: &mut dyn FnMut(&str, String)) {
    if inv.has(&["-n", "--just-print", "--dry-run", "-q", "-p"]) {
        return;
    }
    let targets: Vec<&str> = inv.pos.iter().map(|p| p.text.as_str()).filter(|t| !t.contains('=')).collect();
    if targets.iter().any(|t| *t == "install" || t.starts_with("install-") || *t == "uninstall") {
        let t = targets.iter().find(|t| t.contains("install")).copied().unwrap_or("install");
        warn(
            t,
            format!(
                "runs the Makefile's `{t}` recipe, which normally copies files into system folders such as /usr/local{}. Huh does not read the Makefile, so it cannot tell you exactly what it does.",
                if inv.root { " (as root here)" } else { " (usually needs sudo)" }
            ),
        );
    } else if targets.iter().any(|t| matches!(*t, "clean" | "distclean" | "mrproper")) {
        let t = targets.iter().find(|t| matches!(**t, "clean" | "distclean" | "mrproper")).copied().unwrap_or("clean");
        warn(
            t,
            "runs the Makefile's clean recipe, which deletes build output (the Makefile decides exactly what)."
                .to_string(),
        );
    }
}

/// Warns about a redirection (`>`, `>>`, `&>`, ...).
pub fn check_redir(op: &str, fd: Option<&str>, target: &str, raw: &str, context: &str, out: &mut Vec<Warning>) {
    let overwrite = matches!(op, ">" | ">|" | "&>" | "<>") && !target.starts_with('&');
    let appends = matches!(op, ">>" | "&>>");
    if !(overwrite || appends) || target.is_empty() {
        return;
    }
    if matches!(target, "/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/tty") {
        return;
    }
    let mut text = None;
    if let Some(p) = persistence_text(target, if appends { Write::Append } else { Write::Replace }, "") {
        text = Some(p);
    } else if target.starts_with("/dev/") {
        text = Some(format!("writes raw data straight onto the device {target}, which destroys what is stored on it."));
    } else if overwrite {
        let system =
            ["/etc/", "/boot/", "/usr/", "/bin/", "/sbin/", "/lib/", "/var/"].iter().any(|p| target.starts_with(p));
        let dotfile = target.contains("/.")
            || target.starts_with("~/.") && !target.contains(".config/") && !target.contains(".local/");
        let which = if fd == Some("2") { "error output" } else { "output" };
        text = Some(if system {
            format!("replaces the current contents of the system file {target} (if it exists) with the {which}.")
        } else if dotfile {
            format!(
                "replaces the current contents of the settings file {target} (if it exists): existing settings are lost."
            )
        } else {
            format!("replaces the current contents of {target} if it already exists (use >> to append instead).")
        });
    } else if ["/etc/", "/boot/", "/usr/", "/bin/", "/sbin/"].iter().any(|p| target.starts_with(p)) {
        text = Some(format!("adds to the system file {target}."));
    }
    if let Some(text) = text {
        let w = Warning { part: raw.to_string(), context: context.to_string(), text };
        if !out.contains(&w) {
            out.push(w);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arg(text: &str) -> PosArg {
        PosArg { text: text.to_string(), ..PosArg::default() }
    }

    #[test]
    fn rm_rf_system_path() {
        let inv = Inv {
            prog: "rm".into(),
            path: vec!["rm".into()],
            flags: vec!["-r".into(), "-f".into()],
            pos: vec![arg("/etc")],
            raw: "rm -rf /etc".into(),
            ..Inv::default()
        };
        let mut out = Vec::new();
        check(&inv, &HashSet::new(), &mut out);
        assert!(out.iter().any(|w| w.part == "/etc" && w.text.contains("wreck")));
        assert!(out.iter().any(|w| w.part == "rm"));
    }

    #[test]
    fn unset_variable_in_rm() {
        let mut a = arg("$DIR/*");
        a.vars = vec![("DIR".into(), false)];
        a.glob = true;
        let inv = Inv {
            prog: "rm".into(),
            path: vec!["rm".into()],
            flags: vec!["-r".into()],
            pos: vec![a.clone()],
            raw: "rm -rf $DIR/*".into(),
            ..Inv::default()
        };
        let mut out = Vec::new();
        check(&inv, &HashSet::new(), &mut out);
        assert!(out.iter().any(|w| w.text.contains("empty or unset") && w.text.contains("`/*`")), "{out:?}");
        let mut out = Vec::new();
        check(&inv, &HashSet::from(["DIR".to_string()]), &mut out);
        assert!(!out.iter().any(|w| w.text.contains("empty or unset")));
        a.vars = vec![("HOME".into(), false)];
        a.text = "$HOME/*".into();
        let inv = Inv { pos: vec![a], ..inv };
        let mut out = Vec::new();
        check(&inv, &HashSet::new(), &mut out);
        assert!(!out.iter().any(|w| w.text.contains("empty or unset")));
    }

    #[test]
    fn redirects() {
        let mut out = Vec::new();
        check_redir(">", None, "notes.txt", "> notes.txt", "echo hi > notes.txt", &mut out);
        check_redir(">", None, "/dev/null", "> /dev/null", "x", &mut out);
        check_redir(">>", None, "log.txt", ">> log.txt", "x", &mut out);
        check_redir(">", None, "/etc/hosts", "> /etc/hosts", "x", &mut out);
        assert_eq!(out.len(), 2);
        assert!(out[1].text.contains("system file"));
    }
}
