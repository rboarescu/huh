//! Remembering the last command's output, so `huh why` can explain a failure.
//!
//! The interactive shell runs inside util-linux `script`, which copies everything
//! shown in the terminal to a named pipe. `huh record-sink` reads that pipe and keeps
//! only the recent part in a private file. Two shell hooks print invisible markers
//! around each command, so the output of the last one can be cut out later.
//! Nothing is read or sent anywhere until `huh why` is run.

use anyhow::{Context, Result, anyhow};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

/// Supported interactive shells for `huh shell-init` and `huh setup`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ShellKind {
    Bash,
    Zsh,
    Fish,
}

impl ShellKind {
    #[allow(dead_code)]
    pub fn name(self) -> &'static str {
        match self {
            ShellKind::Bash => "bash",
            ShellKind::Zsh => "zsh",
            ShellKind::Fish => "fish",
        }
    }
}

/// CLI selector for `huh shell-init --shell`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, clap::ValueEnum)]
#[allow(dead_code)]
pub enum ShellArg {
    #[default]
    Auto,
    Bash,
    Zsh,
    Fish,
}

/// Detects the user's shell from `$SHELL` (defaults to `Bash` when unknown or unset).
#[allow(dead_code)]
pub fn detect_shell(shell_env: Option<&str>) -> ShellKind {
    let Some(raw) = shell_env.map(str::trim).filter(|s| !s.is_empty()) else {
        return ShellKind::Bash;
    };
    let base = Path::new(raw).file_name().and_then(|n| n.to_str()).unwrap_or(raw);
    match base {
        "zsh" => ShellKind::Zsh,
        "fish" => ShellKind::Fish,
        _ => ShellKind::Bash,
    }
}

/// Resolves `--shell` against `$SHELL`.
#[allow(dead_code)]
pub fn resolve_shell(arg: ShellArg, shell_env: Option<&str>) -> ShellKind {
    match arg {
        ShellArg::Auto => detect_shell(shell_env),
        ShellArg::Bash => ShellKind::Bash,
        ShellArg::Zsh => ShellKind::Zsh,
        ShellArg::Fish => ShellKind::Fish,
    }
}

/// Returns the interactive rc file path for `shell` (`~/.bashrc`, `~/.zshrc`, or `config.fish`).
#[allow(dead_code)]
pub fn rc_path(shell: ShellKind, xdg_config: Option<&Path>, home: &Path) -> PathBuf {
    rc_path_with_zdotdir(shell, xdg_config, None, home)
}

/// Like `rc_path`, honoring `$ZDOTDIR` for zsh when provided.
#[allow(dead_code)]
pub fn rc_path_with_zdotdir(
    shell: ShellKind,
    xdg_config: Option<&Path>,
    zdotdir: Option<&Path>,
    home: &Path,
) -> PathBuf {
    match shell {
        ShellKind::Bash => home.join(".bashrc"),
        ShellKind::Zsh => zdotdir.filter(|p| !p.as_os_str().is_empty()).unwrap_or(home).join(".zshrc"),
        ShellKind::Fish => xdg_config
            .filter(|p| !p.as_os_str().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("fish/config.fish"),
    }
}

/// Returns `(record_line, hooks_line)` to add to the user's shell rc file.
#[allow(dead_code)]
pub fn init_lines(shell: ShellKind) -> (&'static str, &'static str) {
    match shell {
        ShellKind::Bash => (r#"eval "$(huh shell-init)""#, r#"eval "$(huh shell-init --hooks)""#),
        ShellKind::Zsh => {
            (r#"eval "$(huh shell-init --shell zsh)""#, r#"eval "$(huh shell-init --shell zsh --hooks)""#)
        }
        ShellKind::Fish => ("huh shell-init --shell fish | source", "huh shell-init --shell fish --hooks | source"),
    }
}

/// Invisible terminal sequences (a private OSC number) printed by the shell hooks.
const START: &str = "\x1b]7770;huh;start\x07";
const END: &str = "\x1b]7770;huh;end\x07";

/// The log is cut back to `KEEP` bytes whenever it passes `LIMIT`, so a command
/// that prints for days cannot fill the disk.
const LIMIT: u64 = 1024 * 1024;
const KEEP: usize = 64 * 1024;

/// What `huh why` shows.
pub const MAX_LINES: usize = 40;
pub const MAX_CHARS: usize = 4000;

pub struct Last {
    pub command: String,
    pub exit: Option<i32>,
    pub output: String,
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

/// Goes right after the "is this shell interactive" check in the shell's rc file:
/// replaces the shell with a recorded one. Does nothing when already recorded.
#[cfg_attr(not(test), allow(dead_code))]
pub fn record_script() -> Result<String> {
    record_script_for(ShellKind::Bash)
}

/// Shell-specific recording wrapper for `huh shell-init --shell <shell>`.
pub fn record_script_for(shell: ShellKind) -> Result<String> {
    let huh = std::env::current_exe().context("cannot find the huh binary")?;
    let huh = shell_quote(&huh.to_string_lossy());
    Ok(match shell {
        ShellKind::Bash => format!(
            r#"# huh: record this terminal so `huh why` can read the last command's output.
if [[ $- == *i* && -z ${{HUH_NO_RECORD-}} && -t 0 && -t 1 ]] && command -v script >/dev/null; then
  if [[ ${{HUH_SESSION_TTY-}} != "$(tty)" ]]; then
    if [[ -n ${{HUH_SESSION_NEW-}} ]]; then
      # We are the shell inside the recorder.
      export HUH_SESSION_TTY="$(tty)"
      unset HUH_SESSION_NEW
    else
      __huh_dir="${{XDG_RUNTIME_DIR:-$HOME/.cache}}/huh"
      if mkdir -p "$__huh_dir" && chmod 700 "$__huh_dir"; then
        for __huh_old in "$__huh_dir"/*.log; do
          __huh_pid=${{__huh_old##*/}}
          [[ -d /proc/${{__huh_pid%.log}} ]] || rm -f "$__huh_old" "${{__huh_old%.log}}.status"
        done
        export HUH_SESSION_LOG="$__huh_dir/$$.log" HUH_SESSION_STATUS="$__huh_dir/$$.status"
        __huh_pipe="$__huh_dir/$$.pipe"
        rm -f "$__huh_pipe"
        if [[ -x {huh} ]] && mkfifo -m 600 "$__huh_pipe"; then
          (umask 077; : >| "$HUH_SESSION_STATUS")
          # The reader is detached on purpose: `script` hangs at exit if it has a child of its own.
          ({huh} record-sink "$HUH_SESSION_LOG" "$__huh_pipe" </dev/null >/dev/null 2>&1 &)
          export HUH_SESSION_NEW=1
          exec script -qf "$__huh_pipe" -c "$BASH"
          # Only reached when the recorder could not start: release the reader, carry on unrecorded.
          : <> "$__huh_pipe"
        fi
        unset HUH_SESSION_LOG HUH_SESSION_STATUS HUH_SESSION_NEW __huh_pipe
      fi
      unset __huh_dir __huh_old __huh_pid
    fi
  fi
fi
"#
        ),
        ShellKind::Zsh => format!(
            r#"# huh: record this terminal so `huh why` can read the last command's output.
if [[ -o interactive && -z ${{HUH_NO_RECORD-}} && -t 0 && -t 1 ]] && command -v script >/dev/null; then
  if [[ ${{HUH_SESSION_TTY-}} != "$(tty)" ]]; then
    if [[ -n ${{HUH_SESSION_NEW-}} ]]; then
      # We are the shell inside the recorder.
      export HUH_SESSION_TTY="$(tty)"
      unset HUH_SESSION_NEW
    else
      __huh_dir="${{XDG_RUNTIME_DIR:-$HOME/.cache}}/huh"
      if mkdir -p "$__huh_dir" && chmod 700 "$__huh_dir"; then
        for __huh_old in "$__huh_dir"/*.log(N); do
          __huh_pid=${{__huh_old##*/}}
          [[ -d /proc/${{__huh_pid%.log}} ]] || rm -f "$__huh_old" "${{__huh_old%.log}}.status"
        done
        export HUH_SESSION_LOG="$__huh_dir/$$.log" HUH_SESSION_STATUS="$__huh_dir/$$.status"
        __huh_pipe="$__huh_dir/$$.pipe"
        rm -f "$__huh_pipe"
        if [[ -x {huh} ]] && mkfifo -m 600 "$__huh_pipe"; then
          (umask 077; : >| "$HUH_SESSION_STATUS")
          # The reader is detached on purpose: `script` hangs at exit if it has a child of its own.
          ({huh} record-sink "$HUH_SESSION_LOG" "$__huh_pipe" </dev/null >/dev/null 2>&1 &)
          export HUH_SESSION_NEW=1
          exec script -qf "$__huh_pipe" -c "$SHELL"
          # Only reached when the recorder could not start: release the reader, carry on unrecorded.
          : <> "$__huh_pipe"
        fi
        unset HUH_SESSION_LOG HUH_SESSION_STATUS HUH_SESSION_NEW __huh_pipe
      fi
      unset __huh_dir __huh_old __huh_pid
    fi
  fi
fi
"#
        ),
        ShellKind::Fish => format!(
            r#"# huh: record this terminal so `huh why` can read the last command's output.
if status is-interactive; and test -z "$HUH_NO_RECORD"; and test -t 0; and test -t 1; and command -v script >/dev/null
    set -l __huh_tty (tty 2>/dev/null)
    if test "$HUH_SESSION_TTY" != "$__huh_tty"
        if test -n "$HUH_SESSION_NEW"
            # We are the shell inside the recorder.
            set -gx HUH_SESSION_TTY "$__huh_tty"
            set -e HUH_SESSION_NEW
        else
            set -l __huh_dir "$XDG_RUNTIME_DIR/huh"
            if test -z "$XDG_RUNTIME_DIR"
                set __huh_dir "$HOME/.cache/huh"
            end
            if mkdir -p "$__huh_dir"; and chmod 700 "$__huh_dir"
                for __huh_old in "$__huh_dir"/*.log
                    set -l __huh_base (string replace -r '.*/' '' -- "$__huh_old")
                    set -l __huh_pid (string replace -r '\.log$' '' -- "$__huh_base")
                    if not test -d "/proc/$__huh_pid"
                        rm -f "$__huh_old" "$__huh_dir/$__huh_pid.status"
                    end
                end
                set -gx HUH_SESSION_LOG "$__huh_dir/$fish_pid.log"
                set -gx HUH_SESSION_STATUS "$__huh_dir/$fish_pid.status"
                set -l __huh_pipe "$__huh_dir/$fish_pid.pipe"
                rm -f "$__huh_pipe"
                if test -x {huh}; and mkfifo -m 600 "$__huh_pipe"
                    set -l __huh_umask (umask)
                    umask 077
                    printf '' > "$HUH_SESSION_STATUS"
                    umask $__huh_umask
                    # The reader is detached on purpose: `script` hangs at exit if it has a child of its own.
                    {huh} record-sink "$HUH_SESSION_LOG" "$__huh_pipe" </dev/null >/dev/null 2>&1 &
                    disown $last_pid 2>/dev/null
                    set -gx HUH_SESSION_NEW 1
                    exec script -qf "$__huh_pipe" -c (status fish-path)
                    # Only reached when the recorder could not start: release the reader, carry on unrecorded.
                    printf '' > "$__huh_pipe"
                end
                set -e HUH_SESSION_LOG HUH_SESSION_STATUS HUH_SESSION_NEW
            end
        end
    end
end
"#
        ),
    })
}

/// Goes at the end of ~/.bashrc: marks where each command's output starts and
/// ends, notes its exit code (skipping `huh ...` commands),
/// and binds a key (default Alt+h and Alt+f, or `$HUH_KEY`) to replace the prompt
/// line with `huh`'s top suggestion without running it, and Alt+e to explain the prompt line.
#[cfg_attr(not(test), allow(dead_code))]
pub fn hooks_script() -> String {
    hooks_script_for(ShellKind::Bash)
}

/// Shell-specific command marker hooks and keybindings for `huh shell-init --hooks --shell <shell>`.
pub fn hooks_script_for(shell: ShellKind) -> String {
    match shell {
        ShellKind::Bash => r#"# huh: mark each command's output (only in a recorded terminal) and bind prompt suggestion.
if [[ $- == *i* && -n ${HUH_SESSION_LOG-} && ${HUH_SESSION_TTY-} == "$(tty)" && -z ${__huh_hooked-} ]]; then
  __huh_hooked=1
  # Ghostty only sets up its shell integration in the first shell of a terminal.
  if [[ -n ${GHOSTTY_RESOURCES_DIR-} ]] && ! declare -F __ghostty_hook >/dev/null \
      && [[ -r $GHOSTTY_RESOURCES_DIR/shell-integration/bash/ghostty.bash ]]; then
    builtin source "$GHOSTTY_RESOURCES_DIR/shell-integration/bash/ghostty.bash"
  fi
  __huh_last() {
    local line
    line=$(HISTTIMEFORMAT='' builtin history 1)
    line=${line#"${line%%[![:space:]]*}"}   # leading spaces
    line=${line#*[[:space:]]}                # history number
    builtin printf '%s' "${line#"${line%%[![:space:]]*}"}"
  }
  __huh_pre() {
    local line
    line=$(__huh_last)
    [[ $line == huh* ]] || builtin printf '\e]7770;huh;start\a'
  }
  __huh_post() {
    local status=$? line
    line=$(__huh_last)
    if [[ $line != huh* ]]; then
      builtin printf '\e]7770;huh;end\a'
      builtin printf '%s\n%s\n' "$status" "$line" >| "$HUH_SESSION_STATUS"
    fi
    return "$status"
  }
  PS0="${PS0-}"'$(__huh_pre)'
  if [[ $(builtin declare -p PROMPT_COMMAND 2>/dev/null) == "declare -a"* ]]; then
    PROMPT_COMMAND=(__huh_post "${PROMPT_COMMAND[@]}")
  else
    PROMPT_COMMAND="__huh_post${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
  fi
fi
if [[ $- == *i* && -z ${__huh_key_bound-} ]]; then
  __huh_key_bound=1
  __huh_complete() {
    [[ -n ${READLINE_LINE//[[:space:]]/} ]] || return 0
    local suggestion
    if suggestion=$(huh --no-ask --first-line-only "$READLINE_LINE" 2>/dev/null) && [[ -n $suggestion ]]; then
      READLINE_LINE=$suggestion
      READLINE_POINT=${#READLINE_LINE}
    fi
  }
  # Alt+e: explain the line being typed, below the prompt, without running it.
  __huh_explain() {
    [[ -n ${READLINE_LINE//[[:space:]]/} ]] || return 0
    builtin printf '\n'
    huh explain "$READLINE_LINE" 2>&1
  }
  builtin bind -x '"\ee": __huh_explain'
  if [[ -n ${HUH_KEY+x} ]]; then
    if [[ -n $HUH_KEY ]]; then
      builtin bind -x "\"$HUH_KEY\": __huh_complete"
    fi
  else
    builtin bind -x '"\eh": __huh_complete'
    builtin bind -x '"\ef": __huh_complete'
  fi
fi
"#
        .to_string(),
        ShellKind::Zsh => r#"# huh: mark each command's output (only in a recorded terminal) and bind prompt suggestion.
if [[ -o interactive && -n ${HUH_SESSION_LOG-} && ${HUH_SESSION_TTY-} == "$(tty)" && -z ${__huh_hooked-} ]]; then
  __huh_hooked=1
  __huh_cmd=""
  __huh_active=0
  __huh_preexec() {
    local line=${1#"${1%%[^[:space:]]*}"}
    if [[ $line == huh* ]]; then
      __huh_active=0
      __huh_cmd=""
    else
      __huh_active=1
      __huh_cmd=$line
      builtin printf '\e]7770;huh;start\a'
    fi
  }
  __huh_precmd() {
    local st=$?
    if (( __huh_active )); then
      builtin printf '\e]7770;huh;end\a'
      builtin printf '%s\n%s\n' "$st" "$__huh_cmd" >| "$HUH_SESSION_STATUS"
      __huh_active=0
      __huh_cmd=""
    fi
    return "$st"
  }
  autoload -Uz add-zsh-hook
  add-zsh-hook preexec __huh_preexec
  add-zsh-hook precmd __huh_precmd
fi
if [[ -o interactive && -z ${__huh_key_bound-} ]]; then
  __huh_key_bound=1
  __huh_complete() {
    [[ -n ${BUFFER//[[:space:]]/} ]] || return 0
    local suggestion
    if suggestion=$(huh --no-ask --first-line-only "$BUFFER" 2>/dev/null) && [[ -n $suggestion ]]; then
      BUFFER=$suggestion
      CURSOR=${#BUFFER}
    fi
  }
  # Alt+e: explain the line being typed, below the prompt, without running it.
  __huh_explain() {
    [[ -n ${BUFFER//[[:space:]]/} ]] || return 0
    zle -I
    builtin printf '\n'
    huh explain "$BUFFER" 2>&1
    zle reset-prompt
  }
  zle -N __huh_complete
  zle -N __huh_explain
  bindkey '\ee' __huh_explain
  if [[ -n ${HUH_KEY+x} ]]; then
    if [[ -n $HUH_KEY ]]; then
      bindkey "$HUH_KEY" __huh_complete
    fi
  else
    bindkey '\eh' __huh_complete
    bindkey '\ef' __huh_complete
  fi
fi
"#
        .to_string(),
        ShellKind::Fish => r#"# huh: mark each command's output (only in a recorded terminal) and bind prompt suggestion.
set -l __huh_tty (tty 2>/dev/null)
if status is-interactive; and test -n "$HUH_SESSION_LOG"; and test -n "$__huh_tty"; and test "$HUH_SESSION_TTY" = "$__huh_tty"; and not set -q __huh_hooked
    set -g __huh_hooked 1
    set -g __huh_active 0
    set -g __huh_cmd ""
    function __huh_preexec --on-event fish_preexec
        set -l __huh_line (string trim -l -- "$argv[1]")
        if string match -q 'huh*' -- "$__huh_line"
            set -g __huh_active 0
            set -g __huh_cmd ""
        else
            set -g __huh_active 1
            set -g __huh_cmd "$__huh_line"
            printf '\e]7770;huh;start\a'
        end
    end
    function __huh_postexec --on-event fish_postexec
        set -l __huh_st $status
        if test "$__huh_active" = 1
            printf '\e]7770;huh;end\a'
            printf '%s\n%s\n' "$__huh_st" "$__huh_cmd" > "$HUH_SESSION_STATUS"
            set -g __huh_active 0
            set -g __huh_cmd ""
        end
    end
end
if status is-interactive; and not set -q __huh_key_bound
    set -g __huh_key_bound 1
    function __huh_complete
        set -l __huh_line (commandline -b)
        string match -rq '\S' -- "$__huh_line"; or return 0
        set -l __huh_suggestion (huh --no-ask --first-line-only "$__huh_line" 2>/dev/null)
        if test $status -eq 0; and test -n "$__huh_suggestion"
            commandline -r -- "$__huh_suggestion"
            commandline -C (string length -- "$__huh_suggestion")
        end
    end
    # Alt+e: explain the line being typed, below the prompt, without running it.
    function __huh_explain
        set -l __huh_line (commandline -b)
        string match -rq '\S' -- "$__huh_line"; or return 0
        printf '\n'
        huh explain "$__huh_line" 2>&1
        commandline -f repaint
    end
    bind \ee __huh_explain
    if set -q HUH_KEY
        if test -n "$HUH_KEY"
            bind "$HUH_KEY" __huh_complete
        end
    else
        bind \eh __huh_complete
        bind \ef __huh_complete
    end
end
"#
        .to_string(),
    }
}

/// `huh record-sink <file> <pipe>`: copies the recorder's pipe to the file,
/// keeping the file small. It must never exit early: the recorder writing to
/// the pipe would die with it.
pub fn sink(path: &Path, pipe: &Path) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    let mut written: u64 = 0;
    let mut recent: Vec<u8> = Vec::new();
    let mut buffer = [0u8; 8192];
    // Opening waits for the recorder to open its end; after that the name is not needed.
    let mut input = std::fs::File::open(pipe)?;
    std::fs::remove_file(pipe).ok();
    loop {
        let count = match input.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Ok(()),
        };
        let chunk = &buffer[..count];
        recent.extend_from_slice(chunk);
        if recent.len() > 2 * KEEP {
            recent.drain(..recent.len() - KEEP);
        }
        // A failed write (disk full) loses log text but must not stop the loop.
        if written + count as u64 > LIMIT {
            let tail = &recent[recent.len().saturating_sub(KEEP)..];
            let rewritten = file.set_len(0).and_then(|_| file.rewind()).and_then(|_| file.write_all(tail));
            written = if rewritten.is_ok() { tail.len() as u64 } else { 0 };
        } else if file.write_all(chunk).is_ok() {
            written += count as u64;
        }
    }
}

/// The text between the last start marker and the end marker after it.
pub(crate) fn last_region(log: &str) -> &str {
    let from = log.rfind(START).map_or(0, |at| at + START.len());
    let rest = &log[from..];
    &rest[..rest.find(END).unwrap_or(rest.len())]
}

/// Has the last command in the log finished (an end marker after the last start marker)?
pub(crate) fn region_closed(log: &str) -> bool {
    match (log.rfind(START), log.rfind(END)) {
        (Some(start), Some(end)) => end > start,
        (None, Some(_)) => true,
        _ => false,
    }
}

/// Turns raw terminal output into the text a person saw: colour and cursor
/// sequences removed, progress bars that redraw one line reduced to their last state.
pub fn clean(raw: &str) -> String {
    let mut lines: Vec<String> = vec![String::new()];
    let mut chars = raw.chars().peekable();
    // After a carriage return the next text overwrites the line from its start.
    let mut overwrite = false;
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                // CSI: parameters, then one final letter.
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&c) {
                            break;
                        }
                    }
                }
                // OSC and other strings: up to BEL or ESC \.
                Some(']' | 'P' | '^' | '_') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                // Character set selection takes one more character.
                Some('(' | ')' | '*' | '+') => {
                    chars.next();
                }
                _ => {}
            },
            '\r' => overwrite = true,
            '\n' => {
                overwrite = false;
                lines.push(String::new());
            }
            '\x08' => {
                lines.last_mut().unwrap().pop();
            }
            '\t' => lines.last_mut().unwrap().push('\t'),
            c if c.is_control() => {}
            c => {
                let line = lines.last_mut().unwrap();
                if overwrite {
                    line.clear();
                    overwrite = false;
                }
                line.push(c);
            }
        }
    }
    let kept: Vec<&str> = lines.iter().map(|line| line.trim_end()).collect();
    kept.join("\n").trim_matches('\n').to_string()
}

/// The last `MAX_LINES` lines, and no more than `MAX_CHARS` characters of them.
pub fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut kept = lines[lines.len().saturating_sub(MAX_LINES)..].join("\n");
    if kept.chars().count() > MAX_CHARS {
        kept = kept.chars().skip(kept.chars().count() - MAX_CHARS).collect();
    }
    kept
}

pub(crate) fn parse_status(text: &str) -> (Option<i32>, String) {
    let (exit, command) = text.split_once('\n').unwrap_or((text, ""));
    (exit.trim().parse().ok(), command.trim().to_string())
}

/// The last command of this terminal, as recorded.
pub fn last() -> Result<Last> {
    let not_recorded = || {
        anyhow!(
            "this terminal is not being recorded. Pipe the output instead (`some-command 2>&1 | huh why`), \
             or see \"When a command fails\" in the README to set recording up"
        )
    };
    let log = env_nonempty("HUH_SESSION_LOG").ok_or_else(not_recorded)?;
    let status = env_nonempty("HUH_SESSION_STATUS").ok_or_else(not_recorded)?;
    let status =
        std::fs::read_to_string(&status).map_err(|_| anyhow!("no command has been recorded in this terminal yet"))?;
    let (exit, command) = parse_status(&status);
    let bytes = std::fs::read(&log).with_context(|| format!("cannot read {log}"))?;
    let output = tail(&clean(last_region(&String::from_utf8_lossy(&bytes))));
    Ok(Last { command, exit, output })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_colour_and_terminal_sequences() {
        let raw = "\x1b[1m\x1b[31merror\x1b[0m: no such file\r\n\x1b]133;D;1\x07\x1b(Bdone";
        assert_eq!(clean(raw), "error: no such file\ndone");
    }

    #[test]
    fn keeps_the_last_state_of_a_redrawn_line() {
        assert_eq!(clean("10%\r50%\r100%\r\nfinished\n"), "100%\nfinished");
        assert_eq!(clean("abc\x08\x08xy"), "axy");
    }

    #[test]
    fn cuts_out_the_last_command() {
        let log = format!("old{START}first{END}prompt{START}second\n{END}prompt $ huh why\n{END}");
        assert_eq!(last_region(&log), "second\n");
        // Start marker lost to log trimming, or the command is still running.
        assert_eq!(last_region(&format!("tail of output{END}prompt")), "tail of output");
        assert_eq!(last_region(&format!("{START}still running")), "still running");
    }

    #[test]
    fn keeps_only_the_end_of_long_output() {
        let long: String = (1..=100).map(|n| format!("line {n}\n")).collect();
        let kept = tail(&long);
        assert_eq!(kept.lines().count(), MAX_LINES);
        assert!(kept.ends_with("line 100"));
        assert_eq!(tail(&"x".repeat(MAX_CHARS + 50)).len(), MAX_CHARS);
    }

    #[test]
    fn reads_the_status_file() {
        assert_eq!(parse_status("101\ncargo build --release\n"), (Some(101), "cargo build --release".into()));
        assert_eq!(parse_status(""), (None, String::new()));
    }

    #[test]
    fn shell_snippets_are_valid_bash() {
        for script in [record_script().unwrap(), hooks_script()] {
            let mut child = std::process::Command::new("bash")
                .args(["-n", "/dev/stdin"])
                .stdin(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
            assert!(child.wait().unwrap().success());
        }
    }

    #[test]
    fn hook_snippet_skips_huh_and_completes_readline() {
        let record = record_script().unwrap();
        assert!(record.contains("HUH_NO_RECORD"));
        assert!(record.contains("HUH_SESSION_LOG"));
        assert!(record.contains("HUH_SESSION_STATUS"));

        let hooks = hooks_script();
        assert!(hooks.contains("[[ $line != huh* ]]"));
        assert!(hooks.contains("HUH_KEY"));
        assert!(hooks.contains(r#""\eh": __huh_complete"#));
        assert!(hooks.contains(r#""\ef": __huh_complete"#));
        assert!(hooks.contains(r#"huh --no-ask --first-line-only "$READLINE_LINE""#));
        assert!(hooks.contains(r#""\ee": __huh_explain"#) && hooks.contains(r#"huh explain "$READLINE_LINE""#));

        // Execute the interactive hook setup in bash to verify skipping, completion, and HUH_KEY binding.
        let tmp = std::env::temp_dir().join(format!("huh-hook-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let bin_dir = tmp.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let stub_huh = bin_dir.join("huh");
        std::fs::write(
            &stub_huh,
            "#!/usr/bin/env bash\n\
             if [[ \"$*\" == *\"make deploy.sh executable\"* ]]; then\n\
               printf 'chmod +x deploy.sh\\n'\n\
               exit 0\n\
             fi\n\
             exit 1\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stub_huh, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let status_file = tmp.join("status");
        let log_file = tmp.join("log");
        std::fs::write(&log_file, "").unwrap();
        let script_file = tmp.join("hooks.sh");
        std::fs::write(&script_file, &hooks).unwrap();

        let test_cmd = format!(
            r#"
set -e
export PATH={bin}:$PATH
export HUH_SESSION_LOG={log}
export HUH_SESSION_STATUS={status}
tty() {{ echo /dev/pts/test; }}
export HUH_SESSION_TTY=/dev/pts/test
set -o emacs
set -o history
source {hooks_path}
# Default bindings include both \eh and \ef
bindings=$(bind -X)
[[ $bindings == *'"\eh" "__huh_complete"'* ]]
[[ $bindings == *'"\ef" "__huh_complete"'* ]]

# Empty line stays untouched
READLINE_LINE=""
READLINE_POINT=0
__huh_complete
[[ $READLINE_LINE == "" && $READLINE_POINT == 0 ]]

# Matching request replaces READLINE_LINE and moves READLINE_POINT to end
READLINE_LINE="make deploy.sh executable"
READLINE_POINT=4
__huh_complete
[[ $READLINE_LINE == "chmod +x deploy.sh" ]]
[[ $READLINE_POINT == ${{#READLINE_LINE}} ]]

# Non-matching request (exit 1) leaves READLINE_LINE untouched
READLINE_LINE="write me a poem"
READLINE_POINT=5
__huh_complete
[[ $READLINE_LINE == "write me a poem" && $READLINE_POINT == 5 ]]

# Verify __huh_pre and __huh_post skip huh* (single-line keeps history 1 exact)
builtin history -s "ls -la"; [[ "$(__huh_pre)" == $'\e]7770;huh;start\a' ]]; __huh_post
grep -q "ls -la" {status}
builtin history -s "huh why"; [[ -z "$(__huh_pre)" ]]; __huh_post
grep -q "ls -la" {status}
"#,
            bin = shell_quote(&bin_dir.to_string_lossy()),
            log = shell_quote(&log_file.to_string_lossy()),
            status = shell_quote(&status_file.to_string_lossy()),
            hooks_path = shell_quote(&script_file.to_string_lossy()),
        );

        let output = std::process::Command::new("bash")
            .args(["--norc", "-i", "-c", &test_cmd])
            .env("TERM", "xterm-256color")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "bash hook test failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        // Custom HUH_KEY binds only that key (not \eh or \ef).
        let custom_cmd = format!(
            "set -e\nset -o emacs\nexport HUH_KEY='\\eg'\nsource {}\nbindings=$(bind -X)\n\
             [[ $bindings == *'\"\\eg\" \"__huh_complete\"'* ]]\n\
             [[ $bindings != *'\"\\eh\"'* ]]\n\
             [[ $bindings != *'\"\\ef\"'* ]]\n",
            shell_quote(&script_file.to_string_lossy()),
        );
        let custom_out = std::process::Command::new("bash")
            .args(["--norc", "-i", "-c", &custom_cmd])
            .env("TERM", "xterm-256color")
            .output()
            .unwrap();
        std::fs::remove_dir_all(&tmp).ok();
        assert!(
            custom_out.status.success(),
            "bash custom HUH_KEY test failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&custom_out.stdout),
            String::from_utf8_lossy(&custom_out.stderr)
        );
    }

    #[test]
    fn detect_and_resolve_shell_uses_basename() {
        assert_eq!(detect_shell(Some("/bin/zsh")), ShellKind::Zsh);
        assert_eq!(detect_shell(Some("/usr/local/bin/fish")), ShellKind::Fish);
        assert_eq!(detect_shell(Some("/bin/bash")), ShellKind::Bash);
        assert_eq!(detect_shell(Some("zsh")), ShellKind::Zsh);
        assert_eq!(detect_shell(Some("fish")), ShellKind::Fish);
        assert_eq!(detect_shell(Some("")), ShellKind::Bash);
        assert_eq!(detect_shell(None), ShellKind::Bash);
        assert_eq!(detect_shell(Some("/bin/sh")), ShellKind::Bash);

        assert_eq!(resolve_shell(ShellArg::Auto, Some("/usr/bin/zsh")), ShellKind::Zsh);
        assert_eq!(resolve_shell(ShellArg::Fish, Some("/bin/bash")), ShellKind::Fish);
        assert_eq!(resolve_shell(ShellArg::Bash, Some("/usr/bin/fish")), ShellKind::Bash);
        assert_eq!(resolve_shell(ShellArg::Zsh, None), ShellKind::Zsh);
    }

    #[test]
    fn rc_path_and_init_lines_match_each_shell() {
        let home = Path::new("/home/alice");
        assert_eq!(rc_path(ShellKind::Bash, None, home), PathBuf::from("/home/alice/.bashrc"));
        assert_eq!(rc_path(ShellKind::Zsh, None, home), PathBuf::from("/home/alice/.zshrc"));
        assert_eq!(
            rc_path_with_zdotdir(ShellKind::Zsh, None, Some(Path::new("/home/alice/.config/zsh")), home),
            PathBuf::from("/home/alice/.config/zsh/.zshrc")
        );
        assert_eq!(rc_path(ShellKind::Fish, None, home), PathBuf::from("/home/alice/.config/fish/config.fish"));
        assert_eq!(rc_path(ShellKind::Fish, Some(Path::new("/xdg")), home), PathBuf::from("/xdg/fish/config.fish"));

        assert_eq!(init_lines(ShellKind::Bash), (r#"eval "$(huh shell-init)""#, r#"eval "$(huh shell-init --hooks)""#));
        assert_eq!(
            init_lines(ShellKind::Zsh),
            (r#"eval "$(huh shell-init --shell zsh)""#, r#"eval "$(huh shell-init --shell zsh --hooks)""#)
        );
        assert_eq!(
            init_lines(ShellKind::Fish),
            ("huh shell-init --shell fish | source", "huh shell-init --shell fish --hooks | source")
        );
    }

    #[test]
    fn zsh_and_fish_scripts_contain_hooks_and_pass_installed_syntax_check() {
        assert_eq!(record_script_for(ShellKind::Bash).unwrap(), record_script().unwrap());
        assert_eq!(hooks_script_for(ShellKind::Bash), hooks_script());

        let zsh_rec = record_script_for(ShellKind::Zsh).unwrap();
        assert!(zsh_rec.contains("HUH_NO_RECORD"));
        assert!(zsh_rec.contains("HUH_SESSION_LOG"));
        assert!(zsh_rec.contains("HUH_SESSION_STATUS"));
        assert!(zsh_rec.contains("HUH_SESSION_TTY"));
        assert!(zsh_rec.contains(r#"script -qf "$__huh_pipe" -c "$SHELL""#));

        let zsh_hooks = hooks_script_for(ShellKind::Zsh);
        assert!(zsh_hooks.contains("add-zsh-hook preexec __huh_preexec"));
        assert!(zsh_hooks.contains("add-zsh-hook precmd __huh_precmd"));
        assert!(zsh_hooks.contains(r#"\e]7770;huh;start\a"#));
        assert!(zsh_hooks.contains(r#"\e]7770;huh;end\a"#));
        assert!(zsh_hooks.contains("HUH_SESSION_STATUS"));
        assert!(zsh_hooks.contains("[[ $line == huh* ]]"));
        assert!(zsh_hooks.contains("zle -N __huh_complete"));
        assert!(zsh_hooks.contains(r#"bindkey '\eh' __huh_complete"#));
        assert!(zsh_hooks.contains(r#"bindkey '\ef' __huh_complete"#));
        assert!(zsh_hooks.contains(r#"bindkey '\ee' __huh_explain"#));
        assert!(zsh_hooks.contains("HUH_KEY"));

        let fish_rec = record_script_for(ShellKind::Fish).unwrap();
        assert!(fish_rec.contains("HUH_NO_RECORD"));
        assert!(fish_rec.contains("HUH_SESSION_LOG"));
        assert!(fish_rec.contains("HUH_SESSION_STATUS"));
        assert!(fish_rec.contains("HUH_SESSION_TTY"));
        assert!(fish_rec.contains(r#"script -qf "$__huh_pipe" -c (status fish-path)"#));

        let fish_hooks = hooks_script_for(ShellKind::Fish);
        assert!(fish_hooks.contains("--on-event fish_preexec"));
        assert!(fish_hooks.contains("--on-event fish_postexec"));
        assert!(fish_hooks.contains(r#"\e]7770;huh;start\a"#));
        assert!(fish_hooks.contains(r#"\e]7770;huh;end\a"#));
        assert!(fish_hooks.contains("$HUH_SESSION_STATUS"));
        assert!(fish_hooks.contains("string match -q 'huh*'"));
        assert!(fish_hooks.contains(r#"bind \eh __huh_complete"#));
        assert!(fish_hooks.contains(r#"bind \ef __huh_complete"#));
        assert!(fish_hooks.contains(r#"bind \ee __huh_explain"#));
        assert!(fish_hooks.contains("HUH_KEY"));

        for (bin, scripts) in [("zsh", [zsh_rec, zsh_hooks]), ("fish", [fish_rec, fish_hooks])] {
            for script in scripts {
                let mut child = match std::process::Command::new(bin)
                    .args(["-n", "/dev/stdin"])
                    .stdin(std::process::Stdio::piped())
                    .spawn()
                {
                    Ok(child) => child,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                    Err(e) => panic!("failed to run {bin} -n: {e}"),
                };
                child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
                assert!(child.wait().unwrap().success(), "{bin} -n failed");
            }
        }
    }
}
