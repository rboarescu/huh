# huh: roadmap and status

## Completed: `huh explain` rewrite for vetting LLM-run commands (2026-11-17)

Use case: an agent ran a long command and the user wants to know what it did before trusting it.

- **Real shell parser** (`src/shell.rs`): pipes, lists, `$(...)`, backticks, `${}`, `$(( ))`, `<( )`, subshells, groups, loops, `if`/`case`, functions, assignments, all redirections, heredocs, line continuations; never panics (fuzz-style test), problems become NOTES.
- **Per-program flag meanings** from the curated table (`src/knowledge.txt`), then the catalog, then man pages read from disk (`src/manpage.rs`, `flate2` with the pure-Rust `miniz_oxide` backend for `.gz`). Fixes: `find -mtime +7`, `-delete`, `ssh -i`/`sed -i`, `tar -C`, docker roles, `chmod 755`, `kill -9 $(...)`, `git reset --hard HEAD~1`, `--since`, `awk`, `sed`, `dig`.
- **Recursive explanation** of `$(...)`, `bash -c`, `sudo`, `env`, `xargs`, `nohup`, `time`, `watch`, `timeout`, `find -exec`, `docker exec`, and `ssh host '...'` (marked remote).
- **WARNINGS section**: deletes, overwrites, history rewrites, privilege, remote code, package installs, `kill -9`, unset variables in `rm`, long builds; each names the exact part.
- **Reads stdin** (`huh explain -`, or piped with no argument) for multi-line scripts.
- **Safety**: nothing is executed; a test fails if the explain modules mention `std::process`/`Command::new`. Scripts and Makefiles named in the command are not opened.
- **Verification**: `evals/explain.toml` (70+ cases), parser, man page and decoder unit tests.
- **Next**: more curated programs (kubectl, terraform, brew, systemd timers), `.zst`/`.xz` man pages, optional read-only look into a named script or Makefile.

## Completed: Deterministic rewrite (2026-11-17)

`huh` replaces all hosted/local LLM and network dependencies (`ureq`, TypeSafe Jev, `llama.cpp`, `Ollama`, `d1-2B`, `Granite 4.2 3B`) with a fast, self-contained deterministic binary:

- **Embedded & expanded catalog (203 entries)**: compiled into the binary via `build.rs`, with `package` overrides, platform install command hints (`omarchy`, `arch`, `debian`, `fedora`), `keywords` on every entry, and new commands (`tracepath`, `ip route get`, `omarchy refresh`, `omarchy default`, `omarchy pkg aur add`, `gio trash`).
- **Local matcher (`LocalMatcher`)**: multi-field BM25F, stem/synonym normalization, domain-coherence fit checks, multi-word unquoted phrase & `$PWD` space-in-filename matching, bare `.ext` -> `*.ext` normalization, exact `find -size` parsing (`50MB` -> `-size +50M`), and multi-slot role binding.
- **Part-by-part explanations & `huh explain`**: top suggestion prints indented piece-by-piece descriptions (`--brief` hides them); `huh explain "<command line>"` breaks down pipelines, redirections, `sudo`, combined flags, slots, wildcards, and unknown tokens.
- **Deterministic Error Doctor (`huh why` and bare `huh`)**: exit-code table, live machine checks (`$PATH` Damerau-Levenshtein typo scan, executable bit `metadata()`, `/etc/group`, `/var/lib/pacman/db.lck`, `/var/run/docker.sock`, `$PWD` typo scan), and structured diagnostics for bash/sh/zsh/fish, PolicyKit, pacman, systemd units, SSH/Git publickey, tar, docker, and compilers.
- **Bash integration (`huh shell-init --hooks`)**: readline `bind -x` prompt-line replacement on `Alt+h` / `Alt+f` (configurable via `HUH_KEY`) plus `HUH_SESSION_*` terminal recording.
- **Blind persona verification**: tested across independent Omarchy, beginner, and sysadmin personas, with all reported misses and edge cases resolved and covered by unit tests and evals.

## Completed: Typed matcher pipeline (2026-11-17)

`backend.rs` / `questions.rs` (the text question/answer protocol left from the hosted-model days) are gone: `LocalMatcher::rank` and `LocalMatcher::details` return typed results that `assemble.rs` turns into lines. Output is byte-identical to before. Code is rustfmt-formatted and clippy-clean.

## Completed: Platform module and other distros (2026-11-17)

Fixes "Next" item 4 (other distros) and the install/not-found parts of item 3.

- **`src/platform.rs` + `data/platforms.toml`** are the single source of truth (the catalog's `INSTALLERS` and the doctor's `INSTALL`/`KNOWN_PACKAGES` are gone). Families: arch, omarchy (extends arch), debian, fedora, suse, alpine, void, gentoo, nixos. Detection: os-release `ID`/`ID_LIKE` chain from `/etc` then `/usr/lib`, derivatives that only name a parent (elementary, Zorin, Artix) mapped by family `ids`, and for unknown IDs the package manager binary on PATH. `~/.config/huh/platforms.toml` (or `$HUH_PLATFORMS`) extends the data without a rebuild.
- **Package names per family** (`nmcli` -> `network-manager`, `ip` -> `iproute` on Fedora, `usermod` -> `passwd`/`shadow-utils`, `dig` -> `bind9-dnsutils`/`bind-utils`/`bind`, ...). Unknown programs get the system's provides-search (`pacman -F`, `apt-file search`, `dnf provides`, `zypper search --provides`, `apk search -e cmd:`, `xlocate`, `e-file`, `nix-locate`) instead of a guessed package; only names listed as same-everywhere or mapped are used directly. Package managers and distro tools are never offered for installation (`--all-commands` no longer says `apt install pacman`). `yay` is not installable with pacman.
- **Package tasks are generic**: catalog entries `package install|remove|search`, `system update`, `package owns file`, `package provides program` use `{pkg_*}` placeholders filled per family; the 14 per-manager entries were removed. Eval case names changed accordingly (`pacman install` -> `package install`, ...). On `arch,omarchy` the top suggestion is now `omarchy pkg add` / `omarchy pkg drop` (four expected lines changed for that reason). `huh explain` still recognises every family's commands through generated explain-only variants.
- **PATH detection**: exec bit required, symlinks followed, `~` expanded, empty entries ignored, full bash built-in list (`kill`, `read`, ...). `/usr/sbin` and `/sbin` are searched too but only count for suggestions that run through `sudo` (sudo's PATH has them, a Debian user's does not); otherwise the note says the tool is in sbin, not on PATH.
- **`huh why`**: understands `sh: 1: x: not found`, `sh: x: not found`, bare `x: command not found` (plus bash/zsh/fish); never suggests installing a program that is installed (says where it is instead); a one-edit typo of an installed command (`sl` -> `ls`) beats installing a package unless the name is a known program; typo fixes keep the arguments and pick the command-position word (`gti status` -> `git status`).
- **Portability**: `cargo build --release --target x86_64-unknown-linux-musl` gives a static binary that runs on Debian 11, Ubuntu 20.04 and Alpine (documented in README).
- Verified in Docker (debian 11/12, ubuntu 20.04/24.04, fedora, arch, opensuse tumbleweed, alpine, void, gentoo, nixos/nix with no os-release).

Not done: `huh` panics on a closed pipe (`huh ... | head`); NixOS has no "which package owns this file" command; Tumbleweed uses `zypper update` rather than `dup`; `sudo` is suggested even in containers where the user is root and sudo is absent.

## Next: findings from real-world simulation (2026-11-17)

The evals score 100%, but four simulated users (beginner on Omarchy, sysadmin, failing-command diagnosis, other distros in Docker) found the evals overfit. Fix in this order:

1. **Safety**
   - `reboot` and `shutdown` carry no WARNING and drop the time ("reboot in 10 minutes" gives `reboot`).
   - "restart nginx" gives `omarchy restart audio`, then `reboot`, even though `systemctl` is in the catalog.
   - "older than 7 days" gives `find -mtime -7`, which is the wrong sign.
   - "kill the process on port 3000" gives `kill 3000`, treating the port as a PID.
   - "discard changes in f" gives `gio trash f`.
   - "remove dangling docker images" gives `docker rmi dangling`.
2. **Slot filling** (the biggest source of wrong lines)
   - Folder names prefix-match files: "the projects folder" fills in `projects-download.zip`, including for `rm -r` and `cp`.
   - "my" fills in `My Resume.pdf`.
   - Leftover words become values: `grep password 'word password'`, `ping -c 4 working`.
   - Modifiers are dropped silently: user@ in ssh, `-p` port, `--exclude`, `--since`, `-9`, `-C dir`, the owner in chown, the user in usermod.
3. **Error doctor (`huh why`)**: only 22 of 96 real failures got a useful fix.
   - Every "Permission denied" is blamed on root.
   - A misspelled module gets `pip install <typo>`.
   - `sl: command not found` gets `pacman -S sl` instead of `ls`.
   - Successful output is called a failure.
   - "Could not resolve host" quotes text that isn't in the output.
   - The fallback often quotes the wrong line and prints a literal `Try: nothing`.
   - Not recognised:
     - git errors (not a repo, merge conflict, bad branch);
     - Python Syntax/Indentation/Key errors;
     - pip `externally-managed-environment`;
     - ~~dash's `sh: 1: x: not found`~~ (done 2026-11-17, see "Platform module").
   - ~~`sl: command not found` gets `pacman -S sl` instead of `ls`~~ (done 2026-11-17).
4. ~~**Other distros**~~ Done 2026-11-17 (see "Completed: Platform module and other distros"): per-family package names, one install table, openSUSE/Alpine/Void/Gentoo/NixOS/Artix/elementary/Zorin, PATH detection fixes and a static musl build.
5. **Catalog gaps**:
   - systemctl `--failed`;
   - git reset/restore/stash/checkout and push `-u`;
   - dig, traceroute, nc, lsof, fuser, awk/sed/uniq pipelines;
   - useradd, who, iostat;
   - docker prune and `system df`;
   - package-owns-file (`pacman -Qo`, `dpkg -S`, `rpm -qf`);
   - firewall (ufw/firewalld);
   - `gio trash --empty`, `cd -`, history.
6. **Eval hygiene**: add the simulated requests as a new held-out file before tuning, so fixes are measured on cases the matcher was not tuned against.

## Future ideas (lower priority)

- **zsh and fish hooks**: native ZLE / fish keybindings and preexec/postexec recording hooks alongside bash.
- **Local miss log (`huh misses`)**: optional local JSONL log when no catalog entry fits, to guide future catalog additions.
- **Personal cheat sheet (`huh recent`)**: optional local history of accepted prompt completions for instant repeat recall.
- **Optional `tldr` reference fallback**: show `tldr` examples when an installed command has no catalog entry.
- **Additional catalog entries**: `sed` / `awk` / `jq` recipes and user-requested tools.
