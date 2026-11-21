//! 6-box Leitner spaced repetition for `huh learn review`.
//!
//! Only questions the learner has missed enter the deck, keeping `progress.toml` lean and
//! focusing review sessions on items that need reinforcement.

use crate::date::{self, Date};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;

/// Review intervals in days indexed by `Card::box_level` (`0..=5`).
pub const INTERVALS_DAYS: [i64; 6] = [0, 1, 3, 7, 14, 30];

/// Highest Leitner box index.
pub const MAX_BOX: u8 = (INTERVALS_DAYS.len() - 1) as u8;

/// A single spaced-repetition review card stored in `progress.toml`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    pub question_id: String,
    #[serde(default)]
    pub box_level: u8,
    #[serde(default)]
    pub due_day: String,
    #[serde(default)]
    pub seen: u32,
    #[serde(default)]
    pub missed: u32,
}

/// Updates `cards` after answering `question_id` on UTC date `today`.
///
/// - On a wrong answer (`correct == false`): inserts a new card or resets the existing one to
///   box `0`, due immediately (`today`), incrementing both `seen` and `missed`.
/// - On a right answer (`correct == true`): if a card already exists, promotes it by one box
///   (clamped at `MAX_BOX`), schedules `due_day` by `INTERVALS_DAYS[box_level]`, and increments
///   `seen`. Unknown questions are left out so review stays focused on missed items.
pub fn record_answer(cards: &mut Vec<Card>, question_id: &str, correct: bool, today: Date) {
    if correct {
        if let Some(card) = cards.iter_mut().find(|c| c.question_id == question_id) {
            card.box_level = card.box_level.saturating_add(1).min(MAX_BOX);
            let next_due = date::add_days(today, INTERVALS_DAYS[card.box_level as usize]);
            card.due_day = date::format(next_due);
            card.seen = card.seen.saturating_add(1);
        }
        return;
    }

    let today_str = date::format(today);
    if let Some(card) = cards.iter_mut().find(|c| c.question_id == question_id) {
        card.box_level = 0;
        card.due_day = today_str;
        card.seen = card.seen.saturating_add(1);
        card.missed = card.missed.saturating_add(1);
    } else {
        cards.push(Card { question_id: question_id.to_string(), box_level: 0, due_day: today_str, seen: 1, missed: 1 });
    }
}

/// Returns `true` when `card` is due on or before `today` (or has an unparseable `due_day`).
pub fn is_due(card: &Card, today: Date) -> bool {
    date::parse(&card.due_day).is_none_or(|due| due <= today)
}

/// Returns up to `limit` cards due on `today`, ordered by
/// `(box_level ASC, due_day ASC, missed DESC, question_id ASC)`.
pub fn due_cards<'a>(cards: &'a [Card], today: Date, limit: usize) -> Vec<&'a Card> {
    let mut due: Vec<&'a Card> = cards.iter().filter(|card| is_due(card, today)).collect();
    due.sort_by_key(|card| {
        (
            card.box_level,
            date::parse(&card.due_day),
            card.due_day.as_str(),
            Reverse(card.missed),
            card.question_id.as_str(),
        )
    });
    due.truncate(limit);
    due
}

/// Number of cards due on `today`.
pub fn due_count(cards: &[Card], today: Date) -> usize {
    cards.iter().filter(|card| is_due(card, today)).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> Date {
        date::parse(s).unwrap()
    }

    #[test]
    fn correct_on_unknown_question_does_not_create_card() {
        let mut cards = Vec::new();
        record_answer(&mut cards, "u1-1", true, day("2026-11-19"));
        assert!(cards.is_empty());
    }

    #[test]
    fn box_progression_intervals_and_clamping() {
        let mut cards = Vec::new();
        let d0 = day("2026-11-01");

        // Wrong answer enters box 0, due today (interval 0).
        record_answer(&mut cards, "u1-pwd", false, d0);
        assert_eq!(cards.len(), 1);
        assert_eq!(
            cards[0],
            Card {
                question_id: "u1-pwd".to_string(),
                box_level: 0,
                due_day: "2026-11-01".to_string(),
                seen: 1,
                missed: 1,
            }
        );
        assert!(is_due(&cards[0], d0));

        // Walk through boxes 1..=5, verifying each interval in INTERVALS_DAYS.
        let mut current = d0;
        for expected_box in 1..=5u8 {
            record_answer(&mut cards, "u1-pwd", true, current);
            let interval = INTERVALS_DAYS[expected_box as usize];
            let expected_due = date::add_days(current, interval);
            assert_eq!(cards[0].box_level, expected_box);
            assert_eq!(cards[0].due_day, date::format(expected_due));
            assert_eq!(cards[0].seen, u32::from(expected_box) + 1);
            assert_eq!(cards[0].missed, 1);
            assert!(!is_due(&cards[0], date::add_days(expected_due, -1)));
            assert!(is_due(&cards[0], expected_due));
            current = expected_due;
        }

        // Box 5 stays clamped at 5 with a 30-day interval.
        record_answer(&mut cards, "u1-pwd", true, current);
        assert_eq!(cards[0].box_level, 5);
        assert_eq!(cards[0].due_day, date::format(date::add_days(current, 30)));
        assert_eq!(cards[0].seen, 7);
        assert_eq!(cards[0].missed, 1);
    }

    #[test]
    fn wrong_answer_resets_high_box_to_zero_due_today() {
        let mut cards = vec![Card {
            question_id: "u2-rm".to_string(),
            box_level: 4,
            due_day: "2026-12-01".to_string(),
            seen: 6,
            missed: 2,
        }];
        let today = day("2026-11-19");
        record_answer(&mut cards, "u2-rm", false, today);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].box_level, 0);
        assert_eq!(cards[0].due_day, "2026-11-19");
        assert_eq!(cards[0].seen, 7);
        assert_eq!(cards[0].missed, 3);
        assert!(is_due(&cards[0], today));
    }

    #[test]
    fn due_cards_ordering_and_limit() {
        let today = day("2026-11-19");
        let cards = vec![
            Card { question_id: "future".into(), box_level: 0, due_day: "2026-11-20".into(), seen: 1, missed: 9 },
            Card { question_id: "b1-late".into(), box_level: 1, due_day: "2026-11-19".into(), seen: 3, missed: 2 },
            Card { question_id: "b1-early".into(), box_level: 1, due_day: "2026-11-15".into(), seen: 3, missed: 1 },
            Card { question_id: "b0-tie-b".into(), box_level: 0, due_day: "2026-11-18".into(), seen: 2, missed: 2 },
            Card { question_id: "b0-tie-a".into(), box_level: 0, due_day: "2026-11-18".into(), seen: 2, missed: 2 },
            Card { question_id: "b0-high-miss".into(), box_level: 0, due_day: "2026-11-18".into(), seen: 5, missed: 4 },
            Card { question_id: "b0-older".into(), box_level: 0, due_day: "2026-11-10".into(), seen: 2, missed: 1 },
        ];

        assert_eq!(due_count(&cards, today), 6);

        let all_due: Vec<&str> = due_cards(&cards, today, 10).into_iter().map(|c| c.question_id.as_str()).collect();
        assert_eq!(all_due, vec!["b0-older", "b0-high-miss", "b0-tie-a", "b0-tie-b", "b1-early", "b1-late"]);

        let top3: Vec<&str> = due_cards(&cards, today, 3).into_iter().map(|c| c.question_id.as_str()).collect();
        assert_eq!(top3, vec!["b0-older", "b0-high-miss", "b0-tie-a"]);

        assert!(due_cards(&cards, today, 0).is_empty());
    }

    #[test]
    fn unparseable_due_day_is_treated_as_due() {
        let today = day("2026-11-19");
        for bad in ["", "not-a-date", "2026-02-30", "2026/11/19"] {
            let card = Card { question_id: "q".into(), box_level: 2, due_day: bad.into(), seen: 1, missed: 1 };
            assert!(is_due(&card, today), "expected {bad:?} to be treated as due");
            assert_eq!(due_count(std::slice::from_ref(&card), today), 1);
            assert_eq!(due_cards(std::slice::from_ref(&card), today, 5).len(), 1);
        }
    }

    #[test]
    fn counters_and_box_level_saturate_safely() {
        let today = day("2026-11-19");
        let mut cards = vec![Card {
            question_id: "maxed".into(),
            box_level: u8::MAX,
            due_day: "2026-11-19".into(),
            seen: u32::MAX,
            missed: u32::MAX,
        }];

        record_answer(&mut cards, "maxed", true, today);
        assert_eq!(cards[0].box_level, MAX_BOX);
        assert_eq!(cards[0].seen, u32::MAX);
        assert_eq!(cards[0].missed, u32::MAX);
        assert_eq!(cards[0].due_day, date::format(date::add_days(today, 30)));

        record_answer(&mut cards, "maxed", false, today);
        assert_eq!(cards[0].box_level, 0);
        assert_eq!(cards[0].seen, u32::MAX);
        assert_eq!(cards[0].missed, u32::MAX);
        assert_eq!(cards[0].due_day, "2026-11-19");
    }
}
