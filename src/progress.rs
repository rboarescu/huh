//! Local progress, XP, daily streaks with freeze protection, badges, unit/lesson unlocks,
//! and atomic TOML persistence under `~/.local/state/huh/progress.toml`.

use crate::date::{self, Date};
use crate::srs;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const DEFAULT_DAILY_GOAL: u32 = 50;
pub const MAX_HEARTS: u8 = 5;
pub const MAX_FREEZES: u8 = 3;
pub const XP_CORRECT: u32 = 10;
pub const XP_COMBO_BONUS: u32 = 5;
pub const XP_LESSON_CLEAR: u32 = 25;
pub const XP_FLAWLESS_BONUS: u32 = 15;
pub const XP_REVIEW_CORRECT: u32 = 10;
pub const XP_PRACTICE_CLEAR: u32 = 30;

const MIN_DAILY_GOAL: u32 = 10;
const MAX_DAILY_GOAL: u32 = 1000;
const DANGER_SPOTTER_THRESHOLD: u32 = 5;

/// Static metadata describing an earnable badge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadgeDef {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
}

/// All badges currently defined by `/mode lab`.
pub const BADGES: &[BadgeDef] = &[
    BadgeDef { id: "first_step", title: "First Step", description: "Complete your first lesson in /mode lab." },
    BadgeDef { id: "flawless", title: "Flawless Run", description: "Finish a lesson without losing a single heart." },
    BadgeDef {
        id: "danger_spotter",
        title: "Danger Spotter",
        description: "Spot 5 risky commands in Spot the Danger questions.",
    },
    BadgeDef { id: "hands_on", title: "Hands-On Explorer", description: "Complete your first hands-on practice lab." },
    BadgeDef { id: "streak_3", title: "Three-Day Spark", description: "Keep a 3-day learning streak going." },
    BadgeDef { id: "streak_7", title: "Week Warrior", description: "Keep a 7-day learning streak going." },
    BadgeDef { id: "streak_30", title: "Monthly Master", description: "Keep a 30-day learning streak going." },
    BadgeDef {
        id: "pathfinder",
        title: "Pathfinder",
        description: "Finish Unit 1 (Finding Your Way) — unlocks Terminal Cat.",
    },
    BadgeDef {
        id: "file_keeper",
        title: "File Keeper",
        description: "Finish Unit 2 (Files & Folders) — unlocks Builder Beaver.",
    },
    BadgeDef {
        id: "pipe_plumber",
        title: "Pipe Plumber",
        description: "Finish Unit 3 (Searching & Pipes) — unlocks Shell-Turtle.",
    },
    BadgeDef { id: "locksmith", title: "Locksmith", description: "Finish Unit 4 (Reading Permissions)." },
    BadgeDef {
        id: "process_tamer",
        title: "Process Tamer",
        description: "Finish Unit 5 (Processes & Disk) — unlocks Woodland Bear.",
    },
    BadgeDef { id: "package_pro", title: "Package Pro", description: "Finish Unit 6 (Packages & Manuals)." },
    BadgeDef {
        id: "archive_ace",
        title: "Archive Ace",
        description: "Finish Unit 7 (Archives & Links) — unlocks Terminal Serpent.",
    },
    BadgeDef {
        id: "night_watch",
        title: "Night Watch",
        description: "Finish Unit 8 (Networking & Stay Safe) — unlocks Snowy Owl.",
    },
    BadgeDef { id: "shell_grad", title: "Shell Graduate", description: "Complete all 8 core units in /mode lab." },
    BadgeDef { id: "game_runner", title: "Game Runner", description: "Finish Unit 9 (Apps & Games on Linux)." },
    BadgeDef {
        id: "media_wrangler",
        title: "Media Wrangler",
        description: "Finish Unit 10 (Media & Everyday Files) — unlocks Wave Dolphin.",
    },
    BadgeDef {
        id: "pixel_painter",
        title: "Pixel Painter",
        description: "Finish Unit 11 (Paint Your Own Pet) — unlocks Queen Bee.",
    },
];

/// Looks up a badge definition by its stable ID.
pub fn badge_def(id: &str) -> Option<&'static BadgeDef> {
    BADGES.iter().find(|badge| badge.id == id)
}

/// A badge earned by the learner, recorded with the UTC day it was unlocked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EarnedBadge {
    pub id: String,
    #[serde(default)]
    pub earned_day: String,
}

/// Outcome of updating the daily UTC streak when XP is awarded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreakEvent {
    SameDay,
    Advanced { streak: u32, freeze_earned: bool },
    FreezeUsed { streak: u32, freezes_left: u8 },
    Started { streak: u32 },
}

fn default_freezes() -> u8 {
    1
}

/// Persisted learner progress stored in `~/.local/state/huh/progress.toml`.
///
/// All scalar and inline-array fields appear before `badges` and `srs` so TOML serialization
/// emits scalar keys before `[[badges]]` and `[[srs]]` array-of-table sections.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Progress {
    pub xp: u64,
    pub daily_xp: u32,
    pub daily_xp_day: Option<String>,
    pub streak: u32,
    pub best_streak: u32,
    #[serde(default = "default_freezes")]
    pub freezes: u8,
    pub last_streak_day: Option<String>,
    /// UTC day (`YYYY-MM-DD`) covered by the most recently consumed streak freeze (the missed day).
    pub last_freeze_used_day: Option<String>,
    pub danger_spotted: u32,
    pub completed_lessons: Vec<String>,
    pub completed_practices: Vec<String>,
    pub badges: Vec<EarnedBadge>,
    pub srs: Vec<srs::Card>,
}

impl Default for Progress {
    fn default() -> Progress {
        Progress {
            xp: 0,
            daily_xp: 0,
            daily_xp_day: None,
            streak: 0,
            best_streak: 0,
            freezes: default_freezes(),
            last_streak_day: None,
            last_freeze_used_day: None,
            danger_spotted: 0,
            completed_lessons: Vec::new(),
            completed_practices: Vec::new(),
            badges: Vec::new(),
            srs: Vec::new(),
        }
    }
}

impl Progress {
    /// Repairs out-of-range counters and removes duplicate lessons, practices, badges, or SRS cards
    /// that may have been loaded from a hand-edited file.
    pub fn normalize(&mut self) {
        self.freezes = self.freezes.min(MAX_FREEZES);
        self.best_streak = self.best_streak.max(self.streak);

        let mut seen_lessons = HashSet::with_capacity(self.completed_lessons.len());
        self.completed_lessons.retain(|id| !id.is_empty() && seen_lessons.insert(id.clone()));

        let mut seen_practices = HashSet::with_capacity(self.completed_practices.len());
        self.completed_practices.retain(|id| !id.is_empty() && seen_practices.insert(id.clone()));

        let mut seen_badges = HashSet::with_capacity(self.badges.len());
        self.badges.retain(|badge| !badge.id.is_empty() && seen_badges.insert(badge.id.clone()));

        let mut seen_cards = HashSet::with_capacity(self.srs.len());
        self.srs.retain(|card| !card.question_id.is_empty() && seen_cards.insert(card.question_id.clone()));
        for card in &mut self.srs {
            card.box_level = card.box_level.min(srs::MAX_BOX);
        }
    }

    /// XP earned specifically on UTC date `today`.
    pub fn daily_xp_on(&self, today: Date) -> u32 {
        match self.daily_xp_day.as_deref().and_then(date::parse) {
            Some(day) if day == today => self.daily_xp,
            _ => 0,
        }
    }

    /// Awards `amount` XP on UTC date `today` and updates the daily streak.
    ///
    /// - A 1-day gap (`delta == 2`) automatically consumes a streak freeze when `freezes > 0`
    ///   (recording the missed day in `last_freeze_used_day`) and advances the streak.
    /// - Reaching any multiple-of-7 streak day on consecutive days earns +1 freeze up to `MAX_FREEZES`.
    /// - Backwards clock skew (`delta < 0`) leaves the streak and `last_streak_day` intact.
    pub fn award_xp(&mut self, amount: u32, today: Date) -> StreakEvent {
        if amount == 0 {
            return StreakEvent::SameDay;
        }

        self.freezes = self.freezes.min(MAX_FREEZES);
        self.xp = self.xp.saturating_add(u64::from(amount));

        let today_str = date::format(today);
        if self.daily_xp_day.as_deref().and_then(date::parse) == Some(today) {
            self.daily_xp = self.daily_xp.saturating_add(amount);
        } else {
            self.daily_xp = amount;
            self.daily_xp_day = Some(today_str.clone());
        }

        let event = match self.last_streak_day.as_deref().and_then(date::parse) {
            Some(prev) => {
                let delta = date::days_between(prev, today);
                if delta < 0 {
                    StreakEvent::SameDay
                } else if self.streak == 0 {
                    self.streak = 1;
                    self.last_streak_day = Some(today_str);
                    StreakEvent::Started { streak: 1 }
                } else if delta == 0 {
                    StreakEvent::SameDay
                } else if delta == 1 {
                    self.streak = self.streak.saturating_add(1);
                    self.last_streak_day = Some(today_str);
                    let freeze_earned =
                        if self.streak > 0 && self.streak.is_multiple_of(7) && self.freezes < MAX_FREEZES {
                            self.freezes = self.freezes.saturating_add(1);
                            true
                        } else {
                            false
                        };
                    StreakEvent::Advanced { streak: self.streak, freeze_earned }
                } else if delta == 2 && self.freezes > 0 {
                    self.freezes = self.freezes.saturating_sub(1);
                    self.streak = self.streak.saturating_add(1);
                    self.last_streak_day = Some(today_str);
                    self.last_freeze_used_day = Some(date::format(date::add_days(today, -1)));
                    StreakEvent::FreezeUsed { streak: self.streak, freezes_left: self.freezes }
                } else {
                    self.streak = 1;
                    self.last_streak_day = Some(today_str);
                    StreakEvent::Started { streak: 1 }
                }
            }
            None => {
                self.streak = 1;
                self.last_streak_day = Some(today_str);
                StreakEvent::Started { streak: 1 }
            }
        };

        self.best_streak = self.best_streak.max(self.streak);
        event
    }

    /// Grants badge `id` if not already earned. Returns `true` when newly earned.
    pub fn award_badge(&mut self, id: &str, today: Date) -> bool {
        if id.is_empty() || self.has_badge(id) {
            return false;
        }
        self.badges.push(EarnedBadge { id: id.to_string(), earned_day: date::format(today) });
        true
    }

    /// Returns `true` if badge `id` has been earned.
    pub fn has_badge(&self, id: &str) -> bool {
        self.badges.iter().any(|badge| badge.id == id)
    }

    fn award_streak_badges(&mut self, today: Date, earned: &mut Vec<String>) {
        for (threshold, badge_id) in [(3, "streak_3"), (7, "streak_7"), (30, "streak_30")] {
            if self.streak >= threshold && self.award_badge(badge_id, today) {
                earned.push(badge_id.to_string());
            }
        }
    }

    /// Marks `lesson_id` completed and checks milestone badges (`first_step`, `flawless`,
    /// `streak_3`, `streak_7`, `streak_30`). Returns newly earned badge IDs.
    pub fn complete_lesson(&mut self, lesson_id: &str, flawless: bool, today: Date) -> Vec<String> {
        if !lesson_id.is_empty() && !self.is_lesson_completed(lesson_id) {
            self.completed_lessons.push(lesson_id.to_string());
        }

        let mut earned = Vec::new();
        if !self.completed_lessons.is_empty() && self.award_badge("first_step", today) {
            earned.push("first_step".to_string());
        }
        if flawless && self.award_badge("flawless", today) {
            earned.push("flawless".to_string());
        }
        self.award_streak_badges(today, &mut earned);
        earned
    }

    /// Returns `true` if practice lab `practice_id` has been completed.
    pub fn is_practice_completed(&self, practice_id: &str) -> bool {
        self.completed_practices.iter().any(|id| id == practice_id)
    }

    /// Marks `practice_id` completed and checks milestone badges (`hands_on`,
    /// `streak_3`, `streak_7`, `streak_30`). Returns newly earned badge IDs.
    pub fn complete_practice(&mut self, practice_id: &str, today: Date) -> Vec<String> {
        if !practice_id.is_empty() && !self.is_practice_completed(practice_id) {
            self.completed_practices.push(practice_id.to_string());
        }

        let mut earned = Vec::new();
        if !self.completed_practices.is_empty() && self.award_badge("hands_on", today) {
            earned.push("hands_on".to_string());
        }
        self.award_streak_badges(today, &mut earned);
        earned
    }

    /// Records a correctly answered `spot_danger` question, returning `Some("danger_spotter")`
    /// the first time `danger_spotted` reaches 5.
    pub fn record_danger_spotted(&mut self, today: Date) -> Option<String> {
        self.danger_spotted = self.danger_spotted.saturating_add(1);
        if self.danger_spotted >= DANGER_SPOTTER_THRESHOLD && self.award_badge("danger_spotter", today) {
            Some("danger_spotter".to_string())
        } else {
            None
        }
    }

    pub fn is_lesson_completed(&self, lesson_id: &str) -> bool {
        self.completed_lessons.iter().any(|id| id == lesson_id)
    }

    pub fn is_unit_completed(&self, unit_lesson_ids: &[&str]) -> bool {
        !unit_lesson_ids.is_empty() && unit_lesson_ids.iter().all(|id| self.is_lesson_completed(id))
    }

    pub fn is_unit_unlocked(&self, unit_index: usize, prev_unit_lesson_ids: Option<&[&str]>) -> bool {
        if unit_index == 0 {
            return true;
        }
        prev_unit_lesson_ids.is_some_and(|ids| self.is_unit_completed(ids))
    }

    pub fn is_lesson_unlocked(&self, unit_unlocked: bool, lesson_index: usize, prev_lesson_id: Option<&str>) -> bool {
        if !unit_unlocked {
            return false;
        }
        if lesson_index == 0 {
            return true;
        }
        prev_lesson_id.is_some_and(|id| self.is_lesson_completed(id))
    }
}

// ---------------------------------------------------------------------------
// Config & persistence
// ---------------------------------------------------------------------------

/// Reads integer `daily_goal` from `config.toml` text, clamped to `10..=1000`.
pub fn daily_goal_from_config(text: &str) -> Option<u32> {
    let table = toml::from_str::<toml::Table>(text).ok()?;
    match table.get("daily_goal") {
        Some(toml::Value::Integer(goal)) => {
            Some((*goal).clamp(i64::from(MIN_DAILY_GOAL), i64::from(MAX_DAILY_GOAL)) as u32)
        }
        _ => None,
    }
}

/// Daily XP goal from `~/.config/huh/config.toml`, falling back to `DEFAULT_DAILY_GOAL`.
pub fn daily_goal() -> u32 {
    crate::mode::config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|text| daily_goal_from_config(&text))
        .unwrap_or(DEFAULT_DAILY_GOAL)
}

/// `$XDG_STATE_HOME/huh/progress.toml`, or `~/.local/state/huh/progress.toml`.
pub fn state_path(xdg_state: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    match xdg_state.filter(|v| !v.is_empty()) {
        Some(dir) => Some(Path::new(dir).join("huh/progress.toml")),
        None => Some(Path::new(home.filter(|v| !v.is_empty())?).join(".local/state/huh/progress.toml")),
    }
}

pub fn default_state_path() -> Option<PathBuf> {
    state_path(std::env::var("XDG_STATE_HOME").ok().as_deref(), std::env::var("HOME").ok().as_deref())
}

pub fn load_from(path: &Path) -> Progress {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Progress::default();
    };
    match toml::from_str::<Progress>(&text) {
        Ok(mut progress) => {
            progress.normalize();
            progress
        }
        Err(_) => Progress::default(),
    }
}

pub fn save_to(path: &Path, progress: &Progress) -> Result<()> {
    let dir = path.parent().context("progress state path has no parent directory")?;
    if !dir.as_os_str().is_empty() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let body = toml::to_string_pretty(progress).context("cannot serialize progress state")?;
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("progress.toml");
    let tmp = dir.join(format!("{file_name}.tmp.{}", std::process::id()));
    if let Err(err) = std::fs::write(&tmp, body) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err).with_context(|| format!("cannot write {}", tmp.display()));
    }
    if let Err(err) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err).with_context(|| format!("cannot replace {}", path.display()));
    }
    Ok(())
}

#[allow(dead_code)]
pub fn load_default() -> Progress {
    default_state_path().map_or_else(Progress::default, |p| load_from(&p))
}

#[allow(dead_code)]
pub fn save_default(progress: &Progress) -> Result<()> {
    let path = default_state_path().context("cannot find state directory ($XDG_STATE_HOME or $HOME)")?;
    save_to(&path, progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> Date {
        date::parse(s).unwrap()
    }

    #[test]
    fn defaults_and_badge_catalog() {
        let p = Progress::default();
        assert_eq!(p.xp, 0);
        assert_eq!(p.daily_xp, 0);
        assert_eq!(p.streak, 0);
        assert_eq!(p.best_streak, 0);
        assert_eq!(p.freezes, 1);
        assert_eq!(XP_PRACTICE_CLEAR, 30);
        assert!(p.completed_lessons.is_empty());
        assert!(p.completed_practices.is_empty());
        assert!(p.badges.is_empty());
        assert!(p.srs.is_empty());

        let mut seen_ids = HashSet::new();
        for def in BADGES {
            assert!(!def.id.is_empty());
            assert!(!def.title.is_empty());
            assert!(!def.description.is_empty());
            assert!(seen_ids.insert(def.id), "duplicate badge id {}", def.id);
            assert_eq!(badge_def(def.id), Some(def));
        }
        for required in [
            "first_step",
            "flawless",
            "danger_spotter",
            "hands_on",
            "streak_3",
            "streak_7",
            "streak_30",
            "pathfinder",
            "file_keeper",
            "pipe_plumber",
            "locksmith",
            "process_tamer",
            "package_pro",
            "archive_ace",
            "night_watch",
            "shell_grad",
            "game_runner",
            "media_wrangler",
            "pixel_painter",
        ] {
            assert!(badge_def(required).is_some(), "missing required badge {required}");
        }
        assert_eq!(badge_def("shell_grad").unwrap().description, "Complete all 8 core units in /mode lab.");
        assert_eq!(badge_def("nonexistent"), None);
    }

    #[test]
    fn xp_streaks_freezes_and_clock_skew() {
        let mut p = Progress::default();
        let d1 = day("2026-11-01");

        // Zero XP does not start a streak.
        assert_eq!(p.award_xp(0, d1), StreakEvent::SameDay);
        assert_eq!(p.streak, 0);

        // Day 1 starts streak at 1.
        assert_eq!(p.award_xp(20, d1), StreakEvent::Started { streak: 1 });
        assert_eq!(p.xp, 20);
        assert_eq!(p.daily_xp_on(d1), 20);
        assert_eq!(p.daily_xp_on(date::add_days(d1, 1)), 0);
        assert_eq!(p.streak, 1);
        assert_eq!(p.best_streak, 1);

        // Same day accumulates XP without advancing streak.
        assert_eq!(p.award_xp(15, d1), StreakEvent::SameDay);
        assert_eq!(p.xp, 35);
        assert_eq!(p.daily_xp_on(d1), 35);
        assert_eq!(p.streak, 1);

        // Days 2..=6 advance streak without earning a freeze.
        for offset in 1..6 {
            let d = date::add_days(d1, offset);
            assert_eq!(p.award_xp(10, d), StreakEvent::Advanced { streak: (offset + 1) as u32, freeze_earned: false });
            assert_eq!(p.daily_xp_on(d), 10);
        }
        assert_eq!(p.streak, 6);
        assert_eq!(p.freezes, 1);

        // Day 7 earns +1 freeze (1 -> 2).
        let d7 = date::add_days(d1, 6);
        assert_eq!(p.award_xp(10, d7), StreakEvent::Advanced { streak: 7, freeze_earned: true });
        assert_eq!(p.freezes, 2);

        // Days 8..=14 earn a third freeze up to MAX_FREEZES (3).
        for offset in 7..13 {
            let d = date::add_days(d1, offset);
            assert_eq!(p.award_xp(10, d), StreakEvent::Advanced { streak: (offset + 1) as u32, freeze_earned: false });
        }
        let d14 = date::add_days(d1, 13);
        assert_eq!(p.award_xp(10, d14), StreakEvent::Advanced { streak: 14, freeze_earned: true });
        assert_eq!(p.freezes, MAX_FREEZES);

        // Days 15..=21 do not exceed MAX_FREEZES.
        for offset in 14..21 {
            let d = date::add_days(d1, offset);
            assert_eq!(p.award_xp(10, d), StreakEvent::Advanced { streak: (offset + 1) as u32, freeze_earned: false });
        }
        assert_eq!(p.streak, 21);
        assert_eq!(p.freezes, MAX_FREEZES);

        // Skipping 1 day (delta == 2) consumes one freeze and advances the streak.
        let d23 = date::add_days(d1, 22);
        assert_eq!(p.award_xp(10, d23), StreakEvent::FreezeUsed { streak: 22, freezes_left: 2 });
        assert_eq!(p.freezes, 2);
        assert_eq!(p.best_streak, 22);
        assert_eq!(p.last_freeze_used_day, Some(date::format(date::add_days(d23, -1))));

        // Backwards clock skew leaves streak and last_streak_day intact.
        let saved_day = p.last_streak_day.clone();
        assert_eq!(p.award_xp(10, d7), StreakEvent::SameDay);
        assert_eq!(p.streak, 22);
        assert_eq!(p.last_streak_day, saved_day);

        // Skipping 2 full days (delta == 3) resets streak to 1 without spending a freeze.
        let d26 = date::add_days(d23, 3);
        assert_eq!(p.award_xp(10, d26), StreakEvent::Started { streak: 1 });
        assert_eq!(p.streak, 1);
        assert_eq!(p.best_streak, 22);
        assert_eq!(p.freezes, 2);

        // Skipping 1 day with 0 freezes resets streak to 1.
        p.freezes = 0;
        let d28 = date::add_days(d26, 2);
        assert_eq!(p.award_xp(10, d28), StreakEvent::Started { streak: 1 });
        assert_eq!(p.streak, 1);
        assert_eq!(p.freezes, 0);
    }

    #[test]
    fn counters_saturate_on_max_values() {
        let d1 = day("2026-11-19");
        let d2 = day("2026-11-20");
        let mut p = Progress {
            xp: u64::MAX,
            daily_xp: u32::MAX,
            daily_xp_day: Some("2026-11-19".into()),
            streak: u32::MAX,
            best_streak: u32::MAX,
            last_streak_day: Some("2026-11-19".into()),
            danger_spotted: u32::MAX,
            ..Progress::default()
        };

        assert_eq!(p.award_xp(50, d1), StreakEvent::SameDay);
        assert_eq!((p.xp, p.daily_xp), (u64::MAX, u32::MAX));

        assert_eq!(p.award_xp(50, d2), StreakEvent::Advanced { streak: u32::MAX, freeze_earned: false });
        assert_eq!((p.xp, p.streak, p.best_streak), (u64::MAX, u32::MAX, u32::MAX));

        assert_eq!(p.record_danger_spotted(d2), Some("danger_spotter".to_string()));
        assert_eq!(p.danger_spotted, u32::MAX);
        assert_eq!(p.record_danger_spotted(d2), None);
    }

    #[test]
    fn lesson_completion_badges_and_danger_spotter() {
        let mut p = Progress::default();
        let d1 = day("2026-11-19");

        let earned = p.complete_lesson("u1-where-am-i", false, d1);
        assert_eq!(earned, vec!["first_step"]);
        assert!(p.is_lesson_completed("u1-where-am-i"));
        assert!(p.has_badge("first_step"));
        assert!(!p.has_badge("flawless"));

        // Replaying flawlessly awards `flawless` without duplicating lesson or `first_step`.
        let earned = p.complete_lesson("u1-where-am-i", true, d1);
        assert_eq!(earned, vec!["flawless"]);
        assert_eq!(p.completed_lessons.len(), 1);

        // Completing on a 30-day streak awards all three streak badges once.
        p.streak = 30;
        let earned = p.complete_lesson("u1-hidden-and-details", true, d1);
        assert_eq!(earned, vec!["streak_3", "streak_7", "streak_30"]);
        assert!(p.complete_lesson("u1-hidden-and-details", true, d1).is_empty());

        // Danger spotter unlocks at 5.
        for _ in 0..4 {
            assert_eq!(p.record_danger_spotted(d1), None);
        }
        assert_eq!(p.record_danger_spotted(d1), Some("danger_spotter".to_string()));
        assert_eq!(p.danger_spotted, 5);
        assert_eq!(p.record_danger_spotted(d1), None);
        assert_eq!(p.danger_spotted, 6);
    }

    #[test]
    fn practice_completion_and_hands_on_badge() {
        let mut p = Progress::default();
        let d1 = day("2026-11-19");

        // Empty practice ID does not count as a completion or award `hands_on`.
        assert!(p.complete_practice("", d1).is_empty());
        assert!(p.completed_practices.is_empty());
        assert!(!p.has_badge("hands_on"));

        let earned = p.complete_practice("u1-hidden-clue", d1);
        assert_eq!(earned, vec!["hands_on"]);
        assert!(p.is_practice_completed("u1-hidden-clue"));
        assert!(!p.is_practice_completed("u2-tidy-desk"));
        assert!(p.has_badge("hands_on"));

        // Replaying the same practice neither duplicates the ID nor re-awards `hands_on`.
        assert!(p.complete_practice("u1-hidden-clue", d1).is_empty());
        assert_eq!(p.completed_practices, vec!["u1-hidden-clue"]);

        // Completing a practice on a streak also grants any newly reached streak badges.
        p.streak = 7;
        let earned = p.complete_practice("u2-tidy-desk", d1);
        assert_eq!(earned, vec!["streak_3", "streak_7"]);
        assert_eq!(p.completed_practices, vec!["u1-hidden-clue", "u2-tidy-desk"]);

        p.streak = 30;
        let earned = p.complete_practice("u3-filter-logs", d1);
        assert_eq!(earned, vec!["streak_30"]);
        assert!(p.complete_practice("u3-filter-logs", d1).is_empty());
    }

    #[test]
    fn unit_and_lesson_unlock_progression() {
        let mut p = Progress::default();
        let d1 = day("2026-11-19");
        let unit1 = ["u1-1", "u1-2", "u1-3"];

        assert!(!p.is_unit_completed(&[]));
        assert!(!p.is_unit_completed(&unit1));

        assert!(p.is_unit_unlocked(0, None));
        assert!(!p.is_unit_unlocked(1, Some(&unit1)));
        assert!(!p.is_unit_unlocked(1, None));

        assert!(p.is_lesson_unlocked(true, 0, None));
        assert!(!p.is_lesson_unlocked(false, 0, None));
        assert!(!p.is_lesson_unlocked(true, 1, Some("u1-1")));
        assert!(!p.is_lesson_unlocked(true, 1, None));

        p.complete_lesson("u1-1", false, d1);
        assert!(p.is_lesson_unlocked(true, 1, Some("u1-1")));
        assert!(!p.is_lesson_unlocked(true, 2, Some("u1-2")));
        assert!(!p.is_unit_unlocked(1, Some(&unit1)));

        p.complete_lesson("u1-2", false, d1);
        p.complete_lesson("u1-3", false, d1);
        assert!(p.is_unit_completed(&unit1));
        assert!(p.is_unit_unlocked(1, Some(&unit1)));
    }

    #[test]
    fn normalize_repairs_duplicates_and_clamps_bounds() {
        let mut p = Progress {
            streak: 8,
            best_streak: 2,
            freezes: 99,
            completed_lessons: vec!["u1-1".into(), "".into(), "u1-1".into(), "u1-2".into()],
            completed_practices: vec![
                "u1-hidden-clue".into(),
                "".into(),
                "u1-hidden-clue".into(),
                "u2-tidy-desk".into(),
            ],
            badges: vec![
                EarnedBadge { id: "first_step".into(), earned_day: "2026-11-01".into() },
                EarnedBadge { id: "".into(), earned_day: "2026-11-02".into() },
                EarnedBadge { id: "first_step".into(), earned_day: "2026-11-05".into() },
                EarnedBadge { id: "flawless".into(), earned_day: "2026-11-03".into() },
            ],
            srs: vec![
                srs::Card { question_id: "q1".into(), box_level: 42, due_day: "2026-11-19".into(), seen: 3, missed: 1 },
                srs::Card { question_id: "q1".into(), box_level: 1, due_day: "2026-11-20".into(), seen: 1, missed: 1 },
                srs::Card { question_id: "".into(), box_level: 0, due_day: "2026-11-19".into(), seen: 1, missed: 1 },
            ],
            ..Progress::default()
        };

        p.normalize();
        assert_eq!(p.freezes, MAX_FREEZES);
        assert_eq!(p.best_streak, 8);
        assert_eq!(p.completed_lessons, vec!["u1-1", "u1-2"]);
        assert_eq!(p.completed_practices, vec!["u1-hidden-clue", "u2-tidy-desk"]);
        assert_eq!(
            p.badges,
            vec![
                EarnedBadge { id: "first_step".into(), earned_day: "2026-11-01".into() },
                EarnedBadge { id: "flawless".into(), earned_day: "2026-11-03".into() },
            ]
        );
        assert_eq!(p.srs.len(), 1);
        assert_eq!(p.srs[0].question_id, "q1");
        assert_eq!(p.srs[0].box_level, srs::MAX_BOX);
    }

    #[test]
    fn config_daily_goal_and_state_path() {
        assert_eq!(daily_goal_from_config("daily_goal = 75\n"), Some(75));
        assert_eq!(daily_goal_from_config("daily_goal = 10\n"), Some(10));
        assert_eq!(daily_goal_from_config("daily_goal = 1000\n"), Some(1000));
        assert_eq!(daily_goal_from_config("daily_goal = 5\n"), Some(10));
        assert_eq!(daily_goal_from_config("daily_goal = -20\n"), Some(10));
        assert_eq!(daily_goal_from_config("daily_goal = 5000\n"), Some(1000));
        assert_eq!(daily_goal_from_config("daily_goal = \"50\"\n"), None);
        assert_eq!(daily_goal_from_config("mode = \"normal\"\n"), None);
        assert_eq!(daily_goal_from_config("not toml ["), None);

        assert_eq!(state_path(Some("/s"), Some("/h")).unwrap(), Path::new("/s/huh/progress.toml"));
        assert_eq!(state_path(Some(""), Some("/h")).unwrap(), Path::new("/h/.local/state/huh/progress.toml"));
        assert_eq!(state_path(None, Some("/h")).unwrap(), Path::new("/h/.local/state/huh/progress.toml"));
        assert_eq!(state_path(None, Some("")), None);
        assert_eq!(state_path(None, None), None);
    }

    #[test]
    fn atomic_save_and_load_round_trip() {
        let dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("target/progress-tests").join(format!("{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("progress.toml");

        // Missing file recovers to default (with 1 starting freeze).
        assert_eq!(load_from(&path), Progress::default());

        // Populate every field, including badges, SRS cards, and danger_spotted, to verify TOML round-trip.
        let d1 = day("2026-11-17");
        let d3 = day("2026-11-19");
        let mut p = Progress::default();
        p.award_xp(40, d1);
        p.award_xp(30, d3); // consumes 1 freeze -> last_freeze_used_day is Some
        p.complete_lesson("u1-where-am-i", true, d3);
        p.complete_practice("u1-hidden-clue", d3);
        p.award_badge("pathfinder", d3);
        for _ in 0..5 {
            p.record_danger_spotted(d3);
        }
        srs::record_answer(&mut p.srs, "u1-q1", false, d3);
        srs::record_answer(&mut p.srs, "u1-q2", false, d1);
        srs::record_answer(&mut p.srs, "u1-q2", true, d3);

        save_to(&path, &p).unwrap();
        assert_eq!(load_from(&path), p);

        // No temporary file left behind.
        let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(entries.len(), 1);

        // Backwards-compatible partial TOML keeps default freezes = 1 when omitted, and 0 when explicit.
        std::fs::write(&path, "xp = 120\n").unwrap();
        let partial = load_from(&path);
        assert_eq!(partial.xp, 120);
        assert_eq!(partial.freezes, 1);

        std::fs::write(&path, "xp = 120\nfreezes = 0\n").unwrap();
        let zero_freezes = load_from(&path);
        assert_eq!(zero_freezes.freezes, 0);

        // Phase 2 progress.toml without `completed_practices` loads cleanly with empty vec.
        std::fs::write(
            &path,
            "xp = 210\ndaily_xp = 50\ndaily_xp_day = \"2026-11-19\"\nstreak = 3\nbest_streak = 3\nfreezes = 1\n\
             last_streak_day = \"2026-11-19\"\ndanger_spotted = 2\ncompleted_lessons = [\"u1-where-am-i\"]\n\
             [[badges]]\nid = \"first_step\"\nearned_day = \"2026-11-19\"\n\
             [[srs]]\nquestion_id = \"u1-q1\"\nbox_level = 1\ndue_day = \"2026-11-20\"\nseen = 2\nmissed = 1\n",
        )
        .unwrap();
        let phase2 = load_from(&path);
        assert_eq!(phase2.xp, 210);
        assert_eq!(phase2.completed_lessons, vec!["u1-where-am-i"]);
        assert!(phase2.completed_practices.is_empty());
        assert!(phase2.has_badge("first_step"));
        assert_eq!(phase2.srs.len(), 1);

        // Corrupt TOML recovers cleanly to default.
        std::fs::write(&path, "not valid toml [[[").unwrap();
        assert_eq!(load_from(&path), Progress::default());

        // Out-of-range values are normalized on load.
        std::fs::write(
            &path,
            "freezes = 9\ncompleted_lessons = [\"u1-1\", \"u1-1\"]\n[[srs]]\nquestion_id = \"q1\"\nbox_level = 99\n",
        )
        .unwrap();
        let normalized = load_from(&path);
        assert_eq!(normalized.freezes, MAX_FREEZES);
        assert_eq!(normalized.completed_lessons, vec!["u1-1"]);
        assert_eq!(normalized.srs[0].box_level, srs::MAX_BOX);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
