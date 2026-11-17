//! Remembering the last command's output, so `huh why` can explain a failure.
//!
//! The interactive shell runs inside util-linux `script`, which copies everything
//! shown in the terminal to a named pipe. `huh record-sink` reads that pipe and keeps
//! only the recent part in a private file. Two shell hooks print invisible markers
//! around each command, so the output of the last one can be cut out later.
//! Nothing is read or sent anywhere until `huh why` is run.

use anyhow::{Context, Result, anyhow};
use std::io::{Read, Seek, Write};
use std::path::Path;

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

/// Goes right after the "is this shell interactive" check in ~/.bashrc: replaces
/// the shell with a recorded one. Does nothing when already recorded.
pub fn record_script() -> Result<String> {
    let huh = std::env::current_exe().context("cannot find the huh binary")?;
    Ok(format!(
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
"#,
        huh = shell_quote(&huh.to_string_lossy())
    ))
}

/// Goes at the end of ~/.bashrc: marks where each command's output starts and
/// ends, notes its exit code (skipping `huh ...` commands),
/// and binds a key (default Alt+h and Alt+f, or `$HUH_KEY`) to replace the prompt
/// line with `huh`'s top suggestion without running it.
pub fn hooks_script() -> String {
    r#"# huh: mark each command's output (only in a recorded terminal) and bind prompt suggestion.
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
    .to_string()
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
fn last_region(log: &str) -> &str {
    let from = log.rfind(START).map_or(0, |at| at + START.len());
    let rest = &log[from..];
    &rest[..rest.find(END).unwrap_or(rest.len())]
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

fn parse_status(text: &str) -> (Option<i32>, String) {
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
}
