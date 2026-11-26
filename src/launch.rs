//! `huh` (pane launcher) and `huh find`: the ONLY module of huh that starts a program.
//!
//! It starts exactly one thing at a time: `huh --here --tty <tty>` (huh's own interactive pane), or
//! the program that hosts it: `tmux`, or a terminal emulator (`xdg-terminal-exec`, `$TERMINAL`,
//! ghostty, kitty, ...); or `fzf`, given the list of names `huh find` built and asked for the
//! line picked (fzf's preview runs `huh find --describe`, which only reads). It never starts a
//! command that huh suggested, explained or was given on the command line, nor anything picked. A test in this file checks that no other source file (outside its
//! `#[cfg(test)]` code) uses `std::process::Command`.
//!
//! Deciding what to start (`decide` / `decide_for`) is a pure function of the environment, the
//! installed programs and the terminal; starting it (`start`) is the only part with side effects.

use crate::platform;
use std::path::{Path, PathBuf};

/// Name of the private tmux server `huh` creates when there is no tmux session (`tmux -L`).
/// It keeps the user's own tmux server, sessions and ~/.tmux.conf out of it.
pub const SOCKET: &str = "huh";
/// Window title and class (app-id) given to the pane window, for window rules.
pub const CLASS: &str = "huh";
/// Share of the width the `huh` pane gets.
const WIDTH: &str = "35%";
/// The status line of `huh`'s own tmux session.
const STATUS: &str = "huh \u{2014} Ctrl+B then arrow to switch, exit to close";
/// Environment variable set inside a launched `huh` pane to block nested launches.
pub const PANE_ENV: &str = "HUH_PANE";

/// What launching the `huh` pane will do.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    /// Inside tmux: split this pane. `argv` is the whole tmux command.
    Split(Vec<String>),
    /// Tiling desktop: open a terminal window. `argv` starts the terminal.
    Window { terminal: String, argv: Vec<String> },
    /// Neither: replace this shell with a new tmux session (shell left, `huh` right).
    Session(Vec<String>),
    /// Nothing available: tmux and a tiling desktop are both missing.
    Unavailable { tty: String },
    /// Not appropriate here (reason).
    Refuse(String),
}

/// The terminal emulators tried when `xdg-terminal-exec` and `$TERMINAL` are absent, in order.
const TERMINALS: [&str; 8] = ["ghostty", "kitty", "alacritty", "foot", "wezterm", "konsole", "gnome-terminal", "xterm"];

fn strings(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

fn shell_quote(text: &str) -> String {
    if !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+=:,%@#".contains(c)) {
        return text.to_string();
    }
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The pane command: `huh --here --tty <tty>` (plus `--quiet`, `--noise`, and `--mode`), told
/// where the recordings are (`--dir`) because a new window does not inherit this shell's
/// environment, and to keep its window open on an error (`--hold`) so the message can be read.
fn pane_command(
    huh: &Path,
    tty: &str,
    quiet: bool,
    noise: Option<&str>,
    dir: Option<&Path>,
    log: Option<&Path>,
    initial_mode: Option<&str>,
) -> Vec<String> {
    let mut cmd = vec![huh.display().to_string(), "--here".into(), "--tty".into(), tty.into()];
    // The launching terminal's own recording: the pane follows exactly this terminal.
    if let Some(log) = log {
        cmd.extend(["--log".into(), log.display().to_string()]);
    }
    if let Some(dir) = dir {
        cmd.extend(["--dir".into(), dir.display().to_string()]);
    }
    cmd.push("--hold".into());
    if let Some(level) = noise.filter(|n| !n.is_empty()) {
        cmd.extend(["--noise".into(), level.into()]);
    } else if quiet {
        cmd.push("--quiet".into());
    }
    if let Some(mode) = initial_mode.filter(|m| !m.is_empty()) {
        cmd.extend(["--mode".into(), mode.into()]);
    }
    cmd
}

/// Where this shell's recordings are: the folder of its own recording, else huh's default.
fn recordings_dir(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let own = env("HUH_SESSION_LOG").filter(|v| !v.is_empty())?;
    Path::new(&own).parent().map(Path::to_path_buf)
}

fn joined(cmd: &[String]) -> String {
    cmd.iter().map(|w| shell_quote(w)).collect::<Vec<_>>().join(" ")
}

/// The command line that opens `terminal` running `cmd`, with the "huh" title and
/// class where that terminal has such options.
pub fn window_argv(terminal: &str, cmd: &[String]) -> Vec<String> {
    let head: Vec<String> = match terminal {
        "xdg-terminal-exec" => strings(&["xdg-terminal-exec", "--app-id=huh", "--title=huh", "--"]),
        // GTK application ids need a dot, and Ghostty is GTK.
        "ghostty" => strings(&["ghostty", "--class=huh.pane", "--title=huh", "-e"]),
        "kitty" => strings(&["kitty", "--class", CLASS, "--title", CLASS]),
        "alacritty" => strings(&["alacritty", "--class", CLASS, "--title", CLASS, "-e"]),
        "foot" => strings(&["foot", "--app-id", CLASS, "--title", CLASS]),
        "wezterm" => strings(&["wezterm", "start", "--class", CLASS, "--"]),
        "konsole" => strings(&["konsole", "-e"]),
        "gnome-terminal" => strings(&["gnome-terminal", "--class=huh", "--title=huh", "--"]),
        "xterm" => strings(&["xterm", "-class", CLASS, "-T", CLASS, "-e"]),
        // `$TERMINAL`: the one flag nearly every terminal agrees on.
        other => vec![other.to_string(), "-e".into()],
    };
    head.into_iter().chain(cmd.iter().cloned()).collect()
}

/// tmux, from inside tmux: a pane on the right, a third wide, focus stays where it is.
fn split_argv(pane: Option<&str>, cmd: &[String]) -> Vec<String> {
    let mut argv = strings(&["tmux", "split-window", "-h", "-d", "-l", WIDTH]);
    if let Some(pane) = pane {
        argv.extend(["-t".into(), pane.into()]);
    }
    argv.push(joined(cmd));
    argv
}

/// tmux, new session on a private server with no user config: the user's shell on the left
/// (closing it ends the session), the `huh` pane on the right, following the left pane's tty.
fn session_argv(
    huh: &Path,
    quiet: bool,
    noise: Option<&str>,
    id: u32,
    dir: Option<&Path>,
    initial_mode: Option<&str>,
) -> Vec<String> {
    let name = format!("huh-{id}");
    let left = format!("{name}:0.0");
    let shell = format!("\"${{SHELL:-/bin/sh}}\"; tmux kill-session -t {name}");
    // `#` and `{` are joined at run time so no tmux version can expand the format early.
    let tty = format!("\"$(tmux display -p -t {left} \"$(printf '#%s' '{{pane_tty}}')\")\"");
    let mut right = format!("{PANE_ENV}=1 exec {} --here --tty {tty} --hold", shell_quote(&huh.display().to_string()));
    if let Some(dir) = dir {
        right.push_str(&format!(" --dir {}", shell_quote(&dir.display().to_string())));
    }
    if let Some(level) = noise.filter(|n| !n.is_empty()) {
        right.push_str(&format!(" --noise {}", shell_quote(level)));
    } else if quiet {
        right.push_str(" --quiet");
    }
    if let Some(mode) = initial_mode.filter(|m| !m.is_empty()) {
        right.push_str(&format!(" --mode {}", shell_quote(mode)));
    }
    let mut argv = strings(&["tmux", "-L", SOCKET, "-f", "/dev/null", "new-session", "-s", &name, &shell]);
    let mut then = |words: &[&str]| {
        argv.push(";".into());
        argv.extend(strings(words));
    };
    then(&["set-option", "-g", "mouse", "on"]);
    then(&["set-option", "-g", "allow-passthrough", "on"]);
    then(&["set-option", "-g", "status-left-length", "100"]);
    then(&["set-option", "-g", "status-left", STATUS]);
    then(&["set-option", "-g", "status-right", ""]);
    then(&["set-option", "-g", "status-justify", "left"]);
    then(&["split-window", "-h", "-d", "-l", WIDTH, "-t", &left, &right]);
    argv
}

/// Chooses what to start, with an optional initial pane mode (`watch` or `lab`) and watch noise
/// level (`all`, `new`, `errors`). Pure: `env` reads an environment variable, `have` says whether
/// a program is installed, `tty` is the terminal of standard input (None when it is not one),
/// `huh` the absolute path of this binary, `id` any number unique to this run.
#[allow(clippy::too_many_arguments)]
pub fn decide_for(
    env: &dyn Fn(&str) -> Option<String>,
    have: &dyn Fn(&str) -> bool,
    tty: Option<&str>,
    huh: &Path,
    quiet: bool,
    noise: Option<&str>,
    initial_mode: Option<&str>,
    id: u32,
) -> Plan {
    let set = |name: &str| env(name).is_some_and(|v| !v.is_empty());
    let Some(tty) = tty else {
        return Plan::Refuse("`huh` needs a terminal to sit beside; standard input is not one.".into());
    };
    let tmux = env("TMUX").filter(|v| !v.is_empty());
    let socket = tmux.as_deref().and_then(|t| Path::new(t.split(',').next().unwrap_or("")).file_name());
    if socket.is_some_and(|n| n == SOCKET) || set(PANE_ENV) {
        return Plan::Refuse("this is already a `huh` pane session: the pane is already next to this one.".into());
    }
    let dir = recordings_dir(env);
    let log = env("HUH_SESSION_LOG").filter(|v| !v.is_empty()).map(PathBuf::from);
    let cmd = pane_command(huh, tty, quiet, noise, dir.as_deref(), log.as_deref(), initial_mode);
    if tmux.is_some() {
        return Plan::Split(split_argv(env("TMUX_PANE").filter(|p| !p.is_empty()).as_deref(), &cmd));
    }
    if set("HYPRLAND_INSTANCE_SIGNATURE") || set("SWAYSOCK") || set("I3SOCK") {
        let wanted = env("TERMINAL").unwrap_or_default();
        let wanted = wanted.split_whitespace().next().unwrap_or("").to_string();
        let terminal = if have("xdg-terminal-exec") {
            Some("xdg-terminal-exec".to_string())
        } else if !wanted.is_empty() && have(&wanted) {
            Some(wanted)
        } else {
            TERMINALS.iter().find(|t| have(t)).map(|t| t.to_string())
        };
        if let Some(terminal) = terminal {
            let argv = window_argv(&terminal, &cmd);
            return Plan::Window { terminal, argv };
        }
    }
    if have("tmux") {
        return Plan::Session(session_argv(huh, quiet, noise, id, dir.as_deref(), initial_mode));
    }
    Plan::Unavailable { tty: tty.to_string() }
}

/// Backward-compatible alias for `decide_for(..., None, None, id)`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn decide(
    env: &dyn Fn(&str) -> Option<String>,
    have: &dyn Fn(&str) -> bool,
    tty: Option<&str>,
    huh: &Path,
    quiet: bool,
    id: u32,
) -> Plan {
    decide_for(env, have, tty, huh, quiet, None, None, id)
}

/// The text for `Plan::Unavailable`: how to get tmux, how to open a second window.
pub fn unavailable_message(tty: &str, platforms: &[String]) -> String {
    let install = match platform::install_advice("tmux", None, platforms) {
        Some(a) if a.installs => format!("Install tmux with `{}`, then run `huh` again.", a.command),
        _ => "Install tmux with your package manager, then run `huh` again.".to_string(),
    };
    format!(
        "huh found no tmux and no tiling desktop (Hyprland, sway, i3).\n\
         {install}\n\
         Or open a second terminal window next to this one and run `huh --here --tty {tty}` there."
    )
}

/// What a dry run prints for a plan.
pub fn describe(plan: &Plan, platforms: &[String]) -> String {
    match plan {
        Plan::Split(argv) => format!("inside tmux: would split this pane\n{}", joined(argv)),
        Plan::Window { terminal, argv } => {
            format!("tiling desktop: would open a {terminal} window, detached\n{}", joined(argv))
        }
        Plan::Session(argv) => format!("would replace this shell with a new tmux session\n{}", joined(argv)),
        Plan::Unavailable { tty } => unavailable_message(tty, platforms),
        Plan::Refuse(why) => why.clone(),
    }
}

/// The terminal of standard input, e.g. `/dev/pts/3`.
pub fn stdin_tty() -> Option<String> {
    unsafe extern "C" {
        fn ttyname(fd: i32) -> *const std::ffi::c_char;
    }
    // SAFETY: ttyname returns null or a NUL-terminated string that stays valid until the next call.
    let name = unsafe { ttyname(0) };
    if name.is_null() {
        return None;
    }
    // SAFETY: checked for null above.
    unsafe { std::ffi::CStr::from_ptr(name) }.to_str().ok().map(str::to_string)
}

/// Does what the plan says; an error carries the text to show when there is nothing to start.
pub fn start(plan: Plan, platforms: &[String]) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let build = |argv: &[String]| {
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]);
        command
    };
    match plan {
        Plan::Refuse(why) => anyhow::bail!("{why}"),
        Plan::Unavailable { tty } => anyhow::bail!("{}", unavailable_message(&tty, platforms)),
        Plan::Split(argv) => {
            let status = build(&argv).status()?;
            anyhow::ensure!(status.success(), "tmux could not split this pane ({status})");
        }
        Plan::Window { terminal, argv } => {
            // A process group of its own, no terminal streams: closing this shell leaves it alone.
            build(&argv).process_group(0).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
            println!("huh: opened a {terminal} window (title \"{CLASS}\") that follows this terminal.");
        }
        Plan::Session(argv) => {
            let err = build(&argv).exec();
            anyhow::bail!("could not start tmux: {err}");
        }
    }
    Ok(())
}

/// The `fzf` command line for `huh find`: the words as the starting query, a preview of the
/// details (`huh find --describe {1}`, the first field being the name) on the right, a header.
/// `color` says whether the preview is coloured (fzf then needs `--ansi`).
pub fn fzf_argv(query: &str, huh: &Path, color: bool) -> Vec<String> {
    let huh = shell_quote(&huh.display().to_string());
    let when = if color { "always" } else { "never" };
    let preview = format!("{huh} find --describe {{1}} --color {when}");
    // huh ranks, fzf only shows: fzf's own matching scatters a query over long descriptions and
    // ignores huh's order (catalog first, names before descriptions). Each keystroke asks huh again.
    let reload = format!("change:reload:{huh} find {{q}} --color never");
    let mut argv = strings(&["fzf", "--query", query, "--delimiter", " \u{2014} ", "--preview", &preview]);
    argv.extend(strings(&["--preview-window", "right,55%,wrap", "--header", "Enter: details   Esc: quit"]));
    argv.extend(strings(&["--disabled", "--bind", &reload, "--layout", "reverse", "--no-hscroll", "--no-sort"]));
    if color {
        argv.push("--ansi".into());
    }
    argv
}

/// Runs `fzf` (see `fzf_argv`) with `input` on its standard input and returns the line picked,
/// or `None` when nothing was picked (Esc, Ctrl+C, no match). fzf draws on the terminal itself.
pub fn run_fzf(argv: &[String], input: &str) -> anyhow::Result<Option<String>> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    unsafe extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }
    const SIGPIPE: i32 = 13;
    const SIG_DFL: usize = 0;
    const SIG_IGN: usize = 1;
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        // fzf may quit before it has read everything: that is an ignored error here, not a death by SIGPIPE.
        // SAFETY: only changes the disposition of SIGPIPE, which is restored below.
        unsafe { signal(SIGPIPE, SIG_IGN) };
        let _ = stdin.write_all(input.as_bytes());
        drop(stdin);
        // SAFETY: as above.
        unsafe { signal(SIGPIPE, SIG_DFL) };
    }
    let out = child.wait_with_output()?;
    match out.status.code() {
        Some(0) => Ok(Some(String::from_utf8_lossy(&out.stdout).trim_end().to_string()).filter(|l| !l.is_empty())),
        // 1: no match, 130: interrupted (Esc, Ctrl+C).
        Some(1) | Some(130) => Ok(None),
        _ => anyhow::bail!("fzf failed ({})", out.status),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn fzf_command_line() {
        let argv = fzf_argv("compress files", Path::new(HUH), true);
        assert_eq!(
            argv,
            strings(&[
                "fzf",
                "--query",
                "compress files",
                "--delimiter",
                " \u{2014} ",
                "--preview",
                "/opt/huh/bin/huh find --describe {1} --color always",
                "--preview-window",
                "right,55%,wrap",
                "--header",
                "Enter: details   Esc: quit",
                "--disabled",
                "--bind",
                "change:reload:/opt/huh/bin/huh find {q} --color never",
                "--layout",
                "reverse",
                "--no-hscroll",
                "--no-sort",
                "--ansi",
            ])
        );
        let plain = fzf_argv("", Path::new("/a b/huh"), false);
        assert!(!plain.contains(&"--ansi".to_string()));
        assert!(plain.contains(&"'/a b/huh' find --describe {1} --color never".to_string()), "{plain:?}");
        assert_eq!(plain[2], "", "no words, an empty query");
    }

    const HUH: &str = "/opt/huh/bin/huh";

    fn plan(env: &[(&str, &str)], installed: &[&str], tty: Option<&str>) -> Plan {
        let env: HashMap<String, String> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        decide(&|k| env.get(k).cloned(), &|p| installed.contains(&p), tty, Path::new(HUH), false, 7)
    }

    fn window(plan: Plan) -> (String, Vec<String>) {
        match plan {
            Plan::Window { terminal, argv } => (terminal, argv),
            other => panic!("expected a window, got {other:?}"),
        }
    }

    #[test]
    fn inside_tmux_splits_the_pane() {
        let env = [("TMUX", "/tmp/tmux-1000/default,1,0"), ("TMUX_PANE", "%3")];
        let p = plan(&env, &["tmux", "ghostty"], Some("/dev/pts/4"));
        assert_eq!(
            p,
            Plan::Split(strings(&[
                "tmux",
                "split-window",
                "-h",
                "-d",
                "-l",
                "35%",
                "-t",
                "%3",
                "/opt/huh/bin/huh --here --tty /dev/pts/4 --hold"
            ]))
        );
    }

    #[test]
    fn tmux_wins_over_a_tiling_desktop_and_quiet_is_passed_on() {
        let env: HashMap<&str, &str> = [("TMUX", "/s,1,0"), ("SWAYSOCK", "/run/sway")].into();
        let p = decide(&|k| env.get(k).map(|v| v.to_string()), &|_| true, Some("/dev/pts/1"), Path::new(HUH), true, 1);
        let Plan::Split(argv) = p else { panic!("{p:?}") };
        assert!(argv.last().unwrap().ends_with("--here --tty /dev/pts/1 --hold --quiet"));
        assert!(!argv.contains(&"-t".to_string()), "no TMUX_PANE, no target");
    }

    #[test]
    fn decide_for_passes_initial_mode_and_noise() {
        let split_env: HashMap<&str, &str> = [("TMUX", "/s,1,0")].into();
        let Plan::Split(argv) = decide_for(
            &|k| split_env.get(k).map(|v| v.to_string()),
            &|_| true,
            Some("/dev/pts/1"),
            Path::new(HUH),
            false,
            None,
            Some("lab"),
            1,
        ) else {
            panic!()
        };
        assert!(argv.last().unwrap().ends_with("--here --tty /dev/pts/1 --hold --mode lab"), "{argv:?}");

        let win_env: HashMap<&str, &str> = [("HYPRLAND_INSTANCE_SIGNATURE", "1")].into();
        let Plan::Window { argv, .. } = decide_for(
            &|k| win_env.get(k).map(|v| v.to_string()),
            &|p| p == "ghostty",
            Some("/dev/pts/2"),
            Path::new(HUH),
            false,
            Some("errors"),
            Some("watch"),
            1,
        ) else {
            panic!()
        };
        assert!(argv.ends_with(&strings(&["--hold", "--noise", "errors", "--mode", "watch"])), "{argv:?}");

        let Plan::Session(argv) =
            decide_for(&|_| None, &|p| p == "tmux", Some("/dev/pts/3"), Path::new(HUH), true, None, Some("lab"), 5)
        else {
            panic!()
        };
        assert!(argv.last().unwrap().ends_with("--hold --quiet --mode lab"), "{argv:?}");

        let Plan::Session(argv_noise) =
            decide_for(&|_| None, &|p| p == "tmux", Some("/dev/pts/3"), Path::new(HUH), false, Some("errors"), None, 6)
        else {
            panic!()
        };
        assert!(argv_noise.last().unwrap().ends_with("--hold --noise errors"), "{argv_noise:?}");
    }

    #[test]
    fn tiling_desktops_open_a_window() {
        for var in ["HYPRLAND_INSTANCE_SIGNATURE", "SWAYSOCK", "I3SOCK"] {
            let (terminal, _) = window(plan(&[(var, "x")], &["ghostty"], Some("/dev/pts/2")));
            assert_eq!(terminal, "ghostty", "{var}");
        }
    }

    #[test]
    fn terminal_choice_order() {
        let env = [("HYPRLAND_INSTANCE_SIGNATURE", "x"), ("TERMINAL", "st")];
        let all = ["ghostty", "kitty", "xdg-terminal-exec", "tmux"];
        assert_eq!(window(plan(&env, &all, Some("/dev/pts/2"))).0, "xdg-terminal-exec");
        let no_xdg = ["ghostty", "kitty", "st", "tmux"];
        let (terminal, argv) = window(plan(&env, &no_xdg, Some("/dev/pts/2")));
        assert_eq!(terminal, "st");
        assert_eq!(argv[..2], strings(&["st", "-e"]), "$TERMINAL gets -e");
        let env2 = [("HYPRLAND_INSTANCE_SIGNATURE", "x"), ("TERMINAL", "missing")];
        assert_eq!(window(plan(&env2, &no_xdg, Some("/dev/pts/2"))).0, "ghostty");
        assert_eq!(window(plan(&env2, &["xterm", "kitty"], Some("/dev/pts/2"))).0, "kitty");
        assert_eq!(window(plan(&env2, &["xterm"], Some("/dev/pts/2"))).0, "xterm");
    }

    #[test]
    fn argv_for_each_terminal() {
        let cmd = strings(&["/h/huh", "--here", "--tty", "/dev/pts/9"]);
        let t = |name: &str| window_argv(name, &cmd);
        let with = |head: &[&str]| head.iter().map(|s| s.to_string()).chain(cmd.clone()).collect::<Vec<_>>();
        assert_eq!(t("xdg-terminal-exec"), with(&["xdg-terminal-exec", "--app-id=huh", "--title=huh", "--"]));
        assert_eq!(t("ghostty"), with(&["ghostty", "--class=huh.pane", "--title=huh", "-e"]));
        assert_eq!(t("kitty"), with(&["kitty", "--class", "huh", "--title", "huh"]));
        assert_eq!(t("alacritty"), with(&["alacritty", "--class", "huh", "--title", "huh", "-e"]));
        assert_eq!(t("foot"), with(&["foot", "--app-id", "huh", "--title", "huh"]));
        assert_eq!(t("wezterm"), with(&["wezterm", "start", "--class", "huh", "--"]));
        assert_eq!(t("konsole"), with(&["konsole", "-e"]));
        assert_eq!(t("gnome-terminal"), with(&["gnome-terminal", "--class=huh", "--title=huh", "--"]));
        assert_eq!(t("xterm"), with(&["xterm", "-class", "huh", "-T", "huh", "-e"]));
        assert_eq!(t("st"), with(&["st", "-e"]));
    }

    #[test]
    fn elsewhere_with_tmux_makes_a_session() {
        let Plan::Session(argv) = plan(&[], &["tmux"], Some("/dev/pts/2")) else { panic!() };
        let text = argv.join(" ");
        assert_eq!(argv[..6], strings(&["tmux", "-L", "huh", "-f", "/dev/null", "new-session"]));
        assert!(text.contains("-s huh-7"));
        assert!(text.contains("tmux kill-session -t huh-7"), "closing the shell ends the session");
        assert!(text.contains("mouse on") && text.contains("Ctrl+B then arrow to switch, exit to close"));
        assert!(text.contains("allow-passthrough on"), "{text}");
        assert!(text.contains("split-window -h -d -l 35% -t huh-7:0.0"), "right pane, focus stays left");
        let right = argv.last().unwrap();
        assert!(right.contains("tmux display -p -t huh-7:0.0") && right.contains("pane_tty"), "{right}");
        assert!(right.starts_with("HUH_PANE=1 exec /opt/huh/bin/huh --here --tty "), "{right}");
        assert!(!right.contains("#{"), "the format is not written out whole");
        assert!(!text.contains("tmux.conf"));
    }

    #[test]
    fn nothing_available_says_how_to_install_tmux() {
        let p = plan(&[("HYPRLAND_INSTANCE_SIGNATURE", "x")], &[], Some("/dev/pts/5"));
        assert_eq!(p, Plan::Unavailable { tty: "/dev/pts/5".into() });
        let text = unavailable_message("/dev/pts/5", &["arch".to_string()]);
        assert!(text.contains("pacman -S") && text.contains("tmux"), "{text}");
        assert!(text.contains("huh --here --tty /dev/pts/5"), "{text}");
        let none = unavailable_message("/dev/pts/5", &[]);
        assert!(none.contains("package manager") && none.contains("--here --tty /dev/pts/5"));
    }

    #[test]
    fn a_tiling_desktop_without_a_terminal_falls_back_to_tmux() {
        assert!(matches!(plan(&[("I3SOCK", "x")], &["tmux"], Some("/dev/pts/5")), Plan::Session(_)));
    }

    #[test]
    fn refuses_without_a_terminal_and_inside_its_own_session() {
        assert!(matches!(plan(&[("TMUX", "/t/default,1,0")], &["tmux"], None), Plan::Refuse(_)));
        let p = plan(&[("TMUX", "/tmp/tmux-1000/huh,99,0")], &["tmux"], Some("/dev/pts/6"));
        assert!(matches!(p, Plan::Refuse(ref m) if m.contains("already")), "{p:?}");
        let nested = plan(&[(PANE_ENV, "1")], &["tmux", "ghostty"], Some("/dev/pts/6"));
        assert!(matches!(nested, Plan::Refuse(ref m) if m.contains("already")), "{nested:?}");
    }

    #[test]
    fn the_recording_folder_is_passed_to_the_new_window() {
        // A window opened by the desktop does not inherit this shell's HUH_SESSION_LOG.
        let env = [("HYPRLAND_INSTANCE_SIGNATURE", "x"), ("HUH_SESSION_LOG", "/run/user/1000/rec/42.log")];
        let Plan::Window { argv, .. } = plan(&env, &["ghostty"], Some("/dev/pts/2")) else { panic!() };
        let line = argv.join(" ");
        assert!(
            line.contains("--here --tty /dev/pts/2 --log /run/user/1000/rec/42.log --dir /run/user/1000/rec --hold"),
            "{line}"
        );
        let Plan::Session(argv) = plan(&[("HUH_SESSION_LOG", "/r/1.log")], &["tmux"], Some("/dev/pts/2")) else {
            panic!()
        };
        assert!(argv.iter().any(|a| a.contains("--hold --dir /r")), "{argv:?}");
    }

    #[test]
    fn dry_run_text_shows_the_command() {
        let p = plan(&[("HYPRLAND_INSTANCE_SIGNATURE", "x")], &["ghostty"], Some("/dev/pts/2"));
        assert_eq!(
            describe(&p, &[]),
            "tiling desktop: would open a ghostty window, detached\n\
             ghostty --class=huh.pane --title=huh -e /opt/huh/bin/huh --here --tty /dev/pts/2 --hold"
        );
        let q = plan(&[("TMUX", "/s,1,0")], &[], Some("/dev/pts/2"));
        assert!(
            describe(&q, &[]).starts_with("inside tmux: would split this pane\ntmux split-window -h -d -l 35% '/opt")
        );
    }

    #[test]
    fn quoting_keeps_odd_paths_in_one_word() {
        assert_eq!(joined(&strings(&["/a b/huh", "x'y", "ok"])), r#"'/a b/huh' 'x'\''y' ok"#);
    }

    /// huh starts programs in launch.rs only (session.rs uses Command in its tests, which
    /// start `bash` to syntax-check the snippets).
    #[test]
    fn only_launch_rs_starts_processes() {
        let needles = [
            concat!("process::", "Command"),
            concat!("process::", "{"),
            concat!("Command", "Ext"),
            concat!("Command", "::new"),
        ];
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut checked = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "rs") || path.file_name().is_some_and(|n| n == "launch.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let production = text.split(concat!("#[cfg(", "test)]")).next().unwrap();
            for needle in needles {
                assert!(!production.contains(needle), "{} must not contain `{needle}`", path.display());
            }
            checked += 1;
        }
        assert!(checked > 20);
    }
}
