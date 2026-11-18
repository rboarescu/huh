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

## How it works

A local deterministic matcher (`LocalMatcher`) ranks the catalog (`rank`) and scores each shortlisted command's fit, flags and values (`details`); `assemble::build` turns that into the command line:

1. **Command matching**: Multi-field BM25F over command names, plain-language descriptions, curated `keywords`, flags, slots, and template literals, paired with stem/synonym normalization, domain-coherence fit checks, and an `installed()` tie-breaker.
2. **Slot & flag assembly**: Extracts candidate tokens, quoted phrases, multi-word capitalized names, number words (`third` -> `3`), bare extensions (`.rs` -> `*.rs`), size expressions (`50MB` -> `50M`), and non-hidden entries in the current folder, then fills the template and attaches a part-by-part explanation for the top suggestion.
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
  ! `-delete` (in `find . -name '*.log' -mtime +7 -delete`): deletes every match
    permanently, with no confirmation (no trash). Run the same command without
    -delete first to see what would go.
```

```bash
huh explain "curl -fsSL https://example.com/install.sh | sudo bash -s -- --yes"
huh explain - < deploy.sh          # a whole script, `\` continuations included
pbpaste | huh explain              # no argument and piped input: reads standard input
```

- **Parser**: pipes (`|`, `|&`), lists (`&&`, `||`, `;`, `&`), `$(...)`, backticks, `${VAR}`, `$((...))`, `<(...)`, subshells, `{ ...; }`, `for`/`while`/`until`/`if`/`case`, functions, `[[ ... ]]`, assignments (`A=1 cmd`), all redirections (`>`, `>>`, `2>&1`, `&>`, `<`, `<<`, `<<<`, `>|`), heredocs (bodies are captured, not run) and `\` line continuations. It never panics: unbalanced quotes and brackets are parsed best-effort and reported under NOTES.
- **Recursion**: the inner command of `$(...)`, `bash -c '...'`, `sh -c`, `sudo`, `env`, `xargs`, `nohup`, `time`, `watch`, `timeout`, `find -exec`, `docker run/exec` (marked "inside the container") and `ssh host '...'` (marked "on the remote computer") is explained too, indented below.
- **Flags are explained per program**, from three sources in order: the curated table (`src/knowledge.txt`, about 150 programs and subcommands: git, docker, find with `+N`/`-N` decoded, tar, ssh, sed, awk, chmod with octal and symbolic modes decoded, kill signals, systemctl, ...), then the catalog, then the program's **man page read from disk** (`/usr/share/man/man{1,8}`, plain or `.gz`; `.TP`, `.IP`, `.PP` and mdoc `.It` layouts), which is marked "(from the man page)". Combined short flags (`-xzvf`) are split per program; single-dash long options (`find -name`) are not.
- **Warnings**: deletes (`rm`, `find -delete`, `git clean`, `docker prune`, `dd of=`, `mkfs`, ...), overwrites (`>`, `sed -i`, `tee`, `cp`/`mv`), history rewrites (`git reset --hard`, `push --force`, `rebase`), privilege (`sudo`, `chmod 777`, `chown -R` on system folders), remote code (`curl | sh`, `bash <(curl ...)`), package installs (`pip --break-system-packages`, `npm -g`, `make install`), `kill -9` of computed PIDs, an unset variable in `rm "$DIR"/*`, and things that start a long build or run until Ctrl+C.
- huh does not open scripts or Makefiles named in the command, so for `./run.sh` or `make install` it says so instead of guessing.

### Explain a failed command (`huh why` or bare `huh`)

Without any shell setup, pipe a command's output directly into `huh` or `huh why`:

```bash
some-command 2>&1 | huh
some-command 2>&1 | huh why
```

In a recorded terminal (see below), typing `huh why` — or just `huh` after a non-zero exit code — reads the last command and its tail output (up to 40 lines / 4,000 characters) and prints the diagnosis:

```
$ jq . data.json
bash: jq: command not found
$ huh
Last command: jq . data.json (exit code 127)

The command `jq` is not installed (`command not found`, exit code 127).
Try: omarchy pkg add jq

(Nothing was run.)
```

`huh why` says so when it does not know: output with no error line and no exit code is reported as "does not look like an error" (with what it seems to be), and an unrecognised failure quotes the line that reports it instead of guessing a fix. Refused access to a file is checked against the file itself (owner and mode, read-only), so `sudo` is suggested only for other users' or system files; a missing Python or node module that looks like a typo is called one rather than installed.

### Other commands

```bash
huh check                                            # validate the catalog; "-" marks commands not installed here
huh eval                                             # score evals/cases.toml
huh eval --cases evals/heldout.toml                  # score the held-out suite
huh eval --cases evals/debian.toml --platform debian # score Debian platform filtering
huh eval-errors                                      # score evals/errors.toml
huh eval-errors --cases evals/errors-heldout.toml    # score held-out error cases
huh eval-errors --cases evals/errors-real.toml       # score failures from the real-world test
```

## Terminal recording and prompt completion

Add two lines to `~/.bashrc` (bash) to enable `huh why` without piping and `Alt+h` / `Alt+f` prompt-line replacement:

```bash
[[ $- != *i* ]] && return
eval "$(huh shell-init)"            # right after the interactive check, before anything that prints

# ... the rest of your ~/.bashrc ...

eval "$(huh shell-init --hooks)"    # last line
```

- **Prompt completion (`Alt+h`, `Alt+f`)**: Pressing `Alt+h` (or `Alt+f`, configurable with `HUH_KEY`) replaces the current prompt line (`READLINE_LINE`) with `huh`'s top suggestion (`huh --no-ask --first-line-only "$READLINE_LINE"`) without executing anything.
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
"package search", "system update", "package owns file" and "package provides program". Their templates are
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
- All 195 catalog entries (`catalog/*.toml`) are embedded into the binary at compile time via `build.rs`, so `huh` runs from any directory. Override lookup order: `--catalog`, `$HUH_CATALOG`, `~/.config/huh/catalog`, then the embedded catalog.
- `evals/platforms.toml` holds the package requests for every non-Arch family (`platform = "debian"` on each case); `cargo test` runs all of them, or `huh eval --cases evals/platforms.toml --platform <family>`.
- `huh eval --platform <ids>` scores as another system (`debian`, `fedora`, `suse`, `alpine`, `void`, `gentoo`, `nixos`, `arch,omarchy`, ...).

## Layout

| File | Role |
|---|---|
| `build.rs` | Embeds sorted `catalog/*.toml` entries at compile time |
| `src/main.rs` | CLI (`huh`), request pipeline (`suggest`), `explain` & `why` entry points, output formatting |
| `src/matcher.rs` | `LocalMatcher`: BM25F index, stems/concepts; `rank` orders the catalog, `details` scores fit, flags and slot values |
| `src/candidates.rs` | Request token/phrase/size/extension extraction and `$PWD` entries |
| `src/assemble.rs` | Decision thresholds, shortlisting, slot filling, shell quoting, and suggestion `parts` |
| `src/platform.rs` | System detection (os-release, then package manager on PATH), package managers and names, PATH lookup |
| `data/platforms.toml` | Package manager commands and per-family package names (embedded; `~/.config/huh/platforms.toml` extends it) |
| `src/catalog.rs` | Catalog loading, validation, platform filtering, filling `{pkg_*}` placeholders, `NOTE:` text |
| `src/doctor.rs` | Deterministic Error Doctor (`huh why`) |
| `src/explain.rs` | `huh explain` entry point (`explain_command`) |
| `src/shell.rs` | Bash-subset lexer/parser for `huh explain` (never panics, never runs anything) |
| `src/breakdown.rs` | The explainer engine: walks the parse tree, explains flags and arguments, recurses into nested commands |
| `src/knowledge.rs`, `src/knowledge.txt` | Curated per-program table of options, subcommands and argument roles |
| `src/manpage.rs` | Reads option descriptions from man pages on disk (`flate2` for `.gz`) |
| `src/decode.rs` | chmod modes, `find` tests, signals, docker ports/volumes, sed/awk programs, git refs |
| `src/risk.rs` | Warnings: deletes, overwrites, history rewrites, privilege, remote code, installs, long builds |
| `src/render.rs` | 80-column output of the explanation |
| `evals/explain.toml` | 70+ real-world command lines with expected text and warnings (run by `cargo test`) |
| `src/session.rs` | Terminal recording (`script` + `record-sink`), shell hooks, readline binding |
| `src/eval.rs` | Evaluation runners (`eval` and `eval-errors`) |

## Eval results

Measured with `cargo run --release` on 2026-11-17 (195 catalog entries; all offered on Arch/Omarchy, 180 installed):

| Suite | Cases | Right command first | In top 3 | Exact command line | "Nothing fits" | Median time |
|---|---|---|---|---|---|---|
| `evals/cases.toml` | 238 | 229/229 (100%) | 229/229 (100%) | 224/224 (100%) | 9/9 (100%) | 0 ms |
| `evals/heldout.toml` | 138 | 133/133 (100%) | 133/133 (100%) | 128/128 (100%) | 5/5 (100%) | 0 ms |
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
