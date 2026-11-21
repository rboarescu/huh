# huh: roadmap and status

## Completed: real-world applications — apps, games, media & custom pet sprites (Phase 6, 2026-11-21)

1. **Live assistant & `huh why` upgrades for everyday desktop, gaming & media workflows. Done 2026-11-21.**
   - Done (`catalog/*.toml`, `data/platforms.toml`, `src/knowledge.txt`, `src/hints.txt`, `src/matcher.rs`, `evals/everyday.toml`, `src/eval.rs`): expanded the embedded catalog from 302 to **310 entries** with `dosbox`, the `flatpak` suite (`flatpak-install`, `flatpak-run`, `flatpak-search`, `flatpak-list`, `flatpak-uninstall` with `risk = "big system change"`), `mpv` (`--no-video`, `--loop-file=inf`), `appimage-run` (`env APPIMAGE_EXTRACT_AND_RUN=1 {appimage}`), and expanded `xdg-open` keywords; added `dosbox`, `mpv`, `wine`, and `fuse2` (`libfuse2t64` + `[notes]` for `libfuse2`) to `data/platforms.toml`; enriched `@ffmpeg` and added `@dosbox`, `@flatpak`, `@mpv`, `@vlc`, `@gio`, `@nano`, and `@huh` in `src/knowledge.txt`; added the `wine` hint (`winecfg`) to `src/hints.txt`; added tight domain-coherence guards, `unmodeled_checks`, and `flatpak` intent bonuses in `src/matcher.rs`; and verified 17/17 cases in `evals/everyday.toml` (`everyday_evals_all_pass`) with zero regressions across `evals/hints.toml` and all existing suites.
   - Done (`src/doctor.rs`): added `"dosbox"`, `"flatpak"`, `"mpv"`, `"vlc"`, `"ffmpeg"`, `"xdg-open"`, `"wine"`, and `"gio"` to `COMMON_COMMANDS`, and added `diagnose_appimage_and_exec_format` before `diagnose_not_executable`:
     - AppImage missing FUSE 2 errors (`libfuse.so.2`, `AppImages require FUSE`, `cannot mount AppImage`, `fusermount` + `appimage`, guarded against conversational prose) explain FUSE 2 (`fuse2` / `libfuse2`) and suggest `Try: <target> --appimage-extract-and-run` (with `./app.AppImage` fallback).
     - `Exec format error` runs before exit-126 `chmod +x`: `.exe`/`.com`/`.bat` targets explain classic DOS (`dosbox`) vs Windows (`wine` / Steam Proton) with `Try: dosbox <target>`, while foreign-architecture or headerless binaries suggest `Try: file <target>`.
2. **Non-linear `unlock_after` progression & 3 Real-World Lab Units (Units 9–11, including Bonus Chapter "Paint Your Own Pet"). Done 2026-11-21.**
   - Done (`src/lessons.rs`, `src/learn.rs`, `src/progress.rs`): added `Unit::unlock_after` and `Unit::is_side_quest()` with empty/self/unknown/cycle validation in `lessons::validate`; updated `learn.rs` prerequisite lookups, locked-unit messages, dynamic `1-{} unit` footer, and `[real-world]` labels while keeping `first_open_lesson` focused on core Units 1–8 first and awarding `shell_grad` for completing all 8 core units; and expanded `BADGES` in `src/progress.rs` from 16 to **19 badges** (`game_runner`, `media_wrangler`, `pixel_painter`).
   - Done (`lessons/09-apps-and-games.toml`, `lessons/10-media-and-everyday.toml`, `lessons/11-paint-your-pet.toml`): expanded `/mode lab` to **11 units, 33 lessons, 165 questions, 9 hands-on practice labs** with 3 real-world tracks unlocked right after Unit 1 (*Finding Your Way*):
     - **Unit 9: *Apps & Games on Linux*** (`game_runner`, practice lab *Game Night Setup*): AppImages (`chmod +x`, `./`, `--appimage-extract-and-run`, `libfuse.so.2`), Flatpak & Flathub + `{pkg_install:dosbox}`, and classic DOS games (`unzip -d`, `file`, `dosbox`).
     - **Unit 10: *Media & Everyday Files*** (`media_wrangler`, practice lab *Downloads Cleanup*): `xdg-open`, `file`, `gio trash`, `mpv`/`vlc`, `du -sh`, and `ffmpeg` (`-vn`, `-ss`/`-t`, `-crf`).
     - **Unit 11: *Paint Your Own Pet*** (`pixel_painter`, practice lab *Sprite Studio*): completed the planned *Bonus Chapter* teaching `huh pet --customize`, `~/.config/huh/sprites/`, `head`/`nano`/`grep -n`/`wc -l` on pixel/palette TOML sheets, `cp` backups, `diff -u`, `mv` restore, and safe 32×32 fallback validation.

## Completed: fuzzy slash commands, arrow-key navigation & rotating tooltips (Phase 5, 2026-11-20)

1. **Live `/` fuzzy slash dropdown + Option A fallback. Done 2026-11-20.**
   - Done (`src/slash.rs`, `src/tui.rs`, `src/pane.rs`): typing `/` in the companion pane opens a live inline fuzzy dropdown (up to 6 rows) across all 11 slash commands (`/mode`, `/watch`, `/lab`, `/safety`, `/theme`, `/pet`, `/speed`, `/quiet`, `/tips`, `/find`, `/help`) plus Stage 2/3 sub-arguments (`/theme cat`, `/pet sp tur`, `/mode `, `/safety `, `/speed `, `/quiet `, `/tips `, `/lab `, `/pet style `). `Tab` completes and advances stages; `↑`/`↓` navigates the dropdown or 100-item prompt history; `Esc` dismisses the dropdown or clears the buffer.
   - Done (`src/slash.rs`, `src/pane.rs`): submit-time fuzzy resolution (Option A fallback, working identically in cooked/non-TTY sessions) runs unique prefix/subsequence/1-edit typo matches immediately with a dim `→ /canonical` note (`/th cat` → `→ /theme catppuccin`, `/thme` → `→ /theme`), opens a picker on bare `/` or ambiguous prefixes (`/s`, `/t`), and rejects unknown words (`/finder`, `/modeled`) while never hijacking multi-segment Unix paths (`/etc/hosts`, `/usr/bin/env`).
2. **Inline arrow-key navigation across Lab, slash pickers & `huh setup`. Done 2026-11-20.**
   - Done (`src/tui.rs`, `src/learn.rs`, `src/pane.rs`, `src/setup.rs`): zero-crate Linux raw-mode key decoder (`parse_key_bytes`, `RawModeGuard`) and inline `select_menu` (`↑`/`↓`/`j`/`k`/`Home`/`End` + `Enter` + `Esc`/`q`, plus number/letter hotkeys and `/` slash shortcut) wired across Lab unit/lesson/practice menus, multiple-choice questions, out-of-hearts choices, bare slash pickers (`/mode`, `/theme` pre-selecting the current theme, `/speed`, `/pet species` showing unlock status, `/pet style`), and `huh setup` (plus hook confirmation), with numbered plain-text fallback on non-TTY streams and zero `ESC 7`/`ESC 8` outside `src/pin.rs`.
3. **Rotating header tooltips & `/tips`. Done 2026-11-20.**
   - Done (`src/tips.rs`, `src/pin.rs`, `src/prefs.rs`, `src/pane.rs`, `src/setup.rs`): Row 7 of the pinned header (`  tip: ...`) cycles through 18 concise usage tips every ~15 s (`ROTATE_TICKS = 68`), refreshing only Row 7 in place with zero flicker on the logo or Kitty sprite and updating the DECSTBM scroll region (6 ↔ 7 rows) when toggled. Controlled live via `/tips [on|off|next]`, persisted in `prefs.toml` (`tips`), configured in `huh setup`, or disabled via `tips = false` in `~/.config/huh/config.toml` or `HUH_NO_TIPS=1`.

## Completed: unified companion pane, L2b logo, 32×32 Pebble sprites, typewriter & multi-shell setup (Phase 4, 2026-11-20)

1. **Unified companion pane (`huh`, replacing `huh tutor` and `huh learn`). Done 2026-11-20.**
   - Done (`src/main.rs`, `src/pane.rs`, `src/launch.rs`, `src/learn.rs`; removed `src/tutor.rs`): dropped `huh tutor` and `huh learn` subcommands in favor of bare `huh`, which launches the unified companion pane side-by-side (`tmux` 35% split, tiling desktop window with `huh` / Ghostty `huh.pane` class, or private `tmux -L huh` session with `mouse on`, `allow-passthrough on`, and `HUH_PANE=1`). Supports `--here`, `--mode watch|lab`, `--tty <TTY>`, `--quiet`, and `--dry-run` (`HUH_DRY_RUN=1`), while keeping all one-shot CLI entry points (`cmd | huh`, `huh "..."`, `huh explain`, `huh why`, `huh find`, `huh tools`, `huh pet`, `huh themes`, `huh welcome`, `huh setup`, `huh shell-init`) instant.
   - Done (`src/pane.rs`, `src/learn.rs`, `src/progress.rs`): unified `watch` (default) and `lab` modes inside one `poll`-driven event loop (`220 ms` ticks). Graceful no-hook startup (`[WATCH] no hook`) prints a concise `huh setup` tip and attaches automatically once a recorded shell starts; inside `lab` mode, hands-on sandbox practice labs watch the adjacent shell live, surface gentle `huh why` nudges on non-zero exits, and auto-complete when all sandbox checks pass.
   - Done (`src/pane.rs`, `src/learn.rs`, `src/prefs.rs`): instant slash commands available at every prompt in both `watch` and `lab` modes — `/mode [watch|lab]` (interactive 2-option selector or `/watch`, `/lab`), `/safety [normal|expert]`, `/theme [name]`, `/pet [status|name|species|style|customize]`, `/speed [off|fast|normal|slow|<ms>]`, `/quiet [on|off]`, `/find [words]`, and `/help`.
2. **L2b logo & mode-aware pinned header. Done 2026-11-20.**
   - Done (`src/welcome.rs`, `src/pin.rs`): upgraded the animated logo to the 4×20 `L2b` slab glyph (both `h` ascenders intact; rounded `?` flipping to bold `!`) with `Trigger::Pane`; updated the DECSTBM pinned header to show `[WATCH]` / `[LAB]` status lines, place Pebble starting at column 25 (when width ≥ 38 cols), and manage Kitty Graphics placement/cleanup across `/find` suspend/resume, resizes, and exit.
3. **Pebble: 32×32 Kitty Graphics + animated ASCII fallback & dual-track unlocks. Done 2026-11-20.**
   - Done (`build.rs`, `sprites/*.toml`, `src/sprite.rs`, `src/pet.rs`, `src/style.rs`): renamed default penguin chick from `Pip` to **Pebble** (`DEFAULT_NAME = "Pebble"`) with the finalized mixed roster — **Penguin** (`penguin`, `P1`/`A1`), **Cat** (`cat`, `P4` 16-Bit RPG / `A4` UNIX BBS), **Turtle** (`turtle`, `P1`/`A1`), and **Owl** (`owl`, `P4`/`A1`).
   - Done (`src/sprite.rs`, `src/style.rs`): embedded `sprites/*.toml` at compile time (`EMBEDDED_SPRITES`) and rendered zlib-compressed 32×32 RGBA frames via Kitty Graphics Protocol in Ghostty, Kitty, and WezTerm (including inside `tmux -L huh` via DCS `\x1bPtmux;...` passthrough), tinted with live theme RGBs (`style::set` + `active_sprite_rgb`) and semantic tokens (`accent`, `warning`, `success`, `note`, `dim`, `shadow`). Falls back cleanly to animated 4×10 pure-ASCII art (`pet::art_frame`) everywhere else.
   - Done (`src/sprite.rs`, `src/pet.rs`, `src/main.rs`): added customizable overrides under `~/.config/huh/sprites/<species>.toml` (`huh pet --customize` / `/pet customize`, non-overwriting `export_template`, silent fallback on invalid edits), `--style <auto|pixel|ascii|off>`, and dual-track species unlocks via either `lab` unit badges or passive `watch` milestones (Cat: 25 unique commands; Turtle: 100 commands or 7-day streak; Owl: 250 commands or 5 warnings heeded).
4. **Variable-rate typewriter. Done 2026-11-20.**
   - Done (`src/typewriter.rs`, `src/pane.rs`, `src/learn.rs`): ANSI-safe token streaming (`Token::Escape` / `Token::Char`) targeting a ~2s starter budget (`normal` = 2000 ms, `fast` = 800 ms, `slow` = 3200 ms, `off` = 0 ms, or `0–5000` ms), pacing 16–35 ms/char on short blocks and 60fps chunking on long ones. Applies strictly to companion-pane narrative (slash commands, `?tools`, menus, and one-shot CLI commands stay instant), flushes on Enter or new shell command arrival, and respects `HUH_NO_TYPEWRITER=1`, `HUH_TYPEWRITER_MS`, `typewriter = false`, and `prefs.toml`.
5. **Persistent pane state vs user config, Omarchy-style `huh setup` & multi-shell hooks. Done 2026-11-20.**
   - Done (`src/prefs.rs`, `src/style.rs`, `src/mode.rs`): live `/theme`, `/safety`, `/speed`, `/pet style`, `/quiet`, and `last_mode` persist atomically in `~/.local/state/huh/prefs.toml`, keeping `~/.config/huh/config.toml` strictly user-owned (`default_mode = "watch" | "lab" | "last"`).
   - Done (`src/setup.rs`, `src/session.rs`): added `huh setup` (Omarchy-style guided defaults, Kitty capability detection, `prefs.toml` save, `config.toml` preservation, explicit `[y/N]` idempotent marked-block rc updates) and multi-shell `huh shell-init [--shell auto|bash|zsh|fish] [--hooks]` supporting `bash` (`~/.bashrc`), `zsh` (`${ZDOTDIR:-~}/.zshrc`), and `fish` (`~/.config/fish/config.fish`).

## Planned: next ideas

1. **Pebble-gifting affection milestones:** Pebble gifts the user shiny pebbles as a sign of affection (like real Gentoo/Adélie penguins), tracked locally.
2. **Zelda-style intro animation/promo:** *"It's dangerous to go alone. Here, take this."* handing over a funny old-wisdom/meme terminal relic → user says *"huh?"* → `?` flips to `!` and prints *"huh! installed, now you're ready."*

*(Completed in Phase 6 above: Bonus Chapter "Paint Your Own Pet" shipped as Unit 11 `lessons/11-paint-your-pet.toml`.)*

## Completed: appeal (Radu, 2026-11-19)

1. **Baby-blue look as the standard. Done 2026-11-19.**
   - Done: a `babyblue` theme (truecolour, 256-colour fallback, deeper shades on a light background) is the default. The Omarchy-following `auto` and the others stay available.
   - Done: the ASCII "huh" logo is shown in that colour and stays pinned at the top of the tutor pane while its content scrolls, using a terminal scroll region (DECSTBM) so it is not redrawn. Off with `HUH_NO_PIN=1` or `pin = false`.
2. **A pet: a tamagotchi-style `#!` mascot. Done 2026-11-19.**
   - Done (`src/pet.rs`, `src/date.rs`, `src/pin.rs`, `src/tutor.rs`): default **Penguin chick** (`penguin`, named `Pip`, pure ASCII with `#!` on its belly); other species (`cat`, `turtle`, `owl`) unlock via `huh learn` badges (`pathfinder`, `pipe_plumber`, `night_watch`).
   - Done: lives beside the logo in the `huh tutor` pinned header (auto-hides when narrower than 38 columns; updated in place via `pin::refresh_sequence()` without touching DECSTBM). Off with `HUH_NO_PET=1`, `pet = false`, or `pet = "off"`.
   - Done: 6 moods (`idle`, `happy`, `sleepy`, `hungry`, `alert`, `celebrating`); fed by `exit 0` commands in `huh tutor` (alerts on non-zero exit or `WARNING` without punishing stats; lazy decay floored at 10 fullness / 20 happiness so it never dies).
   - Done: local state only in `~/.local/state/huh/pet.toml` (atomic writes); `huh pet [--name <NAME>] [--species <SPECIES>]` and `pet` / `/pet` inside `huh tutor`.
3. **Better than a pet: a Linux tutorial for kids and beginners, with scoring and levels. Done 2026-11-19.** Mechanics borrowed from Duolingo, not its branding:
   - **Phase 2: `huh learn` core engine + Units 1–3. Done 2026-11-19.**
     - Done (`build.rs`, `lessons/01..03-*.toml`, `src/lessons.rs`): embedded 3-unit, 9-lesson, 45-question curriculum covering *Finding Your Way* (`pwd`, `ls`, `cd`), *Files & Folders* (`mkdir`, `touch`, `cp`, `mv`, `rm`, `cat`, `head`, `tail`), and *Searching & Pipes* (`grep`, `find`, `wc`, `sort`, `uniq`, `|`, `>`, `>>`), validated at test time against `huh`'s parser, explainer, outputs, and risk engines.
     - Done (`src/srs.rs`, `src/progress.rs`, `src/learn.rs`, `src/main.rs`, `src/tutor.rs`): structural shell-command grader (`shell::parse`, flag/quoting/pipe/redirect normalization, zero execution); 4 question types (`explain_choice`, `type_command`, `read_output`, `spot_danger`) reusing `explain`, `outputs`, and `risk_kinds`; 5-heart lessons with immediate feedback, once-only miss re-queue, and **no lockout timer** (instant heart-free missed-item review or fresh retry at 0 hearts); XP + combo/flawless bonuses + daily goal (`daily_goal` in config) + UTC streaks with freezes + 6-box Leitner spaced review (`huh learn review`) + badges (unlocking `cat` and `turtle` in `pet.toml`); `huh learn progress`, interactive menu, and `/learn` inside `huh tutor` with pet reload/celebration.
   - **Phase 3: hands-on practice sandbox + Units 4–8. Done 2026-11-19.**
     - Done (`src/practice.rs`, `src/learn.rs`, `src/progress.rs`, `src/main.rs`, `src/tutor.rs`): `huh learn practice [unit]` (and `/learn practice [unit]` in tutor) prepares a safe throwaway practice folder under `$XDG_DATA_HOME/huh/practice` (default `~/.local/share/huh/practice`) using pure `std::fs` with canonicalized symlink-aware boundary guards and automatic cleanup (zero subprocesses); evaluates 9 check kinds (`dir_exists`, `file_exists`, `not_exists`, `file_contains`, `file_equals`, `file_mode_has`, `file_mode_eq`, `is_symlink`, and `recorded_output` reading `session.rs` / active recordings with a built-in check-question fallback when recording is off); awards `XP_PRACTICE_CLEAR` (30 XP), the `hands_on` badge, and feeds/celebrates the `#!` pet.
     - Done (`lessons/01..08-*.toml`, `src/lessons.rs`): full 8-unit, 24-lesson, 120-question curriculum plus 6 hands-on `[practice]` labs (Units 1, 2, 3, 4, 5, 7), adding *Reading Permissions* (`locksmith`), *Processes & Disk* (`process_tamer`), *Packages & Manuals* (`package_pro`, distro-aware `{pkg_*}` resolution via `src/platform.rs`), *Archives & Links* (`archive_ace`), and *Networking & Stay Safe* (`night_watch` unlocking Snowy Owl `owl`, plus `shell_grad` upon completing all 8 units; 16 badges total).
   - **The pet** reacts to lessons, practice labs, and badge unlocks, tying idea 2 directly into idea 3.
   - **Scope:** offline and local-only, kid-friendly wording, stored in `~/.local/state/huh/progress.toml` (with throwaway labs in `~/.local/share/huh/practice`).

## Measured: round 5 (2026-11-18)

Fresh cases written by new testers who could not see the evals, judged by TypeSafe. The "unsure" answers (71) were resolved by a Sonnet review, which agreed with the judge on 24 of 30 sampled verdicts: all 17 request verdicts, and 7 of 13 error verdicts. The judge marks error answers that only quote the right error line as bad; Sonnet calls them good.

| Fresh cases | Round 4 | Round 5 |
|---|---|---|
| Beginner requests | 79% | 74% (148/200) |
| Sysadmin requests (round 5: 60 of 160 with several values, 10 typed as command lines) | 56% | 47% (76/160) |
| `huh why` (failures and normal output) | 58% | 58% (116/200) |
| Normal output called a failure | 11/48 | 10/50 |

Conclusion: fresh-wording accuracy is flat. Each round fixes what it measured: the old round-4 requests went 56% → 65% after this round's fixes. New wording keeps failing at a similar rate, so hand-written rules have reached their limit.

Safety:
- Request-side safety held, apart from these new findings:
  - Requests that only ask to look got a command that changes something: "show permissions of /etc/shadow" → `chmod 600 /etc/shadow`; "how big is / verify the archive …" → `tar -xzf`. "show", "verify" and "how big" count as instructions, so the question rule never applies.
  - `huh why` suggests `sudo usermod -aG docker $USER` without saying that the docker group is root-equivalent.
- Typed values: 27 of 40 losses are now flagged with a "not included" note. Bare names (`sshd`, `bob`, `eth0`, `v1.4.2`, `/`) are still lost silently.

Transcripts are in `~/.local/share/huh-dev/measure-2026-11-18-round5/`.

## Completed: typing in the tutor pane (2026-11-19)

`huh tutor` now also reads lines typed in its own pane (a stdin reader thread beside the poll loop; only when stdin is a terminal; Ctrl+D ends the typing, Ctrl+C still stops the tutor). A dim `huh> ` prompt follows each block. A command line gets the `huh explain` rendering; plain words get the `huh "..."` suggestions (WARNINGs, hints, current mode; `main::print` became `render_outcome`, which returns the text); `?words` the `huh tools` filter capped at 20 lines; `why` the error doctor on the last finished command; an empty line the last block again in full even under `--quiet`; `/find [words]` opens `huh find` and returns; `help` lists these. Printing is serialised with a mutex, so a block from the followed terminal never lands inside an answer. The handling is a pure function (`tutor::answer_line`: line, state and last command in, text out) with a test per branch. Also fixed: a request with no letters at all (`huh "/"`, `huh "!!"`) panicked in the matcher.

Not verified: how the prompt and a block arriving while you type look in a real terminal (the prompt line is wiped with `\r` and an erase-line when a block comes in), and `/find` with fzf inside the tutor (only the plain finder was tried).

## Completed: `huh find` (2026-11-19)

New `src/find.rs`: a fuzzy finder over the catalog (name, description, keywords) and every installed program (the `huh tools` list). One line per item, `name — description [catalog|installed]`; a program with a catalog entry is listed once. Scoring is a subsequence match with bonuses for word starts and consecutive letters (descriptions and keywords allow only short gaps), name over description, catalog entries first among equals. With `fzf` installed (`launch::fzf_argv`, run by `launch::run_fzf`; the process test stays strict) it gets the list, the words as `--query`, a `--preview` of `huh find --describe {1}` on the right (55%) and the header "Enter: details   Esc: quit"; without it a line-based finder shows the top 15 numbered matches, a number gives the details, Enter searches again and `q` quits (a pure state machine, `find::step`). Piped, it prints the matching lines. `huh find --describe NAME` prints at most about 25 lines: description, installed or the install hint, the command template and WARNING with its risk category for catalog entries, up to 6 flags (built-in table, then man page) and "More: man NAME". Nothing picked is run.

Not verified: the fzf picker and preview in a real terminal (only the argument list is tested), and the plain finder beyond piped input.

## Completed: colour themes and welcome animation (2026-11-18)

New `src/style.rs`: output is styled by semantic role (command, description, part, value, WARNING, risk category, note, cause, `Try:`, dim, header, success, accent) and a theme maps roles to looks. Themes: `auto` (terminal's 16 ANSI colours, default), `omarchy` (read from the current Omarchy `colors.toml` at run time, falls back to `auto`; red and green are swapped back if the theme has them in the wrong slots), `dark`, `light`, `nord`, `gruvbox`, `catppuccin`, `mono`. Selected by `--theme`, `HUH_THEME` or `theme =` in config.toml; `huh themes` shows samples. Colour is off for pipes, `--first-line-only`, `NO_COLOR`, `TERM=dumb` and `--color never`; `--color always` forces it. Nothing is styled until `main` initialises the style, so all tests and evals see plain text.

New `src/welcome.rs`: a 10-frame, about 0.6 s logo animation (5 lines tall, 33 columns wide; "?" turns into "!") and a welcome of 5 lines. Plays at `huh tutor` start, on the first interactive run (`~/.local/state/huh/welcomed`) and on `huh welcome`; skipped for non-terminals, `--first-line-only`, narrow terminals, `TERM=dumb`, `HUH_NO_ANIMATION=1`, `animation = false`. Any key skips it (termios and poll through libc FFI, Linux only); the cursor, colours and terminal mode are restored on exit and on Ctrl+C.

Not verified on a real desktop: how each theme looks in Ghostty and on a light background, and the tutor animation in a `--side` pane.

## Completed: `huh tutor --side` (2026-11-18)

One command opens the tutor beside the current terminal. Inside tmux it splits the pane (35% on the right, focus stays); on Hyprland, sway or i3 it opens a detached terminal window titled/classed `huh-tutor` (xdg-terminal-exec, `$TERMINAL -e`, or the first installed of ghostty, kitty, alacritty, foot, wezterm, konsole, gnome-terminal, xterm); elsewhere with tmux it execs a session on a private server (`tmux -L huh-tutor -f /dev/null`, mouse on, status line hint, shell left, tutor right, closing the shell ends the session); with neither it prints how to install tmux. `--dry-run` / `HUH_DRY_RUN=1` print the command instead of starting it. All process starting lives in `src/launch.rs`, and a test keeps every other module free of `std::process::Command`. `huh tutor --tty` now waits up to 5 s for the recording of a just-opened shell.

Not verified on a real desktop: window placement, the Ghostty/xdg-terminal-exec class and title taking effect, and the tmux session end to end (only dry runs and unit tests so far).

## Completed: safety hardening, items 1-3 (2026-11-18)

Used Claude Code's published auto-mode risk categories (`~/.local/share/huh-dev/auto-mode-defaults.json`, 72 `soft_deny` rules) as a checklist only. huh's own 14 categories, wording and code are original (`src/risk_kinds.rs`); no rule text was copied. Of the 72, about 18 concern shell commands; the rest are about agents, connected apps, browsers and cloud consoles.

1. **Audit.** 432 command lines were run through `huh explain`; the table shows the categories and what was missing.
2. **Gaps closed.** 159 lines that had no specific warning (or only the generic `sudo` one) now have one. New code is in `src/risk_extra.rs`, read from the words of the line, so it works for programs without a knowledge section. Request side: new `warning`/`risk` on catalog flags and slots (`curl -k`, `ssh -o StrictHostKeyChecking=no`, `docker run --privileged`, `-p` published ports, `git push` to main, `--no-verify`) and on entries (`ssh -R`, `ssh-copy-id`, `crontab -e`, shell alias, `systemctl enable`), plus new entries for `python3 -m http.server`, `ngrok` and clearing the shell history. `huh why`: a `Note (category)` under any risky `Try:` line.
3. **Category named everywhere.** `WARNING (category):` for requests, `! [category]` for `huh explain`, `Note (category):` for `huh why`. A test fails when a catalog warning has no `risk`; every `Warning` in code carries a `Risk` by type.

Existing expectation changes: none in the eval files. Warning text did not change; only the category prefix was added. One bug fix: `docker run -p 8080:80` no longer also gets the "password on the command line" warning (the `-p` password rule now applies to `mysql`/`mariadb` only; `docker login -p` is covered by a new rule).

| Category | Examples a user is likely to meet | Covered before? | Added |
|---|---|---|---|
| destroys data | `rm -rf`, `find -delete`, `dd of=/dev/..`, `mkfs`, `git clean -fdx`, `docker volume rm`, `redis-cli FLUSHALL`, `unzip -o`, `gpg --delete-secret-keys`, `cryptsetup luksFormat`, `hdparm --security-erase` | yes for the first group (explain and requests) | `docker volume rm`, `FLUSHALL`, mongo drops, `unzip -o`, `gpg --delete-secret-keys`, `cryptsetup`, `hdparm`, `nvme format`, `pacman --overwrite` (9 lines) |
| rewrites history | `git reset --hard`, `push --force`, `rebase`, `commit --amend`, `filter-branch` | yes | `git update-ref -d` (1) |
| runs code from the internet | `curl \| sh`, `bash <(curl ..)`, `eval "$(curl ..)"` | `explain`: yes | `npx`/`bunx`/`uvx`/`pipx run`/`pnpm dlx`, `deno run <url>`, `pip install git+..`, `cargo install --git`, `pip --extra-index-url` (6 lines) |
| runs unread code | `eval`, `python -c`, `perl -e`, package install scripts, `dpkg -i` | yes | `awk system()`, `php -r`, `LD_PRELOAD=` (4 lines) |
| gives away access | `chmod 777`, `chmod u+s`, `usermod -aG docker`, `sudo`, `ssh-copy-id` | explain: chmod, setuid, `sudo`, generic `usermod`; requests: `add-user-to-group`, typed `chmod` modes | root-equivalent groups named (`docker`, `sudo`, `wheel`, `lxd`..), `visudo`, writes to `/etc/sudoers*`, `authorized_keys`, `ssh -A`, `docker run --privileged/--cap-add/--pid=host`, `setcap`, `umask 000`, `nc -e` (9 lines; requests: `--privileged` flag, `ssh-copy-id`) |
| weakens security | `curl -k`, `wget --no-check-certificate`, `StrictHostKeyChecking=no`, `ufw disable`, `setenforce 0`, `iptables -F`, `--no-verify`, `--break-system-packages` | `curl -k`, `wget`, `ssh -o StrictHostKeyChecking=no`, `ufw`, `iptables -F`, `--break-system-packages` | `git config http.sslVerify false`, `GIT_SSL_NO_VERIFY`, `NODE_TLS_REJECT_UNAUTHORIZED=0`, `npm strict-ssl false`, `UserKnownHostsFile=/dev/null`, `scp`/`rsync` host-key off, `setenforce 0`, AppArmor off, `systemctl stop firewalld`, `--no-verify`, `--allow-unauthenticated`, `--nosignature`, `pip --trusted-host`, `PATH=.:`, `ssh-keygen -N ''`, `sysctl` hardening keys, `chattr -i`, `git safe.directory`, `deno -A` (24 lines; requests: `curl -k`, `ssh` host-key flag, `--no-verify`) |
| exposes a service | `docker run -p`, `python3 -m http.server`, `--host 0.0.0.0`, `ngrok`, `ssh -R`, `nc -l` | `explain`: `docker run -p`, `ufw`/`iptables` generic | `http.server`, `php -S`, `--host/--bind 0.0.0.0`, `vite --host`, `http-server`, `ngrok`, `cloudflared`, `tailscale funnel`, `ssh -R`/`-L 0.0.0.0`, `nc -l`, `socat`, `kubectl port-forward --address`, `--network host`, `firewall-cmd --add-port` (18 lines; requests: `-p` slot, `ssh -R`, `http.server`, `ngrok`) |
| leaks secrets | `--password`, `export TOKEN=`, `user:pass@` URLs, `cat ~/.ssh/id_rsa` | passwords and tokens on the command line | private keys and credential files printed or uploaded (`cat`, `scp`, `curl -T/-F`), `X-Api-Key:` headers, token-shaped words (`ghp_`, `AKIA`..), `echo $TOKEN`, `gh auth token`, `kubectl get secrets -o yaml`, `credential.helper store`, `gpg --export-secret-keys`, `docker login -p`, `sshpass -p`, archives of `~/.ssh` (26 lines) |
| hides tracks | `history -c`, `unset HISTFILE`, `rm ~/.bash_history`, `journalctl --vacuum-*`, truncating logs | requests: journal vacuum only | all of those in explain, `dmesg -C`, `auditctl -e 0`, stopping `auditd`/`rsyslog`, `touch -d`, redirects onto history or logs; request: clear history (15 lines) |
| persists | `crontab -e`, `>> ~/.bashrc`, `systemctl enable`, writes to `/etc/systemd/system` | explain: startup files, git hooks, `crontab -` | `crontab -e`, `at`, `loginctl enable-linger`, systemd/cron/autostart paths; requests: `crontab -e`, alias, `systemctl enable` (3 lines) |
| affects other people or production | `git push origin main`, `npm publish`, `docker push`, `kubectl apply`, `terraform apply`, cloud deletes | explain: `npm publish`, `kubectl`, `terraform`, `helm`, `gh pr merge` | `cargo publish`, `twine`, `gem push`, `docker push`, push to main/master/develop/.., `kubectl config use-context`/`--context prod`/`cordon`/`patch`/`edit`, `aws`/`gsutil`/`gcloud`/`az` deletes, `fly`/`vercel`/`cdk`/`pulumi` deploys, `ansible-playbook`, `gh repo delete`/`--public`, `git remote set-url` (23 lines; requests: push to a shared branch) |
| kills work | `kill -9`, `pkill`, `reboot`, `shutdown`, `docker stop` | yes | `kill -9 -1` and similar were already covered; 1 new line |
| big system change | `apt/pacman/dnf` installs and removals, `insmod`, `modprobe -r`, `sysctl -w`, bootloader, network config | explain: package managers, `make install`, `modprobe` only through `sudo` | `insmod`/`rmmod`/`modprobe`, `sysctl -w`, `update-grub`/`efibootmgr`/`mkinitcpio`, `ip link/route/addr` changes, `snap`/`flatpak`, `-y`/`--noconfirm` (19 lines) |
| heads-up | `tail -f`, `ping`, `top`, `docker run`, `kubectl logs -f` | yes | `kubectl logs -f` |

Deliberately not warned (all still explained by `huh explain` where it knows the program): `python3 -m http.server --bind 127.0.0.1`, `ssh -L`/`-D` on the loopback, `ssh -X`, `ssh -i`, `ssh-keygen -R`, `ssh-add -L`, `cat ~/.ssh/id_rsa.pub` and `.env.example` (public or sample files); `env`/`printenv` and `set | grep TOKEN` on their own (what happens to the output is not in the line, so the warning is on the sink: `echo $TOKEN`, `curl -T ~/.ssh/id_rsa`); `bash x.sh`, `python3 x.py`, `./install.sh` after a download (huh does not read the script); a bare `git push` (the branch is not named); `git tag -d` and `git remote add` (local or routine); `docker pull`, `mount`, `ulimit`, `nmap`, `openssl req` (routine; `nmap` against someone else's network is the user's call); `HISTCONTROL=ignorespace`, `history -w`. Requests for plain package installs, `useradd` and `usermod -L` stay without a WARNING: they are the thing asked for, and `huh explain` warns on the same lines.

New tests: `evals/safety.toml` +33 request cases (28 with `warn = true`, 5 controls with `warn = false`); `evals/explain.toml` +107 cases (one or more per category, checking the `! [category]`, and 7 quiet lines); unit tests for the catalog (`every_catalog_warning_has_a_risk_category`), the categories (`risk_kinds`), the new checks (`risk_extra`) and `huh why` notes.

## Planned: safety hardening (after round 5): items 1-3 done 2026-11-18, item 4 open

Use the published risk categories of Claude Code's auto mode (`claude auto-mode defaults`; a copy is in `~/.local/share/huh-dev/auto-mode-defaults.json`) as a checklist, paraphrased in huh's own words (the rule text is not licensed for reuse):
(done) 1. Audit the shell-relevant categories against huh's WARNINGs in requests and `huh explain`. Likely gaps:
   - weakening TLS or authentication: `curl -k`, `git config http.sslVerify false`, `StrictHostKeyChecking=no`;
   - weakening security: `ufw disable`, `setenforce 0`;
   - exposing local services: `python -m http.server`, binding to `0.0.0.0`;
   - public tunnels: `ngrok`, `ssh -R`;
   - tampering with logs or history: `history -c`, deleting `~/.bash_history`;
   - leaking credentials.
(done) 2. Add the missing warnings, each with eval cases.
(done) 3. Name the risk category in each WARNING ("weakens security", "destroys data", "exposes a service", …), the same way in requests and `huh explain`.
4. Add one TypeSafe judge question per category, so "dangerous without WARNING" is measured against the taxonomy.

The classifier itself cannot be used: it gives no verdict without running the command, it judges an agent's actions in a session rather than a person's command, and it needs a model call per command.

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

## Tutor mode (`huh tutor`): steps 1-3 done 2026-11-18, noise levels and the Omarchy keybinding next

Keep it simple (Radu, 2026-11-18): the point is to explain output fields and flags to people who rarely use the terminal, rustlings-style hints, not a full tutorial system. huh's goal overall is to point the user the right way and let them decide and read the manual.

```
 terminal ──(already recorded: script → huh record-sink, markers, exit code)──▶ private log
                                                                                    │
                                          huh tutor watches it, read-only ◀─────────┘
                                            ├─ command finished → huh explain the line (condensed)
                                            ├─ exit ≠ 0         → huh why on its output
                                            └─ exit 0           → explain the output fields
```

1. ~~**MVP**~~ done 2026-11-18: `huh tutor` (`src/tutor.rs`) polls the newest recording (not its own pane's) or `--tty`, prints header, condensed explanation, diagnosis or output fields; `--quiet`; full-screen programs are named only; says how to enable recording when there is none.
2. ~~**Explain before Enter**~~ done 2026-11-18: `Alt+e` in `shell-init --hooks` prints `huh explain` for the prompt line below it, without running it. (It prints in the terminal itself; sending it to the tutor pane is not done.)
3. ~~**Output explanations**~~ done 2026-11-18: `src/outputs.txt` holds 28 commands (header or shape detection, one sentence per field, at most two notes); `cmd | huh` and `cmd | huh explain` use it before error detection, only when no line looks like an error. `evals/outputs.toml`: 29 outputs (one or more per command) and 20 that must not be recognised.
4. **Noise levels** (everything / only new commands, with an opt-in local history / only failures and warnings), then an Omarchy keybinding that opens the tutor as a side window attached to the focused terminal. **Next.**

Known limits: stdout and stderr arrive mixed in the terminal, so the tutor goes by exit code and error-looking lines; full-screen programs (vim, htop, less) are only named, not explained; when several commands finish within 300 ms only the last is shown; bash only until the zsh/fish hooks exist; ping and one `git status` sample in the evals are typed from the standard format, not captured.

### Next (from that measurement): all done 2026-11-18

1. ~~`huh why` confident wrong answers~~ and 2. ~~`huh why` coverage~~: see "Completed: Error doctor, second measurement round".
3. ~~Requests: wrong noun in a slot~~: see "Completed: Slot filling, round 2".
4. ~~Requests: state questions answered with actions~~ and 5. ~~catalog gaps~~ (including "unmount drive"): see "Completed: Intent and catalog gaps".

The next step is a fresh measurement round (error tests inside a container) against the targets: beginner ≥ 85%, sysadmin ≥ 65%, `huh why` useful fix ≥ 65%, ≤ 5 misleading, 0 non-errors called errors, 0 dangerous suggestions without a WARNING.

## Completed: hints instead of dead ends (2026-11-18)

When nothing fits confidently, huh now points the way instead of printing a bare "No suggestion" (`src/hint.rs`, `src/hints.txt`, `evals/hints.toml`). The hint names a tool, its one-line description, `man <tool>` (or `<tool> --help` when no page is on disk, or a note that it is not installed) and the 1-2 flags whose meaning shares words with the request. Order: a program the request names ("with ffmpeg"; known from the catalog, `knowledge.txt` or `$PATH`), a tool for the topic from the curated table, the closest rejected catalog command (needs a word of its name or two content words in common). It is never a runnable line, `--first-line-only` still prints nothing and exits 1, and the honest refusals (command lines, "what does X do", wiping drives, other people's systems, "does not name what", which now adds the closest tool) are unchanged. Off-topic requests keep a plain "No suggestion". Expert mode shows a candidate held back only by a beginner safeguard as a normal suggestion. New eval keys `pointer` and `no_pointer`.

Not handled: requests where the matcher confidently suggests a wrong line ("pretty print json" gives `cat <target>`); a hint is only for the no-suggestion case.

## Completed: dangerous outputs and "typed values never vanish" (2026-11-18)

The five serious findings of round 4 are fixed, in normal and expert mode alike (`evals/safety.toml`, `evals/expert.toml`):

- A request that is itself a command line (first word a known program, plus flags, `NAME=value` or shell operators; no prose words) gets `huh explain "<line>"` instead of a word match. One that would wreck the system (`rm -rf /`, `rm -rf ~`, `chmod -R 777 /`, `dd … of=/dev/sda`, `mkfs` on a device, a fork bomb) also says "this would destroy the system". "run ls -la in src" is still an instruction.
- Wiping, erasing, zeroing or shredding a disk, partition or `/dev` path gets the existing format/wipe refusal in every mode, and no suggestion that deletes, shreds or truncates a drive can be shown (`rm /dev/sdb`).
- A request whose verb is show/inspect/list/display/get/print/check/find/what/which and that asks for no change never gets a destructive top suggestion: normal mode abstains if nothing read-only fits, expert mode ranks the destructive one last and keeps its WARNING. New entries `docker inspect`, `docker container ip` (the first entry with Go-template braces; `\{` and `\}` in a template are literal braces), `pstree`. `fsck` is now the read-only `fsck -n`; `fsck repair` is the destructive one.
- "all files in X", "everything in X", "the contents of X" mean `rm -r X/*` (with a note that hidden files and X itself stay), never `rm -r X`.
- Typed values (`src/values.rs`): after the line is built, every path, number (with or without unit), IP, URL, host or file name, `user@host`, `@server`, quoted text, git hash and labelled name ("named web", "comment ops@lab", "user bob", "onto release") must appear in the line in some form, or the suggestion says `not included: <phrase around it>`. One lost value that fits the single open placeholder goes there; more than one lost value caps the shown confidence at 50%. Added slots: `df` path, `git clone` `--depth` and folder, `docker stats` container, `ssh-keygen -C`, `docker image prune --filter until=`, `docker run -p/--name/image`; new entry `count lines in files by type` (`find … | xargs wc -l`). `evals/values.toml` (83 cases): 28 misses before, 0 after.
- Changed expectations: five existing cases whose request was itself a command line (`chmod -R 777 uploads`, `tail -f app.log`, `pkill -9 firefox`, `chown -R www-data /var/www`, `docker logs --tail 100 web`) now expect the `huh explain` pointer (each also kept as a worded variant); the two `fsck` cases expect `sudo fsck -n …`.

Not handled: a value that only appears as a plain word (a host name without a dot, "by alice" in unusual positions) is not recognised as a value; `delete the contents of build` in normal mode still abstains for lack of a named target.

## Completed: error doctor, round 4 (2026-11-18)

- **Error-shaped lines** (`line_shape` in `src/doctor.rs`): without an exit code, output counts as a failure only if a line has the shape of an error message. Strong shapes: a severity marker or program prefix at the start (`error:`, `ERROR`, `fatal:`, `npm ERR!`, `E:`, `panic`, `prog: file: reason`, `Traceback (most recent call last)`) and known exact forms (`CONFLICT (`, the empty cherry-pick message). Weak shapes (a known reason such as `No such file or directory` somewhere in the line) are dropped when they sit in a sentence (ends with a period, 5+ words, no `prefix:`), a list item, a code line (`except`, `catch`, `raise`, `throw`, `if err`, grep hits on them), a commit-log line, a log line with a timestamp or `[level]` from another program (answered as "a log line written by another program"), or when a later line says `connected`/`done`/`ok`/`passed`. `make: Nothing to be done` is success.
- New rules: `git stash pop` conflict (stash kept, `git stash drop` after resolving), empty cherry-pick (`--skip`), EADDRINUSE port from the right line, `sudo` missing (`su -c`), `error:` preferred over `warning:`, Python module shadowed by a local file, `tar -C`, `JSON.parse` means bad data, `cp` with an unquoted space, dangling symlinks, dnf superuser, makepkg as root, sed/xargs/zip/find/ssh-add/bash test errors, octal `08`, journalctl access, test names from `FAIL:` / jest, rpm not installed, Bus error, divergent `git pull`, `index.lock` (`pgrep -a git` first), dubious ownership, ignored paths, dnf no match, GPG check, curl 35, npm ERESOLVE.
- Evals: `evals/errors-round4.toml` (49 cases, 10 of them false alarms), 100% required with all other error suites. Before: causes 14/49, fixes 28/49.

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
