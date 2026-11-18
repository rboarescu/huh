# huh: roadmap and status

## Measured: round 4, judged by TypeSafe (2026-11-18)

New judge: `tools/judge/` asks TypeSafe's Jev small yes/no questions about each answer and decides good/bad in code. Calibrated on 471 human-labelled answers: 96% agreement on requests, 93% on `huh why`, ~8% left `unsure`. One consistent judge for every number below; the whole calibration and scoring cost $0.05.

| Set | Beginner | Sysadmin | `huh why` |
|---|---|---|---|
| Frozen set, round-3 binary | 72% | 60% | 62% |
| Frozen set, current binary | 92% | 93% | 74% |
| **Round 4 fresh cases (never seen by fixers)** | **79%** | **56%** | **58%** |

The frozen set has turned into a training set (its failures were quoted to fixers), so its 92–93% overstates; the fresh round is the real number. Fresh sysadmin is 56%: most failures are typed values silently dropped (31 of 55: `df -h` without `/var`, `git clone … into /opt/api depth 1`, `docker stats` without the container) and wrong commands (17). Fresh `huh why` failures: wrong or no cause 68, honest misses 20, claims the output does not show 17, normal output called a failure 11 (prose and logs containing "error"/"failed"/"refused").

Safety held in round 4: 0 dangerous suggestions without a WARNING and 0 state questions answered with a change from the beginner tester; `huh explain` 13/15 fully correct and 0 wrong. Remaining serious ones: `rm -rf /` as a request gives `rm -r /` (with WARNING), "wipe /dev/sdb" gives `rm /dev/sdb`, "docker inspect ip of container api" gives `docker rm api`, "show process tree of pid 1234" gives `kill 1234`, "delete all files in Documents" gives `rm -r Documents`.

## Measured: frozen held-out set after "never mislead" (2026-11-18)

The round-3 requests and error outputs are kept outside the repo as a frozen set (`~/.local/share/huh-dev/heldout/`: `run.py` replays them, `compare.py` lists changed answers, a judge rates only the changes). Caveat: about 40 of its failures were quoted to the fixing agents, so it is not fully unseen.

Of 598 answers, 148 changed. The judge's ratings (bad→good / good→bad): beginner 29 / 0, sysadmin 3 / 0, errors 43 / 3. The three regressions and one new invented cause (a pacman signature error called a "double free") were then fixed: make now explains the recipe's own error first, curl errors glued to the progress meter are split off, and memory-error wording needs glibc's own messages. A re-run changed exactly those four answers.

Applied to round 3's scores, that is roughly beginner 77% → ~94%, sysadmin 63% → ~66%, `huh why` useful fix ~51% → ~70%. These numbers come from two different judges, so treat them as estimates; a fresh round is the real check.

Still wrong (from the judge):
- Sysadmin catalog gaps and multi-value requests: cherry-pick, `git diff a b`, `lsof -p`, `mount <dev> <dir>`, ssh `-L` specs, `pkill <name>` (the name goes into the user slot), and removing every `node_modules` folder.
- Named values left as placeholders: "is docker running" → `pgrep -a <name>`, "enable docker at boot" → `<service>`.
- Beginner gaps: bluetooth pair, current theme, screen resolution, "show usb drives".

## Measured: round 3, after intent, typed slots and the honest error doctor (2026-11-18)

Fresh personas again (no access to evals or source; requesters ran only huh, the error tester only inside Docker). Transcripts: `~/.local/share/huh-dev/measure-2026-11-18-round3/`.

| Metric | Round 1 | Round 2 | Round 3 | Target |
|---|---|---|---|---|
| Beginner requests good | 71% | 76% | 77% (135/175) | ≥ 85% |
| Sysadmin requests good | 38% | 47% | 63% (66/104) | ≥ 65% |
| `huh explain` fully correct | 5/20 | 16/20 | 17/20 | – |
| `huh why` useful safe fix | 23% | 51% | ~51% (~125/244, harder mix incl. buried logs) | ≥ 65% |
| `huh why` misleading | 18 | 12 | 11 | ≤ 5 |
| Non-errors called failures | – | 3/37 | 2/58 | 0 |
| Dangerous without WARNING | – | 1 | 3 | 0 |

Each round fixes what it measured, but fresh wording finds new failures at a similar rate on the beginner and error sides: hand-written rules are not generalising there. Open findings (details in the transcripts):

- **Dangerous**:
  - `echo x >> /etc/hosts` suggests `sudo tee` without `-a` (overwrites);
  - "restore my backup" → `cp backup backup.sh` (overwrites, no WARNING);
  - "delete all .log files in logs" → `rm -r logs` first;
  - "who is in group docker" → `usermod -aG docker` with no WARNING;
  - "chmod 777 everything" → `chmod +x`;
  - "change permissions so only i can read it" → `chmod a-w`.
- **State questions still answered with actions** (9 of ~30 beginner questions): volume, brightness, mounted, ssh running, services running, default browser.
- **Sysadmin**: multi-value requests keep one value (ssh tunnels, scp as user, force push with remote and branch, kill -9, rsync --delete).
- **`huh why`**: wrong "typo for X" when a program is simply not installed (`ping` → `find`, `sudo` → `sed`); CRLF `bad interpreter`; rebase conflicts reported as merge conflicts; ~16 obvious error lines left unrecognised without an exit code (`CONFLICT`, `Error 1`, `not in the sudoers file`).

## Measured: fresh simulated users after the safety round (2026-11-18)

New personas wrote their own requests (they could not read the evals), against the merged binary:

| Persona | Round 1 (2026-11-17) | After safety round |
|---|---|---|
| Beginner on Omarchy | 104/146 good (71%) | 118/155 good (76%) |
| Sysadmin requests | 34/89 good (38%) | 48/102 good (47%) |
| `huh explain` on agent-style lines | 5/20 fully correct, 9 wrong | 16/20 fully correct, 0 wrong |
| `huh why` on real failures | 22/96 useful fix (23%), 18 misleading | 95/186 useful safe fix (51%), 12 misleading; 34/37 non-errors recognised |

Fixed right after the measurement: destructive lines on targets the user never named (`rm -r ~` for "delete user bob and home directory", `find . -type f -delete` for "clean up old packages"), and "explain X" / "what is X" now point to `huh explain`. npm global-install EACCES no longer leads to `sudo npm`.

## Planned: tutor mode (`huh tutor`), after the "Next" fixes below

Idea (2026-11-18, from a real session: `huh "unmount drive"` gave `lsblk`, then `lsblk | huh` only said "not an error", and `lsblk | huh explain` read the output as a 9-command script; that last bug is fixed): a side pane that follows the terminal and explains each command and its output as it happens, like a tutor sitting next to you. Like everything in huh, it only reads and explains; it never runs anything.

```
 terminal ──(already recorded today: script → huh record-sink, markers, exit code)──▶ private log
                                                                                        │
                                              huh tutor (new) watches it, read-only ◀───┘
                                                ├─ command finished → huh explain the line
                                                ├─ exit ≠ 0         → huh why on its output
                                                └─ exit 0           → explain the output (new)
```

1. **MVP**: `huh tutor` in a second pane (Ghostty or tmux split) follows the recording of the terminal next to it; prints the explanation of each finished command and the diagnosis of each failure. Mostly existing pieces.
2. **Explain before Enter**: a key (e.g. Alt+e, like Alt+h today) sends the typed line to the tutor pane before it runs: the most useful moment for a beginner or a command pasted from an AI.
3. **Output explanations**: a hand-written notebook of output formats for the ~30 most common programs (`lsblk`, `ls -l`, `df`, `du`, `free`, `ps`, `git status`/`log`, `ip`, `ss`, `docker ps`, …), so `lsblk | huh` can say what the columns mean. With eval cases, like the command knowledge.
4. **Noise levels** (everything / only new commands, with an opt-in local history / only failures and warnings), then an Omarchy keybinding that opens the tutor as a side window attached to the focused terminal.

Known limits: stdout and stderr arrive mixed in the terminal, so the tutor goes by exit code and error-looking lines; full-screen programs (vim, htop, less) are only named, not explained; bash only until the zsh/fish hooks exist.

### Next (from that measurement): all done 2026-11-18

1. ~~`huh why` confident wrong answers~~ and 2. ~~`huh why` coverage~~: see "Completed: Error doctor, second measurement round".
3. ~~Requests: wrong noun in a slot~~: see "Completed: Slot filling, round 2".
4. ~~Requests: state questions answered with actions~~ and 5. ~~catalog gaps~~ (including "unmount drive"): see "Completed: Intent and catalog gaps".

The next step is a fresh measurement round (error tests inside a container) against the targets: beginner ≥ 85%, sysadmin ≥ 65%, `huh why` useful fix ≥ 65%, ≤ 5 misleading, 0 non-errors called errors, 0 dangerous suggestions without a WARNING.

## Completed: dangerous outputs and "typed values never vanish" (2026-11-18)

The five serious findings of round 4 are fixed, in normal and expert mode alike (`evals/safety.toml`, `evals/expert.toml`):

- A request that is itself a command line (first word a known program, plus flags, `NAME=value` or shell operators; no prose words) gets `huh explain "<line>"` instead of a word match. One that would wreck the system (`rm -rf /`, `rm -rf ~`, `chmod -R 777 /`, `dd … of=/dev/sda`, `mkfs` on a device, a fork bomb) also says "this would destroy the system". "run ls -la in src" is still an instruction.
- Wiping, erasing, zeroing or shredding a disk, partition or `/dev` path gets the existing format/wipe refusal in every mode, and no suggestion that deletes, shreds or truncates a drive can be shown (`rm /dev/sdb`).
- A request whose verb is show/inspect/list/display/get/print/check/find/what/which and that asks for no change never gets a destructive top suggestion: normal mode abstains if nothing read-only fits, expert mode ranks the destructive one last and keeps its WARNING. New entries `docker inspect`, `docker container ip` (the first entry with Go-template braces; `\{` and `\}` in a template are literal braces), `pstree`. `fsck` is now the read-only `fsck -n`; `fsck repair` is the destructive one.
- "all files in X", "everything in X", "the contents of X" mean `rm -r X/*` (with a note that hidden files and X itself stay), never `rm -r X`.
- Typed values (`src/values.rs`): after the line is built, every path, number (with or without unit), IP, URL, host or file name, `user@host`, `@server`, quoted text, git hash and labelled name ("named web", "comment ops@lab", "user bob", "onto release") must appear in the line in some form, or the suggestion says `not included: <phrase around it>`. One lost value that fits the single open placeholder goes there; more than one lost value caps the shown confidence at 50%. Added slots: `df` path, `git clone` `--depth` and folder, `docker stats` container, `ssh-keygen -C`, `docker image prune --filter until=`, `docker run -p/--name/image`; new entry `count lines in files by type` (`find … | xargs wc -l`). `evals/values.toml` (83 cases): 28 misses before, 0 after.
- Changed expectations: five existing cases whose request was itself a command line (`chmod -R 777 uploads`, `tail -f app.log`, `pkill -9 firefox`, `chown -R www-data /var/www`, `docker logs --tail 100 web`) now expect the `huh explain` pointer (each also kept as a worded variant); the two `fsck` cases expect `sudo fsck -n …`.

Not handled: a value that only appears as a plain word (a host name without a dot, "by alice" in unusual positions) is not recognised as a value; `delete the contents of build` in normal mode still abstains for lack of a named target.

## Completed: expert mode and `huh tools` (2026-11-18)

- **Expert mode** (`--expert`, `HUH_MODE=expert`, `mode = "expert"` in `~/.config/huh/config.toml`; `--normal` / `HUH_MODE=normal` override the file): turns off exactly the beginner protections (the evidence requirement for dangerous suggestions, the broad-target drop, the state-question action ban). Broad-target lines keep their WARNING and gain a `note: this acts on ...` line. Every WARNING, the refusals (access to systems or accounts that are not the user's, formatting or wiping drives) and the `huh explain` redirects are unchanged. Notice goes to stderr. Precedence: flag, environment, file, default.
- **Evals**: `evals/expert.toml` (45 cases, optional per-case `mode = "expert"`): vague dangerous requests abstain in normal mode and answer with a WARNING in expert mode, refusals hold in both. All other evals still run in normal mode and pass.
- **`huh tools [word...]`**: every executable on `$PATH` with a one-line description (catalog, then curated table, then the man page NAME line read from disk; links such as `zstdmt` use their target's). No program is run. About 0.4 s for 4,000 programs. `--no-desc` lists names only.
- Next: `.xz` and `.zst` man pages are skipped (no pure-Rust decoder is a dependency); about 40% of programs on a developer machine have no man page at all.

## Completed: sysadmin round (catalog and ranking) (2026-11-18)

Findings: fresh testers asked sysadmin questions the catalog lacked, or got a wrong-but-confident entry (`git commit -m` for cherry-pick, `cp` for `host:path`, `kill` without `-9`, `cut` for whitespace columns).

- **Evals**: `evals/sysadmin.toml` (120 cases: the reported failures plus about 50 variants of my own, exact lines and WARNINGs). 67 of the first 78 cases failed on the old code; `cargo test` requires 100%.
- **New entries (29)**: `git cherry-pick`, `git diff refs`, `git stash push`, `git show`, `git revert`, `lsof`, `ps top`, `grep ip addresses`, `du per folder`, `ls permissions`, `find delete dirs`, `journalctl vacuum time`/`size`, `docker cp`/`docker cp from`, `docker compose restart`, `ssh remote tunnel` (`-R`), `ssh-keygen`, `mount`, `passwd user`, `fsck`, `crontab list`/`list user`/`edit`, `sed replace`, `ip addr add`/`del`, `nohup`, `awk column`.
- **Existing entries changed**: `git switch` (`base` slot), `git push` (`remote`/`branch` slots, `--force` became `--force-with-lease`), `rsync` (`--delete` flag with warning), `scp` (`user` slot: "as deploy"), `chmod` (sticky bit option), `ssh tunnel` (host reader).
- **Ranking** (`matcher.rs`): one predicate per request shape (`asks_squash`, `names_two_refs`, `asks_open_files`, `asks_delete_named_dirs`, `journal_vacuum_kind`, `asks_schedule`, ...) feeding both `command_intent_bonus` and `passes_domain_coherence`, so the new entry gains and the entries it competes with step aside. `SPECIFIC_ENTRIES` take no part in the ranking when the request does not fit them (their keywords were lifting them above `program version`). A `host:path` token now counts as remote (`has_remote_scp`), so `cp` never answers it. `uptime` answers "been up"; `free` answers "human readable"; `docker logs` answers "docker logs --tail".
- **Readers** (`src/readers.rs`): slots may declare `extract = "sys_..."`; the matcher leaves them empty and `refine` fills them after matching (git refs, stash message, push remote and branch, pid, docker cp paths, substitutions, sizes, ssh host, "as user"). `cron_line` writes the line shown under `crontab -e` ("every day at 3am" becomes `0 3 * * * /path/to/backup.sh`).
- **Assembly**: slot formats `{plus1}` ("top 10" is `head -n 11`) and `{abs}` ("older than 7 days" is `7d`); `find ... -exec rm` with a `-name` is no longer treated as acting on the whole folder.
- **Expectation changed**: `evals/safety.toml` "force push" now expects `git push --force-with-lease` (was `--force`); same warning.
- **Not handled**: schedules beyond "every N minutes/hours/day/week/month/weekday at H" (no ranges or lists); `sed` values with spaces are joined from the words between "replace" and "with"; "copy ... into container" needs a path in the request; `mount` without a mount point stays `udisksctl`.

## Completed: never mislead (requests) (2026-11-18)

Governing principle: a wrong answer is worse than "No suggestion". Findings: the "Dangerous" and "State questions" bullets of the round-3 measurement.

- **Evals**: `evals/mislead.toml` (132 cases: the fresh-tester failures, 60+ variants of state questions, casual dangerous requests, and positive controls so abstention does not eat right answers). 70 of the 98 cases of the first draft failed on the old code; `cargo test` requires 100%. The part-1 findings were also added to `evals/safety.toml` (18 cases).
- **Abstention** (`src/guard.rs`, called from `suggest`): state questions never get action entries (new `LocalMatcher::asks_about_state` / `is_action`; symptom reports like "sound isnt working" count as questions unless they ask for a change); dangerous suggestions need evidence (a named value, a typed mode or pattern, or the kind of thing named); refusals for hacking, drive formatting or wiping, and installing an operating system; safety questions about a command point to `huh explain`.
- **Truthfulness**: cp/mv onto an existing file warns; typed chmod modes (new `chmod_mode` reader; a choice slot may now take a typed value) with a warning for modes everyone can write; adding a user to a group always warns; placeholder-only lines are capped at 60%.
- **Binding** (minimal): stray words (`STRAY_WORDS`, participles, upper-case names for paths, the command's own name) never fill path or named slots; "the directory build" and "in logs" bind; `find delete` outranks `rm` for a pattern of files ("delete all .log files in logs"); recovery requests never get `cp`, `mv` or `rsync`.
- **New entries**: `wpctl get-volume`, `wpctl status`, `brightnessctl info`, `findmnt`, `systemctl running`, `xdg-settings get`, `timedatectl`, `echo $PATH`, `echo $SHELL`, `id`, `getent group`; `nc` is now a query; `df` answers "almost full"; `free` answers swap.
- **Abstention cost**: across cases, heldout, safety, slots, slots2 and intent (693 cases) no suggestion turned into "No suggestion"; one line changed for the better (`rm -r <target>` to `rm -r build`).
- **Not handled**: "is ssh running" answers with `pgrep -a ssh` (a process check, not the service); the stray-word list is hand-made; volume and brightness have no Omarchy-native query; "which port is ssh on" abstains.

## Completed: never mislead (`huh why`, round 3, 2026-11-18)

Findings of 244 real failures from fresh testers. New suite `evals/errors-round3.toml` (100 cases; outputs produced in Debian 12 and Arch containers as a normal user with core dumps disabled; pasted text for systemd-less containers, sshd key checks, Go, jest, GitHub, kubectl/terraform, a held dpkg lock and a core dump), registered in `src/eval.rs`. Before the fixes: causes 29/100, fixes 63/100. After: 100/100 and 100/100; the other four error suites stay at 100%.

- **Dangerous answers removed**: a refused `>>` gets `sudo tee -a` and says it appends, a `>` or plain `tee` says it replaces the whole file; piped without a command it says what the output does not tell and gives no Try line. `tee: file: Permission denied` is a write, not "cannot read".
- **Typos only when they are typos**: a name that is a known program, a package-manager tool or a common command (`ping`, `sudo`) is "not installed" with the install command; a file of that name in the current folder gets "run it as `./x`" (or `chmod +x x && ./x`); file-like names (`t.sh`) are never typos; a typo needs distance 1-2 and an installed candidate. A one-edit match to a common command that is not installed is worded as such.
- **Python**: `python3 -m pip` with no pip installs the distro's pip package; a failing `python3 -m venv` names the venv package from the message (`python3.11-venv`).
- **git**: `git commit` with nothing to commit and exit 1 is "made no commit", not "not an error"; a conflicting rebase is a rebase (`--continue`, `--skip`, `--abort`), not a merge; `git pull` without tracking information names the branch.
- **Admin tools** (`useradd`, `usermod`, `groupadd`, `passwd`, `mount`, `apt`, `apt-get`, `dpkg`, `systemctl`, ...) refused for lack of privileges get `sudo <your command>` instead of an `ls -ld` on the program name; `sed -i` on a file in `/etc` is explained from its temporary-file path. CRLF scripts (`bad interpreter ... ^M`, `required file not found`) get `sed -i 's/\r$//'`.
- **Non-errors**: table rows (`docker images` with repositories named `failed-jobs`) and prose sentences ("Permission denied responses should be logged") are not error lines; `free -h` is recognised as a memory table.
- **New coverage**: make `missing separator` and recipe `Error N`, `tar` option without value, jq parse errors, curl 60 (expired) and 56, ssh key permissions too open (`chmod 600`), Go `[build failed]`, jest and pytest failure summaries, `npm test` failures, "not in the sudoers file" (ask an administrator; never edit sudoers), `makepkg` without PKGBUILD, `grep: Unmatched (`, xz format, symlink loops, systemd not booted (containers, WSL), gzip not overwriting, `multiple definition`, double free and `Aborted (core dumped)`, `Too many open files`, protected branch and 403 pushes, apt `Unable to locate package`, a held dpkg lock (never delete lock files), kubectl connection refused.
- **Not handled**: the explanations of the coverage items are generic (no exit-code-specific advice for other curl codes, other Go build errors, or the many pytest failures beyond the first five names); `systemctl` without a system bus inside WSL1 is not told apart; the typo list only knows commands installed on this machine.

## Completed: Intent and catalog gaps (2026-11-18)

Items 4 and 5 of "Next (from that measurement)".

- **Evals**: `evals/intent.toml` (100 cases: the measured failures, question and instruction forms of the same topic, polite forms, off-topic requests). 74 of 100 failed on the old matcher; all pass now and `cargo test` requires 100%. Per-family cases for "is X installed", "list installed packages" and "clean up old packages" went into `evals/platforms.toml`.
- **Mood** (`matcher.rs` `QueryContext::mood`): a question about state versus an instruction, read from the opening words. Questions demote action entries by 4.0; instructions demote a bare query by 1.0 only when an action scores within 1.5 of it and the request does not ask to show or list.
- **`kind`** on catalog entries (`query`, `action`, `mixed`), derived when left out (`Command::kind`), and **`opposites`** on entries and choice options (negation-aware; see README).
- **Minimum-evidence floor** (`rests_on_generic_words`): a match built only on generic words or a shared program name is rejected.
- **New facts** `pkg_query`, `pkg_list`, `pkg_clean` for every family in `data/platforms.toml`.
- **New entries**: package installed / list / cache clean, wifi current, `nmcli radio wifi`, suspend, hibernate, `bluetoothctl` connect / disconnect / devices / info, bluetooth status, `udisksctl` mount / unmount / power-off, `umount`, sensors, `last -n`, graphics card, `<program> --version`, run script, userdel (`-r` only when the home folder is asked for), `usermod -L`, docker build, `docker rm -f`, `git branch -d/-D`, `git rebase` (`-i`), `ssh -L`, `curl -X POST` with JSON, `wl-copy`, `wl-paste`, `wpctl set-volume`, `omarchy brightness display N%`.
- **Changed expectations**: safety.toml "turn off wifi" (was none, now `nmcli radio wifi off`) and "delete user bob and home directory" (was none, now `sudo userdel -r bob` with a warning).
- **Not handled**: `brightnessctl set` on non-Omarchy systems; "who owns this file" lists the folder with `ls -l` (no file named); the mood reader knows English openers only; the floor's generic-word list is hand-made; opposites are checked only for the option the matcher would pick.

## Completed: Slot filling, round 2 (2026-11-18)

- **Evals**: `evals/slots2.toml` (51 cases: the measured failures plus variants); 37 of the first 50 failed before, all pass now and `cargo test` requires 100%. Nothing in it depends on the machine: users, groups, shells and services come from `tests/fixtures/sysinfo` (`HUH_SYSINFO_DIR` overrides the real files for manual runs).
- **Typed slots** (`type = "service" | "user" | "group" | "shell" | "container" | "package"` on a catalog slot; `src/sysinfo.rs` reads `/etc/passwd`, `/etc/group`, `/etc/shells` and the systemd unit folders, read-only). A name on the machine's list wins; an unlisted bare word never fills a service slot unless it looks like a daemon (`journalctl -u older` is gone); shell names map to their path (`bash` is `/bin/bash`).
- **Labels**: "group docker", "unit sshd", "user bob", "service web", "container db", "branch main" mark the value beside them (before or after) and are never values themselves. Several values in one sentence fill several slots ("add bob to group docker" is `usermod -aG docker bob`).
- **Exclusions**: the value after "excluding", "except", "without", "but not", "other than" is an exclude value only (`grep --exclude-dir=`, `rsync --exclude=`, `tar --exclude=`), never a target or destination.
- **Other binding fixes**: ".log" is `*.log`, not a folder; "recursively" and trailing fillers are not part of a name; `-9` is not a program name; binary files (.xlsx .pdf .zip .png ...) are not grep targets unless the request names that file; `find delete` reads its folder like `find`.
- **No more silent drops**: new on existing entries: `unzip -d`, `docker logs --tail`, `docker exec -u`, `pkill -u`, `git stash -u`, `dig @server`, `curl -C -`, `sort -u`, `tar` one member, `useradd -s`, `grep`/`rsync` exclude; "older than N" gets a "not included" note when the line has no place for it.
- **Catalog entries touched** (slots/flags only, no new entries): useradd, pkill, unzip, docker logs, docker exec, git stash, dig, curl, sort, tar, rsync, grep (new slot or flag); `type` added to journalctl, search service log, systemctl, systemctl --user, add user to group, chown, docker exec/logs/restart/stop/rm/start.
- **Not handled**: "reset to origin/main" (no catalog fit: ranking/entry), "vacuum journal logs older than 7 days" (no `journalctl --vacuum-time` entry; it now gives plain `journalctl` plus a note), "docker compose restart service web" (no compose restart entry: gives `systemctl restart web`), and containers/packages cannot be checked against a list.

## Completed: Error doctor, second measurement round (2026-11-18)

Items 1 and 2 of "Next (from that measurement)". New suite `evals/errors-measured.toml` (66 cases, outputs produced in Debian containers; pasted text for pacman, docker daemon, ssh host keys, missing dev libraries and out-of-memory kills). Before the fixes: causes 35/65 (54%), fixes 30/65 (46%). After: 66/66 and 66/66 (the suite gained one case), and the other three error suites stay at 100%.

- **Wrong answers removed**: a refused redirect or `tee` is a write (`echo x | sudo tee /etc/foo`, never `chmod +x`); with no command, `bash: /etc/foo: Permission denied` is "could not be written or run" and names the admin-rights problem; `pacman` keeps the user's operation (`sudo pacman -Syu`), and piped without a command it says "run the same pacman command with sudo" with no placeholder; `docker nosuchsub` no longer claims exit code 127 (the code is only stated when given); the docker-group sentence is labelled "On this computer" and only claimed when `/etc/group` was read; gzip/gunzip errors use gzip wording and the real file name; `-lssl` / `ssl.h` name the development package per family (libssl-dev, openssl-devel, openssl, ...), else the provides search; Python tracebacks point at the last frame outside the standard library and site-packages (including `<string>`).
- **No more "does not look like an error" without evidence**: with no exit code, only a line shaped like an error (`error:`, `fatal:`, `npm ERR!`, `Traceback`, `Name: ... reason`, a well-known reason such as `Permission denied`) counts as a failure. Output with none is "not an error" only when it is recognised success text; otherwise huh says it does not recognise the output and quotes the most error-like line. Sentences that merely contain "error" or "failed" are no longer called failures.
- **Garbled subjects fixed**: `tar: x.tar: Cannot open`, `sed: can't read x`, `git switch nosuch`.
- **New coverage**: curl exit 28, `Host key verification failed` (`ssh-keygen -R`, with the security meaning), git (no push destination, non-fast-forward, `branch -d` not merged, clone of a missing repository, `add nofile`, remote already exists, `commit` with nothing to commit, `merge nosuch`, command typos), `cargo biuld`, rustc error location, gcc errors and undefined references, node Reference/SyntaxError, npm E404, jq and awk syntax, `find -nme`, `mkdir` exists, `rm dir`, `cat dir`, `cp dir`, `sed` unterminated, `curl` with no URL, `systemctl cat` of a missing unit, `make` with no rule, docker unknown command and missing container, pip "No matching distribution", Rust panics, Node heap and Go out-of-memory.
- **Next**: `cargo`/`pytest` failures with many errors still report only the first; the dev-library table covers about a dozen libraries; `ssh-keygen -R` ignores non-default ports.

## Completed: Safety (2026-11-18)

- **Evals**: `evals/safety.toml` (110 cases, each with the expected command line and, where it matters, `warn = true/false`); 89 of the first 100 failed on the old matcher, all pass now and `cargo test` requires 100%.
- **WARNING**: a command, option or flag can carry its own `warning` text (`catalog.rs`, `assemble::build`); per-option warnings make `systemctl stop/disable/restart`, `git reset --soft/--hard`, `git push --force`, `docker system prune --volumes` and `chmod/chown -R` warn while `status`, `start` and plain `push` stay quiet. Marked destructive: `reboot`, `shutdown`, `kill`, `pkill`, `docker stop`, `docker compose down`, `tmux kill-session`, `virsh destroy/shutdown/reboot`, `package remove`, `omarchy refresh`, `omarchy system logout`, `append to system file`, `modprobe`, plus the new `find delete`, `kill port`, `git restore`, `git clean`, `docker image/container/system prune`, `gio trash empty`.
- **Slot readers** (`src/extract.rs`, `extract = "..."` on a slot): `shutdown_time` (`+10`, `23:00`), `file_age` (`+7` older than 7 days, `-7` last 7 days / this week, `-1` today), `file_glob`, `port`, `host`, `domain`, `service`, `count`. A number after "port" can no longer fill a PID slot.
- **New catalog entries**: `shutdown cancel`, `systemctl status`, `systemctl failed`, `find delete`, `kill port` (`fuser -k N/tcp`), `ss port`, `nc`, `git restore/stash/reset/clean`, `git push -u origin HEAD`/`--force` flags, `git diff --staged`, `git log -n N`, `docker image prune`, `docker container prune`, `docker system df`, `gio trash empty`, `gio list trash`, `cd -`, `who`, `iostat`, `useradd`, `dig`, `curl -I`.
- **Matcher**: reboot/shutdown only when the request is about the computer (never a service, VM or device); a service name beats `omarchy restart`; recovery wording (undo, recover, restore, accidentally) rejects every deleting command; a flag written in the request (`tail -f`) is always applied; printer, scanner, playlist and "print a document" are off topic; `xdg-open` needs something to open; `omarchy restart/refresh` need a named part. "What does <command> do" is answered with a `huh explain` hint.
- **Existing eval expectations changed**: `shutdown now`/`reboot` became `sudo shutdown -h now`/`sudo shutdown -r now` (they were unsafe: no sudo, no time).
- **Not done**: `mv`, `cp`, `tar -x` and `unzip` can overwrite files and still print no WARNING; there are no `dd`/`mkfs` entries; the binder still mis-fills some bare names ("disable docker at boot" needs the word "service").

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

## Completed: sysadmin round (values) (2026-11-18)

Named values that were dropped, misplaced or left as placeholders (`evals/slots3.toml`, 59 cases; 19 of the first 48 failed before the fixes, none after).

- **Roles**: "named/called X" is the program name for `pkill` (never its `-u` user); docker, git, tmux and similar topic words are a name to act on when the request points at them ("is docker running", "enable docker at boot"); "all files" / "every folder" are never a file name; "on/for <file>" fills a path slot for chmod/chown (`chmod 600 on id_rsa`).
- **New slots on existing entries**: `docker exec` gets a `shell` slot (default `sh`, so `exec bash in container db` works), `curl` an `outdir` slot (`--output-dir`), `journalctl` a `priority` slot (`-p err` from "errors"/"warnings"/"critical"), `ssh tunnel` a `target` slot (default `localhost`) so `-L 8080:db01:5432 jump` is kept whole.
- **Readers** (`extract.rs`): `forward_spec` parses "<local> to <host>:<port>", "<local> to <host> <port>" and "on/via/through <host>" for `local_port`, `remote_port`, `forward_target` and `tunnel_host`; `log_priority`; `dig` takes a server after "at"/"with" when it is an IP.
- **Never silent**: "last/first/top N lines" with no place in the line now gets a `not included` note.
- `pkill` keywords gain "force quit". One expectation changed: `journalctl errors from last boot` now keeps the severity (`journalctl -b -p err`).

Not done: grep has no line-limit flag (only the note); `curl --output-dir` needs curl 7.73+.

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

1. ~~**Safety**~~ Done 2026-11-18 (see "Completed: Safety"). Original findings:
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
