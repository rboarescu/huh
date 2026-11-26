//! The `#!` terminal pet: pure ASCII mascot, local-only state machine, lazy time decay (never
//! dies), atomic TOML persistence under `~/.local/state/huh/pet.toml`, and status rendering.

use crate::date::{self, Date};
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
pub const BEAVER_WATCH_COMMANDS: u64 = 50;
pub const TURTLE_WATCH_COMMANDS: u64 = 100;
pub const TURTLE_WATCH_STREAK: u32 = 7;
pub const DOLPHIN_WATCH_UNIQUE: usize = 35;
pub const DOLPHIN_WATCH_COMMANDS: u64 = 120;
pub const BEAR_WATCH_COMMANDS: u64 = 150;
pub const BEAR_WATCH_STREAK: u32 = 10;
pub const SNAKE_WATCH_UNIQUE: usize = 45;
pub const SNAKE_WATCH_COMMANDS: u64 = 180;
pub const BEE_WATCH_COMMANDS: u64 = 200;
pub const BEE_WATCH_STREAK: u32 = 14;
pub const OWL_WATCH_COMMANDS: u64 = 250;
pub const OWL_WATCH_WARNINGS: u32 = 5;
const MAX_UNIQUE_PROGRAMS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Species {
    #[default]
    Penguin,
    Cat,
    Beaver,
    Turtle,
    Dolphin,
    Bear,
    Snake,
    Bee,
    Owl,
}

impl Species {
    pub const ALL: [Species; 9] = [
        Species::Penguin,
        Species::Cat,
        Species::Beaver,
        Species::Turtle,
        Species::Dolphin,
        Species::Bear,
        Species::Snake,
        Species::Bee,
        Species::Owl,
    ];
    pub const VALID: &'static str = "penguin, cat, beaver, turtle, dolphin, bear, snake, bee, owl";

    pub fn parse(value: &str) -> Result<Species> {
        match value.trim().to_ascii_lowercase().as_str() {
            "penguin" => Ok(Species::Penguin),
            "cat" => Ok(Species::Cat),
            "beaver" | "bobr" => Ok(Species::Beaver),
            "turtle" => Ok(Species::Turtle),
            "dolphin" => Ok(Species::Dolphin),
            "bear" => Ok(Species::Bear),
            "snake" => Ok(Species::Snake),
            "bee" => Ok(Species::Bee),
            "owl" => Ok(Species::Owl),
            other => bail!("unknown pet species \"{other}\"; valid species: {}", Self::VALID),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Species::Penguin => "penguin",
            Species::Cat => "cat",
            Species::Beaver => "beaver",
            Species::Turtle => "turtle",
            Species::Dolphin => "dolphin",
            Species::Bear => "bear",
            Species::Snake => "snake",
            Species::Bee => "bee",
            Species::Owl => "owl",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Species::Penguin => "Penguin chick",
            Species::Cat => "Terminal Cat",
            Species::Beaver => "Builder Beaver",
            Species::Turtle => "Shell-Turtle",
            Species::Dolphin => "Wave Dolphin",
            Species::Bear => "Woodland Bear",
            Species::Snake => "Terminal Serpent",
            Species::Bee => "Queen Bee",
            Species::Owl => "Snowy Owl",
        }
    }

    pub fn default_name(self) -> &'static str {
        match self {
            Species::Penguin | Species::Cat | Species::Turtle | Species::Owl => DEFAULT_NAME,
            Species::Beaver => "Bóbr",
            Species::Dolphin => "Flow",
            Species::Bear => "Vector",
            Species::Snake => "Hiss",
            Species::Bee => "Anabee",
        }
    }

    pub fn unlock_badge(self) -> Option<(&'static str, &'static str)> {
        match self {
            Species::Penguin => None,
            Species::Cat => Some(("pathfinder", "finish Unit 1 (Finding Your Way) in /mode lab")),
            Species::Beaver => Some(("file_keeper", "finish Unit 2 (Files & Folders) in /mode lab")),
            Species::Turtle => Some(("pipe_plumber", "finish Unit 3 (Searching & Pipes) in /mode lab")),
            Species::Dolphin => Some(("media_wrangler", "finish Unit 10 (Media & Everyday Files) in /mode lab")),
            Species::Bear => Some(("process_tamer", "finish Unit 5 (Processes & Disk) in /mode lab")),
            Species::Snake => Some(("archive_ace", "finish Unit 7 (Archives & Links) in /mode lab")),
            Species::Bee => Some(("pixel_painter", "finish Unit 11 (Paint Your Own Pet) in /mode lab")),
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
            Species::Beaver => {
                Some(format!("watch {}/{BEAVER_WATCH_COMMANDS} commands", pet.commands_fed.min(BEAVER_WATCH_COMMANDS)))
            }
            Species::Turtle => Some(format!(
                "watch {}/{TURTLE_WATCH_COMMANDS} commands / {}d/{TURTLE_WATCH_STREAK}d streak",
                pet.commands_fed.min(TURTLE_WATCH_COMMANDS),
                pet.day_streak.max(pet.best_day_streak).min(TURTLE_WATCH_STREAK)
            )),
            Species::Dolphin => Some(format!(
                "watch {}/{DOLPHIN_WATCH_UNIQUE} unique commands or {}/{DOLPHIN_WATCH_COMMANDS} commands",
                pet.unique_programs.len().min(DOLPHIN_WATCH_UNIQUE),
                pet.commands_fed.min(DOLPHIN_WATCH_COMMANDS)
            )),
            Species::Bear => Some(format!(
                "watch {}/{BEAR_WATCH_COMMANDS} commands / {}d/{BEAR_WATCH_STREAK}d streak",
                pet.commands_fed.min(BEAR_WATCH_COMMANDS),
                pet.day_streak.max(pet.best_day_streak).min(BEAR_WATCH_STREAK)
            )),
            Species::Snake => Some(format!(
                "watch {}/{SNAKE_WATCH_UNIQUE} unique commands or {}/{SNAKE_WATCH_COMMANDS} commands",
                pet.unique_programs.len().min(SNAKE_WATCH_UNIQUE),
                pet.commands_fed.min(SNAKE_WATCH_COMMANDS)
            )),
            Species::Bee => Some(format!(
                "watch {}/{BEE_WATCH_COMMANDS} commands / {}d/{BEE_WATCH_STREAK}d streak",
                pet.commands_fed.min(BEE_WATCH_COMMANDS),
                pet.day_streak.max(pet.best_day_streak).min(BEE_WATCH_STREAK)
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
        let (badge, unit) = match self {
            Species::Penguin => return None,
            Species::Cat => ("pathfinder", "Unit 1"),
            Species::Beaver => ("file_keeper", "Unit 2"),
            Species::Turtle => ("pipe_plumber", "Unit 3"),
            Species::Dolphin => ("media_wrangler", "Unit 10"),
            Species::Bear => ("process_tamer", "Unit 5"),
            Species::Snake => ("archive_ace", "Unit 7"),
            Species::Bee => ("pixel_painter", "Unit 11"),
            Species::Owl => ("night_watch", "Unit 8"),
        };
        Some(format!("badge `{badge}` ({unit} in `/mode lab`) or {watch}"))
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

        (Species::Beaver, Mood::Idle) => {
            if tick % 24 == 23 {
                ["  ()__()  ", " (=-.-=)  ", "  /#! \\###", "  `---'   "]
            } else {
                ["  ()__()  ", " (=o.o=)  ", "  /#! \\###", "  `---'   "]
            }
        }
        (Species::Beaver, Mood::Happy) => {
            if (tick / 4) % 2 == 1 {
                ["  ()__() '", " (=^.^=)  ", "  /#! \\###", "  `---'   "]
            } else {
                ["  ()__()  ", " (=^.^=)  ", "  /#! \\###", "  `---'   "]
            }
        }
        (Species::Beaver, Mood::Sleepy) => {
            if (tick / 3) % 2 == 1 {
                ["  ()__() Z", " (=-.-=)  ", "  /#! \\###", "  `---'   "]
            } else {
                ["  ()__() z", " (=-.-=)  ", "  /#! \\###", "  `---'   "]
            }
        }
        (Species::Beaver, Mood::Hungry) => {
            if (tick / 3) % 2 == 1 {
                ["  ()__() .", " (=o_o=)  ", "  /#! \\###", "  `---'   "]
            } else {
                ["  ()__() ?", " (=o_o=)  ", "  /#! \\###", "  `---'   "]
            }
        }
        (Species::Beaver, Mood::Alert) => {
            if tick % 2 == 1 {
                ["  ()__()  ", " (=O.O=)  ", "  /#! \\###", "  `---'   "]
            } else {
                ["  ()__() !", " (=O.O=)  ", "  /#! \\###", "  `---'   "]
            }
        }
        (Species::Beaver, Mood::Celebrating) => {
            if tick % 2 == 1 {
                ["  ()__() +", " (=^w^=)  ", " \\/#! \\###", "  `---'   "]
            } else {
                ["  ()__() *", " (=^w^=)  ", " \\/#! \\###", "  `---'   "]
            }
        }

        (Species::Dolphin, Mood::Idle) => {
            if tick % 24 == 23 {
                ["    __    ", " <(- \\__/\\", "  (#! _/  ", " ~~~~~~~~~"]
            } else {
                ["    __    ", " <(o \\__/\\", "  (#! _/  ", " ~~~~~~~~~"]
            }
        }
        (Species::Dolphin, Mood::Happy) => {
            if (tick / 4) % 2 == 1 {
                ["    __   '", " <(^ \\__/\\", "  (#! _/  ", " ~~~~~~~~~"]
            } else {
                ["    __    ", " <(^ \\__/\\", "  (#! _/  ", " ~~~~~~~~~"]
            }
        }
        (Species::Dolphin, Mood::Sleepy) => {
            if (tick / 3) % 2 == 1 {
                ["    __   Z", " <(- \\__/\\", "  (#! _/  ", " ~~~~~~~~~"]
            } else {
                ["    __   z", " <(- \\__/\\", "  (#! _/  ", " ~~~~~~~~~"]
            }
        }
        (Species::Dolphin, Mood::Hungry) => {
            if (tick / 3) % 2 == 1 {
                ["    __   .", " <(o \\__/\\", "  (#! _/  ", " ~~~~~~~~~"]
            } else {
                ["    __   ?", " <(o \\__/\\", "  (#! _/  ", " ~~~~~~~~~"]
            }
        }
        (Species::Dolphin, Mood::Alert) => {
            if tick % 2 == 1 {
                ["    __    ", " <(O \\__/\\", "  (#! _/  ", " ~~~~~~~~~"]
            } else {
                ["    __   !", " <(O \\__/\\", "  (#! _/  ", " ~~~~~~~~~"]
            }
        }
        (Species::Dolphin, Mood::Celebrating) => {
            if tick % 2 == 1 {
                ["    __   +", " <(^ \\__/\\", " \\(#! _/  ", " ~~~~~~~~~"]
            } else {
                ["    __   *", " <(^ \\__/\\", " \\(#! _/  ", " ~~~~~~~~~"]
            }
        }

        (Species::Bear, Mood::Idle) => {
            if tick % 24 == 23 {
                ["  ()__()  ", " ( -^- )  ", "  / #! \\  ", "  c(__)c  "]
            } else {
                ["  ()__()  ", " ( o^o )  ", "  / #! \\  ", "  c(__)c  "]
            }
        }
        (Species::Bear, Mood::Happy) => {
            if (tick / 4) % 2 == 1 {
                ["  ()__() '", " ( ^o^ )  ", "  / #! \\  ", "  c(__)c  "]
            } else {
                ["  ()__()  ", " ( ^o^ )  ", "  / #! \\  ", "  c(__)c  "]
            }
        }
        (Species::Bear, Mood::Sleepy) => {
            if (tick / 3) % 2 == 1 {
                ["  ()__() Z", " ( -v- )  ", "  / #! \\  ", "  c(__)c  "]
            } else {
                ["  ()__() z", " ( -v- )  ", "  / #! \\  ", "  c(__)c  "]
            }
        }
        (Species::Bear, Mood::Hungry) => {
            if (tick / 3) % 2 == 1 {
                ["  ()__() .", " ( o_o )  ", "  / #! \\  ", "  c(__)c  "]
            } else {
                ["  ()__() ?", " ( o_o )  ", "  / #! \\  ", "  c(__)c  "]
            }
        }
        (Species::Bear, Mood::Alert) => {
            if tick % 2 == 1 {
                ["  ()__()  ", " ( O^O )  ", "  / #! \\  ", "  c(__)c  "]
            } else {
                ["  ()__() !", " ( O^O )  ", "  / #! \\  ", "  c(__)c  "]
            }
        }
        (Species::Bear, Mood::Celebrating) => {
            if tick % 2 == 1 {
                ["  ()__() +", " ( ^o^ )  ", "\\( #! )/  ", "  c(__)c  "]
            } else {
                ["  ()__() *", " ( ^o^ )  ", "\\( #! )/  ", "  c(__)c  "]
            }
        }

        (Species::Snake, Mood::Idle) => {
            if tick % 24 == 23 {
                ["   .--.   ", "  /|--|\\  ", "  \\|#!|/  ", " ~(___)~  "]
            } else {
                ["   .--.   ", "  /|oo|\\  ", "  \\|#!|/  ", " ~(___)~  "]
            }
        }
        (Species::Snake, Mood::Happy) => {
            if (tick / 4) % 2 == 1 {
                ["   .--.  '", "  /|^^|\\  ", "  \\|#!|/  ", " ~(___)~  "]
            } else {
                ["   .--.   ", "  /|^^|\\  ", "  \\|#!|/  ", " ~(___)~  "]
            }
        }
        (Species::Snake, Mood::Sleepy) => {
            if (tick / 3) % 2 == 1 {
                ["   .--.  Z", "  /|--|\\  ", "  \\|#!|/  ", " ~(___)~  "]
            } else {
                ["   .--.  z", "  /|--|\\  ", "  \\|#!|/  ", " ~(___)~  "]
            }
        }
        (Species::Snake, Mood::Hungry) => {
            if (tick / 3) % 2 == 1 {
                ["   .--.  .", "  /|o_|\\  ", "  \\|#!|/  ", " ~(___)~  "]
            } else {
                ["   .--.  ?", "  /|o_|\\  ", "  \\|#!|/  ", " ~(___)~  "]
            }
        }
        (Species::Snake, Mood::Alert) => {
            if tick % 2 == 1 {
                ["   .--.   ", "  /|OO|\\  ", "  \\|#!|/  ", " ~(___)~  "]
            } else {
                ["   .--.  !", "  /|OO|\\  ", "  \\|#!|/  ", " ~(___)~  "]
            }
        }
        (Species::Snake, Mood::Celebrating) => {
            if tick % 2 == 1 {
                ["   .--.  +", "  /|^^|\\~ ", "  \\|#!|/  ", " ~(___)~  "]
            } else {
                ["   .--.  *", "  /|^^|\\~ ", "  \\|#!|/  ", " ~(___)~  "]
            }
        }

        (Species::Bee, Mood::Idle) => {
            if tick % 24 == 23 {
                ["   .w.    ", " =( -.- )=", "   (#!#)  ", "    `v'   "]
            } else {
                ["   .w.    ", " =( o.o )=", "   (#!#)  ", "    `v'   "]
            }
        }
        (Species::Bee, Mood::Happy) => {
            if (tick / 4) % 2 == 1 {
                ["   .w.   '", " =( ^.^ )=", "   (#!#)  ", "    `v'   "]
            } else {
                ["   .w.    ", " =( ^.^ )=", "   (#!#)  ", "    `v'   "]
            }
        }
        (Species::Bee, Mood::Sleepy) => {
            if (tick / 3) % 2 == 1 {
                ["   .w.   Z", " =( -.- )=", "   (#!#)  ", "    `v'   "]
            } else {
                ["   .w.   z", " =( -.- )=", "   (#!#)  ", "    `v'   "]
            }
        }
        (Species::Bee, Mood::Hungry) => {
            if (tick / 3) % 2 == 1 {
                ["   .w.   .", " =( o_o )=", "   (#!#)  ", "    `v'   "]
            } else {
                ["   .w.   ?", " =( o_o )=", "   (#!#)  ", "    `v'   "]
            }
        }
        (Species::Bee, Mood::Alert) => {
            if tick % 2 == 1 {
                ["   .w.    ", " =( O_O )=", "   (#!#)  ", "    `v'   "]
            } else {
                ["   .w.   !", " =( O_O )=", "   (#!#)  ", "    `v'   "]
            }
        }
        (Species::Bee, Mood::Celebrating) => {
            if tick % 2 == 1 {
                ["   .w.   +", " =\\ ^.^ /=", "   (#!#)  ", "    `v'   "]
            } else {
                ["   .w.   *", " =\\ ^.^ /=", "   (#!#)  ", "    `v'   "]
            }
        }
    }
}

/// 4-line pure printable ASCII art (`<= ART_MAX_WIDTH` cols) carrying `#!`.
pub fn art(species: Species, mood: Mood) -> [&'static str; ART_HEIGHT] {
    art_frame(species, mood, 0)
}

/// Static metadata describing a collectible shiny pebble gifted by the `#!` companion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PebbleDef {
    pub id: &'static str,
    pub name: &'static str,
    pub flavor: &'static str,
    pub milestone_hint: &'static str,
}

/// A shiny pebble gifted by the companion, stored in `~/.local/state/huh/pet.toml`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GiftedPebble {
    pub id: String,
    #[serde(default)]
    pub gifted_day: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PebbleTrigger {
    CommandsFed(u64),
    DayStreak(u32),
    UniquePrograms(usize),
    WarningsHeeded(u32),
    LabMilestones(u32),
}

impl PebbleTrigger {
    fn progress(self, pet: &Pet) -> (u64, u64) {
        match self {
            PebbleTrigger::CommandsFed(target) => (pet.commands_fed.min(target), target),
            PebbleTrigger::DayStreak(target) => {
                (u64::from(pet.day_streak.max(pet.best_day_streak).min(target)), u64::from(target))
            }
            PebbleTrigger::UniquePrograms(target) => (pet.unique_programs.len().min(target) as u64, target as u64),
            PebbleTrigger::WarningsHeeded(target) => (u64::from(pet.warnings_heeded.min(target)), u64::from(target)),
            PebbleTrigger::LabMilestones(target) => (u64::from(pet.lab_milestones.min(target)), u64::from(target)),
        }
    }

    fn is_met(self, pet: &Pet) -> bool {
        let (cur, target) = self.progress(pet);
        cur >= target
    }
}

macro_rules! define_pebbles {
    ($(($id:expr, $name:expr, $flavor:expr, $hint:expr, $trigger:expr)),* $(,)?) => {
        /// Curated catalog of shiny pebbles gifted as affection milestones are reached.
        pub const PEBBLE_CATALOG: &[PebbleDef] = &[
            $(PebbleDef { id: $id, name: $name, flavor: $flavor, milestone_hint: $hint }),*
        ];
        const PEBBLE_TRIGGERS: &[(&str, PebbleTrigger)] = &[
            $(($id, $trigger)),*
        ];
    };
}

define_pebbles!(
    (
        "beach_quartz",
        "Smooth Beach Quartz",
        "A milky quartz pebble nudged to your feet, the classic penguin hello.",
        "run 5 clean commands",
        PebbleTrigger::CommandsFed(5)
    ),
    (
        "river_agate",
        "Banded River Agate",
        "Warm striped stone polished by steady command streams.",
        "run 25 clean commands",
        PebbleTrigger::CommandsFed(25)
    ),
    (
        "tidal_basalt",
        "Tidal Basalt",
        "Smooth dark shore stone, heavy with growing muscle memory.",
        "run 75 clean commands",
        PebbleTrigger::CommandsFed(75)
    ),
    (
        "sunlit_amber",
        "Sunlit Amber",
        "Glowing golden resin saved for a trusted shell companion.",
        "run 150 clean commands",
        PebbleTrigger::CommandsFed(150)
    ),
    (
        "starlight_obsidian",
        "Starlight Obsidian",
        "Volcanic mirror stone gleaming after three hundred clean exits.",
        "run 300 clean commands",
        PebbleTrigger::CommandsFed(300)
    ),
    (
        "three_tide_jasper",
        "Three-Tide Jasper",
        "Brick-red pebble gathered across three consecutive mornings.",
        "reach a 3-day streak",
        PebbleTrigger::DayStreak(3)
    ),
    (
        "frost_moonstone",
        "Frost Moonstone",
        "Cool blue-sheen stone from a full week side by side.",
        "reach a 7-day streak",
        PebbleTrigger::DayStreak(7)
    ),
    (
        "aurora_opal",
        "Aurora Opal",
        "Iridescent opal catching every colour of a two-week streak.",
        "reach a 14-day streak",
        PebbleTrigger::DayStreak(14)
    ),
    (
        "glacier_diamond",
        "Glacier Diamond",
        "A rare, clear ice-prism pebble for thirty days of shared craft.",
        "reach a 30-day streak",
        PebbleTrigger::DayStreak(30)
    ),
    (
        "curious_seaglass",
        "Curious Sea-Glass",
        "Frosted sea-green glass from exploring fifteen distinct tools.",
        "try 15 unique programs",
        PebbleTrigger::UniquePrograms(15)
    ),
    (
        "explorer_jade",
        "Explorer Jade",
        "Deep-green jade kept by seasoned Gentoo explorers.",
        "try 40 unique programs",
        PebbleTrigger::UniquePrograms(40)
    ),
    (
        "guardian_slate",
        "Guardian Slate",
        "Crisp etched slate, a nod for heeding three safety warnings.",
        "heed 3 safety warnings",
        PebbleTrigger::WarningsHeeded(3)
    ),
    (
        "scholars_pearl",
        "Scholar's Pearl",
        "A bright tidepool pearl brought back from your first Lab milestone.",
        "reach 1 Lab milestone",
        PebbleTrigger::LabMilestones(1)
    ),
    (
        "crystal_geode",
        "Crystal Geode",
        "Split-open violet quartz sparkling after five Lab milestones.",
        "reach 5 Lab milestones",
        PebbleTrigger::LabMilestones(5)
    ),
);

/// Hidden easter-egg pebbles granted only by discovering secret lesson codes.
pub const SECRET_PEBBLES: &[PebbleDef] = &[
    PebbleDef {
        id: "secret_vault",
        name: "Vault Moonstone",
        flavor: "Unlocked by explorers who read the fine print inside secret.key.",
        milestone_hint: "Hidden in Unit 4",
    },
    PebbleDef {
        id: "secret_manual",
        name: "Manual Bookmark",
        flavor: "Real wizards always read to the bottom of the manual.",
        milestone_hint: "Hidden in Unit 6",
    },
    PebbleDef {
        id: "secret_konami",
        name: "8-Bit Star",
        flavor: "30 lives, 9 companions, 0 bugs.",
        milestone_hint: "Hidden in Unit 9",
    },
    PebbleDef {
        id: "secret_relic",
        name: "Ancient Cave Pebble",
        flavor: "Handed over by a bearded terminal hermit beside green phosphor embers.",
        milestone_hint: "Hidden in Unit 9",
    },
];

/// Looks up a pebble definition by its stable ID (including hidden easter-egg pebbles).
pub fn pebble_def(id: &str) -> Option<&'static PebbleDef> {
    PEBBLE_CATALOG.iter().chain(SECRET_PEBBLES.iter()).find(|p| p.id == id)
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
    #[serde(default)]
    pub lab_milestones: u32,
    #[serde(default)]
    pub pebbles: Vec<GiftedPebble>,
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
            lab_milestones: 0,
            pebbles: Vec::new(),
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

fn is_default_mascot_name(name: &str) -> bool {
    let lower = name.trim().to_lowercase();
    Species::ALL.iter().any(|sp| sp.default_name().to_lowercase() == lower)
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
        let mut seen_pebbles = Vec::with_capacity(self.pebbles.len());
        let mut clean_pebbles = Vec::with_capacity(self.pebbles.len());
        for pebble in &self.pebbles {
            let id = pebble.id.trim();
            if !id.is_empty() && !seen_pebbles.iter().any(|s: &String| s == id) {
                seen_pebbles.push(id.to_string());
                clean_pebbles
                    .push(GiftedPebble { id: id.to_string(), gifted_day: pebble.gifted_day.trim().to_string() });
            }
        }
        self.pebbles = clean_pebbles;
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

    /// Returns `true` if the pebble with `id` has already been gifted.
    pub fn has_pebble(&self, id: &str) -> bool {
        self.pebbles.iter().any(|p| p.id == id)
    }

    /// Number of catalog pebbles collected so far.
    pub fn pebbles_collected(&self) -> usize {
        PEBBLE_CATALOG.iter().filter(|def| self.has_pebble(def.id)).count()
    }

    /// Number of hidden easter-egg pebbles discovered so far.
    pub fn secret_pebbles_collected(&self) -> usize {
        SECRET_PEBBLES.iter().filter(|def| self.has_pebble(def.id)).count()
    }

    /// Grants a hidden easter-egg pebble by ID once, returning its definition if newly added.
    pub fn grant_secret_pebble(&mut self, id: &str, today: Date) -> Option<&'static PebbleDef> {
        let def = SECRET_PEBBLES.iter().find(|p| p.id == id)?;
        if self.has_pebble(def.id) {
            return None;
        }
        self.pebbles.push(GiftedPebble { id: def.id.to_string(), gifted_day: date::format(today) });
        Some(def)
    }

    /// Evaluates all pebble milestone conditions on `today`, appending any newly earned pebbles
    /// (idempotently — no duplicates) and returning their definitions in catalog order.
    pub fn check_pebble_gifts(&mut self, today: Date) -> Vec<&'static PebbleDef> {
        let day_str = date::format(today);
        let mut gifted = Vec::new();
        for (def, (trigger_id, trigger)) in PEBBLE_CATALOG.iter().zip(PEBBLE_TRIGGERS.iter()) {
            debug_assert_eq!(def.id, *trigger_id);
            if !self.has_pebble(def.id) && trigger.is_met(self) {
                self.pebbles.push(GiftedPebble { id: def.id.to_string(), gifted_day: day_str.clone() });
                gifted.push(def);
            }
        }
        gifted
    }

    /// Feeds the pet after a clean `exit 0` command, evaluates pebble milestones, and returns
    /// `(resulting_mood, newly_gifted_pebbles)`. Sets `Mood::Celebrating` when a pebble is gifted.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn feed_clean(&mut self, program: Option<&str>, now_epoch: u64) -> (Mood, Vec<&'static PebbleDef>) {
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

        let gifted = self.check_pebble_gifts(today);
        let mood = if !gifted.is_empty()
            || (self.cmd_streak > 0 && self.cmd_streak.is_multiple_of(10))
            || (day_advanced && self.day_streak >= 3)
        {
            Mood::Celebrating
        } else {
            self.mood_at(now_epoch, None)
        };
        (mood, gifted)
    }

    /// Feeds the pet after a clean `exit 0` command and returns its resulting mood.
    pub fn feed(&mut self, program: Option<&str>, now_epoch: u64) -> Mood {
        self.feed_clean(program, now_epoch).0
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
        let best_streak = self.day_streak.max(self.best_day_streak);
        if self.unique_programs.len() >= CAT_WATCH_UNIQUE && self.unlock_species(Species::Cat) {
            newly_unlocked.push(Species::Cat);
        }
        if self.commands_fed >= BEAVER_WATCH_COMMANDS && self.unlock_species(Species::Beaver) {
            newly_unlocked.push(Species::Beaver);
        }
        if (self.commands_fed >= TURTLE_WATCH_COMMANDS || best_streak >= TURTLE_WATCH_STREAK)
            && self.unlock_species(Species::Turtle)
        {
            newly_unlocked.push(Species::Turtle);
        }
        if (self.unique_programs.len() >= DOLPHIN_WATCH_UNIQUE || self.commands_fed >= DOLPHIN_WATCH_COMMANDS)
            && self.unlock_species(Species::Dolphin)
        {
            newly_unlocked.push(Species::Dolphin);
        }
        if (self.commands_fed >= BEAR_WATCH_COMMANDS || best_streak >= BEAR_WATCH_STREAK)
            && self.unlock_species(Species::Bear)
        {
            newly_unlocked.push(Species::Bear);
        }
        if (self.unique_programs.len() >= SNAKE_WATCH_UNIQUE || self.commands_fed >= SNAKE_WATCH_COMMANDS)
            && self.unlock_species(Species::Snake)
        {
            newly_unlocked.push(Species::Snake);
        }
        if (self.commands_fed >= BEE_WATCH_COMMANDS || best_streak >= BEE_WATCH_STREAK)
            && self.unlock_species(Species::Bee)
        {
            newly_unlocked.push(Species::Bee);
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

    /// Records a heeded safety warning on `today`, resets the command streak, and evaluates both
    /// watch species unlocks and pebble milestones. Sets `Mood::Celebrating` on unlocks or gifts.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn note_warning(&mut self, today: Date) -> (Mood, Vec<Species>, Vec<&'static PebbleDef>) {
        let mood = self.react_alert();
        let unlocked = self.record_warning();
        let gifted = self.check_pebble_gifts(today);
        let resulting_mood = if !unlocked.is_empty() || !gifted.is_empty() { Mood::Celebrating } else { mood };
        (resulting_mood, unlocked, gifted)
    }

    /// Increments `lab_milestones` and checks for newly unlocked pebbles on `today`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn record_lab_milestone(&mut self, today: Date) -> Vec<&'static PebbleDef> {
        self.lab_milestones = self.lab_milestones.saturating_add(1);
        self.check_pebble_gifts(today)
    }

    /// Monotonically updates `lab_milestones` to at least `count` and checks for newly unlocked pebbles.
    /// Useful when syncing from `Progress` counts in Pane/Lab hooks without double-counting.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn sync_lab_milestones(&mut self, count: u32, today: Date) -> Vec<&'static PebbleDef> {
        if count > self.lab_milestones {
            self.lab_milestones = count;
        }
        self.check_pebble_gifts(today)
    }

    /// Merges `lab_milestones` and any missing `pebbles` from another `Pet` snapshot (e.g. disk).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn merge_pebbles_from(&mut self, other: &Pet) {
        self.lab_milestones = self.lab_milestones.max(other.lab_milestones);
        for pebble in &other.pebbles {
            let id = pebble.id.trim();
            if !id.is_empty() && !self.has_pebble(id) {
                self.pebbles
                    .push(GiftedPebble { id: id.to_string(), gifted_day: pebble.gifted_day.trim().to_string() });
            }
        }
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

    /// Unlocks all companion species, returning only the ones newly unlocked.
    pub fn unlock_all_species(&mut self) -> Vec<Species> {
        Species::ALL.into_iter().filter(|&sp| self.unlock_species(sp)).collect()
    }

    /// Tops up fullness and happiness to 100% and refreshes the fed timestamp.
    pub fn pamper(&mut self, now_epoch: u64) {
        self.fullness = 100;
        self.happiness = 100;
        self.last_fed_epoch = now_epoch;
    }

    pub fn set_species(&mut self, species: Species) -> Result<()> {
        if self.is_unlocked(species) {
            self.species = species;
            if is_default_mascot_name(&self.name) {
                self.name = species.default_name().to_string();
            }
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

impl Pet {
    /// Returns the next un-gifted pebble closest to completion `(def, current, target)`.
    pub fn next_pebble(&self) -> Option<(&'static PebbleDef, u64, u64)> {
        PEBBLE_CATALOG
            .iter()
            .zip(PEBBLE_TRIGGERS.iter())
            .enumerate()
            .filter(|(_, (def, _))| !self.has_pebble(def.id))
            .map(|(idx, (def, (_, trigger)))| {
                let (cur, target) = trigger.progress(self);
                let ratio =
                    if target == 0 { 0 } else { (u128::from(cur.min(target)) * 1_000_000) / u128::from(target) };
                ((ratio, std::cmp::Reverse(idx)), (def, cur, target))
            })
            .max_by_key(|(key, _)| *key)
            .map(|(_, info)| info)
    }

    /// Renders the companion's pebble collection card (gifted pebbles with dates + next milestone hint).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn render_pebbles(&self) -> String {
        let mut out = String::new();
        let collected = self.pebbles_collected();
        let total = PEBBLE_CATALOG.len();
        let secret_collected = self.secret_pebbles_collected();
        let is_secret = |id: &str| SECRET_PEBBLES.iter().any(|p| p.id == id);
        let normal_pebbles: Vec<&GiftedPebble> = self.pebbles.iter().filter(|g| !is_secret(&g.id)).collect();
        let _ = writeln!(
            out,
            "{} {}/{} collected",
            paint(Role::Header, &format!("Pebble Collection — {}:", self.name)),
            paint(Role::Value, &collected.to_string()),
            total
        );
        if normal_pebbles.is_empty() {
            let _ = writeln!(
                out,
                "  {}",
                paint(Role::Dim, "No shiny pebbles gifted yet — keep running clean commands or /mode lab lessons.")
            );
        } else {
            for gifted in normal_pebbles {
                let date_badge = if gifted.gifted_day.is_empty() {
                    String::new()
                } else {
                    format!(" {}", paint(Role::Dim, &format!("[{}]", gifted.gifted_day)))
                };
                if let Some(def) = pebble_def(&gifted.id) {
                    let _ = writeln!(out, "  * {}{date_badge}", paint(Role::Success, def.name));
                    let _ = writeln!(out, "    {}", paint(Role::Description, def.flavor));
                } else {
                    let _ = writeln!(out, "  * {}{date_badge}", gifted.id);
                }
            }
        }
        if secret_collected > 0 {
            let _ = writeln!(out);
            let _ = writeln!(out, "{}", paint(Role::Header, "Secret pebbles discovered:"));
            for gifted in self.pebbles.iter().filter(|g| is_secret(&g.id)) {
                let date_badge = if gifted.gifted_day.is_empty() {
                    String::new()
                } else {
                    format!(" {}", paint(Role::Dim, &format!("[{}]", gifted.gifted_day)))
                };
                if let Some(def) = pebble_def(&gifted.id) {
                    let _ = writeln!(out, "  * {}{date_badge}", paint(Role::Accent, def.name));
                    let _ = writeln!(out, "    {}", paint(Role::Description, def.flavor));
                }
            }
        }
        let _ = writeln!(out);
        if let Some((next_def, cur, target)) = self.next_pebble() {
            let _ = writeln!(
                out,
                "{} {} — {} ({}/{})",
                paint(Role::Header, "Next pebble:"),
                paint(Role::Accent, next_def.name),
                next_def.milestone_hint,
                paint(Role::Value, &cur.to_string()),
                target
            );
        } else {
            let _ = writeln!(out, "{}", paint(Role::Success, "All shiny pebbles in the collection have been gifted!"));
        }
        out
    }

    /// Full status card printed by `huh pet`.
    pub fn render_status(&self, now_epoch: u64, transient: Option<Mood>, hidden_in_tutor: bool) -> String {
        let mut out = String::new();
        for line in card_lines(self, now_epoch, transient) {
            let _ = writeln!(out, "{line}");
        }
        let _ = writeln!(out);
        let _ = writeln!(out, "{}", paint(Role::Header, "Species:"));
        for species in Species::ALL {
            let marker = if species == self.species { "*" } else { " " };
            let status = if species == self.species {
                paint(Role::Success, "active")
            } else if self.is_unlocked(species) {
                paint(Role::Description, "unlocked")
            } else if let Some(summary) = species.unlock_summary(self) {
                paint(Role::Dim, &format!("locked — {summary}"))
            } else {
                paint(Role::Description, "unlocked")
            };
            let _ = writeln!(out, "  {marker} {:<8} {:<14} {status}", species.name(), species.label());
        }
        let collected = self.pebbles_collected();
        let total = PEBBLE_CATALOG.len();
        let secret = self.secret_pebbles_collected();
        let secret_note = if secret > 0 { format!(" (+{secret} secret)") } else { String::new() };
        let pebble_tail = if let Some((next_def, cur, target)) = self.next_pebble() {
            paint(Role::Dim, &format!(" · next: {} ({cur}/{target}) · huh pet --pebbles", next_def.name))
        } else {
            paint(Role::Success, " · collection complete! (huh pet --pebbles)")
        };
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "{} {}/{total} collected{secret_note}{pebble_tail}",
            paint(Role::Header, "Pebbles:"),
            paint(Role::Value, &collected.to_string())
        );
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
}

/// Full status card printed by `huh pet`.
pub fn render_status(pet: &Pet, now_epoch: u64, transient: Option<Mood>, hidden_in_tutor: bool) -> String {
    pet.render_status(now_epoch, transient, hidden_in_tutor)
}

/// Warm one-line Watch/Lab toast when the companion gifts a newly unlocked shiny pebble.
#[cfg_attr(not(test), allow(dead_code))]
pub fn format_pebble_gift(pet_name: &str, def: &PebbleDef) -> String {
    format!(
        "{} {} — {}",
        paint(Role::Success, &format!("{pet_name} gifted a shiny pebble:")),
        paint(Role::Header, def.name),
        def.flavor
    )
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
        for locked in [
            Species::Cat,
            Species::Beaver,
            Species::Turtle,
            Species::Dolphin,
            Species::Bear,
            Species::Snake,
            Species::Bee,
            Species::Owl,
        ] {
            assert!(!pet.is_unlocked(locked), "{locked:?} should start locked");
        }
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
            (Species::Beaver, "file_keeper", "watch 0/50 commands"),
            (Species::Turtle, "pipe_plumber", "watch 0/100 commands / 0d/7d streak"),
            (Species::Dolphin, "media_wrangler", "watch 0/35 unique commands or 0/120 commands"),
            (Species::Bear, "process_tamer", "watch 0/150 commands / 0d/10d streak"),
            (Species::Snake, "archive_ace", "watch 0/45 unique commands or 0/180 commands"),
            (Species::Bee, "pixel_painter", "watch 0/200 commands / 0d/14d streak"),
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

        pet.rename("Bóbr").unwrap();
        assert_eq!(pet.name, "Bóbr");
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
        assert!(status.contains("Pebbles: 0/"), "{status}");
        assert_eq!(status, pet.render_status(BASE_EPOCH, None, true));

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

        // Unique program tracking is capped at 256 entries, while higher counts unlock the remaining species.
        for i in 0..300 {
            pet.feed(Some(&format!("extra{i}")), BASE_EPOCH + 1000 + i as u64);
        }
        assert_eq!(pet.unique_programs.len(), 256);
        assert_eq!(
            pet.check_watch_unlocks(),
            vec![
                Species::Beaver,
                Species::Turtle,
                Species::Dolphin,
                Species::Bear,
                Species::Snake,
                Species::Bee,
                Species::Owl,
            ]
        );
        for sp in Species::ALL {
            assert!(pet.is_unlocked(sp), "{sp:?} should be unlocked");
        }

        // Turtle unlocks at 7d streak; Bear at 10d streak; Bee at 14d streak.
        let mut streaker = Pet::default();
        for day in 0..u64::from(TURTLE_WATCH_STREAK) {
            streaker.feed(Some("ls"), BASE_EPOCH + day * 86_400);
        }
        assert_eq!(streaker.day_streak, TURTLE_WATCH_STREAK);
        assert_eq!(streaker.check_watch_unlocks(), vec![Species::Turtle]);
        assert!(!streaker.is_unlocked(Species::Cat));
        assert!(!streaker.is_unlocked(Species::Bear));

        for day in u64::from(TURTLE_WATCH_STREAK)..u64::from(BEAR_WATCH_STREAK) {
            streaker.feed(Some("ls"), BASE_EPOCH + day * 86_400);
        }
        assert_eq!(streaker.check_watch_unlocks(), vec![Species::Bear]);
        for day in u64::from(BEAR_WATCH_STREAK)..u64::from(BEE_WATCH_STREAK) {
            streaker.feed(Some("ls"), BASE_EPOCH + day * 86_400);
        }
        assert_eq!(streaker.check_watch_unlocks(), vec![Species::Bee]);

        // Beaver unlocks cleanly at 50 repeated commands.
        let mut builder = Pet::default();
        for i in 0..BEAVER_WATCH_COMMANDS {
            builder.feed(Some("mkdir"), BASE_EPOCH + i * 10);
        }
        assert_eq!(builder.check_watch_unlocks(), vec![Species::Beaver]);

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

    #[test]
    fn default_name_auto_switches_and_preserves_custom_names() {
        let mut pet = Pet::default();
        for sp in Species::ALL {
            pet.unlock_species(sp);
        }
        assert_eq!(pet.name, "Pebble");
        for (sp, expected) in [
            (Species::Bee, "Anabee"),
            (Species::Bear, "Vector"),
            (Species::Snake, "Hiss"),
            (Species::Beaver, "Bóbr"),
            (Species::Dolphin, "Flow"),
            (Species::Penguin, "Pebble"),
        ] {
            pet.set_species(sp).unwrap();
            assert_eq!(pet.name, expected, "expected canonical default name for {sp:?}");
        }

        // Custom nicknames stay intact across species switches.
        pet.rename("Whiskers").unwrap();
        for sp in [Species::Bee, Species::Beaver, Species::Snake, Species::Dolphin, Species::Bear] {
            pet.set_species(sp).unwrap();
            assert_eq!(pet.name, "Whiskers");
        }
    }

    #[test]
    fn pebble_catalog_and_serde_backward_compatibility() {
        assert!(PEBBLE_CATALOG.len() >= 10);
        assert_eq!(PEBBLE_CATALOG.len(), PEBBLE_TRIGGERS.len());
        let mut ids = std::collections::HashSet::new();
        for (def, (tid, _)) in PEBBLE_CATALOG.iter().zip(PEBBLE_TRIGGERS.iter()) {
            assert_eq!(def.id, *tid);
            assert!(!def.id.is_empty() && !def.name.is_empty() && !def.flavor.is_empty());
            assert!(!def.milestone_hint.is_empty());
            assert!(ids.insert(def.id), "duplicate catalog id {}", def.id);
            assert_eq!(pebble_def(def.id), Some(def));
        }
        assert_eq!(pebble_def("missing"), None);

        // Legacy pet.toml without pebbles or lab_milestones loads cleanly.
        let legacy_toml = r#"
name = "Tuxi"
species = "penguin"
unlocked_species = ["penguin", "cat"]
fullness = 85
happiness = 90
commands_fed = 12
cmd_streak = 2
best_cmd_streak = 6
day_streak = 2
best_day_streak = 4
last_active_day = "2026-11-18"
last_fed_epoch = 1795046400
last_program = "ls"
unique_programs = ["ls", "git"]
warnings_heeded = 1
"#;
        let mut legacy: Pet = toml::from_str(legacy_toml).unwrap();
        legacy.normalize();
        assert_eq!(legacy.name, "Tuxi");
        assert_eq!(legacy.commands_fed, 12);
        assert_eq!(legacy.lab_milestones, 0);
        assert!(legacy.pebbles.is_empty());

        // Partial [[pebbles]] without gifted_day + duplicates normalize cleanly.
        let dup_toml = r#"
name = "Pebble"
[[pebbles]]
id = "beach_quartz"
[[pebbles]]
id = "  beach_quartz  "
gifted_day = "2026-11-19"
[[pebbles]]
id = "   "
"#;
        let mut duped: Pet = toml::from_str(dup_toml).unwrap();
        duped.normalize();
        assert_eq!(duped.pebbles, vec![GiftedPebble { id: "beach_quartz".into(), gifted_day: String::new() }]);

        // Round-trip save_to / load_from with pebbles and lab_milestones.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/pet-tests")
            .join(format!("pebble-rt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("pet.toml");
        let mut pet = Pet::default();
        let today = date::from_epoch(BASE_EPOCH);
        assert_eq!(pet.record_lab_milestone(today).len(), 1);
        save_to(&path, &pet).unwrap();
        assert_eq!(load_from(&path), pet);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pebble_milestones_trigger_idempotently_and_celebrate() {
        let mut pet = Pet::default();
        let today = date::from_epoch(BASE_EPOCH);

        // Clean commands 1..4 do not trigger pebbles; command 5 gifts Smooth Beach Quartz and celebrates.
        for i in 1..5 {
            let (mood, gifted) = pet.feed_clean(Some("ls"), BASE_EPOCH + i * 10);
            assert!(gifted.is_empty(), "unexpected gift at cmd {i}");
            assert_ne!(mood, Mood::Celebrating);
        }
        let (mood5, gifted5) = pet.feed_clean(Some("ls"), BASE_EPOCH + 50);
        assert_eq!(mood5, Mood::Celebrating);
        assert_eq!(gifted5.iter().map(|p| p.id).collect::<Vec<_>>(), vec!["beach_quartz"]);
        assert_eq!(pet.pebbles[0].gifted_day, "2026-11-19");

        // Re-running check_pebble_gifts is strictly idempotent.
        assert!(pet.check_pebble_gifts(today).is_empty());
        assert_eq!(pet.pebbles.len(), 1);

        // Consecutive days trigger 3-day streak pebble.
        pet.feed_clean(Some("pwd"), BASE_EPOCH + 86_400);
        assert_eq!(pet.day_streak, 2);
        let (mood_d3, gifted_d3) = pet.feed_clean(Some("id"), BASE_EPOCH + 2 * 86_400);
        assert_eq!(mood_d3, Mood::Celebrating);
        assert!(gifted_d3.iter().any(|p| p.id == "three_tide_jasper"));

        // Unique programs trigger Curious Sea-Glass at 15 unique programs.
        for i in 0..12 {
            let _ = pet.feed_clean(Some(&format!("tool{i}")), BASE_EPOCH + 2 * 86_400 + (i + 1) * 10);
        }
        assert!(pet.has_pebble("curious_seaglass"));

        // Warnings heeded trigger Guardian Slate at 3 with Mood::Celebrating, then Alert on 4th.
        let mut warner = Pet::default();
        let (m1, u1, g1) = warner.note_warning(today);
        assert_eq!((m1, u1.is_empty(), g1.is_empty()), (Mood::Alert, true, true));
        let (m2, _, g2) = warner.note_warning(today);
        assert_eq!((m2, g2.is_empty()), (Mood::Alert, true));
        let (m3, u3, g3) = warner.note_warning(today);
        assert_eq!(m3, Mood::Celebrating);
        assert!(u3.is_empty());
        assert_eq!(g3.iter().map(|p| p.id).collect::<Vec<_>>(), vec!["guardian_slate"]);
        let (m4, u4, g4) = warner.note_warning(today);
        assert_eq!((m4, u4.is_empty(), g4.is_empty()), (Mood::Alert, true, true));
        let (m5, u5, g5) = warner.note_warning(today);
        assert_eq!(m5, Mood::Celebrating);
        assert_eq!(u5, vec![Species::Owl]);
        assert!(g5.is_empty());

        // Lab milestones: 1 -> scholars_pearl, 5 -> crystal_geode; sync_lab_milestones is monotonic.
        let mut learner = Pet::default();
        assert_eq!(
            learner.record_lab_milestone(today).iter().map(|p| p.id).collect::<Vec<_>>(),
            vec!["scholars_pearl"]
        );
        assert!(learner.sync_lab_milestones(1, today).is_empty());
        assert!(learner.sync_lab_milestones(0, today).is_empty());
        assert_eq!(learner.lab_milestones, 1);
        for _ in 0..3 {
            assert!(learner.record_lab_milestone(today).is_empty());
        }
        assert_eq!(learner.record_lab_milestone(today).iter().map(|p| p.id).collect::<Vec<_>>(), vec!["crystal_geode"]);

        // Merging pebbles from another snapshot preserves existing and adds missing without duplicates.
        let mut base = Pet::default();
        base.record_lab_milestone(today);
        base.merge_pebbles_from(&warner);
        assert!(base.has_pebble("scholars_pearl"));
        assert!(base.has_pebble("guardian_slate"));
        base.merge_pebbles_from(&warner);
        assert_eq!(base.pebbles.len(), 2);
    }

    #[test]
    fn pebble_rendering_and_toast_formatting() {
        let mut pet = Pet::default();
        let empty = pet.render_pebbles();
        assert!(!empty.contains('\x1b'));
        assert!(empty.contains("0/"), "{empty}");
        assert!(empty.contains("No shiny pebbles gifted yet"), "{empty}");
        assert!(empty.contains("Next pebble: Smooth Beach Quartz"), "{empty}");
        assert!(empty.contains("(0/5)"), "{empty}");

        let today = date::from_epoch(BASE_EPOCH);
        for i in 0..5 {
            pet.feed_clean(Some(&format!("cmd{i}")), BASE_EPOCH + i * 10);
        }
        let partial = pet.render_pebbles();
        assert!(!partial.contains('\x1b'));
        assert!(partial.contains("1/"), "{partial}");
        assert!(partial.contains("Smooth Beach Quartz [2026-11-19]"), "{partial}");
        assert!(partial.contains("classic penguin hello"), "{partial}");
        assert!(partial.contains("Next pebble:"), "{partial}");

        let status = render_status(&pet, BASE_EPOCH + 50, None, false);
        assert!(!status.contains('\x1b'));
        assert!(status.contains("Pebbles: 1/"), "{status}");
        assert!(status.contains("huh pet --pebbles"), "{status}");

        let toast = format_pebble_gift(&pet.name, pebble_def("beach_quartz").unwrap());
        assert!(!toast.contains('\x1b'));
        assert!(!toast.contains('\n'));
        assert!(toast.contains("Pebble gifted a shiny pebble: Smooth Beach Quartz"), "{toast}");
        assert!(toast.contains("classic penguin hello"), "{toast}");

        // Unlock all pebbles -> collection complete card.
        let mut full = Pet {
            commands_fed: 300,
            day_streak: 30,
            best_day_streak: 30,
            warnings_heeded: 3,
            lab_milestones: 5,
            unique_programs: (0..40).map(|i| format!("p{i}")).collect(),
            ..Pet::default()
        };
        let gifted_all = full.check_pebble_gifts(today);
        assert_eq!(gifted_all.len(), PEBBLE_CATALOG.len());
        assert_eq!(full.pebbles_collected(), PEBBLE_CATALOG.len());
        let complete = full.render_pebbles();
        assert!(complete.contains("All shiny pebbles in the collection have been gifted!"), "{complete}");
        let complete_status = full.render_status(BASE_EPOCH, None, false);
        assert!(complete_status.contains("collection complete!"), "{complete_status}");
    }

    #[test]
    fn secret_pebbles_unlock_helpers_and_rendering() {
        let today = date::from_epoch(BASE_EPOCH);
        let mut pet = Pet::default();

        // Uncollected secret pebbles never leak in normal pebble card or status.
        let initial_card = pet.render_pebbles();
        assert!(!initial_card.contains("Secret pebbles discovered"), "{initial_card}");
        assert!(!initial_card.contains("8-Bit Star"), "{initial_card}");
        assert!(!pet.render_status(BASE_EPOCH, None, false).contains("secret"), "unexpected secret note");

        // Unknown ID or normal ID is rejected by grant_secret_pebble.
        assert!(pet.grant_secret_pebble("does_not_exist", today).is_none());
        assert!(pet.grant_secret_pebble("beach_quartz", today).is_none());

        // Granting a valid secret pebble is idempotent and preserves X/14 normal count.
        let granted = pet.grant_secret_pebble("secret_konami", today).unwrap();
        assert_eq!(granted.name, "8-Bit Star");
        assert!(pet.grant_secret_pebble("secret_konami", today).is_none());
        assert_eq!(pet.pebbles_collected(), 0);
        assert_eq!(pet.secret_pebbles_collected(), 1);

        let card = pet.render_pebbles();
        assert!(card.contains("0/14 collected"), "{card}");
        assert!(card.contains("Secret pebbles discovered:"), "{card}");
        assert!(card.contains("8-Bit Star [2026-11-19]"), "{card}");
        assert!(!card.contains("Vault Moonstone"), "{card}");

        let status = pet.render_status(BASE_EPOCH, None, false);
        assert!(status.contains("Pebbles: 0/14 collected (+1 secret)"), "{status}");

        // unlock_species, unlock_all_species, and pamper work cleanly.
        assert!(pet.unlock_species(Species::Cat));
        assert!(!pet.unlock_species(Species::Cat));
        let newly_unlocked = pet.unlock_all_species();
        assert_eq!(newly_unlocked.len(), Species::ALL.len() - 2);
        assert!(pet.unlock_all_species().is_empty());

        pet.fullness = 20;
        pet.happiness = 30;
        pet.pamper(BASE_EPOCH + 100);
        assert_eq!(pet.fullness, 100);
        assert_eq!(pet.happiness, 100);
        assert_eq!(pet.last_fed_epoch, BASE_EPOCH + 100);
    }
}
