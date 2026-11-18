# huh: roadmap and status

## Completed: Slot filling (2026-11-18)

- **Entries**: the current folder now marks folders (`projects/`), so "the projects folder" is the folder, never `projects-download.zip`. An entry matches only when the request says all its distinctive words (stopwords and "my" never count) and an exact name beats a partial one. A multi-word name ("Vacation Photos") is consumed whole; a folder that receives a copy gets a trailing `/`.
- **Values**: "my home folder" is `~`, "my files"/"this folder" is `.` for grep, "jpg files" is `*.jpg`, "a file named notes" is `-name 'notes*'`, a path written in the request beats an entry that shares a word with it, flags typed as written (`-ri`, `-R`) are never values and turn their options on, `-r` follows what is being copied, deleted or searched.
- **Leftover words**: verbs, fillers and question words ("word", "working", "respond", "see", "each", "usb", "port") are not values; "for the word X" gives `X`. A bare word with no cue ("called", quotes, path shape, known program) scores low, so the line carries a "not sure" note (or asks) instead of passing for certain.
- **Modifiers**: `ssh` takes `user@` and `-p`, `ping` a count, `tar` `--exclude=` and `-C`, `journalctl` `--since`, `git log` `-N`, `usermod` a user (catalog: new slots in those six entries). A modifier no slot can hold (`excluding X`, `since X`, `on port N`, `as user` for ssh) gets "not included: `...`" under the line.
- **Tests**: `evals/slots.toml` (52 cases, 39 failed before; `cargo test` requires all, new optional `note` field), plus the existing evals unchanged.
- **Not handled**: wrong commands from ranking (e.g. "compress the projects folder into a tar.gz" gives `gzip`, "force quit spotify" gives `kill -9`, "go back to the previous folder" has no `cd -`); `scp`/`rsync` ports and users; several values for one slot; relative dates beyond the words after `since`.

## Completed: Error doctor accuracy pass (2026-11-18)

Governing rule: a confident wrong cause or fix is worse than "huh does not recognise this". New suite `evals/errors-real.toml` (67 cases from real runs of the 96-failure test) went from 21/67 causes and 23/67 fixes to 67/67 and 67/67; `cargo test` now requires 100% on all three error suites. The eval format gained optional `exit` (omitted = piped input), `forbid` (words that must not appear) and `files` (files made in a temp folder, `$TMP`).

- **Permission denied**: stats the file (read-only). Your own file with no read/write bit gets `chmod u+r`/`u+w`; a missing file checks its folder; sudo only for files of another user or under /etc, /var, /usr, /root, /sys, /proc, /dev, /boot. Redirect or `cat` failures are no longer mistaken for "not executable". Several failing files are each named (up to three).
- **Modules**: Python import names map to pip names (`yaml` -> `pyyaml`, `cv2`, `PIL`, `bs4`, `sklearn` and more); a near-miss of a common, installed or local module is called a typo (`grep` to find it) instead of `pip install <typo>`; PEP 668 systems get a venv command. Same for node `Cannot find module`.
- **Successful output**: without an exit code and without an error-looking line, huh says the output does not look like an error and what it seems to be; exit 0 says it succeeded.
- **Network**: name lookup failures quote only text in the output and suggest `getent hosts <host>` (DNS wording keeps the `ping` check); `ping`/`wget` forms recognised; "Connection refused" says nothing listens on that port (`ss -ltn` for local, `nc -zv` for remote), not `ping`.
- **Rules added**: docker pull denied / repository missing, `pacman` without root, git (not a repository, bad branch or pathspec, `src refspec`, merge conflicts and unmerged paths, empty stash), Makefile `missing separator`, Python SyntaxError/IndentationError/KeyError/AttributeError/JSONDecodeError and other tracebacks (file and line), pip `externally-managed-environment`, npm `Missing script`, unzip on a non-zip, `Killed` (out of memory), `Segmentation fault`.
- **Changed**: `tar xzf` on a non-gzip file suggests `file <name>` with the real file name instead of a guessed `tar -xf`; `systemctl --user` suggestions keep `--user` and "could not be found" is recognised; the fallback quotes the first error line (or the last line of a traceback) instead of the last line; paths in "does not exist" messages are extracted cleanly.
- **Still not handled**: compiler errors beyond quoting the first one, most npm/cargo failures beyond the quoted line, Python errors whose frame is in a library only, and any failure whose output carries no error keyword (reported as "does not look like an error" when the exit code is unknown).

## Completed: Platform and package-name fixes from the Docker verification (2026-11-17)

- **Names**: "install dig/docker" map program to package per family (the `[programs]`/`[packages]` data, via `pkg_role` on package tasks and `assemble::package_line`); fixed suse (`btrfsprogs`, `iperf`, `rust`), void (`fish-shell`, `rust`, `bind-utils`; `nmap`/`unrar` fall back to `xlocate`), alpine (`rust`; `snapper`/`unrar` marked not packaged with `""`), nixos (`gnumake`, npm in `nodejs`). Names were checked in containers.
- **Releases**: new `[notes]` table keeps the current package name and adds a remark for releases that lack it (`eza` on Debian 11/12/Ubuntu < 24.04, `btop`/`gh` on Debian 11/Ubuntu 20.04); shown under the suggestion and in `huh why`.
- **Nix**: new `nix` family (Nix on a non-NixOS system, found by `nix` on PATH) updates with `nix profile upgrade --all`; `nixos` keeps `nixos-rebuild`. No "owns" task there, and a request for one no longer falls through to `nix search ... owns`; `nix-locate` templates take a file name when given a path (no `bin//usr/...`).
- **No package manager** (busybox): package requests say "could not detect a package manager" instead of `which htop`.
- **Wording**: "remove htop" is a package removal when the object is a known program or on PATH (files still delete); "a package called X" filler is dropped; "which package owns /usr/bin/ls" ranks first with ~93%.
- **`huh why`**: no `Try:` line when there is nothing to try; an installed program is reported with its path and "the failure is elsewhere".
- **Tests**: `evals/platforms.toml` (per-family cases with a `platform` field, run by `cargo test` and `huh eval --platform`), new unit tests in `platform.rs`, `candidates.rs`, `doctor.rs`.
- **Next**: per-release package data only if the notes prove too coarse; `nix profile add` once old nix versions are gone.

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
2. ~~**Slot filling**~~ Done 2026-11-18 (see "Completed: Slot filling"). Left: scp/rsync users and ports, several values for one slot.
3. ~~**Error doctor (`huh why`)**~~ Done 2026-11-18 (see "Completed: Error doctor accuracy pass"). Original problems, kept for history: only 22 of 96 real failures got a useful fix.
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
