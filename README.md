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

The pipeline keeps the clean two-stage structure (`Questions` -> `Answers` -> `assemble::build`) driven by a local deterministic matcher (`LocalMatcher`) instead of a hosted or local LLM:

1. **Command matching**: Multi-field BM25F over command names, plain-language descriptions, curated `keywords`, flags, slots, and template literals, paired with stem/synonym normalization, domain-coherence fit checks, and an `installed()` tie-breaker.
2. **Slot & flag assembly**: Extracts candidate tokens, quoted phrases, multi-word capitalized names, number words (`third` -> `3`), bare extensions (`.rs` -> `*.rs`), size expressions (`50MB` -> `50M`), and non-hidden entries in the current folder, then fills the template and attaches a part-by-part explanation for the top suggestion.
3. **Reverse command explainer (`huh explain`)**: Quote-aware shell tokenizer splits pipelines (`|`, `&&`, `||`, `;`), redirections (`>`, `>>`, `<`, `2>&1`), `sudo`, combined short flags (`-sh`, `-la`), formatted slots, choice options, and positional arguments against the catalog, flagging anything unknown as `not in the catalog` and warning on destructive commands.
4. **Deterministic Error Doctor (`huh why`)**: Combines exit code meanings, live machine checks (`$PATH` typo scan with Damerau-Levenshtein distance, `metadata()` executable check, `/etc/group`, `/var/lib/pacman/db.lck`, `/var/run/docker.sock`, `$PWD` typo scan, `/etc/os-release` package manager detection), and structured error rules.

## Usage

```bash
cargo build --release
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

```bash
$ huh explain "du -sh Downloads/* | sort -h"
  du            Show how much disk space a folder or file takes up
  -s            summarize: display only a total for each argument
  -h            human-readable: print sizes in easy-to-read units (e.g. 1K, 234M, 2G)
  Downloads/*   the folder or file to measure (wildcard matching all items inside Downloads)
  |             pipes standard output from the previous command into the next command
  sort          Put the lines of a file in alphabetical or numerical order
  -h            human-numeric-sort: compare human-readable numbers such as 2K or 1G
```

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

The command `jq` is not installed (`command not found`, exit code 127). You can also install it with `omarchy pkg add jq`.
Try: sudo pacman -S jq

(Nothing was run.)
```

### Other commands

```bash
huh check                                            # validate the catalog; "-" marks commands not installed here
huh eval                                             # score evals/cases.toml
huh eval --cases evals/heldout.toml                  # score the held-out suite
huh eval --cases evals/debian.toml --platform debian # score Debian platform filtering
huh eval-errors                                      # score evals/errors.toml
huh eval-errors --cases evals/errors-heldout.toml    # score held-out error cases
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

- Catalog entries can set `platform = "arch"`, `"debian"`, `"fedora"`, or `"omarchy"`. Only entries matching `ID`/`ID_LIKE` in `/etc/os-release` are offered by default.
- Commands whose program is not installed are still suggested so a missing tool does not push a worse command to the top; instead `huh` prints `NOTE: \`<program>\` is not installed. Install it with: <install command>` using platform and package overrides (`smartmontools` for `smartctl`, `libvirt` for `virsh`, `iproute2` for `ss`/`ip`, `zbar` for `zbarimg`, `upower` for `upower`).
- All 203 catalog entries (`catalog/*.toml`) are embedded into the binary at compile time via `build.rs`, so `huh` runs from any directory. Override lookup order: `--catalog`, `$HUH_CATALOG`, `~/.config/huh/catalog`, then the embedded catalog.

## Layout

| File | Role |
|---|---|
| `build.rs` | Embeds sorted `catalog/*.toml` entries at compile time |
| `src/main.rs` | CLI (`huh`), request pipeline (`suggest`), `explain` & `why` entry points, output formatting |
| `src/matcher.rs` | `LocalMatcher`: BM25F index, stems/concepts; `rank` orders the catalog, `details` scores fit, flags and slot values |
| `src/candidates.rs` | Request token/phrase/size/extension extraction and `$PWD` entries |
| `src/assemble.rs` | Decision thresholds, shortlisting, slot filling, shell quoting, and suggestion `parts` |
| `src/catalog.rs` | Catalog loading, validation, platform filtering, install hints |
| `src/doctor.rs` | Deterministic Error Doctor (`huh why`) |
| `src/explain.rs` | Command line explainer (`huh explain`) |
| `src/session.rs` | Terminal recording (`script` + `record-sink`), shell hooks, readline binding |
| `src/eval.rs` | Evaluation runners (`eval` and `eval-errors`) |

## Eval results

Measured with `cargo run --release` on 2026-11-17 (203 catalog entries; 195 offered on Arch/Omarchy, 179 installed):

| Suite | Cases | Right command first | In top 3 | Exact command line | "Nothing fits" | Median time |
|---|---|---|---|---|---|---|
| `evals/cases.toml` | 230 | 221/221 (100%) | 221/221 (100%) | 216/216 (100%) | 9/9 (100%) | 0 ms |
| `evals/heldout.toml` | 134 | 129/129 (100%) | 129/129 (100%) | 124/124 (100%) | 5/5 (100%) | 0 ms |
| `evals/debian.toml` (`--platform debian`) | 5 | 4/4 (100%) | 4/4 (100%) | 4/4 (100%) | 1/1 (100%) | 0 ms |

Error Doctor (`eval-errors`, release build):

| Suite | Cases | Names the cause | Suggested command fits | Median time |
|---|---|---|---|---|
| `evals/errors.toml` | 30 | 30/30 (100%) | 30/30 (100%) | 0 ms |
| `evals/errors-heldout.toml` | 12 | 12/12 (100%) | 12/12 (100%) | 0 ms |

## Tools

- `tools/seed.mjs` drafts a catalog entry from a Fig completion spec (MIT). All catalog entries in `catalog/` were written and curated by hand.
- `tools/nl2bash.py` builds a local breadth sample from NL2Bash (GPL-3.0; git-ignored).

## License

MIT — see [LICENSE](LICENSE).
