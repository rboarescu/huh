//! The one file to review: all question wording and all thresholds.
//! (Per-command questions and descriptions are in `catalog/*.toml`.)
//! Questions are built from catalog entries;
//! the state they refer to is `{"request": "<what the user typed>"}`.

use crate::backend::{Question, Questions};
use crate::candidates::Candidates;
use crate::catalog::{Command, SlotKind};
use serde_json::{Value, json};

/// Thresholds for yes/no answers: above YES counts as yes, below NO as no,
/// in between the question is not settled. See the table in `assemble.rs`.
pub const YES: f64 = 0.7;
pub const NO: f64 = 0.3;
/// How many commands get a second look, and the least share of the command
/// Choice a command needs to be one of them.
pub const MAX_SUGGESTIONS: usize = 3;
pub const MIN_COMMAND_PROBABILITY: f64 = 0.10;

pub const COMMAND: &str = "command";

pub fn state(request: &str) -> Value {
    json!({ "request": request })
}

pub fn fit_id(command: &str) -> String {
    format!("{command}.fit")
}

pub fn item_id(command: &str, id: &str) -> String {
    format!("{command}.{id}")
}

/// Companion yes/no for a path or text slot: is a value named at all?
pub fn named_id(command: &str, slot: &str) -> String {
    format!("{command}.{slot}?")
}

/// Request A: which command? There is deliberately no "is this a terminal task"
/// question here: every wording tried either blocked plain questions about the
/// machine or let off-topic requests through. The per-command fit check in
/// request B is what rejects requests the catalog cannot serve.
pub fn command_questions(catalog: &[Command]) -> Questions {
    let mut questions = Questions::new();
    questions.insert(
        COMMAND.into(),
        Question::Choice {
            instructions: "Which command does the job the user describes in `request`?".into(),
            criteria: catalog.iter().map(|c| (c.name.clone(), Some(c.description.clone()))).collect(),
        },
    );
    questions
}

/// Request B: for each shortlisted command, does it fit, which flags, which values.
pub fn detail_questions(commands: &[&Command], candidates: &Candidates) -> Questions {
    let mut questions = Questions::new();
    for command in commands {
        questions.insert(
            fit_id(&command.name),
            Question::Noul {
                // Asks about the kind of task only. Asking "would this command do what
                // the request asks" scored low whenever the request also needed a flag.
                instructions: format!("Is this the kind of task `request` is about? {}", command.description),
            },
        );
        for flag in &command.flags {
            questions.insert(
                item_id(&command.name, &flag.id),
                Question::Noul { instructions: flag.question.clone() },
            );
        }
        for slot in &command.slots {
            match slot.kind {
                SlotKind::Choice => {
                    questions.insert(
                        item_id(&command.name, &slot.id),
                        Question::Choice {
                            instructions: slot.question.clone().unwrap_or_default(),
                            criteria: slot
                                .options
                                .iter()
                                .map(|o| (o.name.clone(), Some(o.description.clone())))
                                .collect(),
                        },
                    );
                }
                SlotKind::Path | SlotKind::Text => {
                    let offered = candidates.for_slot(slot.kind);
                    if offered.is_empty() {
                        continue;
                    }
                    let what = slot.what.as_deref().unwrap_or_default();
                    questions.insert(
                        named_id(&command.name, &slot.id),
                        Question::Noul { instructions: format!("Does `request` name {what}?") },
                    );
                    questions.insert(
                        item_id(&command.name, &slot.id),
                        Question::Choice {
                            instructions: format!("Which of these does `request` name as {what}?"),
                            criteria: offered.into_iter().map(|value| (value.to_string(), None)).collect(),
                        },
                    );
                }
            }
        }
    }
    questions
}
