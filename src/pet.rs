//! The `#!` terminal pet: pure ASCII mascot, local-only state machine, lazy time decay (never
//! dies), atomic TOML persistence under `~/.local/state/huh/pet.toml`, and status rendering.

use crate::date;
use crate::style::{Role, paint};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub const ART_HEIGHT: usize = 4;
pub const ART_MAX_WIDTH: usize = 10;
pub const DEFAULT_NAME: &str = "Pebble";
pub const MIN_FULLNESS: u8 = 10;
pub const MIN_HAPPINESS: u8 = 20;
pub const CAT_WATCH_UNIQUE: usize = 25;
pub const TURTLE_WATCH_COMMANDS: u64 = 100;
pub const TURTLE_WATCH_STREAK: u32 = 7;
pub const OWL_WATCH_COMMANDS: u64 = 250;
pub const OWL_WATCH_WARNINGS: u32 = 5;
const MAX_UNIQUE_PROGRAMS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Species {
    #[default]
    Penguin,
    Cat,
    Turtle,
    Owl,
}

impl Species {
    pub const ALL: [Species; 4] = [Species::Penguin, Species::Cat, Species::Turtle, Species::Owl];
    pub const VALID: &'static str = "penguin, cat, turtle, owl";

    pub fn parse(value: &str) -> Result<Species> {
        match value.trim().to_ascii_lowercase().as_str() {
            "penguin" => Ok(Species::Penguin),
            "cat" => Ok(Species::Cat),
            "turtle" => Ok(Species::Turtle),
            "owl" => Ok(Species::Owl),
            other => bail!("unknown pet species \"{other}\"; valid species: {}", Self::VALID),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Species::Penguin => "penguin",
            Species::Cat => "cat",
            Species::Turtle => "turtle",
            Species::Owl => "owl",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Species::Penguin => "Penguin chick",
            Species::Cat => "Terminal Cat",
            Species::Turtle => "Shell-Turtle",
            Species::Owl => "Snowy Owl",
        }
    }

    pub fn unlock_badge(self) -> Option<(&'static str, &'static str)> {
        match self {
            Species::Penguin => None,
            Species::Cat => Some(("pathfinder", "finish Unit 1 (Finding Your Way) in /mode lab")),
            Species::Turtle => Some(("pipe_plumber", "finish Unit 3 (Searching & Pipes) in /mode lab")),
            Species::Owl => Some(("night_watch", "finish Unit 8 (Networking & Stay Safe) in /mode lab")),
        }
    }

    fn watch_unlock_hint(self, pet: &Pet) -> Option<String> {
        match self {
            Species::Penguin => None,
            Species::Cat => Some(format!(
                "watch {}/{CAT_WATCH_UNIQUE} unique commands",
                pet.unique_programs.len().min(CAT_WATCH_UNIQUE)
            )),
            Species::Turtle => Some(format!(
                "watch {}/{TURTLE_WATCH_COMMANDS} commands / {}d/{TURTLE_WATCH_STREAK}d streak",
                pet.commands_fed.min(TURTLE_WATCH_COMMANDS),
                pet.day_streak.max(pet.best_day_streak).min(TURTLE_WATCH_STREAK)
            )),
            Species::Owl => Some(format!(
                "watch {}/{OWL_WATCH_COMMANDS} commands / {}/{OWL_WATCH_WARNINGS} warnings",
                pet.commands_fed.min(OWL_WATCH_COMMANDS),
                pet.warnings_heeded.min(OWL_WATCH_WARNINGS)
            )),
        }
    }

    pub fn unlock_summary(self, pet: &Pet) -> Option<String> {
        let watch = self.watch_unlock_hint(pet)?;
        match self {
            Species::Penguin => None,
            Species::Cat => Some(format!("badge `pathfinder` (Unit 1 in `/mode lab`) or {watch}")),
            Species::Turtle => Some(format!("badge `pipe_plumber` (Unit 3 in `/mode lab`) or {watch}")),
            Species::Owl => Some(format!("badge `night_watch` (Unit 8 in `/mode lab`) or {watch}")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mood {
    Idle,
    Happy,
    Sleepy,
    Hungry,
    Alert,
    Celebrating,
}

impl Mood {
    #[cfg_attr(not(test), allow(dead_code))]
    pub const ALL: [Mood; 6] = [Mood::Idle, Mood::Happy, Mood::Sleepy, Mood::Hungry, Mood::Alert, Mood::Celebrating];

    pub fn name(self) -> &'static str {
        match self {
            Mood::Idle => "idle",
            Mood::Happy => "happy",
            Mood::Sleepy => "sleepy",
            Mood::Hungry => "hungry",
            Mood::Alert => "alert",
            Mood::Celebrating => "celebrating",
        }
    }

    pub fn role(self) -> Role {
        match self {
            Mood::Idle | Mood::Happy => Role::Accent,
            Mood::Sleepy => Role::Dim,
            Mood::Hungry => Role::Note,
            Mood::Alert => Role::Warning,
            Mood::Celebrating => Role::Success,
        }
    }

    fn tip(self) -> &'static str {
        match self {
            Mood::Idle => "Ready in the pane — every clean command keeps your #! companion fed.",
            Mood::Happy => "Cruising on a steady streak of clean commands.",
            Mood::Sleepy => "Quiet for a while — run any command to wake back up.",
            Mood::Hungry => "Running low — a couple of clean commands will top it up.",
            Mood::Alert => "Spotted a failed or risky command — check the note above.",
            Mood::Celebrating => "Milestone reached — nice work.",
        }
    }
}

/// 4-line pure printable ASCII art (`<= ART_MAX_WIDTH` cols) carrying `#!` for animation tick `tick`.
pub fn art_frame(species: Species, mood: Mood, tick: u64) -> [&'static str; ART_HEIGHT] {
    match (species, mood) {
        (Species::Penguin, Mood::Idle) => {
            if tick % 24 == 23 {
                ["  .--.  ", " (->- ) ", " / #! \\ ", " \\_==_/ "]
            } else {
                ["  .--.  ", " (o>o ) ", " / #! \\ ", " \\_==_/ "]
            }
        }
        (Species::Penguin, Mood::Happy) => {
            if (tick / 4) % 2 == 1 {
                ["  .--. '", " (^>^ ) ", " / #! \\ ", " \\_==_/ "]
            } else {
                ["  .--.  ", " (^>^ ) ", " / #! \\ ", " \\_==_/ "]
            }
        }
        (Species::Penguin, Mood::Sleepy) => {
            if (tick / 3) % 2 == 1 {
                ["  .--. Z", " (-v- ) ", " / #! \\ ", " \\_==_/ "]
            } else {
                ["  .--. z", " (-v- ) ", " / #! \\ ", " \\_==_/ "]
            }
        }
        (Species::Penguin, Mood::Hungry) => {
            if (tick / 3) % 2 == 1 {
                ["  .--. .", " (o_o ) ", " / #! \\ ", " \\_==_/ "]
            } else {
                ["  .--. ?", " (o_o ) ", " / #! \\ ", " \\_==_/ "]
            }
        }
        (Species::Penguin, Mood::Alert) => {
            if tick % 2 == 1 {
                ["  .--.  ", " (O_O ) ", " / #! \\ ", " \\_==_/ "]
            } else {
                ["  .--. !", " (O_O ) ", " / #! \\ ", " \\_==_/ "]
            }
        }
        (Species::Penguin, Mood::Celebrating) => {
            if tick % 2 == 1 {
                [" \\.--./ +", " (^v^ ) ", " / #! \\ ", " \\_==_/ "]
            } else {
                [" \\.--./ *", " (^v^ ) ", " / #! \\ ", " \\_==_/ "]
            }
        }

        (Species::Cat, Mood::Idle) => {
            if tick % 24 == 23 {
                [" /\\_/\\    ", "(=-.-=)   ", " |#! |__~ ", " (_|_)    "]
            } else {
                [" /\\_/\\    ", "(=o.o=)   ", " |#! |__~ ", " (_|_)    "]
            }
        }
        (Species::Cat, Mood::Happy) => {
            if (tick / 4) % 2 == 1 {
                [" /\\_/\\   '", "(=^.^=)   ", " |#! |__~ ", " (_|_)    "]
            } else {
                [" /\\_/\\    ", "(=^.^=)   ", " |#! |__~ ", " (_|_)    "]
            }
        }
        (Species::Cat, Mood::Sleepy) => {
            if (tick / 3) % 2 == 1 {
                [" /\\_/\\   Z", "(=-.-=)   ", " |#! |__~ ", " (_|_)    "]
            } else {
                [" /\\_/\\   z", "(=-.-=)   ", " |#! |__~ ", " (_|_)    "]
            }
        }
        (Species::Cat, Mood::Hungry) => {
            if (tick / 3) % 2 == 1 {
                [" /\\_/\\   .", "(=o_o=)   ", " |#! |__~ ", " (_|_)    "]
            } else {
                [" /\\_/\\   ?", "(=o_o=)   ", " |#! |__~ ", " (_|_)    "]
            }
        }
        (Species::Cat, Mood::Alert) => {
            if tick % 2 == 1 {
                [" /\\_/\\    ", "(=O.O=)   ", " |#! |__  ", " (_|_)    "]
            } else {
                [" /\\_/\\   !", "(=O.O=)   ", " |#! |__! ", " (_|_)    "]
            }
        }
        (Species::Cat, Mood::Celebrating) => {
            if tick % 2 == 1 {
                [" /\\_/\\   +", "(=^w^=)   ", "\\|#! |/~  ", " (_|_)    "]
            } else {
                [" /\\_/\\   *", "(=^w^=)   ", "\\|#! |/~  ", " (_|_)    "]
            }
        }

        (Species::Turtle, Mood::Idle) => {
            if tick % 24 == 23 {
                ["  ___   _ ", " /#! \\_(-)", "(_____)_/ ", " \"   \"    "]
            } else {
                ["  ___   _ ", " /#! \\_(o)", "(_____)_/ ", " \"   \"    "]
            }
        }
        (Species::Turtle, Mood::Happy) => {
            if (tick / 4) % 2 == 1 {
                ["  ___   ' ", " /#! \\_(^)", "(_____)_/ ", " \"   \"    "]
            } else {
                ["  ___   _ ", " /#! \\_(^)", "(_____)_/ ", " \"   \"    "]
            }
        }
        (Species::Turtle, Mood::Sleepy) => {
            if (tick / 3) % 2 == 1 {
                ["  ___   Z ", " /#! \\_(-)", "(_____)_/ ", " \"   \"    "]
            } else {
                ["  ___   z ", " /#! \\_(-)", "(_____)_/ ", " \"   \"    "]
            }
        }
        (Species::Turtle, Mood::Hungry) => {
            if (tick / 3) % 2 == 1 {
                ["  ___   . ", " /#! \\_(o)", "(_____)_/ ", " \"   \"    "]
            } else {
                ["  ___   ? ", " /#! \\_(o)", "(_____)_/ ", " \"   \"    "]
            }
        }
        (Species::Turtle, Mood::Alert) => {
            if tick % 2 == 1 {
                ["  ___     ", " /#! \\_(O)", "(_____)_/ ", " \"   \"    "]
            } else {
                ["  ___   ! ", " /#! \\_(O)", "(_____)_/ ", " \"   \"    "]
            }
        }
        (Species::Turtle, Mood::Celebrating) => {
            if tick % 2 == 1 {
                ["  ___   + ", " /#! \\_(^)", "(_____)_/ ", " \"   \"    "]
            } else {
                ["  ___   * ", " /#! \\_(^)", "(_____)_/ ", " \"   \"    "]
            }
        }

        (Species::Owl, Mood::Idle) => {
            if tick % 24 == 23 {
                [" ,___,   ", " (-,-)   ", " /#!#\\   ", " -\"-\"-   "]
            } else {
                [" ,___,   ", " (o,o)   ", " /#!#\\   ", " -\"-\"-   "]
            }
        }
        (Species::Owl, Mood::Happy) => {
            if (tick / 4) % 2 == 1 {
                [" ,___,  '", " (^,^)   ", " /#!#\\   ", " -\"-\"-   "]
            } else {
                [" ,___,   ", " (^,^)   ", " /#!#\\   ", " -\"-\"-   "]
            }
        }
        (Species::Owl, Mood::Sleepy) => {
            if (tick / 3) % 2 == 1 {
                [" ,___,  Z", " (-,-)   ", " /#!#\\   ", " -\"-\"-   "]
            } else {
                [" ,___,  z", " (-,-)   ", " /#!#\\   ", " -\"-\"-   "]
            }
        }
        (Species::Owl, Mood::Hungry) => {
            if (tick / 3) % 2 == 1 {
                [" ,___,  .", " (o_o)   ", " /#!#\\   ", " -\"-\"-   "]
            } else {
                [" ,___,  ?", " (o_o)   ", " /#!#\\   ", " -\"-\"-   "]
            }
        }
        (Species::Owl, Mood::Alert) => {
            if tick % 2 == 1 {
                [" ,___,   ", " (O,O)   ", " /#!#\\   ", " -\"-\"-   "]
            } else {
                [" ,___,  !", " (O,O)   ", " /#!#\\   ", " -\"-\"-   "]
            }
        }
        (Species::Owl, Mood::Celebrating) => {
            if tick % 2 == 1 {
                [" ,___,  +", " (^v^)   ", "\\/#!#\\/  ", " -\"-\"-   "]
            } else {
                [" ,___,  *", " (^v^)   ", "\\/#!#\\/  ", " -\"-\"-   "]
            }
        }
    }
}

/// 4-line pure printable ASCII art (`<= ART_MAX_WIDTH` cols) carrying `#!`.
pub fn art(species: Species, mood: Mood) -> [&'static str; ART_HEIGHT] {
    art_frame(species, mood, 0)
}

fn default_name() -> String {
    DEFAULT_NAME.to_string()
}

fn default_unlocked() -> Vec<Species> {
    vec![Species::Penguin]
}

fn default_stat() -> u8 {
    70
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pet {
    #[serde(default = "default_name")]
    pub name: String,
    pub species: Species,
    #[serde(default = "default_unlocked")]
    pub unlocked_species: Vec<Species>,
    #[serde(default = "default_stat")]
    pub fullness: u8,
    #[serde(default = "default_stat")]
    pub happiness: u8,
    pub commands_fed: u64,
    pub cmd_streak: u32,
    pub best_cmd_streak: u32,
    pub day_streak: u32,
    pub best_day_streak: u32,
    pub last_active_day: Option<String>,
    pub last_fed_epoch: u64,
    pub last_program: Option<String>,
    #[serde(default)]
    pub unique_programs: Vec<String>,
    #[serde(default)]
    pub warnings_heeded: u32,
}

impl Default for Pet {
    fn default() -> Pet {
        Pet {
            name: default_name(),
            species: Species::Penguin,
            unlocked_species: default_unlocked(),
            fullness: default_stat(),
            happiness: default_stat(),
            commands_fed: 0,
            cmd_streak: 0,
            best_cmd_streak: 0,
            day_streak: 0,
            best_day_streak: 0,
            last_active_day: None,
            last_fed_epoch: 0,
            last_program: None,
            unique_programs: Vec::new(),
            warnings_heeded: 0,
        }
    }
}

/// Compact pet summary passed into the pinned tutor header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Banner {
    pub name: String,
    pub species: Species,
    pub mood: Mood,
}

/// True for characters that occupy a single terminal cell (printable ASCII plus single-width
/// Latin/Greek/Cyrillic blocks; rejects control, zero-width/combining marks, variation selectors,
/// and double-width CJK/emoji characters).
fn is_single_width(c: char) -> bool {
    matches!(
        c as u32,
        0x0020..=0x007E
            | 0x00A1..=0x00AC
            | 0x00AE..=0x00FF
            | 0x0100..=0x024F
            | 0x0250..=0x02AF
            | 0x0370..=0x03FF
            | 0x0400..=0x0482
            | 0x048A..=0x052F
            | 0x1E00..=0x1EFF
            | 0x1F00..=0x1FFF
    )
}

fn is_valid_name(trimmed: &str) -> bool {
    !trimmed.is_empty() && trimmed.chars().count() <= 20 && trimmed.chars().all(is_single_width)
}

impl Pet {
    /// Repairs out-of-range stats, missing default unlocks, or invalid names loaded from disk.
    pub fn normalize(&mut self) {
        if !self.unlocked_species.contains(&Species::Penguin) {
            self.unlocked_species.insert(0, Species::Penguin);
        }
        let mut deduped = Vec::with_capacity(self.unlocked_species.len());
        for sp in &self.unlocked_species {
            if !deduped.contains(sp) {
                deduped.push(*sp);
            }
        }
        self.unlocked_species = deduped;
        if !self.is_unlocked(self.species) {
            self.species = Species::Penguin;
        }
        self.fullness = self.fullness.clamp(MIN_FULLNESS, 100);
        self.happiness = self.happiness.clamp(MIN_HAPPINESS, 100);
        let trimmed = self.name.trim();
        if !is_valid_name(trimmed) {
            self.name = default_name();
        } else if trimmed.len() != self.name.len() {
            self.name = trimmed.to_string();
        }
        let mut seen_progs = Vec::with_capacity(self.unique_programs.len().min(MAX_UNIQUE_PROGRAMS));
        for prog in &self.unique_programs {
            let clean = prog.trim().to_ascii_lowercase();
            if !clean.is_empty() && !seen_progs.contains(&clean) {
                seen_progs.push(clean);
                if seen_progs.len() >= MAX_UNIQUE_PROGRAMS {
                    break;
                }
            }
        }
        self.unique_programs = seen_progs;
    }

    /// Fullness and happiness after lazy idle decay at `now_epoch`. Floors ensure the pet never dies.
    pub fn effective_stats(&self, now_epoch: u64) -> (u8, u8) {
        let fullness = self.fullness.clamp(MIN_FULLNESS, 100);
        let happiness = self.happiness.clamp(MIN_HAPPINESS, 100);
        if self.last_fed_epoch == 0 || now_epoch <= self.last_fed_epoch {
            return (fullness, happiness);
        }
        let idle_hours = (now_epoch - self.last_fed_epoch) / 3600;
        let full_drop = (idle_hours / 2).min(100) as u8;
        let happy_drop = (idle_hours.saturating_sub(12) / 4).min(100) as u8;
        (fullness.saturating_sub(full_drop).max(MIN_FULLNESS), happiness.saturating_sub(happy_drop).max(MIN_HAPPINESS))
    }

    pub fn mood_at(&self, now_epoch: u64, transient: Option<Mood>) -> Mood {
        if let Some(m) = transient {
            return m;
        }
        let (fullness, happiness) = self.effective_stats(now_epoch);
        let idle_hours =
            if self.last_fed_epoch == 0 { 0 } else { now_epoch.saturating_sub(self.last_fed_epoch) / 3600 };
        if idle_hours >= 48 || fullness <= 30 {
            Mood::Hungry
        } else if idle_hours >= 18 {
            Mood::Sleepy
        } else if happiness >= 75 || self.cmd_streak >= 3 {
            Mood::Happy
        } else {
            Mood::Idle
        }
    }

    /// Feeds the pet after a clean `exit 0` command and returns its resulting mood.
    pub fn feed(&mut self, program: Option<&str>, now_epoch: u64) -> Mood {
        let (fullness, happiness) = self.effective_stats(now_epoch);
        let clean_prog = program.map(str::trim).filter(|p| !p.is_empty());
        let repeated = clean_prog.is_some() && self.last_program.as_deref() == clean_prog;
        let (full_gain, happy_gain) = if repeated { (4, 2) } else { (10, 6) };
        self.fullness = fullness.saturating_add(full_gain).clamp(MIN_FULLNESS, 100);
        self.happiness = happiness.saturating_add(happy_gain).clamp(MIN_HAPPINESS, 100);
        self.commands_fed = self.commands_fed.saturating_add(1);
        self.cmd_streak = self.cmd_streak.saturating_add(1);
        self.best_cmd_streak = self.best_cmd_streak.max(self.cmd_streak);
        self.last_program = clean_prog.map(str::to_string);
        if let Some(prog) = clean_prog {
            let lower = prog.to_ascii_lowercase();
            if self.unique_programs.len() < MAX_UNIQUE_PROGRAMS && !self.unique_programs.contains(&lower) {
                self.unique_programs.push(lower);
            }
        }
        self.last_fed_epoch = now_epoch;

        let today = date::from_epoch(now_epoch);
        let today_str = date::format(today);
        let day_advanced = match self.last_active_day.as_deref().and_then(date::parse) {
            Some(prev) => {
                let delta = date::days_between(prev, today);
                if delta == 0 {
                    false
                } else if delta == 1 {
                    self.day_streak = self.day_streak.saturating_add(1);
                    self.last_active_day = Some(today_str);
                    true
                } else if delta > 1 {
                    self.day_streak = 1;
                    self.last_active_day = Some(today_str);
                    true
                } else {
                    false
                }
            }
            None => {
                self.day_streak = 1;
                self.last_active_day = Some(today_str);
                true
            }
        };
        self.best_day_streak = self.best_day_streak.max(self.day_streak);

        if (self.cmd_streak > 0 && self.cmd_streak.is_multiple_of(10)) || (day_advanced && self.day_streak >= 3) {
            Mood::Celebrating
        } else {
            self.mood_at(now_epoch, None)
        }
    }

    /// Reacts to a failed or warned command: resets the command streak without lowering stats.
    pub fn react_alert(&mut self) -> Mood {
        self.cmd_streak = 0;
        Mood::Alert
    }

    /// Checks passive Watch-mode unlock thresholds and unlocks any newly qualified species.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn check_watch_unlocks(&mut self) -> Vec<Species> {
        let mut newly_unlocked = Vec::new();
        if self.unique_programs.len() >= CAT_WATCH_UNIQUE && self.unlock_species(Species::Cat) {
            newly_unlocked.push(Species::Cat);
        }
        if (self.commands_fed >= TURTLE_WATCH_COMMANDS
            || self.day_streak.max(self.best_day_streak) >= TURTLE_WATCH_STREAK)
            && self.unlock_species(Species::Turtle)
        {
            newly_unlocked.push(Species::Turtle);
        }
        if (self.commands_fed >= OWL_WATCH_COMMANDS || self.warnings_heeded >= OWL_WATCH_WARNINGS)
            && self.unlock_species(Species::Owl)
        {
            newly_unlocked.push(Species::Owl);
        }
        newly_unlocked
    }

    /// Records that a dangerous-command warning was shown/heeded and checks watch unlocks without touching stats.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn record_warning(&mut self) -> Vec<Species> {
        self.warnings_heeded = self.warnings_heeded.saturating_add(1);
        self.check_watch_unlocks()
    }

    pub fn is_unlocked(&self, species: Species) -> bool {
        species == Species::Penguin || self.unlocked_species.contains(&species)
    }

    pub fn unlock_species(&mut self, species: Species) -> bool {
        if !self.unlocked_species.contains(&Species::Penguin) {
            self.unlocked_species.insert(0, Species::Penguin);
        }
        if self.unlocked_species.contains(&species) {
            false
        } else {
            self.unlocked_species.push(species);
            true
        }
    }

    pub fn set_species(&mut self, species: Species) -> Result<()> {
        if self.is_unlocked(species) {
            self.species = species;
            Ok(())
        } else if let Some((badge, reason)) = species.unlock_badge() {
            let watch = species.watch_unlock_hint(self).unwrap_or_default();
            bail!(
                "\"{}\" ({}) is locked — earn the `{badge}` badge ({reason}) or {watch}",
                species.name(),
                species.label()
            );
        } else {
            unreachable!("penguin is always unlocked")
        }
    }

    pub fn rename(&mut self, name: &str) -> Result<()> {
        let trimmed = name.trim();
        if !is_valid_name(trimmed) {
            bail!("pet name must be 1 to 20 printable single-width characters");
        }
        self.name = trimmed.to_string();
        Ok(())
    }

    pub fn banner(&self, now_epoch: u64, transient: Option<Mood>) -> Banner {
        Banner { name: self.name.clone(), species: self.species, mood: self.mood_at(now_epoch, transient) }
    }
}

// ---------------------------------------------------------------------------
// Config & persistence
// ---------------------------------------------------------------------------

/// `pet = false` or `pet = "off"` from the text of `config.toml`; anything else keeps it on.
pub fn pet_off_in_config(text: &str) -> bool {
    let Ok(table) = toml::from_str::<toml::Table>(text) else {
        return false;
    };
    match table.get("pet") {
        Some(toml::Value::Boolean(on)) => !on,
        Some(toml::Value::String(s)) => s.trim().eq_ignore_ascii_case("off"),
        _ => false,
    }
}

pub fn enabled_with(env_off: bool, config_off: bool) -> bool {
    !env_off && !config_off
}

/// Whether the pet should appear in the pinned tutor header right now.
pub fn enabled() -> bool {
    let env_off = std::env::var("HUH_NO_PET").is_ok_and(|v| !v.is_empty() && v != "0");
    let config_off = crate::mode::config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|text| pet_off_in_config(&text));
    enabled_with(env_off, config_off)
}

/// `$XDG_STATE_HOME/huh/pet.toml`, or `~/.local/state/huh/pet.toml`.
pub fn state_path(xdg_state: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    match xdg_state.filter(|v| !v.is_empty()) {
        Some(dir) => Some(Path::new(dir).join("huh/pet.toml")),
        None => Some(Path::new(home.filter(|v| !v.is_empty())?).join(".local/state/huh/pet.toml")),
    }
}

pub fn default_state_path() -> Option<PathBuf> {
    state_path(std::env::var("XDG_STATE_HOME").ok().as_deref(), std::env::var("HOME").ok().as_deref())
}

pub fn load_from(path: &Path) -> Pet {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Pet::default();
    };
    match toml::from_str::<Pet>(&text) {
        Ok(mut pet) => {
            pet.normalize();
            pet
        }
        Err(_) => Pet::default(),
    }
}

pub fn save_to(path: &Path, pet: &Pet) -> Result<()> {
    let dir = path.parent().context("pet state path has no parent directory")?;
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let body = toml::to_string_pretty(pet).context("cannot serialize pet state")?;
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("pet.toml");
    let tmp = dir.join(format!("{file_name}.tmp.{}", std::process::id()));
    std::fs::write(&tmp, body).with_context(|| format!("cannot write {}", tmp.display()))?;
    if let Err(err) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err).with_context(|| format!("cannot replace {}", path.display()));
    }
    Ok(())
}

pub fn load_default() -> Pet {
    default_state_path().map_or_else(Pet::default, |p| load_from(&p))
}

pub fn save_default(pet: &Pet) -> Result<()> {
    let path = default_state_path().context("cannot find state directory ($XDG_STATE_HOME or $HOME)")?;
    save_to(&path, pet)
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn meter(value: u8) -> String {
    let filled = (value as usize).min(100).div_ceil(10).min(10);
    format!("[{}{}]", "█".repeat(filled), "·".repeat(10 - filled))
}

fn card_lines(pet: &Pet, now_epoch: u64, transient: Option<Mood>) -> Vec<String> {
    let mood = pet.mood_at(now_epoch, transient);
    let (fullness, happiness) = pet.effective_stats(now_epoch);
    let grid = art(pet.species, mood);
    let day_label = format!("{}d daily", pet.day_streak);
    let right = [
        format!(
            "{} · {}",
            paint(Role::Header, &format!("{} ({})", pet.name, pet.species.label())),
            paint(mood.role(), mood.name())
        ),
        format!(
            "fullness  {} {}%   happiness {} {}%",
            paint(Role::Value, &meter(fullness)),
            paint(Role::Value, &fullness.to_string()),
            paint(Role::Value, &meter(happiness)),
            paint(Role::Value, &happiness.to_string()),
        ),
        format!(
            "fed: {} · streak: {} (best {}) · {day_label}",
            paint(Role::Value, &pet.commands_fed.to_string()),
            paint(Role::Value, &pet.cmd_streak.to_string()),
            pet.best_cmd_streak,
        ),
        paint(Role::Dim, mood.tip()),
    ];
    let mut lines = Vec::with_capacity(ART_HEIGHT);
    for (row, art_row) in grid.iter().enumerate() {
        let padded = format!("{art_row:<ART_MAX_WIDTH$}");
        lines.push(format!("  {}  {}", paint(mood.role(), &padded), right[row]));
    }
    lines
}

/// Full status card printed by `huh pet`.
pub fn render_status(pet: &Pet, now_epoch: u64, transient: Option<Mood>, hidden_in_tutor: bool) -> String {
    let mut out = String::new();
    for line in card_lines(pet, now_epoch, transient) {
        let _ = writeln!(out, "{line}");
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "{}", paint(Role::Header, "Species:"));
    for species in Species::ALL {
        let marker = if species == pet.species { "*" } else { " " };
        let status = if species == pet.species {
            paint(Role::Success, "active")
        } else if pet.is_unlocked(species) {
            paint(Role::Description, "unlocked")
        } else if let Some(summary) = species.unlock_summary(pet) {
            paint(Role::Dim, &format!("locked — {summary}"))
        } else {
            paint(Role::Description, "unlocked")
        };
        let _ = writeln!(out, "  {marker} {:<8} {:<14} {status}", species.name(), species.label());
    }
    if hidden_in_tutor {
        let _ = writeln!(out);
        let _ = writeln!(out, "{}", paint(Role::Note, "(hidden in the pane via HUH_NO_PET or config.toml)"));
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{}",
        paint(
            Role::Dim,
            "Rename: huh pet --name <name> · Switch: huh pet --species <species> · Hide: HUH_NO_PET=1 or pet = false"
        )
    );
    out
}

/// Compact card printed when typing `pet` inside the unified `huh` pane.
pub fn render_pane_card(pet: &Pet, now_epoch: u64, transient: Option<Mood>) -> String {
    let mut out = String::new();
    for line in card_lines(pet, now_epoch, transient) {
        let _ = writeln!(out, "{line}");
    }
    out.push('\n');
    out
}

/// Compact card printed when typing `pet` inside `huh tutor`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn render_tutor_card(pet: &Pet, now_epoch: u64, transient: Option<Mood>) -> String {
    render_pane_card(pet, now_epoch, transient)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE_EPOCH: u64 = 1_795_046_400; // 2026-11-19 00:00:00 UTC

    #[test]
    fn all_art_grids_are_bounded_pure_ascii_and_carry_shebang() {
        for species in Species::ALL {
            for mood in Mood::ALL {
                assert_eq!(art(species, mood), art_frame(species, mood, 0), "{species:?} {mood:?}");
                let mut distinct = std::collections::HashSet::new();
                for tick in 0..48 {
                    let grid = art_frame(species, mood, tick);
                    assert_eq!(grid.len(), ART_HEIGHT, "{species:?} {mood:?} tick={tick}");
                    for line in grid {
                        assert!(
                            line.chars().all(|c| c.is_ascii_graphic() || c == ' '),
                            "{species:?} {mood:?} tick={tick} non-ascii line: {line:?}"
                        );
                        assert!(
                            line.chars().count() <= ART_MAX_WIDTH,
                            "{species:?} {mood:?} tick={tick} too wide ({}): {line:?}",
                            line.chars().count()
                        );
                    }
                    let joined = grid.join("\n");
                    assert!(joined.contains("#!"), "{species:?} {mood:?} tick={tick} missing `#!`");
                    if species == Species::Penguin {
                        assert_eq!(
                            grid[0].find(".--."),
                            Some(2),
                            "Penguin {mood:?} tick={tick} row 0 misaligned: {:?}",
                            grid[0]
                        );
                    }
                    distinct.insert(grid);
                }
                assert!(distinct.len() >= 2, "{species:?} {mood:?} expected animated frames across 0..48");
            }
        }
        assert_eq!(art(Species::Cat, Mood::Idle), [" /\\_/\\    ", "(=o.o=)   ", " |#! |__~ ", " (_|_)    "]);
    }

    #[test]
    fn default_pet_state_and_unlocks() {
        let pet = Pet::default();
        assert_eq!(pet.name, "Pebble");
        assert_eq!(pet.species, Species::Penguin);
        assert_eq!(pet.fullness, 70);
        assert_eq!(pet.happiness, 70);
        assert_eq!(pet.mood_at(BASE_EPOCH, None), Mood::Idle);
        assert!(pet.is_unlocked(Species::Penguin));
        assert!(!pet.is_unlocked(Species::Cat));
        assert!(!pet.is_unlocked(Species::Turtle));
        assert!(!pet.is_unlocked(Species::Owl));
    }

    #[test]
    fn lazy_decay_floors_so_pet_never_dies() {
        let pet = Pet { last_fed_epoch: BASE_EPOCH, ..Pet::default() };
        assert_eq!(pet.mood_at(BASE_EPOCH + 18 * 3600, None), Mood::Sleepy);
        assert_eq!(pet.mood_at(BASE_EPOCH + 48 * 3600, None), Mood::Hungry);
        let (full, happy) = pet.effective_stats(BASE_EPOCH + 1000 * 3600);
        assert_eq!(full, MIN_FULLNESS);
        assert_eq!(happy, MIN_HAPPINESS);
        assert_eq!(pet.mood_at(BASE_EPOCH + 1000 * 3600, None), Mood::Hungry);
    }

    #[test]
    fn feeding_diminishing_returns_streaks_and_celebration() {
        let mut pet = Pet { fullness: 40, happiness: 50, last_fed_epoch: BASE_EPOCH, ..Pet::default() };
        // Distinct programs give +10 fullness, +6 happiness.
        let m1 = pet.feed(Some("ls"), BASE_EPOCH + 60);
        assert_eq!((pet.fullness, pet.happiness, pet.cmd_streak), (50, 56, 1));
        assert_eq!(m1, Mood::Idle);

        // Repeating the exact same program gives +4 fullness, +2 happiness.
        pet.feed(Some("ls"), BASE_EPOCH + 120);
        assert_eq!((pet.fullness, pet.happiness, pet.cmd_streak), (54, 58, 2));

        // 3 in a row -> Happy.
        let m3 = pet.feed(Some("df"), BASE_EPOCH + 180);
        assert_eq!(pet.cmd_streak, 3);
        assert_eq!(m3, Mood::Happy);

        // Feed up to 10 in a row on the same day -> Celebrating.
        for i in 4..=9 {
            pet.feed(Some(&format!("cmd{i}")), BASE_EPOCH + i * 60);
        }
        let m10 = pet.feed(Some("cmd10"), BASE_EPOCH + 600);
        assert_eq!(pet.cmd_streak, 10);
        assert_eq!(m10, Mood::Celebrating);

        // Consecutive UTC days advance daily streak; >= 3 consecutive days celebrates on first command of day.
        assert_eq!(pet.day_streak, 1);
        pet.feed(Some("pwd"), BASE_EPOCH + 86_400 + 60);
        assert_eq!(pet.day_streak, 2);
        let day3_mood = pet.feed(Some("id"), BASE_EPOCH + 2 * 86_400 + 60);
        assert_eq!(pet.day_streak, 3);
        assert_eq!(day3_mood, Mood::Celebrating);

        // A 2-day gap resets day_streak to 1 without breaking best_day_streak.
        pet.feed(Some("uname"), BASE_EPOCH + 5 * 86_400);
        assert_eq!(pet.day_streak, 1);
        assert_eq!(pet.best_day_streak, 3);

        // Clock skew backwards leaves day_streak and last_active_day untouched.
        let saved_day = pet.last_active_day.clone();
        pet.feed(Some("who"), BASE_EPOCH);
        assert_eq!(pet.day_streak, 1);
        assert_eq!(pet.last_active_day, saved_day);

        // Counter increments saturate instead of overflowing on hand-edited max values.
        let mut maxed = Pet {
            commands_fed: u64::MAX,
            cmd_streak: u32::MAX,
            day_streak: u32::MAX,
            last_active_day: Some(date::format(date::from_epoch(BASE_EPOCH))),
            ..Pet::default()
        };
        maxed.feed(Some("ls"), BASE_EPOCH + 86_400);
        assert_eq!((maxed.commands_fed, maxed.cmd_streak, maxed.day_streak), (u64::MAX, u32::MAX, u32::MAX));
    }

    #[test]
    fn alert_resets_streak_without_punishing_stats() {
        let mut pet = Pet { fullness: 65, happiness: 80, cmd_streak: 4, ..Pet::default() };
        let mood = pet.react_alert();
        assert_eq!(mood, Mood::Alert);
        assert_eq!(pet.cmd_streak, 0);
        assert_eq!((pet.fullness, pet.happiness), (65, 80));
    }

    #[test]
    fn react_alert_after_idle_does_not_double_decay() {
        let mut pet = Pet { fullness: 80, happiness: 80, cmd_streak: 5, last_fed_epoch: BASE_EPOCH, ..Pet::default() };
        let later = BASE_EPOCH + 24 * 3600;
        let expected = pet.effective_stats(later);
        assert_eq!(expected, (68, 77));

        assert_eq!(pet.react_alert(), Mood::Alert);
        assert_eq!(pet.cmd_streak, 0);
        assert_eq!(pet.effective_stats(later), expected);

        // Repeated alerts while still idle must never double-count lazy decay.
        for _ in 0..5 {
            assert_eq!(pet.react_alert(), Mood::Alert);
        }
        assert_eq!(pet.effective_stats(later), expected);
        assert_eq!((pet.fullness, pet.happiness, pet.last_fed_epoch), (80, 80, BASE_EPOCH));
    }

    #[test]
    fn species_unlocking_and_renaming() {
        let mut pet = Pet::default();
        for (sp, badge, watch_hint) in [
            (Species::Cat, "pathfinder", "watch 0/25 unique commands"),
            (Species::Turtle, "pipe_plumber", "watch 0/100 commands / 0d/7d streak"),
            (Species::Owl, "night_watch", "watch 0/250 commands / 0/5 warnings"),
        ] {
            let err = pet.set_species(sp).unwrap_err().to_string();
            assert!(err.contains(badge), "{err}");
            assert!(err.contains("/mode lab"), "{err}");
            assert!(err.contains(watch_hint), "{err}");
            assert!(!err.contains("huh learn"), "{err}");
        }
        assert!(pet.unlock_species(Species::Cat));
        assert!(!pet.unlock_species(Species::Cat));
        pet.set_species(Species::Cat).unwrap();
        assert_eq!(pet.species, Species::Cat);

        pet.rename("  Tuxi  ").unwrap();
        assert_eq!(pet.name, "Tuxi");
        pet.rename("Éclair").unwrap();
        assert_eq!(pet.name, "Éclair");
        pet.rename("Ștefan").unwrap();
        assert_eq!(pet.name, "Ștefan");

        for invalid in [
            "   ",
            "123456789012345678901",
            "bad\nname",
            "bad\tname",
            "Pip 🐧",
            "✨Pip",
            "ペンギン",
            "漢字",
            "한글",
            "ＡＢＣ",
            "e\u{0301}",
            "A\u{FE0F}",
            "a\u{200B}b",
            "a\u{00AD}b",
        ] {
            assert!(pet.rename(invalid).is_err(), "expected rename to reject {invalid:?}");
            let mut dirty = Pet { name: invalid.to_string(), ..Pet::default() };
            dirty.normalize();
            assert_eq!(dirty.name, DEFAULT_NAME, "expected normalize to reset {invalid:?}");
        }

        let mut accented = Pet { name: "  Éclair  ".to_string(), ..Pet::default() };
        accented.normalize();
        assert_eq!(accented.name, "Éclair");

        assert_eq!(Species::parse("TURTLE").unwrap(), Species::Turtle);
        assert!(Species::parse("dragon").unwrap_err().to_string().contains(Species::VALID));
    }

    #[test]
    fn config_opt_out_and_state_path() {
        assert!(pet_off_in_config("pet = false\n"));
        assert!(pet_off_in_config("pet = \"off\"\n"));
        assert!(pet_off_in_config("pet = \"OFF\"\n"));
        assert!(!pet_off_in_config("pet = true\n"));
        assert!(!pet_off_in_config("pet = \"penguin\"\n"));
        assert!(!pet_off_in_config("not toml ["));
        assert!(enabled_with(false, false));
        assert!(!enabled_with(true, false));
        assert!(!enabled_with(false, true));

        assert_eq!(state_path(Some("/s"), Some("/h")).unwrap(), Path::new("/s/huh/pet.toml"));
        assert_eq!(state_path(None, Some("/h")).unwrap(), Path::new("/h/.local/state/huh/pet.toml"));
        assert_eq!(state_path(None, None), None);
    }

    #[test]
    fn atomic_save_and_load_round_trip() {
        let dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("target/pet-tests").join(format!("{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("pet.toml");
        assert_eq!(load_from(&path), Pet::default());

        let mut pet = Pet::default();
        pet.rename("Bloop").unwrap();
        pet.unlock_species(Species::Turtle);
        pet.set_species(Species::Turtle).unwrap();
        pet.feed(Some("ls"), BASE_EPOCH);
        save_to(&path, &pet).unwrap();
        assert_eq!(load_from(&path), pet);

        // Corrupt or out-of-range files recover cleanly.
        std::fs::write(&path, "not valid toml [[[").unwrap();
        assert_eq!(load_from(&path), Pet::default());
        std::fs::write(&path, "fullness = 0\nhappiness = 250\nspecies = \"owl\"\n").unwrap();
        let recovered = load_from(&path);
        assert_eq!(recovered.fullness, MIN_FULLNESS);
        assert_eq!(recovered.happiness, 100);
        assert_eq!(recovered.species, Species::Penguin);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_and_tutor_card_render_plain_text_in_tests() {
        let pet = Pet::default();
        let status = render_status(&pet, BASE_EPOCH, None, true);
        assert!(!status.contains('\x1b'));
        assert!(status.contains("Pebble (Penguin chick)"), "{status}");
        assert!(status.contains("#!"), "{status}");
        assert!(status.contains("pathfinder"), "{status}");
        assert!(status.contains("/mode lab"), "{status}");
        assert!(status.contains("HUH_NO_PET"), "{status}");

        let card = render_tutor_card(&pet, BASE_EPOCH, Some(Mood::Celebrating));
        assert_eq!(card, render_pane_card(&pet, BASE_EPOCH, Some(Mood::Celebrating)));
        assert!(!card.contains('\x1b'));
        assert!(card.contains("celebrating"), "{card}");
        assert!(card.ends_with("\n\n"));

        for mood in Mood::ALL {
            let tip = mood.tip();
            assert!(!tip.contains("Tummy rumbling"), "{mood:?}: {tip}");
            assert!(!tip.contains("huh tutor"), "{mood:?}: {tip}");
        }
    }

    #[test]
    fn watch_unlocks_and_record_warning() {
        // Unique commands normalize lowercase, deduplicate, and unlock Cat at 25.
        let mut pet = Pet::default();
        pet.feed(Some("LS"), BASE_EPOCH);
        pet.feed(Some("ls"), BASE_EPOCH + 10);
        assert_eq!(pet.unique_programs, vec!["ls".to_string()]);
        assert!(pet.check_watch_unlocks().is_empty());

        for i in 1..CAT_WATCH_UNIQUE {
            pet.feed(Some(&format!("prog{i}")), BASE_EPOCH + 10 + i as u64);
        }
        assert_eq!(pet.unique_programs.len(), CAT_WATCH_UNIQUE);
        assert_eq!(pet.check_watch_unlocks(), vec![Species::Cat]);
        assert!(pet.is_unlocked(Species::Cat));
        assert!(pet.check_watch_unlocks().is_empty());

        // Unique program tracking is capped at 256 entries, while commands_fed unlocks Turtle and Owl.
        for i in 0..300 {
            pet.feed(Some(&format!("extra{i}")), BASE_EPOCH + 1000 + i as u64);
        }
        assert_eq!(pet.unique_programs.len(), 256);
        assert_eq!(pet.check_watch_unlocks(), vec![Species::Turtle, Species::Owl]);
        assert!(pet.is_unlocked(Species::Turtle));
        assert!(pet.is_unlocked(Species::Owl));

        // Turtle also unlocks via 7-day streak alone.
        let mut streaker = Pet::default();
        for day in 0..u64::from(TURTLE_WATCH_STREAK) {
            streaker.feed(Some("ls"), BASE_EPOCH + day * 86_400);
        }
        assert_eq!(streaker.day_streak, TURTLE_WATCH_STREAK);
        assert_eq!(streaker.check_watch_unlocks(), vec![Species::Turtle]);
        assert!(!streaker.is_unlocked(Species::Cat));

        // record_warning unlocks Owl at 5 warnings without touching lazy decay or stats.
        let mut watcher = Pet { fullness: 80, happiness: 80, last_fed_epoch: BASE_EPOCH, ..Pet::default() };
        let later = BASE_EPOCH + 24 * 3600;
        let expected_stats = watcher.effective_stats(later);
        for _ in 0..(OWL_WATCH_WARNINGS - 1) {
            assert!(watcher.record_warning().is_empty());
        }
        assert_eq!(watcher.record_warning(), vec![Species::Owl]);
        assert!(watcher.is_unlocked(Species::Owl));
        assert_eq!(watcher.effective_stats(later), expected_stats);
        assert_eq!((watcher.fullness, watcher.happiness, watcher.last_fed_epoch), (80, 80, BASE_EPOCH));

        // Saturates on u32::MAX without panicking.
        let mut maxed = Pet { warnings_heeded: u32::MAX, ..Pet::default() };
        assert_eq!(maxed.record_warning(), vec![Species::Owl]);
        assert_eq!(maxed.warnings_heeded, u32::MAX);

        // Unlock summary displays both /mode lab badge and watch progress.
        let fresh = Pet::default();
        assert_eq!(Species::Penguin.unlock_summary(&fresh), None);
        let cat_summary = Species::Cat.unlock_summary(&fresh).unwrap();
        assert!(
            cat_summary.contains("pathfinder") && cat_summary.contains("/mode lab") && cat_summary.contains("0/25")
        );
    }
}
