//! `huh tutor --side`: the ONLY module of huh that starts a program.
//!
//! It starts exactly one thing: `huh tutor --tty <tty>` (huh's own read-only viewer), or the
//! program that hosts it: `tmux`, or a terminal emulator (`xdg-terminal-exec`, `$TERMINAL`,
//! ghostty, kitty, ...). It never starts a command that huh suggested, explained or was given
//! on the command line. A test in this file checks that no other source file (outside its
//! `#[cfg(test)]` code) uses `std::process::Command`.
//!
//! Deciding what to start (`decide`) is a pure function of the environment, the installed
//! programs and the terminal; starting it (`start`) is the only part with side effects.

use crate::platform;
use std::path::{Path, PathBuf};

/// Name of the private tmux server `--side` creates when there is no tmux session (`tmux -L`).
/// It keeps the user's own tmux server, sessions and ~/.tmux.conf out of it.
pub const SOCKET: &str = "huh-tutor";
/// Window title and class (app-id) given to the tutor window, for window rules.
pub const CLASS: &str = "huh-tutor";
/// Share of the width the tutor pane gets.
const WIDTH: &str = "35%";
/// The status line of the tutor's own tmux session.
const STATUS: &str = "huh tutor \u{2014} Ctrl+B then arrow to switch, exit to close";

/// What `--side` will do.
#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    /// Inside tmux: split this pane. `argv` is the whole tmux command.
    Split(Vec<String>),
    /// Tiling desktop: open a terminal window. `argv` starts the terminal.
    Window { terminal: String, argv: Vec<String> },
    /// Neither: replace huh with a new tmux session (shell left, tutor right).
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

/// The tutor command: `huh tutor --tty <tty>` (plus `--quiet`), told where the recordings are
/// (`--dir`) because a new window does not inherit this shell's environment, and to keep its
/// window open on an error (`--hold`) so the message can be read.
fn tutor_command(huh: &Path, tty: &str, quiet: bool, dir: Option<&Path>) -> Vec<String> {
    let mut cmd = vec![huh.display().to_string(), "tutor".into(), "--tty".into(), tty.into()];
    if let Some(dir) = dir {
        cmd.extend(["--dir".into(), dir.display().to_string()]);
    }
    cmd.push("--hold".into());
    if quiet {
        cmd.push("--quiet".into());
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

/// The command line that opens `terminal` running `cmd`, with the "huh-tutor" title and
/// class where that terminal has such options.
pub fn window_argv(terminal: &str, cmd: &[String]) -> Vec<String> {
    let head: Vec<String> = match terminal {
        "xdg-terminal-exec" => strings(&["xdg-terminal-exec", "--app-id=huh-tutor", "--title=huh-tutor", "--"]),
        // GTK application ids need a dot, and Ghostty is GTK.
        "ghostty" => strings(&["ghostty", "--class=huh.tutor", "--title=huh-tutor", "-e"]),
        "kitty" => strings(&["kitty", "--class", CLASS, "--title", CLASS]),
        "alacritty" => strings(&["alacritty", "--class", CLASS, "--title", CLASS, "-e"]),
        "foot" => strings(&["foot", "--app-id", CLASS, "--title", CLASS]),
        "wezterm" => strings(&["wezterm", "start", "--class", CLASS, "--"]),
        "konsole" => strings(&["konsole", "-e"]),
        "gnome-terminal" => strings(&["gnome-terminal", "--class=huh-tutor", "--title=huh-tutor", "--"]),
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
/// (closing it ends the session), the tutor on the right, following the left pane's tty.
fn session_argv(huh: &Path, quiet: bool, id: u32, dir: Option<&Path>) -> Vec<String> {
    let name = format!("huh-{id}");
    let left = format!("{name}:0.0");
    let shell = format!("\"${{SHELL:-/bin/sh}}\"; tmux kill-session -t {name}");
    // `#` and `{` are joined at run time so no tmux version can expand the format early.
    let tty = format!("\"$(tmux display -p -t {left} \"$(printf '#%s' '{{pane_tty}}')\")\"");
    let mut right = format!("exec {} tutor --tty {tty} --hold", shell_quote(&huh.display().to_string()));
    if let Some(dir) = dir {
        right.push_str(&format!(" --dir {}", shell_quote(&dir.display().to_string())));
    }
    if quiet {
        right.push_str(" --quiet");
    }
    let mut argv = strings(&["tmux", "-L", SOCKET, "-f", "/dev/null", "new-session", "-s", &name, &shell]);
    let mut then = |words: &[&str]| {
        argv.push(";".into());
        argv.extend(strings(words));
    };
    then(&["set-option", "-g", "mouse", "on"]);
    then(&["set-option", "-g", "status-left-length", "100"]);
    then(&["set-option", "-g", "status-left", STATUS]);
    then(&["set-option", "-g", "status-right", ""]);
    then(&["set-option", "-g", "status-justify", "left"]);
    then(&["split-window", "-h", "-d", "-l", WIDTH, "-t", &left, &right]);
    argv
}

/// Chooses what to start. Pure: `env` reads an environment variable, `have` says whether a
/// program is installed, `tty` is the terminal of standard input (None when it is not one),
/// `huh` the absolute path of this binary, `id` any number unique to this run.
pub fn decide(
    env: &dyn Fn(&str) -> Option<String>,
    have: &dyn Fn(&str) -> bool,
    tty: Option<&str>,
    huh: &Path,
    quiet: bool,
    id: u32,
) -> Plan {
    let set = |name: &str| env(name).is_some_and(|v| !v.is_empty());
    let Some(tty) = tty else {
        return Plan::Refuse("`huh tutor --side` needs a terminal to sit beside; standard input is not one.".into());
    };
    let tmux = env("TMUX").filter(|v| !v.is_empty());
    let socket = tmux.as_deref().and_then(|t| Path::new(t.split(',').next().unwrap_or("")).file_name());
    if socket.is_some_and(|n| n == SOCKET) {
        return Plan::Refuse(
            "this is already a `huh tutor --side` session: the tutor is the pane next to this one.".into(),
        );
    }
    let dir = recordings_dir(env);
    let cmd = tutor_command(huh, tty, quiet, dir.as_deref());
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
        return Plan::Session(session_argv(huh, quiet, id, dir.as_deref()));
    }
    Plan::Unavailable { tty: tty.to_string() }
}

/// The text for `Plan::Unavailable`: how to get tmux, how to open a second window.
pub fn unavailable_message(tty: &str, platforms: &[String]) -> String {
    let install = match platform::install_advice("tmux", None, platforms) {
        Some(a) if a.installs => format!("Install tmux with `{}`, then run `huh tutor --side` again.", a.command),
        _ => "Install tmux with your package manager, then run `huh tutor --side` again.".to_string(),
    };
    format!(
        "huh tutor --side found no tmux and no tiling desktop (Hyprland, sway, i3).\n\
         {install}\n\
         Or open a second terminal window next to this one and run `huh tutor --tty {tty}` there."
    )
}

/// What a dry run prints for a plan.
pub fn describe(plan: &Plan, platforms: &[String]) -> String {
    match plan {
        Plan::Split(argv) => format!("inside tmux: would split this pane\n{}", joined(argv)),
        Plan::Window { terminal, argv } => {
            format!("tiling desktop: would open a {terminal} window, detached\n{}", joined(argv))
        }
        Plan::Session(argv) => format!("would replace huh with a new tmux session\n{}", joined(argv)),
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
            println!("huh tutor: opened a {terminal} window (class and title \"{CLASS}\") that follows this terminal.");
        }
        Plan::Session(argv) => {
            let err = build(&argv).exec();
            anyhow::bail!("could not start tmux: {err}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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
                "/opt/huh/bin/huh tutor --tty /dev/pts/4 --hold"
            ]))
        );
    }

    #[test]
    fn tmux_wins_over_a_tiling_desktop_and_quiet_is_passed_on() {
        let env: HashMap<&str, &str> = [("TMUX", "/s,1,0"), ("SWAYSOCK", "/run/sway")].into();
        let p = decide(&|k| env.get(k).map(|v| v.to_string()), &|_| true, Some("/dev/pts/1"), Path::new(HUH), true, 1);
        let Plan::Split(argv) = p else { panic!("{p:?}") };
        assert!(argv.last().unwrap().ends_with("--tty /dev/pts/1 --hold --quiet"));
        assert!(!argv.contains(&"-t".to_string()), "no TMUX_PANE, no target");
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
        let cmd = strings(&["/h/huh", "tutor", "--tty", "/dev/pts/9"]);
        let t = |name: &str| window_argv(name, &cmd);
        let with = |head: &[&str]| head.iter().map(|s| s.to_string()).chain(cmd.clone()).collect::<Vec<_>>();
        assert_eq!(
            t("xdg-terminal-exec"),
            with(&["xdg-terminal-exec", "--app-id=huh-tutor", "--title=huh-tutor", "--"])
        );
        assert_eq!(t("ghostty"), with(&["ghostty", "--class=huh.tutor", "--title=huh-tutor", "-e"]));
        assert_eq!(t("kitty"), with(&["kitty", "--class", "huh-tutor", "--title", "huh-tutor"]));
        assert_eq!(t("alacritty"), with(&["alacritty", "--class", "huh-tutor", "--title", "huh-tutor", "-e"]));
        assert_eq!(t("foot"), with(&["foot", "--app-id", "huh-tutor", "--title", "huh-tutor"]));
        assert_eq!(t("wezterm"), with(&["wezterm", "start", "--class", "huh-tutor", "--"]));
        assert_eq!(t("konsole"), with(&["konsole", "-e"]));
        assert_eq!(t("gnome-terminal"), with(&["gnome-terminal", "--class=huh-tutor", "--title=huh-tutor", "--"]));
        assert_eq!(t("xterm"), with(&["xterm", "-class", "huh-tutor", "-T", "huh-tutor", "-e"]));
        assert_eq!(t("st"), with(&["st", "-e"]));
    }

    #[test]
    fn elsewhere_with_tmux_makes_a_session() {
        let Plan::Session(argv) = plan(&[], &["tmux"], Some("/dev/pts/2")) else { panic!() };
        let text = argv.join(" ");
        assert_eq!(argv[..6], strings(&["tmux", "-L", "huh-tutor", "-f", "/dev/null", "new-session"]));
        assert!(text.contains("-s huh-7"));
        assert!(text.contains("tmux kill-session -t huh-7"), "closing the shell ends the session");
        assert!(text.contains("mouse on") && text.contains("Ctrl+B then arrow to switch, exit to close"));
        assert!(text.contains("split-window -h -d -l 35% -t huh-7:0.0"), "right pane, focus stays left");
        let right = argv.last().unwrap();
        assert!(right.contains("tmux display -p -t huh-7:0.0") && right.contains("pane_tty"), "{right}");
        assert!(right.starts_with("exec /opt/huh/bin/huh tutor --tty "));
        assert!(!right.contains("#{"), "the format is not written out whole");
        assert!(!text.contains("tmux.conf"));
    }

    #[test]
    fn nothing_available_says_how_to_install_tmux() {
        let p = plan(&[("HYPRLAND_INSTANCE_SIGNATURE", "x")], &[], Some("/dev/pts/5"));
        assert_eq!(p, Plan::Unavailable { tty: "/dev/pts/5".into() });
        let text = unavailable_message("/dev/pts/5", &["arch".to_string()]);
        assert!(text.contains("pacman -S") && text.contains("tmux"), "{text}");
        assert!(text.contains("huh tutor --tty /dev/pts/5"), "{text}");
        let none = unavailable_message("/dev/pts/5", &[]);
        assert!(none.contains("package manager") && none.contains("--tty /dev/pts/5"));
    }

    #[test]
    fn a_tiling_desktop_without_a_terminal_falls_back_to_tmux() {
        assert!(matches!(plan(&[("I3SOCK", "x")], &["tmux"], Some("/dev/pts/5")), Plan::Session(_)));
    }

    #[test]
    fn refuses_without_a_terminal_and_inside_its_own_session() {
        assert!(matches!(plan(&[("TMUX", "/t/default,1,0")], &["tmux"], None), Plan::Refuse(_)));
        let p = plan(&[("TMUX", "/tmp/tmux-1000/huh-tutor,99,0")], &["tmux"], Some("/dev/pts/6"));
        assert!(matches!(p, Plan::Refuse(ref m) if m.contains("already")), "{p:?}");
    }

    #[test]
    fn the_recording_folder_is_passed_to_the_new_window() {
        // A window opened by the desktop does not inherit this shell's HUH_SESSION_LOG.
        let env = [("HYPRLAND_INSTANCE_SIGNATURE", "x"), ("HUH_SESSION_LOG", "/run/user/1000/rec/42.log")];
        let Plan::Window { argv, .. } = plan(&env, &["ghostty"], Some("/dev/pts/2")) else { panic!() };
        let line = argv.join(" ");
        assert!(line.contains("--tty /dev/pts/2 --dir /run/user/1000/rec --hold"), "{line}");
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
             ghostty --class=huh.tutor --title=huh-tutor -e /opt/huh/bin/huh tutor --tty /dev/pts/2 --hold"
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
