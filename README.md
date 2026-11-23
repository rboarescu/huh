# huh

Say what you want to do in plain language and get a terminal command back — or explain an unfamiliar command or a failure.

```
$ huh "make deploy.sh executable"
1. chmod +x deploy.sh
   Change who is allowed to read, write or run a file or folder (96%)
     +x         The file should be runnable as a program or script
     deploy.sh  the file or folder whose permissions should change
```

Nothing is ever run; `huh` only prints suggestions and explanations.

`huh` runs as a **100% deterministic, zero-LLM, zero-network single binary** (~2.7 MB release binary with the command catalog compiled in, <1 ms median latency in release, 0 MB background RAM, zero API keys or model servers).

## In plain terms (for new users)

huh is a helper for the terminal (the text window where you type commands). You tell it in plain English what you want, and it shows you the command to type. It **never runs anything itself**: it only shows you text, and you decide whether to use it.

It has no AI inside and doesn't use the internet. Everything it knows comes from three places:

```
              WHERE HUH GETS ITS INFORMATION

  1. ITS OWN NOTEBOOK              2. YOUR COMPUTER
  (built into the program)         (it looks, never changes)
  ┌──────────────────────┐         ┌─────────────────────────┐
  │ ~310 commands people │         │ • which Linux you have  │
  │ wrote down by hand:  │         │ • which programs are    │
  │ what each does, its  │         │   installed             │
  │ options, and which   │         │ • file names in the     │
  │ ones are dangerous   │         │   folder you're in      │
  └──────────┬───────────┘         │ • the manuals (man      │
             │                     │   pages) on your disk   │
             │                     └────────────┬────────────┘
             │                                  │
             ▼                                  ▼
          ┌────────────────────────────────────────┐
          │                  huh                   │
          │  matches your words against what it    │
          │  knows, then prints an answer          │
          └────────────────────────────────────────┘
                              ▲
                              │
  3. WHAT YOU GIVE IT
  ┌────────────────────────────────────────────────┐
  │ • a sentence you type ("free up disk space")   │
  │ • an error message you pipe into it            │
  │ • a command you don't understand               │
  └────────────────────────────────────────────────┘

        It does NOT use: the internet, an AI, or run commands.
```

### Three ways it can help you

**1. "I don't know the command."** Describe what you want:

```
huh "make backup.sh runnable"
→ chmod +x backup.sh
```

It fills in your real file names (it can see the folder you're in) and explains each piece of the command. If the command deletes things or turns the computer off, it adds a **WARNING** line. If the program isn't installed, it tells you how to install it the right way for *your* Linux.

**2. "Something went wrong and I don't understand the error."** Send the error into it:

```
cat notes.txt 2>&1 | huh why
→ You own notes.txt but it has no read permission.
  Try: chmod u+r notes.txt
```

It reads the error, checks your computer (does the file exist? who owns it?) and suggests a fix. If you set up [terminal recording](#terminal-recording-and-prompt-completion) once (`huh setup`), `huh why` remembers the last command and its output by itself, and bare `huh` opens a live companion pane beside your shell. That memory is private, held in memory, and gone when you log out.

**3. "Someone (or an AI) gave me a command. Is it safe?"** Ask it to explain before you run anything:

```
huh explain "curl https://site/install.sh | sudo bash"
→ explains each part, then:
  WARNING: downloads a script and runs it as administrator
  without you seeing it first.
```

It looks up each part in its notebook and in your computer's manuals, and points out the risky bits. It only reads the text you give it and never runs any of it.

(The outputs above are shortened.)

### What to keep in mind

- **It matches words; it doesn't understand like a person.** Simple, clear requests ("delete old.log", "show disk space") work best. Vague or chatty ones can get a wrong answer.
- **Always read the command before running it**, especially anything with a WARNING.
- **Read the `not included:` notes.** Every path, number, address, name or quoted text you type must be in the line or be named in a `not included: ...` note under it; when several are missing the confidence drops, because the command is probably the wrong one.
- **A `hint:` is a pointer, not a command.** When nothing fits confidently, huh may name the closest tool and where to read about it (`hint: no exact match. Closest: ffmpeg (...) — see `man ffmpeg`, especially -i (the input file)`). It never prints a command line there: you decide, and the manual tells you the rest. A plain "No suggestion" means huh has nothing honest to point at, which beats a guess.

## How it works

A local deterministic matcher (`LocalMatcher`) ranks the catalog (`rank`) and scores each shortlisted command's fit, flags and values (`details`); `assemble::build` turns that into the command line:

1. **Command matching**: Multi-field BM25F over command names, plain-language descriptions, curated `keywords`, flags, slots, and template literals, paired with stem/synonym normalization, domain-coherence fit checks, and an `installed()` tie-breaker.
2. **Slot & flag assembly**: Extracts candidate tokens, quoted phrases, multi-word capitalized names, number words (`third` -> `3`), bare extensions (`.rs` -> `*.rs`), size expressions (`50MB` -> `50M`), and non-hidden entries in the current folder, then fills the template and attaches a part-by-part explanation for the top suggestion.
   **System files read for values** (read-only, `src/sysinfo.rs`; huh never runs a program to learn them): `/etc/passwd` (user names), `/etc/group` (group names), `/etc/shells` (login shells; "bash" becomes `/bin/bash`), and the `*.service` files in `/usr/lib/systemd/system`, `/etc/systemd/system`, `~/.config/systemd/user` and `/usr/lib/systemd/user` (service names). A catalog slot opts in with `type = "service" | "user" | "group" | "shell" | "container" | "package"`. Containers and packages have no list (asking Docker would mean running it), so only a label ("container db") marks them. Label words ("group docker", "unit sshd", "user bob", "service web", "port 8080", "branch main") mark the value next to them and are never values themselves; words after "excluding", "except", "without" or "but not" are only ever exclusions. Set `HUH_SYSINFO_DIR=<folder>` (files `passwd`, `group`, `shells`, `units`) to read a different set of files; the evals and `cargo test` always use `tests/fixtures/sysinfo`, never the real machine.
3. **Reverse command explainer (`huh explain`)**: A bash-subset parser (`src/shell.rs`) builds a tree of the command line, then every part is explained per program, with warnings. Details in "Explain a shell command" below.
4. **Deterministic Error Doctor (`huh why`)**: Combines exit code meanings, live machine checks (`$PATH` typo scan with Damerau-Levenshtein distance, `metadata()` executable check, `/etc/group`, `/var/lib/pacman/db.lck`, `/var/run/docker.sock`, `$PWD` typo scan, platform and package manager detection from `src/platform.rs`), and structured error rules.

## Usage

```bash
cargo build --release                       # target/release/huh
```

For a binary that runs on any Linux (Debian 11, Ubuntu 20.04, RHEL 8 and Alpine included), build it
static against musl. The crate is pure Rust, so no C toolchain is needed:

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl   # target/x86_64-unknown-linux-musl/release/huh
```

The default (glibc) build needs glibc 2.34 or newer and does not start on musl systems such as Alpine.

### Companion pane (`huh`)

Running bare `huh` in a terminal opens the unified [companion pane](#companion-pane-huh) beside your shell (defaulting to live `watch` mode, or `lab` mode for interactive Linux lessons). One-shot CLI commands (`cmd | huh`, `huh "..."`, `huh explain`, `huh why`, `huh find`, `huh tools`, `huh pet`, `huh themes`, `huh welcome`, `huh setup`, `huh shell-init`) stay instant:

```bash
huh                          # open the unified companion pane beside your shell
huh --mode lab               # start directly in interactive Linux lab mode
huh --here                   # run the companion pane in the current terminal
huh --tty /dev/pts/3         # follow a specific recorded terminal in this pane
huh --quiet                  # explain each program only once per run
huh --dry-run                # print the launch command without starting the pane
```

### Suggest a command

```bash
huh "show the last 50 lines of error.log"
huh switch theme to "Tokyo Night"
huh --brief "make deploy.sh executable"   # hide the part-by-part breakdown
huh --no-cwd "..."                        # do not offer names from the current folder
huh --no-ask "..."                        # never ask a y/n question; list uncertain flags as notes
huh --all-commands "..."                  # also offer commands of other systems (e.g. apt on Arch)
```

### Explain a shell command

For when an agent (or a web page) hands you a long command and you want to know what it does before trusting it. `huh explain` only reads text; it never runs, spawns or evaluates anything from the command line (a test fails if the explain modules mention `std::process`).

```bash
$ huh explain "find . -name '*.log' -mtime +7 -delete"
This line does 1 thing:
  1. find: Search for files and folders under the places you name

Part by part:
1. find . -name '*.log' -mtime +7 -delete
   find           Search for files and folders under the places you name
   .              The folder to search in (. means the current folder)
   -name '*.log'  The file name matches the pattern *.log (case-sensitive; the
                  name only, not the folder path)
   -mtime +7      Match files whose content was last changed more than 7 days ago
   -delete        DELETE every match, permanently

WARNINGS
  ! [destroys data] `-delete` (in `find . -name '*.log' -mtime +7 -delete`):
    deletes every match permanently, with no confirmation (no trash). Run the
    same command without -delete first to see what would go.
```

```bash
huh explain "curl -fsSL https://example.com/install.sh | sudo bash -s -- --yes"
huh explain - < deploy.sh          # a whole script, `\` continuations included
pbpaste | huh explain              # no argument and piped input: reads standard input
```

- **Parser**: pipes (`|`, `|&`), lists (`&&`, `||`, `;`, `&`), `$(...)`, backticks, `${VAR}`, `$((...))`, `<(...)`, subshells, `{ ...; }`, `for`/`while`/`until`/`if`/`case`, functions, `[[ ... ]]`, assignments (`A=1 cmd`), all redirections (`>`, `>>`, `2>&1`, `&>`, `<`, `<<`, `<<<`, `>|`), heredocs (bodies are captured, not run) and `\` line continuations. It never panics: unbalanced quotes and brackets are parsed best-effort and reported under NOTES.
- **Recursion**: the inner command of `$(...)`, `bash -c '...'`, `sh -c`, `sudo`, `env`, `xargs`, `nohup`, `time`, `watch`, `timeout`, `find -exec`, `docker run/exec` (marked "inside the container") and `ssh host '...'` (marked "on the remote computer") is explained too, indented below.
- **Flags are explained per program**, from three sources in order: the curated table (`src/knowledge.txt`, ~290 programs and subcommands: git, docker, find with `+N`/`-N` decoded, tar, ssh, sed, awk, chmod with octal and symbolic modes decoded, kill signals, systemctl, ffmpeg, dosbox, flatpak, mpv, gio, nano, huh, ...), then the catalog, then the program's **man page read from disk** (`/usr/share/man/man{1,8}`, plain or `.gz`; `.TP`, `.IP`, `.PP` and mdoc `.It` layouts), which is marked "(from the man page)". Combined short flags (`-xzvf`) are split per program; single-dash long options (`find -name`) are not.
- **Warnings** (each one names its category, see [Warnings](#warnings)): deletes (`rm`, `find -delete`, `git clean`, `docker prune`, `dd of=`, `mkfs`, ...), overwrites (`>`, `sed -i`, `tee`, `cp`/`mv`), history rewrites (`git reset --hard`, `push --force`, `rebase`), privilege (`sudo`, `chmod 777`, `chown -R` on system folders), remote code (`curl | sh`, `bash <(curl ...)`), package installs (`pip --break-system-packages`, `npm -g`, `make install`), `kill -9` of computed PIDs, an unset variable in `rm "$DIR"/*`, and things that start a long build or run until Ctrl+C. Also turned-off checks (`curl -k`, `git config http.sslVerify false`, `StrictHostKeyChecking=no`, `setenforce 0`, `ufw disable`, `--no-verify`), services opened to the network (`python3 -m http.server`, `--host 0.0.0.0`, `ngrok`, `ssh -R`, `nc -l`), secrets on the command line or printed (`cat ~/.ssh/id_rsa`, `export TOKEN=...`), erased traces (`history -c`, `unset HISTFILE`, `journalctl --vacuum-*`, truncating `/var/log`), things set up to run later (`crontab -e`, shell startup files, `systemctl enable`), changes to shared things (`npm publish`, `docker push`, `git push origin main`, `kubectl apply`, `terraform apply`, cloud `delete`s), and rights handed out (`usermod -aG docker`, `visudo`, `ssh -A`, `docker run --privileged`).
- huh does not open scripts or Makefiles named in the command, so for `./run.sh` or `make install` it says so instead of guessing.

### Warnings

A warning informs; it never forbids. huh shows it and you decide. Every warning names one of these 14 categories, spelled the same way in all three places:

- a request: `WARNING (weakens security): turns off ...`
- `huh explain`: `! [weakens security] ...` under WARNINGS
- `huh why`, when the `Try:` command is itself risky: `Note (gives away access): ...` under the `Try:` line (plain package installs, `sudo` itself and long-running commands get no note)

| Category | Meaning |
|---|---|
| destroys data | deletes or overwrites data in a way you cannot undo |
| rewrites history | changes git history, so earlier commits can be lost or differ from what others have |
| runs code from the internet | downloads code and runs it before you have read it |
| runs unread code | runs code that huh cannot see into: eval, inline scripts, installer scripts |
| gives away access | hands someone or something more rights: permissions, groups, root, sudoers |
| weakens security | turns off a check or protection: certificates, host keys, firewall, SELinux |
| exposes a service | makes a service reachable from other computers or the internet |
| leaks secrets | puts a password, token or private key where others or logs can see it |
| hides tracks | erases history or logs that would show what happened |
| persists | makes something run again later: cron, startup files, services at boot |
| affects other people or production | changes something shared: a shared branch, a registry, a cluster, production |
| kills work | stops programs or the whole machine, losing unsaved work |
| big system change | changes the whole system: packages, kernel, system settings, admin rights |
| heads-up | does not end by itself or prints a lot: a note, not a danger |

The list lives in `src/risk_kinds.rs`. In the catalog a warning is written as `warning = "..."` plus `risk = "weakens security"` (on the entry, an option, a flag, or a text slot); a test fails when a catalog warning has no `risk`, and another when this table misses a category.

### Explain a failed command (`huh why` or `cmd | huh`)

Without any shell setup, pipe a command's output directly into `huh` or `huh why`:

```bash
some-command 2>&1 | huh
some-command 2>&1 | huh why
```

In a recorded terminal (see [terminal recording](#terminal-recording-and-prompt-completion)), typing `huh why` reads the last command and its tail output (up to 40 lines / 4,000 characters) and prints the diagnosis:

```
$ jq . data.json
bash: jq: command not found
$ huh why
Last command: jq . data.json (exit code 127)

The command `jq` is not installed (`command not found`, exit code 127).
Try: omarchy pkg add jq

(Nothing was run.)
```

`huh why` says so when it does not know. Without an exit code, only a line shaped like an error message (`error: ...`, `fatal: ...`, `program: file: reason`) counts as a failure; output with none is called "not an error" only when it is recognised success text (a clean `git status`, a passing test run, a listing), and otherwise huh says it does not recognise the output and quotes the closest line. An unrecognised failure quotes the line that reports it instead of guessing a fix, an exit code is only stated when it was given, and facts about this computer (such as docker group membership) are labelled as checks made here. A refused write into a system path gets `sudo tee` (`tee -a` for `>>`, with a note that `>` replaces the whole file), never `chmod +x`; admin tools such as `useradd` or `apt-get install` refused for lack of privileges get `sudo` in front of your command; a missing command that is a known program (or a file in the current folder) is never called a typo; a clean `git commit` refusal with exit 1 is "made no commit", never "not an error"; table rows, prose sentences, list items, code lines (`except ...`), commit-log lines, timestamped log lines of other programs and warnings followed by a success line (`connected`, `done`) are not error lines, and `make: Nothing to be done` is success; `pacman` keeps the operation you ran; a missing library or header names the development package for your distro. Refused access to a file is checked against the file itself (owner and mode, read-only), so `sudo` is suggested only for other users' or system files; a missing Python or node module that looks like a typo is called one rather than installed. AppImage missing-FUSE 2 errors (`libfuse.so.2`, `AppImages require FUSE`, `cannot mount AppImage`) explain FUSE 2 mounting (`fuse2` / `libfuse2`) and suggest `Try: <target> --appimage-extract-and-run`; `Exec format error` is checked before exit-126 `chmod +x` rules so Windows/DOS `.exe`/`.com`/`.bat` targets suggest `Try: dosbox <target>` (and note `wine` / Steam Proton) while foreign-architecture or headerless binaries suggest `Try: file <target>`.

### Other commands

```bash
huh check                                            # validate the catalog; "-" marks commands not installed here
huh eval                                             # score evals/cases.toml
huh eval --cases evals/heldout.toml                  # score the held-out suite
huh eval --cases evals/everyday.toml               # score everyday desktop, gaming & media requests
huh eval --cases evals/debian.toml --platform debian # score Debian platform filtering
huh eval-errors                                      # score evals/errors.toml
huh eval-errors --cases evals/errors-heldout.toml    # score held-out error cases
huh eval-errors --cases evals/errors-real.toml       # score failures from the real-world test
```

### Expert mode

Normal mode is cautious on purpose. If you know your way around a shell and find that too careful, switch on expert mode: `huh --expert "delete stuff"`, `HUH_MODE=expert`, or `mode = "expert"` in `~/.config/huh/config.toml`, or toggle it live inside the companion pane with `/safety expert` (saved to `~/.local/state/huh/prefs.toml`). `--normal` (or `HUH_MODE=normal`) overrides the file; the order is flag, then environment, then `config.toml`, then `prefs.toml`, then normal. An unknown value is an error that lists the valid ones (`normal`, `expert`).

Expert mode relaxes only the beginner protections, so you get the best-ranked command instead of "No suggestion":

- a dangerous command no longer needs a named object (`rm <target>` for "delete stuff");
- a command on a whole folder you did not type is kept (`rm -r ~`), with the WARNING and an extra note such as `note: this acts on your whole home folder`;
- a question about state ("is nginx running") ranks like any request (actions are still ranked below queries, just not removed).

It prints `expert mode: fewer beginner safeguards; warnings are still shown.` on stderr first, so `--first-line-only` and pipes stay clean. Formatting or wiping a drive is refused in normal mode; expert mode shows `mkfs`, `wipefs` or `dd` with a `WARNING (destroys data)` and a reminder to check the device with `lsblk` first. Never changed: every WARNING line, the refusal to help with access to systems, accounts or networks that are not yours, and the redirect of "is X dangerous" or "what does X do" to `huh explain`.

### List your tools

```bash
huh tools                  # every program on $PATH, one line each: name, description
huh tools compress         # only programs whose name or description contains every word
huh tools network --no-desc
```

The last line says how many matched ("143 of 2214 programs match"). Programs come from the directories of `$PATH` (the first one wins when a name appears twice; files that are not executable are skipped). The description is the catalog's, else the built-in table's, else the NAME line of the program's man page, read from disk (plain or `.gz`; `.xz` and `.zst` pages are skipped). Nothing is run, not even `--help` or `man`, and nothing is cached.

### Find a command or tool

```bash
huh find                  # pick from everything (fzf if installed, a plain finder otherwise)
huh find compress         # start with the words "compress" typed in
huh find compress | head  # piped: print the matching lines, best first, and exit
huh find --describe tar   # the details of one entry, as shown in the preview
```

`huh find` searches two lists with one fuzzy search (the letters you type must appear in order; word starts and runs of letters score higher, and a match in the name counts more than one in the description):

- huh's catalog entries: name, description and keywords, tagged `[catalog]`;
- every program on `$PATH`, with the description `huh tools` gives it, tagged `[installed]`. A program that has a catalog entry is listed once, as the entry.

Each item is one line, `name — description [tag]`. **With `fzf` installed** the list goes to fzf with your words as the starting query, a preview pane on the right (about 55%, the details below) and the header "Enter: details   Esc: quit". Enter prints the details of the chosen item. **Without fzf** a small line-based finder runs: type words, it prints the 15 best matches numbered, type a number to see the details, press Enter for a new search, `q` to quit.

The details (`huh find --describe NAME`, at most about 25 lines) are: the name and description; whether the program is installed, and if not the install command for this system; for a catalog entry its command template and any WARNING with its risk category; up to 6 common flags with one-line meanings (the built-in table first, the man page second); and "More: man NAME".

Nothing is run: not the item you pick, not the program, not `--help`. The only program huh starts is `fzf` itself (in `src/launch.rs`), and its preview only calls `huh find --describe`. Colours follow the theme and are off when piped or with `NO_COLOR`. Listing the programs takes about 0.3 s, the same as `huh tools`.

## Colours and themes

Output is coloured by role (command lines, flags, WARNING labels, risk categories, hints, the `Try:` line). The text itself is the same with or without colour.

| Theme | Colours |
|---|---|
| `babyblue` (default) | truecolour, the same in every terminal: a baby-blue accent and logo (`#89CFF0`), a lighter blue for commands (`#3F97E4`) and a deeper blue for headers and flags (`#2F80D8`). Descriptions and body text keep the terminal's own colour. WARNING and the risk category are red (`#E5484D`) and orange (`#E8750E`), success and `Try:` green, hints and values lavender (`#8A74E0`), causes amber, so danger never looks like the accent. The blues are mid-tones that read on a black, a `#1e1e1e` and a white background; when `COLORFGBG` says the background is light (7 or 15), every colour switches to a deeper shade. Without 24-bit colour (`COLORTERM` not `truecolor`/`24bit` and `TERM` not containing `direct`) the colours become the nearest of the 256-colour codes, never the 16 ANSI slots |
| `auto` | on Omarchy, the active theme's exact colours (a theme can put green in the "red" slot, so the slot numbers are not trusted); elsewhere, the terminal's own 16 ANSI colours |
| `omarchy` | truecolour values read at run time from `~/.local/state/omarchy/current/theme/colors.toml` (accent, foreground, background, color1-color6); falls back to `auto` if the file is missing or unreadable |
| `dark`, `light`, `nord`, `gruvbox`, `catppuccin` | built-in truecolour palettes (One Dark, One Light, Nord, Gruvbox, Catppuccin Mocha) |
| `mono` | bold, dim and underline only |

Run `huh themes` to see a sample of each. Choose one with `--theme NAME`, `HUH_THEME=NAME`, `theme = "nord"` in `~/.config/huh/config.toml`, or `/theme NAME` inside the companion pane (saved to `~/.local/state/huh/prefs.toml`). Precedence: flag, environment, `config.toml`, `prefs.toml`, then `babyblue`. To get the old look back, use `theme = "auto"` (the Omarchy theme's colours, else the terminal's 16 colours) or `theme = "omarchy"`. An unknown name lists the valid ones.

Colour is off when output is not a terminal (pipes), with `--first-line-only` (the Alt+h key binding), when `NO_COLOR` is set (any value), when `TERM=dumb`, and with `--color never`. `--color always` forces it on. The `huh` companion pane writes to a terminal, so it is coloured.

## Welcome animation

The `L2b` slab "huh" logo (4 lines tall, 20 columns wide, with rounded `?` flipping to bold `!`), in the theme's accent colour (baby blue by default), draws itself in under a second (any key skips it), followed by a five-line welcome with three example commands. It plays when the `huh` companion pane starts (both bare `huh` and `huh --here`), once the first time you run huh in a terminal (recorded in `$XDG_STATE_HOME/huh/welcomed`, default `~/.local/state/huh/welcomed`), and whenever you run `huh welcome`.

It never plays into a pipe or when input is not a terminal, with `--first-line-only`, in a terminal narrower than the art, with `TERM=dumb`, with `HUH_NO_ANIMATION=1`, or with `animation = false` in `~/.config/huh/config.toml`. To switch it off for good, set `animation = false`. To see it again after the first run, use `huh welcome` or delete the `welcomed` file.

### Pinned header in the companion pane

When the `huh` companion pane runs in a terminal at least 15 rows tall (and wide enough for the logo), the animation plays first and then settles as a header pinned at the top of the pane: the logo and caption on the left, **Pebble** beside it starting at column 25 (when at least 38 columns wide), a live status bar on row 6, and a rotating usage tip on row 7 (when tips are enabled):

- `watch` mode: `[WATCH] /dev/pts/N · normal · /mode · /help` (or `no hook` until recording starts)
- `lab` mode: `[LAB] XP 120 · 3d · /dev/pts/N · /mode · /help`
- Row 7 (`src/tips.rs`, `src/pin.rs`): `tip: ...` cycling through 18 concise tips every ~15 s (68 ticks), refreshing only row 7 in place with zero flicker on the logo or Pebble sprite. Toggle or cycle live with `/tips [on|off|next]` (the scroll region switches seamlessly between 6 and 7 header rows), or disable persistently via `prefs.toml`, `tips = false` in `~/.config/huh/config.toml`, or `HUH_NO_TIPS=1` (`HUH_NO_TIPS=1` > `config.toml` > `prefs.toml` > enabled by default).

Everything the pane prints scrolls underneath using a terminal scroll region (`ESC[top;bottom r`), so the header is drawn once and not redrawn on every block, and the `huh> ` prompt stays on the bottom row. Resizing the pane redraws it for the new size (dropping the region below 15 rows and restoring it when it grows). While `/find` runs, both the scroll region and any Kitty Graphics image are suspended cleanly and restored afterwards. On exit (Ctrl+C or an error) the region and image are cleared and the cursor is left below everything.

It is not used when output is not a terminal, in a terminal shorter than 15 rows or narrower than the logo, with `TERM=dumb`, with `HUH_NO_PIN=1`, or with `pin = false` in `~/.config/huh/config.toml`; the pane then prints a plain start message. If the pane is killed without a chance to clean up (`kill -9`, SIGTERM), run `reset` to get the terminal's scrolling back.

### Terminal pet (Pebble)

A `#!` companion named **Pebble** lives beside the logo in the pinned header (at least 38 columns wide; auto-hides on narrower panes). Clean commands (`exit 0`) in the followed terminal and right answers or practice clears in `lab` mode feed Pebble and build streaks (`happy` / `celebrating`), idle hours make it `sleepy` or `hungry` (floored at 10 fullness / 20 happiness so it never dies), and failed or warned commands briefly show `alert` without punishing its stats. Header updates redraw in place without disturbing scroll state.

- **Species roster (9 species × 6 moods)**:
  - **Penguin chick** (`penguin`, default, named **Pebble**): `P1` Modern Indie Chibi 32×32 pixel sprite / `A1` rounded ASCII
  - **Terminal Cat** (`cat`, default name **Pebble**): `P4` 16-Bit RPG pixel sprite / `A4` UNIX BBS ASCII
  - **Builder Beaver** (`beaver`, default name **Bóbr**): `P3` Plush River Bóbr 32×32 pixel sprite / `A2` ASCII
  - **Shell-Turtle** (`turtle`, default name **Pebble**): `P1` pixel sprite / `A1` rounded ASCII
  - **Wave Dolphin** (`dolphin`, default name **Flow**): `P2` 16-Bit Ocean Rider 32×32 pixel sprite / `A2` ASCII
  - **Woodland Bear** (`bear`, default name **Vector**): `P1` Cozy Scarf Cub 32×32 pixel sprite / `A1` ASCII
  - **Terminal Serpent** (`snake`, default name **Hiss**): `P1` Coiled Chibi Python 32×32 pixel sprite / `A2` ASCII
  - **Queen Bee** (`bee`, default name **Anabee**): `P3` Honeycomb Fairy Queen 32×32 pixel sprite / `A3` ASCII
  - **Snowy Owl** (`owl`, default name **Pebble**): `P4` 16-Bit RPG wizard pixel sprite / `A1` rounded ASCII
  - Switching species updates the pet's canonical default name automatically when still on a default mascot name, while preserving any custom nickname set with `/pet name` or `huh pet --name`.
- **32×32 Kitty Graphics + animated ASCII fallback**: In Ghostty, Kitty, and WezTerm — including inside `huh`'s own private `tmux -L huh` session (`allow-passthrough on`) or outer tmux with `HUH_KITTY_GRAPHICS=1` — your companion renders as 32×32 pixel art tinted to the active theme (`accent`, `warning`, `success`, `note`, `dim`, `shadow`) with a `#!` belly badge. Everywhere else (or with `style = "ascii"`), it renders as animated 4×10 pure printable ASCII art carrying `#!`.
- **Dual-track unlocks**: Every extra species unlocks via **either** its `lab` unit badge **or** a `watch` milestone:
  - **Cat** (`cat`): `pathfinder` badge (Unit 1 in `lab`) **or** 25 unique commands in `watch`
  - **Beaver** (`beaver`, **Bóbr**): `file_keeper` badge (Unit 2 in `lab`) **or** 50 commands fed in `watch`
  - **Turtle** (`turtle`): `pipe_plumber` badge (Unit 3 in `lab`) **or** 100 commands fed or a 7-day streak in `watch`
  - **Dolphin** (`dolphin`, **Flow**): `media_wrangler` badge (Unit 10 in `lab`) **or** 35 unique commands or 120 commands fed in `watch`
  - **Bear** (`bear`, **Vector**): `process_tamer` badge (Unit 5 in `lab`) **or** 150 commands fed or a 10-day streak in `watch`
  - **Snake** (`snake`, **Hiss**): `archive_ace` badge (Unit 7 in `lab`) **or** 45 unique commands or 180 commands fed in `watch`
  - **Bee** (`bee`, **Anabee**): `pixel_painter` badge (Unit 11 in `lab`) **or** 200 commands fed or a 14-day streak in `watch`
  - **Owl** (`owl`): `night_watch` badge (Unit 8 in `lab`) **or** 250 commands fed or 5 warnings heeded in `watch`
- **Custom sprite sheets**: Export the built-in 32×32 TOML sheet to `$XDG_CONFIG_HOME/huh/sprites/<species>.toml` (default `~/.config/huh/sprites/<species>.toml`) with `huh pet --customize` or `/pet customize` (existing files are never overwritten; invalid overrides fall back cleanly to the built-in sheet).

Check status, rename, switch species/render style, or export a custom sprite with `huh pet` (`--name NAME`, `--species SPECIES`, `--style auto|pixel|ascii|off`, `--customize`), or type `/pet ...` / `pet` inside the companion pane. State stays local in `$XDG_STATE_HOME/huh/pet.toml` (default `~/.local/state/huh/pet.toml`). Hide Pebble with `HUH_NO_PET=1`, `pet = false` / `pet = "off"` in `~/.config/huh/config.toml`, or `--style off`.

## Companion pane (`huh`)

Running bare `huh` opens one unified companion pane that combines live shell watching (`watch` mode) and the offline Linux course (`lab` mode), replacing the old `huh tutor` and `huh learn` subcommands. Nothing typed in the pane is ever executed.

- **Side-by-side launch (`huh`)**: When run without arguments in a terminal, `huh` opens the pane next to your shell:
  1. **Inside tmux** (`$TMUX` set): `tmux split-window -h -d -l 35%` next to this pane (focus stays in yours), running `huh --here --tty <this pane's tty>`.
  2. **Not in tmux, on a tiling desktop** (Hyprland, sway, or i3): opens a detached terminal window running `huh --here --tty <this tty>` using `xdg-terminal-exec` (Omarchy), else `$TERMINAL -e`, else the first installed of `ghostty`, `kitty`, `alacritty`, `foot`, `wezterm`, `konsole`, `gnome-terminal`, `xterm`. Window title/class is `huh` (Ghostty uses `--class=huh.pane --title=huh`, as GTK requires a dot). On Hyprland you can add a rule such as `windowrule = float on, size 600 800, match:class ^huh(\.pane)?$` (huh never edits your WM config).
  3. **Anywhere else, with tmux installed**: replaces your shell with a private tmux session (`tmux -L huh -f /dev/null`) with `mouse on`, `allow-passthrough on` (enabling 32×32 Kitty Graphics inside the split), status bar `"huh — Ctrl+B then arrow to switch, exit to close"`, your shell on the left, and the `huh` pane (`HUH_PANE=1`) on the right. Closing the left shell ends the session.
  4. **Neither tmux nor a tiling desktop**: prints the `tmux` install command for your distro and how to run `huh --here --tty <this tty>` in a second window.

  Use `huh --here` to run in the current pane, `huh --mode watch|lab` to pick the starting mode, `huh --tty /dev/pts/N` to follow a specific recorded terminal, `huh --quiet` to explain each program once per run, and `huh --dry-run` (or `HUH_DRY_RUN=1`) to print the launch command instead of starting it. Nested launches inside `tmux -L huh` or `HUH_PANE=1` are refused cleanly. All process launching lives in `src/launch.rs` (a test checks that no other source file uses `std::process::Command`).
- **`cmd | huh`** (or `cmd | huh explain`): instant one-shot output explanations for piped output from common commands (`ls -l`, `df`, `du`, `lsblk`, `stat`, `findmnt`, `free`, `ps aux`/`ps -ef`, `top`, `ip addr`/`ip -br addr`, `ss`, `ping`, `git status`/`log`/`branch -vv`, `systemctl status`, `journalctl`, `docker ps`, `uname -a`, `uptime`, `who`, `last`, `id`, `sensors`, `nmcli dev`), defined in `src/outputs.txt`. Error-shaped or unrecognized text falls through to `huh why`.

```
$ lsblk | huh
This looks like the output of `lsblk`: disks and partitions.
  NAME         device name; ├─ and └─ show partitions or layers inside the disk
               above
  ...
  crypt        an encrypted volume that has been unlocked; its contents appear
               under the mount points
  [SWAP]       used as swap space (overflow memory), not as a folder
* to unmount something, use a path from MOUNTPOINTS, not from NAME.
```

### Watch mode

`watch` mode is the default when opening `huh` (configurable via `default_mode = "watch" | "lab" | "last"` in `~/.config/huh/config.toml`).

- **Smooth startup without hooks**: If shell recording is not active yet, `watch` mode starts cleanly (`[WATCH] no hook`), shows a brief note pointing to `huh setup`, lets you type commands or questions or switch to `/mode lab` right away, and attaches automatically once a recorded shell appears.
- **Live command following**: For each finished command in the watched terminal, it prints the command and exit code, a concise breakdown of its flags and arguments, then `huh why` diagnosis on failure or `outputs.txt` field meanings on recognized output. Full-screen programs (`vim`, `htop`) get one line.
- **Interactive `huh> ` prompt**: Type directly in the pane (on interactive Linux TTYs, `tui::LineEditor` supports `←`/`→`, `Home`/`End`, `Ctrl+A`/`E`/`U`/`W`, 100-item `↑`/`↓` history, and live `/` fuzzy completions, falling back cleanly to cooked line reading in non-TTY/piped sessions; `Ctrl+C` quits, `Ctrl+D` on an empty line closes input while continuing to watch):
  - a command line (`tar -xzf a.tgz`, `ls | wc -l`, `/etc/hosts`) → `huh explain` breakdown (multi-segment Unix paths are never treated as slash commands)
  - plain words (`make deploy.sh executable`) → `huh "..."` suggestions with WARNINGs and hints
  - `?word [word...]` → `huh tools` filter (capped at 20 lines)
  - `why` → `huh why` on the last finished command in the watched terminal
  - `pet` → Pebble's status card
  - `/...` → fuzzy slash commands (live dropdown on TTY, submit-time resolution everywhere)
  - empty line (`Enter`) → replay the last watched command block in full (even under `--quiet`)

### Lab mode

Switch into `lab` mode at launch (`huh --mode lab`) or live from any prompt (`/mode lab` or `/lab`) for an offline, beginner-friendly Linux course built directly on `huh`'s parser, explainer, output table, platform detector, and safety warnings.

- **Full 11-unit curriculum (33 lessons, 165 questions, 9 hands-on labs, 19 badges)** embedded at build time from `lessons/01..11-*.toml` (core Units 1–8 plus 3 real-world tracks unlocked immediately after Unit 1 via `unlock_after`, while *Continue next lesson* stays on the core path until Unit 8 finishes):
  - **Core track** (completing all 8 core units earns `shell_grad`):
    1. *Finding Your Way* (`pwd`, `ls`, `cd`) — badge `pathfinder` (also unlocks **Terminal Cat**), plus practice lab
    2. *Files & Folders* (`mkdir`, `touch`, `cp`, `mv`, `rm`, `cat`, `head`, `tail`) — badge `file_keeper` (also unlocks **Builder Beaver** / Bóbr), plus practice lab
    3. *Searching & Pipes* (`grep`, `find`, `wc`, `sort`, `uniq`, `|`, `>`, `>>`) — badge `pipe_plumber` (also unlocks **Shell-Turtle**), plus practice lab
    4. *Reading Permissions* (`ls -l` rwx bits, `chmod`, `whoami`, `id`) — badge `locksmith`, plus practice lab
    5. *Processes & Disk* (`ps aux`, `pgrep`, `kill`, `uptime`, `df -h`, `du -sh`, `free -h`) — badge `process_tamer` (also unlocks **Woodland Bear** / Vector), plus practice lab
    6. *Packages & Manuals* (`which`, `man`, `--help`, and **distro-aware** package commands resolved via `src/platform.rs` for Arch, Omarchy, Debian/Ubuntu, Fedora, SUSE, Alpine, Void, Gentoo, or Nix) — badge `package_pro`
    7. *Archives & Links* (`tar`, `gzip`, `unzip`, `diff`, `sha256sum`, `ln -s`) — badge `archive_ace` (also unlocks **Terminal Serpent** / Hiss), plus practice lab
    8. *Networking & Stay Safe* (`ping`, `ip -br addr`, `ss`, `curl`, `ssh`, plus the 5-risk *Spot the Danger* boss lesson) — badge `night_watch` (also unlocks **Snowy Owl**), plus `shell_grad` upon finishing all 8 core units
  - **3 Real-World Tracks** (tagged `real-world`; unlocked right after Unit 1 *Finding Your Way*):
    9. *Apps & Games on Linux* (AppImages & FUSE 2, Flatpak & Flathub, DOSBox & `.exe` checks) — badge `game_runner`, plus practice lab (*Game Night Setup*)
    10. *Media & Everyday Files* (`xdg-open`, `file`, `gio trash`, `mpv`, `vlc`, `ffmpeg` basics) — badge `media_wrangler` (also unlocks **Wave Dolphin** / Flow), plus practice lab (*Downloads Cleanup*)
    11. *Paint Your Own Pet* (`huh pet --customize`, `~/.config/huh/sprites/`, pixel/palette TOML editing, backups & safe fallback) — badge `pixel_painter` (also unlocks **Queen Bee** / Anabee), plus practice lab (*Sprite Studio*)
- **4 question types**: *What does it do?* (condensed `huh explain`), *Type the command* (graded structurally via `shell::parse` so equivalent flags, quotes, and safe redirects match), *Read the output* (`outputs::describe`), and *Spot the danger* (`huh`'s 14 risk categories).
- **Live auto-checking hands-on practice labs**: Prepares a safe throwaway folder under `$XDG_DATA_HOME/huh/practice` (default `~/.local/share/huh/practice`) using pure `std::fs` with canonicalized symlink-escape guards (zero command execution). As you run real Linux commands in your shell beside the pane, `lab` mode monitors the sandbox and recorded output live, nudges gently with `huh why` on non-zero exits, ticks off checklist items automatically, awards 30 XP and the `hands_on` badge, celebrates with Pebble, and cleans up the folder when done (with manual `Enter`/`check`/`hint`/`reset` and check-question fallback if recording is off).
- **Kid-friendly 5-heart loop & spaced review**: Right answers earn XP and feed Pebble; misses re-queue once at the end of the lesson. Reaching 0 hearts never locks you out (`[r]` heart-free review, `[t]` fresh retry, `[q]` menu). Missed questions enter a 6-box Leitner deck (intervals `0, 1, 3, 7, 14, 30` days), paired with UTC daily streaks, streak freezes (start with 1, +1 per 7-day milestone, cap 3), `daily_goal = 50` in `config.toml`, and local progress in `$XDG_STATE_HOME/huh/progress.toml`.
- **Arrow-key navigation & live `/` commands**: On interactive TTYs (`src/tui.rs`, `src/learn.rs`, `src/pane.rs`), Lab unit/lesson/practice menus, multiple-choice questions, and out-of-hearts choices support inline `↑`/`↓` (plus `j`/`k`, `Home`/`End`) + `Enter` + `Esc` (`q`), alongside number/letter hotkeys and a `/` shortcut to invoke slash commands directly from any menu. Lab text prompts also use `LineEditor` for live `/` fuzzy completion while polling practice sandboxes, falling back to numbered plain-text prompts on non-TTY streams.

### Slash commands, fuzzy palette & persistent preferences

Slash commands work from **every** prompt in both `watch` and `lab` modes, always print instantaneously (never typewritten), and save live preferences atomically to `$XDG_STATE_HOME/huh/prefs.toml` (default `~/.local/state/huh/prefs.toml`) without ever modifying your user-owned `~/.config/huh/config.toml`:

| Slash command | Effect |
|---|---|
| `/mode [watch\|lab]`, `/watch`, `/lab` | Switch live between `watch` and `lab` modes (bare `/mode` opens a Watch/Lab arrow picker) |
| `/safety [normal\|expert]` | Switch safety guardrails live (`prefs.toml` `safety`) |
| `/theme [name]` | Open the 9-theme arrow picker (pre-selecting the active theme) or switch colour palette live and re-tint Pebble's 32×32 sprite (`prefs.toml` `theme`) |
| `/pet [status\|name <N>\|species <S>\|style <auto\|pixel\|ascii\|off>\|customize]` | View Pebble's card + dual-track unlocks, rename, pick/switch species (showing unlocked vs lock condition), pick/change render style, or export `~/.config/huh/sprites/<species>.toml` |
| `/speed [off\|fast\|normal\|slow\|<ms>]` | Open the typewriter speed picker or set budget (`off`, `fast` ~0.8s, `normal` ~2s, `slow` ~3.2s, or `0`–`5000` ms) |
| `/quiet [on\|off]` | Toggle per-program quiet deduplication live (`prefs.toml` `quiet`) |
| `/tips [on\|off\|next]` | Toggle, enable/disable, or cycle the Row-7 rotating header tip (`prefs.toml` `tips`) |
| `/find [words]` | Open [`huh find`](#find-a-command-or-tool) (suspends and restores the pinned header cleanly) |
| `/help` or `help` | Instant summary of pane inputs and slash commands |

- **Live `/` fuzzy dropdown (`src/slash.rs`, `src/tui.rs`)**: Typing `/` at the companion prompt opens a live inline dropdown (up to 6 rows) across all 11 slash commands (`/mode`, `/watch`, `/lab`, `/safety`, `/theme`, `/pet`, `/speed`, `/quiet`, `/tips`, `/find`, `/help`) plus Stage 2/3 sub-arguments (`/theme cat`, `/pet sp tur`, `/mode `, `/safety `, `/speed `, `/quiet `, `/tips `, `/lab `, `/pet style `). `Tab` completes and advances into Stage 2/3 sub-arguments, `↑`/`↓` navigates the dropdown (or cycles prompt history when closed), `Esc` closes the dropdown (or clears the line if already closed), and `Enter` submits or fills commands expecting text (such as `/pet name `).
- **Submit-time fuzzy resolution (Option A fallback) & Unix path guardrail**: Submitting directly (both in raw mode and cooked/non-TTY fallback) resolves abbreviations and 1-edit typos via `slash::resolve_submit`: unique matches print a dim `→ /canonical` note and run immediately (`/th cat` → `→ /theme catppuccin`, `/thme` → `→ /theme`), bare `/` or ambiguous prefixes (`/s`, `/t`) open an interactive picker (`tui::select_menu` on TTY, numbered list on non-TTY), and unknown slash tokens (`/finder`, `/modeled`) print a concise unknown-command notice without executing. Multi-segment Unix paths with a second `/` (`/etc/hosts`, `/usr/bin/env`) are never hijacked and stay normal input.
- **Inline arrow-key pickers**: Bare `/mode`, `/theme`, `/speed`, `/pet species`, and `/pet style` open `tui::select_menu` (`↑`/`↓` + `Enter` + `Esc`, plus hotkeys and `/`) on interactive TTYs while preserving plain-text output on non-TTY streams.

### Variable-rate typewriter

Narrative explanations inside the companion pane stream through an ANSI-safe variable-rate typewriter (`src/typewriter.rs`):

- **Adaptive pacing**: Targets a ~2s starter budget (`normal` = 2000 ms, `fast` = 800 ms, `slow` = 3200 ms, or `0–5000` ms). Short explanations pace character-by-character (16–35 ms/char) so they feel crisp rather than artificially dragged out; long blocks stream smoothly at ~60fps (16 ms ticks) to finish within budget.
- **Narrative only**: Applies to companion-pane explanations (`watch` command blocks, `huh explain`, `huh "..."`, `why`, Enter-replay, and `lab` lesson text). Slash commands, `?tools`, menus/prompts, and all one-shot CLI commands are always instant.
- **Instant flush & disable**: Pressing Enter (or arrival of a new watched command) flushes the rest of the text in one unbroken write. Disable or tune via `/speed`, `HUH_NO_TYPEWRITER=1`, `HUH_TYPEWRITER_MS=<ms>`, or `typewriter = false` / `typewriter = "fast"` / `typewriter_ms = 800` in `~/.config/huh/config.toml`.

## Setup and shell integration

### Guided setup (`huh setup`)

Run `huh setup` once for an Omarchy-style interactive walkthrough:

1. Detects your login shell (`bash`, `zsh`, or `fish`), shell rc path, and Kitty Graphics capability (Ghostty, Kitty, WezTerm, including `huh`'s `tmux -L huh` passthrough).
2. Prompts via inline arrow-key menus (`↑`/`↓` + `Enter`, or number hotkeys on interactive TTYs; Enter-for-Recommended numbered fallback when non-interactive) for default pane mode (`watch`, `lab`, `last`), theme, pet render style (`auto`, `ascii`, `off`), typewriter speed, and rotating header tooltips (`on` Recommended / `off`).
3. Saves choices atomically to `~/.local/state/huh/prefs.toml` while preserving any existing `~/.config/huh/config.toml` untouched.
4. Previews the exact shell integration lines and prompts for confirmation (arrow selector on TTY, `[y/N]` fallback on non-TTY) before inserting or updating idempotent marked blocks (`# >>> huh shell-init (record) >>>` and `# >>> huh shell-init (hooks) >>>`) in your shell's rc file.

### Terminal recording and prompt completion

To configure shell integration manually (for `huh why`, live `watch` mode, `Alt+h`/`Alt+f`, and `Alt+e`), add the two lines for your shell using `huh shell-init [--shell auto|bash|zsh|fish] [--hooks]`:

- **Bash** (`~/.bashrc`):

```bash
[[ $- != *i* ]] && return
eval "$(huh shell-init)"            # right after the interactive check, before anything that prints

# ... the rest of your ~/.bashrc ...

eval "$(huh shell-init --hooks)"    # last line
```

- **Zsh** (`${ZDOTDIR:-~}/.zshrc`):

```zsh
[[ -o interactive ]] || return
eval "$(huh shell-init --shell zsh)"

# ... the rest of your ~/.zshrc ...

eval "$(huh shell-init --shell zsh --hooks)"
```

- **Fish** (`~/.config/fish/config.fish`):

```fish
if status is-interactive
    huh shell-init --shell fish | source

    # ... the rest of your config.fish ...

    huh shell-init --shell fish --hooks | source
end
```

- **Prompt completion (`Alt+h`, `Alt+f`)**: Replaces the current prompt line with `huh`'s top suggestion (`huh --no-ask --first-line-only "..."`) without executing anything (customizable via `HUH_KEY`).
- **Explain before Enter (`Alt+e`)**: Shows `huh explain` for the line you are typing, below the prompt, without running it.
- **Terminal recording**: `huh shell-init` runs the shell inside util-linux `script`, streaming output through `huh record-sink` to a private 1 MB ring buffer in `$XDG_RUNTIME_DIR/huh/<pid>.log` (`0600` permissions, memory-backed, gone at logout).
- **Skipping & opt-out**: `huh ...` commands are skipped in hooks so asking about a failure does not overwrite it. Disable recording for a shell with `HUH_NO_RECORD=1`.

## Platforms, missing programs, and catalog

`src/platform.rs` is the one place that knows what system this is and how it installs software. It feeds the
catalog, the `NOTE:` line under a suggestion and the install hints of `huh why`, so they cannot disagree.

**Detecting the system.** The `ID` and `ID_LIKE` words of `/etc/os-release` (then `/usr/lib/os-release`) are
mapped to a family: `arch` (Manjaro, EndeavourOS, Artix, CachyOS, ...), `omarchy` (extends arch), `debian`
(Ubuntu, Mint, Pop, elementary, Zorin, Kali, ...), `fedora` (RHEL, Rocky, Alma, Amazon, ...), `suse`, `alpine`,
`void`, `gentoo`, `nixos`, and `nix` (the Nix package manager on some other system, with no NixOS in os-release:
it differs only in updating with `nix profile upgrade --all` instead of rebuilding the system). A derivative that only names its parent (`ID_LIKE=ubuntu`) still lands in `debian`. When
os-release is missing or lists nothing known, the package manager binary found on PATH decides (`pacman`, `apt`,
`dnf`, `zypper`, `apk`, `xbps-install`, `emerge`, `nix`).

**Package tasks work on every manager.** The catalog has one entry each for "package install", "package remove",
"package search", "system update", "package owns file", "package provides program", "package installed" (`{pkg_query}`:
`pacman -Q vlc`, `dpkg -s vlc`, `rpm -q vlc`, ...), "package list" (`{pkg_list}`) and "package cache clean"
(`{pkg_clean}`: `sudo pacman -Sc`, `sudo apt autoremove && sudo apt clean`, `sudo dnf clean all`, ...; marked destructive).
Each family sets them as `query`, `list` and `clean` next to `install`, `remove`, ... in `data/platforms.toml`. Their templates are
`{pkg_install}` and friends, filled from the family's commands in `data/platforms.toml`
(`sudo apt install`, `sudo zypper install`, `sudo apk add`, `sudo xbps-install -S`, `sudo emerge --ask`,
`nix profile install nixpkgs#...`, `omarchy pkg add` on Omarchy, ...). A task a family cannot do (no "which package
owns this file" on NixOS) is simply not offered there. `yay` and `omarchy pkg aur add` stay Arch and Omarchy
entries (`platform = "arch"` / `"omarchy"`).

**Names you type.** "install dig" or "install docker" is a program name, not a package name: when
`data/platforms.toml` knows the program, huh installs the family's package (`bind9-dnsutils`, `bind-utils`,
`bind-tools`, `docker.io`, ...). A package the family does not carry (`""` in `[packages]`, or no row for a known
program) turns the line into the provides search with a note saying so, and a package that only some releases lack
(`eza` on Debian 11/12 and Ubuntu before 24.04, `btop` and `gh` on Debian 11 and Ubuntu 20.04) keeps its name and
gets the `[notes]` remark. Names you type that the data does not know are used as typed. "remove htop" (a bare
program or package name, not a file in the folder) is a package removal; "remove old.log" and "delete the build
folder" still delete files. Filler like "a package called" never ends up in the value. If no package manager is
detected at all (busybox), package requests answer "could not detect a package manager" instead of guessing.

**Missing programs.** A suggestion whose program is not installed gets a `NOTE:` line. The package name is looked up
per family, so the hint is `sudo apt install network-manager` for `nmcli` on Debian and
`sudo dnf install iproute` for `ip` on Fedora. The rule for a program huh has no row for: a program name is used as
the package name only when `data/platforms.toml` says so (the `same` list holds the ones that match on every distro,
such as `htop`, `tmux`, `git`; `[packages]` rows cover the rest). Anything else gets the system's own search
instead of a guess: `pacman -F <prog>`, `apt-file search bin/<prog>`, `dnf provides '*/bin/<prog>'`,
`zypper search --provides <prog>`, `apk search -e cmd:<prog>`, `xlocate <prog>`, `e-file <prog>`,
`nix-locate bin/<prog>`. Package managers and distro tools (`pacman`, `apt`, `omarchy`, `yay`, ...) are never
offered for installation with another distro's manager, even with `--all-commands`. huh only prints these
commands; it never runs them.

**Finding programs.** A program counts as installed when an executable file (exec bit set, symlinks followed) is on
`$PATH`; a leading `~` in a PATH entry is expanded. `/usr/local/sbin`, `/usr/sbin` and `/sbin` are searched too, but
only for suggestions that run through `sudo` (sudo has those directories in its own PATH; a normal user's PATH on
Debian does not). A tool found only in sbin and suggested without `sudo` gets a note that it is not on PATH.
Shell built-ins (`cd`, `kill`, `history`, ...) are always available.

**Adding a distro or package.** Edit `data/platforms.toml` (compiled in), or put the same kind of file at
`~/.config/huh/platforms.toml` (or point `$HUH_PLATFORMS` at one) to extend it without rebuilding. Entries are merged
over the built-in data:

```toml
same = ["frobnicate"]                  # package has this name on every distro

[families.mylinux]                     # a new distro: no catalog files needed
ids = ["mylinux", "mylinux-lts"]       # os-release IDs
detect = ["mypm"]                      # package manager binary, for unknown IDs
install = "sudo mypm add"              # `{}` marks the argument; otherwise it is appended
remove = "sudo mypm del"
search = "mypm find"
update = "sudo mypm upgrade"
owns = "mypm owner"                    # optional
provides = "mypm who-provides {}"      # optional, used for unknown programs

[packages]
nmcli = { mylinux = "nm-cli" }         # package name where it differs (omit a family to say "unknown")
```

**Other**
- Catalog entries can set `platform` to any family name (or an os-release ID). Only entries matching this system are offered by default; `huh --all-commands` offers the rest.
- **Questions and instructions** (`kind`, `opposites`). huh reads the opening of the request: "is/are/do i have/what/which/who/where/how much/check if" is a question about state; "can you", "please", "how do i", "i want", "what command" and plain imperatives are instructions. A question about state strongly demotes entries that change the system, so "is vlc installed" gets `pacman -Q vlc` and not an install, and "what wifi am i connected to" gets a query and not `nmcli ... connect`; an instruction mildly demotes a bare query when an action fits as well (unless it asks to show, list or check). Each entry has `kind = "query"` (shows, lists, checks) or `kind = "action"` (starts, stops, sets, installs, deletes, connects). Left out, it is derived from the name and template (destructive entries and names or templates with words like start, stop, set, install, toggle, connect, power are actions); set it where the guess is wrong. `kind = "mixed"` marks an entry whose options are both (`tar` lists or creates) and is never demoted. `opposites = ["sleep", ...]` on an entry, or on a choice option, lists words that rule it out when the request says them without a negation ("put it to sleep" rules out `stay-awake`; "stop it from going to sleep" does not): the entry is dropped when the option the matcher would pick has an opposite in the request. A minimum-evidence floor rejects a match that rests only on generic words ("order" for `sort`, "make" for `cp`, "last" for `tail`) or on a program name many entries share ("git"). `evals/intent.toml` holds these cases and must stay at 100% in `cargo test`.
- All 310 catalog entries (`catalog/*.toml`) are embedded into the binary at compile time via `build.rs`, so `huh` runs from any directory (including everyday gaming, app, and media tools such as `dosbox`, `flatpak` install/run/search/list/uninstall, `mpv`, `appimage run` for AppImages without FUSE, and expanded `xdg-open`, paired with `wine` hints in `src/hints.txt`, `dosbox`/`mpv`/`wine`/`fuse2` package mappings in `data/platforms.toml`, and domain-coherence guards in `src/matcher.rs`). Override lookup order: `--catalog`, `$HUH_CATALOG`, `~/.config/huh/catalog`, then the embedded catalog.
- **Safety**: a suggestion that deletes data, powers off or restarts the machine, stops services or processes, or rewrites git history prints a `WARNING (category):` line saying what is lost or opened up. A catalog entry sets `destructive = true` and `warning = "..."`; a single option (`[[slots.options]]`) or flag can carry its own `warning`, so `systemctl status` is quiet while `systemctl stop` warns. `shutdown` and `reboot` honour a time ("reboot in 10 minutes", "shut down at 23:00", "cancel the shutdown"). Requests to undo or recover something are never answered with a delete, and "what does rm -rf do" points to `huh explain "rm -rf"` instead of suggesting `rm`. `evals/safety.toml` (cases may set `warn = true/false`) must stay at 100% in `cargo test`.
- Slot readers (`extract = "..."` on a slot, `src/extract.rs`) now also include `percent` (`50%`), `device` (`/dev/sdb1`), `mac`, `filename`, `local_port`/`remote_port`, `json`, `user_name`, `image_tag`, `rebase_target`, `context_dir` and `versioned_program`.
- **Never mislead** (`src/guard.rs`, `evals/mislead.toml`): a wrong answer is worse than "No suggestion", so huh abstains, with a short reason, in these cases. (1) A question about state ("is ssh running", "what's my volume", "sound isnt working") is never answered with an action entry; if no query fits, nothing is suggested. (2) A dangerous suggestion (one that prints a WARNING) needs the verb and something to act on: a named value, a typed pattern or mode, or at least the kind of thing ("close a program"); "delete stuff" and "remove it" get nothing. (3) A common word the request does not present as a name ("slow", "synced", `PATH` as a file, the command's own name) stays a `<placeholder>`. (4) A copy or move onto a file that exists warns that it overwrites; `chmod 777` and `-R 777` warn that everyone can change it; a typed mode (777, 755, 600) is kept and "only i can read it" means 600. (5) "is X dangerous", "is it safe to run X" and "should i run X" point to `huh explain "X"`; hacking someone else's wifi or account, formatting or wiping a drive, and installing an operating system get no command. (6) A line made only of placeholders never claims more than 60%.
- `evals/platforms.toml` holds the package requests for every non-Arch family (`platform = "debian"` on each case); `cargo test` runs all of them, or `huh eval --cases evals/platforms.toml --platform <family>`.
- `huh eval --platform <ids>` scores as another system (`debian`, `fedora`, `suse`, `alpine`, `void`, `gentoo`, `nixos`, `arch,omarchy`, ...).

## Layout

| File | Role |
|---|---|
| `build.rs` | Embeds sorted `catalog/*.toml`, `lessons/*.toml`, and `sprites/*.toml` files at compile time |
| `src/main.rs` | CLI (`huh`), request pipeline (`suggest`), `explain`, `why`, `setup`, and companion-pane launch entry points |
| `src/matcher.rs` | `LocalMatcher`: BM25F index, stems/concepts, domain-coherence guards; `rank` orders the catalog, `details` scores fit, flags and slot values |
| `src/candidates.rs` | Request token/phrase/size/extension extraction and `$PWD` entries |
| `src/sysinfo.rs` | Read-only user, group, shell and service-unit lists for typed slots (fixture: `tests/fixtures/sysinfo`) |
| `src/readers.rs` | Readers for the sysadmin entries (`extract = "sys_..."`): refs, stash messages, container paths, substitutions, and the `crontab -e` line |
| `src/assemble.rs` | Decision thresholds, shortlisting, slot filling, shell quoting, and suggestion `parts` |
| `src/platform.rs` | System detection (os-release, then package manager on PATH), package managers and names, PATH lookup |
| `data/platforms.toml` | Package manager commands and per-family package names (including `dosbox`, `mpv`, `wine`, `fuse2`; embedded; `~/.config/huh/platforms.toml` extends it) |
| `src/catalog.rs` | Catalog loading, validation, platform filtering, filling `{pkg_*}` placeholders, `NOTE:` text |
| `src/doctor.rs` | Deterministic Error Doctor (`huh why`), including AppImage FUSE 2 (`libfuse.so.2`) and `Exec format error` (`.exe`/DOSBox, foreign binaries) diagnoses |
| `src/explain.rs` | `huh explain` entry point (`explain_command`) |
| `src/shell.rs` | Bash-subset lexer/parser for `huh explain` (never panics, never runs anything) |
| `src/breakdown.rs` | The explainer engine: walks the parse tree, explains flags and arguments, recurses into nested commands |
| `src/knowledge.rs`, `src/knowledge.txt` | Curated per-program table of options, subcommands and argument roles (291 `@` entries, including `@dosbox`, `@flatpak`, `@mpv`, `@vlc`, `@gio`, `@nano`, `@ffmpeg`, `@huh`) |
| `src/manpage.rs` | Reads option descriptions from man pages on disk (`flate2` for `.gz`) |
| `src/decode.rs` | chmod modes, `find` tests, signals, docker ports/volumes, sed/awk programs, git refs |
| `src/risk.rs` | Warnings for `huh explain`: deletes, overwrites, history rewrites, privilege, remote code, installs, long builds |
| `src/risk_extra.rs` | More warnings, read from the plain words of the line (so they work without a knowledge section): TLS and host-key checks, exposed services, secrets, traces, shared things |
| `src/risk_kinds.rs` | The 14 risk categories and their one-line meanings |
| `src/render.rs` | 80-column output of the explanation |
| `evals/explain.toml` | 70+ real-world command lines with expected text and warnings (run by `cargo test`) |
| `evals/everyday.toml` | 17 everyday desktop, gaming, media, and `wine` hint cases (`dosbox`, `flatpak`, `mpv`, `appimage run`, `xdg-open`, `wine`; run by `cargo test`) |
| `src/session.rs` | Terminal recording (`script` + `record-sink`), bash/zsh/fish shell hooks, prompt key bindings |
| `src/setup.rs` | `huh setup`: Omarchy-style interactive setup (`tui::select_menu` on TTY), Kitty detection, rotating-tips preference, and idempotent bash/zsh/fish rc block installation |
| `src/outputs.rs`, `src/outputs.txt` | Output explanations (`lsblk`, `df`, `git status` ...) for `cmd \| huh`; cases in `evals/outputs.toml` (run by `cargo test`) |
| `src/launch.rs` | Side-pane launcher (`tmux` split, tiling window, private `tmux -L huh` with `allow-passthrough on`) and `fzf` runner — the only module that starts a program |
| `src/pane.rs` | Unified `huh` companion pane (`watch` + `lab` modes), `poll` event loop, `LineEditor` + arrow-key slash pickers, fuzzy slash execution, dual-track pet observation, and interactive prompt |
| `src/tui.rs` | Zero-crate Linux raw-mode key decoder, inline `select_menu` (`↑`/`↓`/`Enter`/`Esc`/hotkeys/`/`), and live `/` fuzzy dropdown `LineEditor` with 100-item history |
| `src/slash.rs` | Definitions for all 11 slash commands, Stage 1–3 fuzzy/typo completions, submit-time fuzzy resolution, and Unix path guardrails |
| `src/tips.rs` | 18 rotating header usage tips (`~15 s` cadence) and `HUH_NO_TIPS=1` > `config.toml` > `prefs.toml` precedence |
| `src/prefs.rs` | Atomic state preferences (`~/.local/state/huh/prefs.toml`, including `tips`) and initial pane mode precedence (`--mode` > `config.toml` > `prefs.toml`) |
| `src/typewriter.rs` | ANSI-safe variable-rate typewriter streaming (~2s starter budget, 60fps chunking, instant flush on Enter) |
| `src/pin.rs` | Pinned DECSTBM companion header (`L2b` logo + `[WATCH]`/`[LAB]` status bar + Row-7 rotating tips), Kitty Graphics lifecycle, and flicker-free in-place refresh |
| `src/pet.rs`, `src/date.rs` | `#!` terminal pet state machine (**Pebble**, **Anabee**, **Vector**, **Hiss**, **Bóbr**, **Flow**), animated 4×10 ASCII fallback, watch/lab dual-track unlocks across 9 species, lazy decay, atomic persistence, UTC civil dates |
| `src/sprite.rs`, `sprites/*.toml` | Embedded 32×32 pixel sprite sheets (`penguin`, `cat`, `beaver`, `turtle`, `dolphin`, `bear`, `snake`, `bee`, `owl`), theme tinting, Kitty Graphics APC encoding (with tmux DCS wrapping), and `~/.config/huh/sprites/*.toml` overrides |
| `src/learn.rs` | `/mode lab` interactive course runner (`LabHost` with `select_menu` arrow navigation), non-linear `unlock_after` prerequisite tracks (`real-world` side quests), structural command grader, 4 question types, 5-heart loop, live auto-checking practice runner, spaced review |
| `src/practice.rs` | Safe throwaway practice sandbox (`$XDG_DATA_HOME/huh/practice`), symlink-guarded filesystem checks, recorded output + fallback evaluator |
| `src/lessons.rs`, `lessons/*.toml` | Embedded 11-unit curriculum (8 core + 3 real-world tracks unlocking after Unit 1; 33 lessons, 165 questions, 9 practice labs), `unlock_after` dependency/cycle validation, distro-aware `{pkg_*}` resolution |
| `src/progress.rs`, `src/srs.rs` | Local XP, daily goals, UTC streaks with freezes, 19 badges (`shell_grad` for core Units 1–8), unlock checks, practice tracking, and 6-box Leitner spaced repetition |
| `src/style.rs`, `src/welcome.rs` | Colour themes (`babyblue` default, live `set()` switching) and `L2b` welcome logo animation |
| `src/find.rs` | `huh find`: fuzzy search over catalog entries and installed programs |
| `src/eval.rs` | Evaluation runners (`eval` and `eval-errors`) |
| `src/mode.rs` | Normal/expert mode and its config loader |
| `src/tools.rs` | `huh tools`: PATH scan and man page descriptions |

## Eval results

Measured with `cargo run --release` on 2026-11-17 (217 catalog entries):

| Suite | Cases | Right command first | In top 3 | Exact command line | "Nothing fits" | Median time |
|---|---|---|---|---|---|---|
| `evals/cases.toml` | 238 | 229/229 (100%) | 229/229 (100%) | 224/224 (100%) | 9/9 (100%) | 0 ms |
| `evals/heldout.toml` | 138 | 133/133 (100%) | 133/133 (100%) | 128/128 (100%) | 5/5 (100%) | 0 ms |
| `evals/safety.toml` | 110 | 100% | 100% | 100% | 100% | 0 ms |
| `evals/slots.toml`, `evals/slots2.toml` | 52 + 51 | slot values: 100% exact line | | | | 0 ms |
| `evals/sysadmin.toml` | 120 | 120/120 (100%) exact line, warnings included | | | | 0 ms |
| `evals/expert.toml` (cases tagged `mode = "expert"` run in expert mode) | 45 | 100% (answers, refusals, warnings) | | | | 0 ms |
| `evals/debian.toml` (`--platform debian`) | 5 | 4/4 (100%) | 4/4 (100%) | 4/4 (100%) | 1/1 (100%) | 0 ms |

Error Doctor (`eval-errors`, release build):

| Suite | Cases | Names the cause | Suggested command fits | Median time |
|---|---|---|---|---|
| `evals/errors.toml` | 30 | 30/30 (100%) | 30/30 (100%) | 0 ms |
| `evals/errors-heldout.toml` | 12 | 12/12 (100%) | 12/12 (100%) | 0 ms |
| `evals/errors-real.toml` | 67 | 67/67 (100%) | 67/67 (100%) | 0 ms |

## Tools

- `tools/seed.mjs` drafts a catalog entry from a Fig completion spec (MIT). All catalog entries in `catalog/` were written and curated by hand.
- `tools/nl2bash.py` builds a local breadth sample from NL2Bash (GPL-3.0; git-ignored).

## License

MIT — see [LICENSE](LICENSE).
