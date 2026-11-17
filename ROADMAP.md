# huh: roadmap and status

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
     - dash's `sh: 1: x: not found`.
4. **Other distros**
   - Install hints use the binary name as the package name (`apt install nmcli`, `systemctl`, `ps`), and `KNOWN_PACKAGES` in explain.rs is Arch-only (`apt install bind`, `libvirt`).
   - explain.rs keeps its own install table, which disagrees with catalog.rs (`pacman -S` instead of `omarchy pkg add`).
   - Unknown platforms (openSUSE, Alpine, NixOS, Gentoo, Void, Artix, and Ubuntu-only `ID_LIKE` such as elementary) get no package manager; "install htop" gives `which htop`.
   - PATH detection:
     - misses `/usr/sbin` for non-root Debian users;
     - counts non-executable files as installed;
     - doesn't expand `~`;
     - `kill` is missing from the builtins list.
   - The release binary needs glibc ≥ 2.34. It does not start on Debian 11, Ubuntu 20.04, RHEL 8 or Alpine, so a static musl build is needed.
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
