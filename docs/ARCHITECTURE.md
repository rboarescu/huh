# `huh` Architecture & Technical Reference

This document holds the deep internal reference for contributors: matching and slot extraction, safety rules, platform package resolution, terminal recording, themes/preferences separation, evaluation harnesses, and full module layout.

For installation and day-to-day usage, see the main [README](../README.md).

---

## Core Design & Hard Invariants

1. **Zero LLM, zero network, 5 crates total:** `Cargo.toml` depends only on `anyhow`, `clap`, `flate2`, `serde`, and `toml`. All command specs, man-page tables, lessons, and 32×32 pet sprite sheets compile directly into the binary via `build.rs`.
2. **Subprocess isolation (`launch::tests::only_launch_rs_starts_processes`):** A compile-time test scans every production source file and fails if any module other than [`src/launch.rs`](../src/launch.rs) references `process::Command` or spawns external processes (`tmux`, terminal emulators, or `fzf`).
3. **Symlink-safe practice sandboxes ([`src/practice.rs`](../src/practice.rs)):** Hands-on Lab sandboxes live under `$XDG_DATA_HOME/huh/practice` and perform pure `std::fs` setup and verification with canonicalized path checks rejecting `..`, absolute paths, and symlink escapes.
4. **Exclusive scroll-region cursor ownership ([`src/pin.rs`](../src/pin.rs)):** Only `pin.rs` emits `ESC 7` / `ESC 8` to manage the DECSTBM pinned header and Kitty Graphics sprite lifecycle; [`src/tui.rs`](../src/tui.rs) uses purely relative cursor movements for inline menus and live `/` dropdowns.
5. **Never execute suggestions:** Suggested commands, `huh explain` inputs, and Lab `type_command` answers are parsed statically into an AST by [`src/shell.rs`](../src/shell.rs) and never evaluated in a shell.

---

## How the Four Engines Work

### 1. Request Matching & Slot Assembly (`matcher.rs`, `candidates.rs`, `assemble.rs`)

- **Command ranking (`LocalMatcher`):** Multi-field BM25F index across command `name`, `description`, curated `keywords`, `flags`, `slots`, and template literals, combined with stem/synonym normalization, domain-coherence rules (`passes_domain_coherence`), intent bonuses, and an installed-program tie-breaker.
- **Candidate & value extraction (`candidates.rs`, `extract.rs`, `values.rs`, `readers.rs`):** Extracts quoted strings, paths, bare extensions (`.rs` → `*.rs`), sizes (`50MB` → `50M`), percentages, ports, devices (`/dev/sdb1`), MACs, JSON payloads, non-hidden `$PWD` entries, and exclusions after *"excluding"*, *"except"*, *"without"*, *"but not"*.
- **Read-only system introspection (`src/sysinfo.rs`):** Reads `/etc/passwd`, `/etc/group`, `/etc/shells`, and systemd `*.service` unit files only (never spawns tools to query Docker or package DBs). Override directory with `HUH_SYSINFO_DIR=<folder>` (evals and tests use `tests/fixtures/sysinfo`).
- **Thresholding & honest abstention (`src/guard.rs`, `evals/mislead.toml`):**
  1. Questions about state (`"is ssh running"`, `"what's my volume"`) never receive action entries.
  2. Destructive suggestions require both the action verb and a target/object (`"delete stuff"` abstains in normal mode; expert mode still prints `WARNING`).
  3. Common nouns stay placeholders unless presented as filenames/targets.
  4. Copies/moves onto existing targets warn of overwrites; `chmod 777` warns of open access.
  5. Requests asking `"is X dangerous"` or `"should i run X"` redirect to `huh explain "X"`; unauthorized access or drive-wiping requests abstain cleanly (with closest tool pointer via `hints.txt` when honest).
- **Questions vs instructions (`kind`, `opposites`):** Opening phrases distinguish state queries (`kind = "query"`) from imperatives (`kind = "action"`), with `kind = "mixed"` (such as `tar`) neutral and `opposites` ruling out contradictory options.

### 2. Static Command Explainer (`shell.rs`, `breakdown.rs`, `knowledge.txt`, `manpage.rs`, `risk*.rs`)

- **Parser (`src/shell.rs`):** Parses pipes (`|`, `|&`), lists (`&&`, `||`, `;`, `&`), `$(...)`, backticks, `${VAR}`, `$((...))`, `<(...)`, subshells, `{ ...; }`, `for`/`while`/`until`/`if`/`case`, functions, `[[ ... ]]`, env assignments (`A=1 cmd`), redirections, heredocs, and `\` line continuations. Best-effort on malformed syntax with parse notes under `NOTES`.
- **Recursive breakdown (`src/breakdown.rs`):** Recurses inside `$(...)`, `bash -c`/`sh -c`, `sudo`, `env`, `xargs`, `nohup`, `time`, `watch`, `timeout`, `find -exec`, `docker run`/`exec`, and `ssh host '...'`.
- **Three-tier flag resolution:** (1) `src/knowledge.txt` (291 curated `@command` tables including `git`, `docker`, `find`, `chmod` octal/symbolic decoding, `kill`, `systemctl`, `dosbox`, `flatpak`, `mpv`, `vlc`, `ffmpeg`, `gio`), (2) catalog slot/flag definitions, and (3) local man pages on disk under `/usr/share/man/man{1,8}` (plain or `.gz`, marked `(from the man page)`).

### 3. Deterministic Error Doctor & Output Explainer (`doctor.rs`, `outputs.rs`, `outputs.txt`)

- **`outputs::describe`:** Recognizes tabular/structured output (`ls -l`, `df`, `du`, `lsblk`, `stat`, `findmnt`, `free`, `ps`, `top`, `ip addr`, `ss`, `ping`, `git status`/`log`/`branch`, `systemctl status`, `journalctl`, `docker ps`, `uname -a`, `uptime`, `who`, `last`, `id`, `sensors`, `nmcli dev`) when output is piped (`cmd | huh`).
- **`doctor::explain` (`huh why`):** Uses exit codes (`0`..`255`), live machine checks (`$PATH` Damerau-Levenshtein typo scan, `stat` ownership/mode check, `/etc/group`, `/var/lib/pacman/db.lck`, `/var/run/docker.sock`, `$PWD` typos), and targeted failure rules including:
  - AppImage missing FUSE 2 (`libfuse.so.2`, `AppImages require FUSE`) → `--appimage-extract-and-run` + `fuse2`/`libfuse2` install advice
  - `Exec format error` (checked before exit-126 `chmod +x`) → `dosbox <target>` (+ `wine` / Steam Proton note) for `.exe`/`.com`/`.bat`, or `file <target>` for foreign/headerless binaries
  - Strict gating on piped text with no exit code (`has_error_shaped_line` + `is_context_noise`) so normal prose or success output is never misdiagnosed as a failure.

### 4. Platform Detection & Catalog Resolution (`src/platform.rs`, `data/platforms.toml`, `src/catalog.rs`)

- **System families:** Maps `/etc/os-release` `ID`/`ID_LIKE` (else `$PATH` package manager fallback) across `arch`, `omarchy` (extends `arch`), `debian`, `fedora`, `suse`, `tumbleweed` (extends `suse`), `alpine`, `void`, `gentoo`, `nixos`, and `nix` (standalone Nix).
- **Unified package facts:** `{pkg_install}`, `{pkg_remove}`, `{pkg_search}`, `{pkg_update}`, `{pkg_owns}`, `{pkg_provides}`, `{pkg_query}`, `{pkg_list}`, and `{pkg_clean}` fill templates in both `catalog/*.toml` and `lessons/*.toml`.
- **Missing-program `NOTE:`:** Scans `$PATH` (plus `sbin` dirs for `sudo` entries). Known packages print exact distro install commands (and `[notes]` caveats); unknown programs print `provides` searches rather than guessing package names; distro tools (`pacman`, `omarchy`, `yay`, `flatpak`) are never recommended via another family's manager.
- **User overrides:** Extend or override the embedded catalog and platform tables at runtime via `--catalog <DIR>`, `$HUH_CATALOG`, `~/.config/huh/catalog/*.toml`, `$HUH_PLATFORMS`, or `~/.config/huh/platforms.toml`.

---

## Companion Pane, Shell Recording, Config vs State

### Shell Recording & Shortcuts (`src/session.rs`, `src/setup.rs`)

- Supports **Bash**, **Zsh** (respecting `$ZDOTDIR`), and **Fish** via `huh setup` or manual `huh shell-init [--shell auto|bash|zsh|fish] [--hooks]`.
- Uses `util-linux` `script` + `huh record-sink` streaming terminal output into a private `0600` 1 MB ring buffer at `$XDG_RUNTIME_DIR/huh/<pid>.log` (kept `64 KB` trimmed tail, gone at logout, skipped for `huh ...` commands, disabled per shell with `HUH_NO_RECORD=1`).
- Keybindings: `Alt+h` / `Alt+f` (`$HUH_KEY`) replaces the prompt line with `huh --no-ask --first-line-only "..."` without running it; `Alt+e` explains the current prompt line underneath.

### Precedence & On-Disk Layout

| Kind | Path | Owned By | Purpose |
|---|---|---|---|
| User config | `$XDG_CONFIG_HOME/huh/config.toml` | User (never modified by live slash commands) | `default_mode`, `mode`/`safety`, `theme`, `animation`, `pin`, `pet`, `typewriter`, `typewriter_ms`, `tips`, `daily_goal`, `miss_log` |
| Custom sprites | `$XDG_CONFIG_HOME/huh/sprites/*.toml` | User (`huh pet --customize`) | 32×32 palette + grid overrides; invalid files fall back silently |
| Runtime prefs | `$XDG_STATE_HOME/huh/prefs.toml` | Pane / CLI (atomic write via tmp + rename) | Live `/mode`, `/safety`, `/theme`, `/speed`, `/noise` (`/quiet`), `/tips`, `/pet style`, `huh misses --enable\|--disable` |
| Seen programs | `$XDG_STATE_HOME/huh/seen.txt` | Watch (`noise = "new"`) | Atomic list of already-explained command names persisted across restarts |
| Miss log | `$XDG_STATE_HOME/huh/misses.jsonl` | CLI / Watch (`huh misses`) | Opt-in local JSONL log (`0600`, capped at 1,000 entries) of unmatched queries |
| Companion state | `$XDG_STATE_HOME/huh/pet.toml` | Companion (atomic merge/save) | Pet name, species, stats, streaks, dual-track unlock counters, gifted pebbles |
| Course state | `$XDG_STATE_HOME/huh/progress.toml` | Lab (atomic write) | XP, daily goal, UTC streaks + freezes, 19 badges, completed lessons/labs, 6-box Leitner SRS deck |
| Practice sandboxes | `$XDG_DATA_HOME/huh/practice/` | Lab (`practice.rs`) | Throwaway directories + cleanup |

**Resolution order:** CLI flag > environment variable (`HUH_MODE`, `HUH_THEME`, `HUH_NO_TIPS`, `HUH_NO_TYPEWRITER`, `HUH_TYPEWRITER_MS`, `HUH_NO_PIN`, `HUH_NO_PET`, `HUH_KITTY_GRAPHICS`, `HUH_MISS_LOG`) > `config.toml` > `prefs.toml` > defaults.

---

## Benchmarks & Eval Commands

Run the built-in evaluation suites directly from any checkout:

```bash
huh check                                            # validate all 316 catalog entries
huh eval                                             # evals/cases.toml (238 cases)
huh eval --cases evals/heldout.toml                  # evals/heldout.toml (138 cases)
huh eval --cases evals/safety.toml                   # evals/safety.toml (110 cases)
huh eval --cases evals/sysadmin.toml                 # evals/sysadmin.toml (120 cases)
huh eval --cases evals/everyday.toml                 # evals/everyday.toml (17 cases)
huh eval --cases evals/debian.toml --platform debian # distro simulation (arch, debian, fedora, suse, tumbleweed, alpine, void, gentoo, nixos)
huh eval-errors                                      # evals/errors.toml (30 cases)
huh eval-errors --cases evals/errors-real.toml       # evals/errors-real.toml (67 real-world failures)
```

All `evals/*.toml` suites run automatically in `cargo test --release --quiet` (**468 passed, 0 failed, 2 ignored** manual dump helpers).

---

## Repository Layout

| File | Role |
|---|---|
| [`build.rs`](../build.rs) | Embeds sorted `catalog/*.toml` (316), `lessons/*.toml` (11), and `sprites/*.toml` (9) at compile time |
| [`src/main.rs`](../src/main.rs) | CLI (`huh`), request pipeline, `explain`, `why`, `misses`, `setup`, `pet`, `check`, and companion-pane launch |
| [`src/matcher.rs`](../src/matcher.rs) | `LocalMatcher`: BM25F index, stems/concepts, domain coherence, unmodeled checks, slot ranking |
| [`src/candidates.rs`](../src/candidates.rs), [`src/extract.rs`](../src/extract.rs), [`src/values.rs`](../src/values.rs) | Token, phrase, path, size, port, device, extension, and typed candidate extraction |
| [`src/sysinfo.rs`](../src/sysinfo.rs), [`src/readers.rs`](../src/readers.rs) | Read-only `/etc/{passwd,group,shells}` + systemd units and specialized sysadmin slot extractors |
| [`src/assemble.rs`](../src/assemble.rs), [`src/guard.rs`](../src/guard.rs) | Shortlisting, slot filling, shell quoting, missing-program `NOTE:`, and normal-mode safety abstentions |
| [`src/hint.rs`](../src/hint.rs), [`src/hints.txt`](../src/hints.txt) | Closest-tool pointer table (`ffmpeg`, `convert`, `pandoc`, `wine`, etc.) when no exact catalog command applies |
| [`src/platform.rs`](../src/platform.rs), [`data/platforms.toml`](../data/platforms.toml) | OS detection, 10 Linux distro families (plus standalone Nix), package-task templates, and per-family package names |
| [`src/catalog.rs`](../src/catalog.rs), `catalog/*.toml` | Catalog schema, validation, platform filtering, and 316 curated command definitions |
| [`src/doctor.rs`](../src/doctor.rs) | Deterministic Error Doctor (`huh why`: exit codes, typo scans, permissions, FUSE 2, Exec format error, crashes) |
| [`src/outputs.rs`](../src/outputs.rs), [`src/outputs.txt`](../src/outputs.txt) | Tabular & status command output explainer for `cmd \| huh` |
| [`src/shell.rs`](../src/shell.rs), [`src/explain.rs`](../src/explain.rs), [`src/breakdown.rs`](../src/breakdown.rs), [`src/decode.rs`](../src/decode.rs) | Static bash-subset parser and recursive command breakdown |
| [`src/knowledge.rs`](../src/knowledge.rs), [`src/knowledge.txt`](../src/knowledge.txt), [`src/manpage.rs`](../src/manpage.rs) | 291 curated `@command` flag/subcommand tables and `/usr/share/man` `.gz` fallback |
| [`src/risk.rs`](../src/risk.rs), [`src/risk_extra.rs`](../src/risk_extra.rs), [`src/risk_kinds.rs`](../src/risk_kinds.rs), [`src/render.rs`](../src/render.rs) | 14 standardized warning detectors, risk taxonomy, and 80-column output renderer |
| [`src/launch.rs`](../src/launch.rs) | Side-pane launcher (`tmux` split, tiling desktop window, `tmux -L huh`) and `fzf` runner — sole module spawning subprocesses |
| [`src/session.rs`](../src/session.rs), [`src/setup.rs`](../src/setup.rs) | `script` PTY recording sink, bash/zsh/fish shell hooks/bindings, and `huh setup` wizard |
| [`src/pane.rs`](../src/pane.rs), [`src/tui.rs`](../src/tui.rs), [`src/slash.rs`](../src/slash.rs) | Unified `watch` + `lab` event loop, raw-mode `select_menu`, live `/` fuzzy dropdown `LineEditor`, 13 slash commands |
| [`src/pin.rs`](../src/pin.rs), [`src/tips.rs`](../src/tips.rs), [`src/typewriter.rs`](../src/typewriter.rs), [`src/style.rs`](../src/style.rs), [`src/welcome.rs`](../src/welcome.rs) | DECSTBM pinned header, 18 rotating header tips, ANSI-safe variable-rate typewriter, 9 color themes, `L2b` welcome animation |
| [`src/pet.rs`](../src/pet.rs), [`src/date.rs`](../src/date.rs), [`src/sprite.rs`](../src/sprite.rs), `sprites/*.toml` | `#!` companion state machine (9 species × 6 moods), pebble milestones, Kitty Graphics APC/DCS renderer, 4×10 ASCII fallback, custom sheets |
| [`src/learn.rs`](../src/learn.rs), [`src/lessons.rs`](../src/lessons.rs), [`src/practice.rs`](../src/practice.rs), `lessons/*.toml` | `/mode lab` runner, 11-unit / 33-lesson / 165-question / 9-lab curriculum, `unlock_after` branching, sandbox checker |
| [`src/cheats.rs`](../src/cheats.rs) | Hidden lesson easter eggs updating local companion/course state (zero shell execution) |
| [`src/misses.rs`](../src/misses.rs) | Pure-std opt-in JSONL local miss log (`0600`, atomic 1,000-entry rotation) for `huh misses` |
| [`src/progress.rs`](../src/progress.rs), [`src/srs.rs`](../src/srs.rs), [`src/prefs.rs`](../src/prefs.rs), [`src/mode.rs`](../src/mode.rs) | XP, UTC streaks with freezes, 19 badges, 6-box Leitner SRS, atomic `prefs.toml`, and `config.toml` loaders |
| [`src/find.rs`](../src/find.rs), [`src/tools.rs`](../src/tools.rs), [`src/eval.rs`](../src/eval.rs) | Fuzzy command/binary finder, PATH tool scanner, and evaluation runners |
