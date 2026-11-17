//! The question and answer types every decision-model backend speaks.

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub type Questions = BTreeMap<String, Question>;
pub type Answers = BTreeMap<String, Answer>;

#[derive(Serialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    /// Pick one option. Keys are option names (the model sees them), values describe them.
    Choice {
        instructions: String,
        criteria: BTreeMap<String, Option<String>>,
    },
    /// Probability that a statement is true.
    Noul { instructions: String },
}

#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
    },
    Noul {
        noul: f64,
    },
    #[serde(other)]
    Other,
}

pub trait Backend {
    fn ask(&self, state: &Value, questions: &Questions) -> Result<Answers>;
}

/// A missing or mistyped answer is an error, never a silent "no".
pub fn noul(answers: &Answers, id: &str) -> Result<f64> {
    match answers.get(id) {
        Some(Answer::Noul { noul }) => Ok(*noul),
        _ => Err(anyhow!("backend returned no yes/no answer for `{id}`")),
    }
}

pub fn choice<'a>(answers: &'a Answers, id: &str) -> Result<(&'a str, &'a BTreeMap<String, f64>)> {
    match answers.get(id) {
        Some(Answer::Choice { choice, probabilities }) => Ok((choice, probabilities)),
        _ => Err(anyhow!("backend returned no choice answer for `{id}`")),
    }
}
