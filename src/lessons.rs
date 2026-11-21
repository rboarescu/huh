//! Curriculum loader and validator for `huh learn`.
//!
//! Units live in `lessons/*.toml` and are embedded at build time by `build.rs`.
//! Nothing here ever runs a command; everything is pure data parsing and checks.

use crate::pet::Species;
use crate::platform;
use crate::practice::{self, Practice};
use crate::progress;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

include!(concat!(env!("OUT_DIR"), "/embedded_lessons.rs"));

const RESOLVED_PKG_KEYS: [&str; 5] = ["pkg_install", "pkg_remove", "pkg_search", "pkg_update", "pkg_query"];

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unit {
    pub id: String,
    pub number: u8,
    pub title: String,
    pub summary: String,
    pub badge: String,
    #[serde(default)]
    pub unlocks_species: Option<String>,
    #[serde(default)]
    pub unlock_after: Option<String>,
    #[serde(default)]
    pub practice: Option<Practice>,
    pub lessons: Vec<Lesson>,
}

impl Unit {
    /// Returns `true` when this unit branches off an explicit prerequisite via `unlock_after`
    /// rather than sitting on the linear core progression track.
    pub fn is_side_quest(&self) -> bool {
        self.unlock_after.is_some()
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lesson {
    pub id: String,
    pub title: String,
    pub intro: String,
    pub questions: Vec<Question>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Question {
    ExplainChoice {
        id: String,
        command: String,
        prompt: String,
        choices: Vec<String>,
        answer: usize,
        hint: String,
        note: String,
    },
    TypeCommand {
        id: String,
        prompt: String,
        accepted: Vec<String>,
        hints: Vec<String>,
        note: String,
    },
    ReadOutput {
        id: String,
        command: String,
        output: String,
        prompt: String,
        choices: Vec<String>,
        answer: usize,
        hint: String,
        note: String,
        #[serde(default)]
        #[cfg_attr(not(test), allow(dead_code))]
        expect_output_match: bool,
    },
    SpotDanger {
        id: String,
        prompt: String,
        #[serde(default)]
        command: Option<String>,
        choices: Vec<String>,
        answer: usize,
        risk: crate::risk_kinds::Risk,
        hint: String,
        note: String,
    },
}

impl Question {
    pub fn id(&self) -> &str {
        match self {
            Question::ExplainChoice { id, .. }
            | Question::TypeCommand { id, .. }
            | Question::ReadOutput { id, .. }
            | Question::SpotDanger { id, .. } => id,
        }
    }

    pub fn kind_label(&self) -> &'static str {
        match self {
            Question::ExplainChoice { .. } => "What does it do?",
            Question::TypeCommand { .. } => "Type the command",
            Question::ReadOutput { .. } => "Read the output",
            Question::SpotDanger { .. } => "Spot the danger",
        }
    }

    pub fn prompt(&self) -> &str {
        match self {
            Question::ExplainChoice { prompt, .. }
            | Question::TypeCommand { prompt, .. }
            | Question::ReadOutput { prompt, .. }
            | Question::SpotDanger { prompt, .. } => prompt,
        }
    }

    pub fn note(&self) -> &str {
        match self {
            Question::ExplainChoice { note, .. }
            | Question::TypeCommand { note, .. }
            | Question::ReadOutput { note, .. }
            | Question::SpotDanger { note, .. } => note,
        }
    }

    pub fn hint_at(&self, step: usize) -> &str {
        match self {
            Question::ExplainChoice { hint, .. }
            | Question::ReadOutput { hint, .. }
            | Question::SpotDanger { hint, .. } => hint,
            Question::TypeCommand { hints, .. } => {
                if hints.is_empty() {
                    ""
                } else {
                    &hints[step.min(hints.len() - 1)]
                }
            }
        }
    }

    pub fn is_spot_danger(&self) -> bool {
        matches!(self, Question::SpotDanger { .. })
    }
}

fn resolve_pkg_fact(platforms: &[String], key: &str) -> String {
    let fallback = ["debian".to_string()];
    let active = if platform::manager(platforms).is_some() { platforms } else { &fallback };
    platform::fact(active, key).or_else(|| platform::fact(&fallback, key)).unwrap_or_default()
}

fn resolve_placeholders(text: &str, platforms: &[String]) -> String {
    if !text.contains("{pkg_") {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{pkg_") {
        out.push_str(&rest[..start]);
        let after_brace = &rest[start + 1..];
        let Some(end) = after_brace.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let token = &after_brace[..end];
        let (key, arg) = match token.split_once(':') {
            Some((k, a)) => (k, Some(a)),
            None => (token, None),
        };
        if RESOLVED_PKG_KEYS.contains(&key) {
            let fact = resolve_pkg_fact(platforms, key);
            match arg {
                Some(arg) => out.push_str(&platform::fill(&fact, arg)),
                None => out.push_str(fact.replace(" {}", "").replace("{}", "").trim()),
            }
        } else {
            out.push('{');
            out.push_str(token);
            out.push('}');
        }
        rest = &after_brace[end + 1..];
    }
    out.push_str(rest);
    out
}

fn resolve_unit(unit: &mut Unit, platforms: &[String]) {
    unit.summary = resolve_placeholders(&unit.summary, platforms);
    for lesson in &mut unit.lessons {
        lesson.intro = resolve_placeholders(&lesson.intro, platforms);
        for q in &mut lesson.questions {
            match q {
                Question::ExplainChoice { command, prompt, choices, hint, note, .. } => {
                    *command = resolve_placeholders(command, platforms);
                    *prompt = resolve_placeholders(prompt, platforms);
                    for choice in choices {
                        *choice = resolve_placeholders(choice, platforms);
                    }
                    *hint = resolve_placeholders(hint, platforms);
                    *note = resolve_placeholders(note, platforms);
                }
                Question::TypeCommand { prompt, accepted, hints, note, .. } => {
                    *prompt = resolve_placeholders(prompt, platforms);
                    for cmd in accepted {
                        *cmd = resolve_placeholders(cmd, platforms);
                    }
                    for hint in hints {
                        *hint = resolve_placeholders(hint, platforms);
                    }
                    *note = resolve_placeholders(note, platforms);
                }
                Question::ReadOutput { command, output, prompt, choices, hint, note, .. } => {
                    *command = resolve_placeholders(command, platforms);
                    *output = resolve_placeholders(output, platforms);
                    *prompt = resolve_placeholders(prompt, platforms);
                    for choice in choices {
                        *choice = resolve_placeholders(choice, platforms);
                    }
                    *hint = resolve_placeholders(hint, platforms);
                    *note = resolve_placeholders(note, platforms);
                }
                Question::SpotDanger { prompt, command, choices, hint, note, .. } => {
                    *prompt = resolve_placeholders(prompt, platforms);
                    if let Some(cmd) = command {
                        *cmd = resolve_placeholders(cmd, platforms);
                    }
                    for choice in choices {
                        *choice = resolve_placeholders(choice, platforms);
                    }
                    *hint = resolve_placeholders(hint, platforms);
                    *note = resolve_placeholders(note, platforms);
                }
            }
        }
    }
    if let Some(practice) = &mut unit.practice {
        practice.goal = resolve_placeholders(&practice.goal, platforms);
        for hint in &mut practice.hints {
            *hint = resolve_placeholders(hint, platforms);
        }
    }
}

pub fn load_embedded_for(platforms: &[String]) -> Result<Vec<Unit>> {
    let mut units = Vec::with_capacity(EMBEDDED_LESSONS.len());
    for &(name, text) in EMBEDDED_LESSONS {
        let mut unit: Unit = toml::from_str(text).with_context(|| format!("invalid lesson file {name}"))?;
        resolve_unit(&mut unit, platforms);
        units.push(unit);
    }
    units.sort_by_key(|u| u.number);
    validate(&units)?;
    Ok(units)
}

pub fn load_embedded() -> Result<Vec<Unit>> {
    load_embedded_for(&platform::detect())
}

fn require_non_empty(value: &str, label: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{label} must not be empty");
    }
    if value.contains("{pkg_") {
        bail!("{label} contains an unresolved package placeholder: {value}");
    }
    Ok(())
}

fn validate_choices(qid: &str, choices: &[String], answer: usize) -> Result<()> {
    if !(3..=4).contains(&choices.len()) {
        bail!("question `{qid}` must have 3 or 4 choices (found {})", choices.len());
    }
    for (idx, choice) in choices.iter().enumerate() {
        require_non_empty(choice, &format!("question `{qid}` choice {idx}"))?;
    }
    if answer >= choices.len() {
        bail!("question `{qid}` answer index {answer} is out of bounds for {} choices", choices.len());
    }
    Ok(())
}

pub fn validate(units: &[Unit]) -> Result<()> {
    if units.is_empty() {
        bail!("curriculum must contain at least one unit");
    }

    let mut unit_ids = HashSet::new();
    let mut unit_numbers = HashSet::new();
    let mut lesson_ids = HashSet::new();
    let mut question_ids = HashSet::new();
    let mut practice_ids = HashSet::new();

    for unit in units {
        require_non_empty(&unit.id, "unit id")?;
        require_non_empty(&unit.title, &format!("unit `{}` title", unit.id))?;
        require_non_empty(&unit.summary, &format!("unit `{}` summary", unit.id))?;
        require_non_empty(&unit.badge, &format!("unit `{}` badge", unit.id))?;

        if unit.number == 0 {
            bail!("unit `{}` number must be >= 1", unit.id);
        }
        if !unit_ids.insert(unit.id.as_str()) {
            bail!("duplicate unit id `{}`", unit.id);
        }
        if !unit_numbers.insert(unit.number) {
            bail!("duplicate unit number {}", unit.number);
        }
        if progress::badge_def(&unit.badge).is_none() {
            bail!("unit `{}` references unknown badge `{}`", unit.id, unit.badge);
        }
        if let Some(species) = &unit.unlocks_species {
            Species::parse(species)
                .with_context(|| format!("unit `{}` has invalid unlocks_species `{species}`", unit.id))?;
        }
        if let Some(practice) = &unit.practice {
            practice::validate_practice(practice)
                .with_context(|| format!("unit `{}` has invalid practice lab", unit.id))?;
            if !practice_ids.insert(practice.id.as_str()) {
                bail!("duplicate practice id `{}`", practice.id);
            }
        }
        if unit.lessons.is_empty() {
            bail!("unit `{}` must contain at least one lesson", unit.id);
        }

        for lesson in &unit.lessons {
            require_non_empty(&lesson.id, &format!("lesson id in unit `{}`", unit.id))?;
            require_non_empty(&lesson.title, &format!("lesson `{}` title", lesson.id))?;
            require_non_empty(&lesson.intro, &format!("lesson `{}` intro", lesson.id))?;
            if !lesson_ids.insert(lesson.id.as_str()) {
                bail!("duplicate lesson id `{}`", lesson.id);
            }
            if lesson.questions.is_empty() {
                bail!("lesson `{}` must contain at least one question", lesson.id);
            }

            for question in &lesson.questions {
                let qid = question.id();
                require_non_empty(qid, &format!("question id in lesson `{}`", lesson.id))?;
                if !question_ids.insert(qid) {
                    bail!("duplicate question id `{qid}`");
                }
                require_non_empty(question.prompt(), &format!("question `{qid}` prompt"))?;
                require_non_empty(question.note(), &format!("question `{qid}` note"))?;

                match question {
                    Question::ExplainChoice { command, choices, answer, hint, .. } => {
                        require_non_empty(command, &format!("question `{qid}` command"))?;
                        require_non_empty(hint, &format!("question `{qid}` hint"))?;
                        validate_choices(qid, choices, *answer)?;
                    }
                    Question::TypeCommand { accepted, hints, .. } => {
                        if accepted.is_empty() {
                            bail!("question `{qid}` must list at least one accepted command");
                        }
                        for cmd in accepted {
                            require_non_empty(cmd, &format!("question `{qid}` accepted command"))?;
                        }
                        if hints.is_empty() {
                            bail!("question `{qid}` must list at least one hint");
                        }
                        for hint in hints {
                            require_non_empty(hint, &format!("question `{qid}` hint"))?;
                        }
                    }
                    Question::ReadOutput { command, output, choices, answer, hint, .. } => {
                        require_non_empty(command, &format!("question `{qid}` command"))?;
                        require_non_empty(output, &format!("question `{qid}` output"))?;
                        require_non_empty(hint, &format!("question `{qid}` hint"))?;
                        validate_choices(qid, choices, *answer)?;
                    }
                    Question::SpotDanger { command, choices, answer, hint, .. } => {
                        if let Some(cmd) = command {
                            require_non_empty(cmd, &format!("question `{qid}` command"))?;
                        }
                        require_non_empty(hint, &format!("question `{qid}` hint"))?;
                        validate_choices(qid, choices, *answer)?;
                    }
                }
            }
        }
    }

    let id_to_idx: HashMap<&str, usize> = units.iter().enumerate().map(|(i, u)| (u.id.as_str(), i)).collect();
    for unit in units {
        if let Some(dep) = &unit.unlock_after {
            if dep.trim().is_empty() {
                bail!("unit `{}` unlock_after must not be empty", unit.id);
            }
            if dep == &unit.id {
                bail!("unit `{}` unlock_after cannot reference itself", unit.id);
            }
            if !unit_ids.contains(dep.as_str()) {
                bail!("unit `{}` unlock_after references unknown unit `{dep}`", unit.id);
            }
        }
    }
    for (start_idx, unit) in units.iter().enumerate() {
        let mut seen = HashSet::new();
        let mut cur = Some(start_idx);
        while let Some(idx) = cur {
            if !seen.insert(idx) {
                bail!("unit `{}` has a circular prerequisite chain", unit.id);
            }
            cur = match units[idx].unlock_after.as_deref() {
                Some(dep) => id_to_idx.get(dep).copied(),
                None => idx.checked_sub(1),
            };
        }
    }

    Ok(())
}

pub fn find_unit<'a>(units: &'a [Unit], query: &str) -> Option<&'a Unit> {
    let q = query.trim();
    if q.is_empty() {
        return None;
    }
    if let Ok(num) = q.parse::<u8>() {
        return units.iter().find(|u| u.number == num);
    }
    let lower = q.to_ascii_lowercase();
    units
        .iter()
        .find(|u| u.id.eq_ignore_ascii_case(&lower))
        .or_else(|| {
            units.iter().find(|u| {
                u.id.to_ascii_lowercase().starts_with(&lower) || u.title.to_ascii_lowercase().starts_with(&lower)
            })
        })
        .or_else(|| {
            units
                .iter()
                .find(|u| u.title.to_ascii_lowercase().contains(&lower) || u.id.to_ascii_lowercase().contains(&lower))
        })
}

pub fn find_question<'a>(units: &'a [Unit], qid: &str) -> Option<(&'a Unit, &'a Lesson, &'a Question)> {
    let target = qid.trim();
    if target.is_empty() {
        return None;
    }
    for unit in units {
        for lesson in &unit.lessons {
            for question in &lesson.questions {
                if question.id() == target {
                    return Some((unit, lesson, question));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::practice::Check;
    use crate::risk_kinds::Risk;

    fn assert_command_explains_cleanly(catalog: &[crate::catalog::Command], qid: &str, cmd: &str) {
        let parsed = crate::shell::parse(cmd);
        assert!(parsed.notes.is_empty(), "question `{qid}` command `{cmd}` had parse notes: {:?}", parsed.notes);
        let exp = crate::explain::explain_command(catalog, cmd);
        assert!(!exp.blocks.is_empty(), "question `{qid}` command `{cmd}` produced no blocks");
        assert!(!exp.steps.is_empty(), "question `{qid}` command `{cmd}` produced no steps");
        assert!(
            exp.steps.iter().all(|s| !s.text.trim().is_empty()),
            "question `{qid}` command `{cmd}` had an empty step headline"
        );
    }

    #[test]
    fn embedded_curriculum_is_valid_and_complete() {
        let units = load_embedded().expect("embedded curriculum should parse and validate");
        assert_eq!(units.len(), 11);
        let expected_ids = [
            "finding-your-way",
            "files-and-folders",
            "searching-and-pipes",
            "reading-permissions",
            "processes-and-disk",
            "packages-and-manuals",
            "archives-and-links",
            "networking-and-stay-safe",
            "apps-and-games",
            "media-and-everyday",
            "paint-your-pet",
        ];
        for (idx, expected_id) in expected_ids.iter().enumerate() {
            assert_eq!(units[idx].number, (idx + 1) as u8);
            assert_eq!(units[idx].id, *expected_id);
            if idx < 8 {
                assert_eq!(units[idx].unlock_after, None, "core unit `{expected_id}` should not have unlock_after");
                assert!(!units[idx].is_side_quest());
            } else {
                assert_eq!(
                    units[idx].unlock_after.as_deref(),
                    Some("finding-your-way"),
                    "real-world unit `{expected_id}` should unlock right after Unit 1"
                );
                assert!(units[idx].is_side_quest());
            }
        }
        assert_eq!(units[8].badge, "game_runner");
        assert_eq!(units[9].badge, "media_wrangler");
        assert_eq!(units[10].badge, "pixel_painter");

        let mut total_lessons = 0;
        let mut total_questions = 0;
        let mut explain_choice = 0;
        let mut type_command = 0;
        let mut read_output = 0;
        let mut spot_danger = 0;

        for unit in &units {
            assert_eq!(unit.lessons.len(), 3, "unit `{}` should have 3 lessons", unit.id);
            if let Some(species_name) = &unit.unlocks_species {
                let species = Species::parse(species_name).unwrap();
                let (expected_badge, _) = species.unlock_badge().expect("unlocked species should map to a badge");
                assert_eq!(unit.badge, expected_badge, "unit `{}` badge should match pet.rs", unit.id);
            }
            for lesson in &unit.lessons {
                total_lessons += 1;
                assert_eq!(lesson.questions.len(), 5, "lesson `{}` should have 5 questions", lesson.id);
                for q in &lesson.questions {
                    total_questions += 1;
                    match q {
                        Question::ExplainChoice { .. } => explain_choice += 1,
                        Question::TypeCommand { .. } => type_command += 1,
                        Question::ReadOutput { .. } => read_output += 1,
                        Question::SpotDanger { .. } => spot_danger += 1,
                    }
                }
            }
        }

        assert_eq!(total_lessons, 33);
        assert_eq!(total_questions, 165);
        assert!(explain_choice > 0, "expected explain_choice questions");
        assert!(type_command > 0, "expected type_command questions");
        assert!(read_output > 0, "expected read_output questions");
        assert!(spot_danger >= 20, "expected at least 20 spot_danger questions (found {spot_danger})");

        // Unit 8 Lesson 3 is the 5-risk "Spot the Danger" boss lesson.
        let boss = &units[7].lessons[2];
        let mut boss_risks = HashSet::new();
        for q in &boss.questions {
            match q {
                Question::SpotDanger { risk, .. } => {
                    boss_risks.insert(*risk);
                }
                other => panic!("Unit 8 Lesson 3 should only contain SpotDanger questions, found `{}`", other.id()),
            }
        }
        assert_eq!(boss_risks.len(), 5, "boss lesson must cover 5 distinct risk categories: {boss_risks:?}");
        for expected in
            [Risk::RunsInternetCode, Risk::WeakensSecurity, Risk::ExposesService, Risk::LeaksSecrets, Risk::HidesTracks]
        {
            assert!(boss_risks.contains(&expected), "boss lesson missing risk {expected:?}");
        }
    }

    #[test]
    fn unit_six_resolves_cleanly_across_distros() {
        let catalog = crate::catalog::load_embedded().unwrap();
        for distro in ["arch", "omarchy", "debian", "fedora", "alpine", "suse", "nixos", ""] {
            let platforms = if distro.is_empty() { Vec::new() } else { vec![distro.to_string()] };
            let units =
                load_embedded_for(&platforms).unwrap_or_else(|e| panic!("load_embedded_for({distro:?}) failed: {e}"));
            for unit_num in [6_u8, 9_u8] {
                let unit = units
                    .iter()
                    .find(|u| u.number == unit_num)
                    .unwrap_or_else(|| panic!("unit {unit_num} should exist"));
                let mut checked = 0;

                for lesson in &unit.lessons {
                    for q in &lesson.questions {
                        match q {
                            Question::ExplainChoice { id, command, .. } | Question::ReadOutput { id, command, .. } => {
                                assert!(
                                    !command.contains("{pkg_"),
                                    "distro `{distro}` `{id}` had unresolved placeholder: {command}"
                                );
                                assert_command_explains_cleanly(&catalog, id, command);
                                checked += 1;
                            }
                            Question::TypeCommand { id, accepted, .. } => {
                                for cmd in accepted {
                                    assert!(
                                        !cmd.contains("{pkg_"),
                                        "distro `{distro}` `{id}` had unresolved placeholder: {cmd}"
                                    );
                                    assert_command_explains_cleanly(&catalog, id, cmd);
                                    checked += 1;
                                }
                            }
                            Question::SpotDanger { id, command, choices, answer, risk, .. } => {
                                let danger_cmd = command.as_deref().unwrap_or(&choices[*answer]);
                                assert!(
                                    !danger_cmd.contains("{pkg_"),
                                    "distro `{distro}` `{id}` had unresolved placeholder: {danger_cmd}"
                                );
                                assert_command_explains_cleanly(&catalog, id, danger_cmd);
                                let exp = crate::explain::explain_command(&catalog, danger_cmd);
                                assert!(
                                    exp.warnings.iter().any(|w| w.risk == *risk),
                                    "distro `{distro}` spot_danger `{id}` `{danger_cmd}` missed risk {risk:?}"
                                );
                                if command.is_none() {
                                    for (idx, choice) in choices.iter().enumerate() {
                                        if idx == *answer {
                                            continue;
                                        }
                                        assert!(
                                            !choice.contains("{pkg_"),
                                            "distro `{distro}` `{id}` had unresolved placeholder: {choice}"
                                        );
                                        assert_command_explains_cleanly(&catalog, id, choice);
                                        let d_exp = crate::explain::explain_command(&catalog, choice);
                                        assert!(
                                            !d_exp.warnings.iter().any(|w| w.risk == *risk),
                                            "distro `{distro}` spot_danger `{id}` distractor `{choice}` emitted {risk:?}"
                                        );
                                    }
                                }
                                checked += 1;
                            }
                        }
                    }
                }

                assert!(
                    checked >= 15,
                    "distro `{distro}` should verify all Unit {unit_num} commands (checked {checked})"
                );
            }
        }
    }

    #[test]
    fn every_command_parses_and_explains_cleanly() {
        let catalog = crate::catalog::load_embedded().unwrap();
        let units = load_embedded().unwrap();
        let mut checked = 0;

        for unit in &units {
            for lesson in &unit.lessons {
                for q in &lesson.questions {
                    match q {
                        Question::ExplainChoice { id, command, .. } | Question::ReadOutput { id, command, .. } => {
                            assert_command_explains_cleanly(&catalog, id, command);
                            checked += 1;
                        }
                        Question::TypeCommand { id, accepted, .. } => {
                            for cmd in accepted {
                                assert_command_explains_cleanly(&catalog, id, cmd);
                                checked += 1;
                            }
                        }
                        Question::SpotDanger { id, command: Some(cmd), .. } => {
                            assert_command_explains_cleanly(&catalog, id, cmd);
                            checked += 1;
                        }
                        Question::SpotDanger { command: None, .. } => {}
                    }
                }
            }
        }

        assert!(checked >= 140, "expected to verify all curriculum commands, checked {checked}");
    }

    #[test]
    fn read_output_samples_match_outputs_engine() {
        let units = load_embedded().unwrap();
        let mut matched = 0;

        for unit in &units {
            for lesson in &unit.lessons {
                for q in &lesson.questions {
                    if let Question::ReadOutput { id, output, expect_output_match: true, .. } = q {
                        let described = crate::outputs::describe(output);
                        assert!(described.is_some(), "question `{id}` output was not recognised by outputs::describe");
                        matched += 1;
                    }
                }
            }
        }

        assert!(
            matched >= 13,
            "expected at least 13 ReadOutput questions with expect_output_match = true (got {matched})"
        );
    }

    #[test]
    fn spot_danger_commands_match_risk_engine() {
        let catalog = crate::catalog::load_embedded().unwrap();
        let units = load_embedded().unwrap();
        let mut checked = 0;

        for unit in &units {
            for lesson in &unit.lessons {
                for q in &lesson.questions {
                    if let Question::SpotDanger { id, command, choices, answer, risk, .. } = q {
                        let danger_cmd = command.as_deref().unwrap_or(&choices[*answer]);
                        let parsed = crate::shell::parse(danger_cmd);
                        assert!(
                            parsed.notes.is_empty(),
                            "spot_danger `{id}` command `{danger_cmd}` had parse notes: {:?}",
                            parsed.notes
                        );
                        let exp = crate::explain::explain_command(&catalog, danger_cmd);
                        assert!(
                            exp.warnings.iter().any(|w| w.risk == *risk),
                            "spot_danger `{id}` command `{danger_cmd}` did not emit risk {:?}; warnings: {:?}",
                            risk,
                            exp.warnings
                        );
                        if command.is_none() {
                            for (idx, choice) in choices.iter().enumerate() {
                                if idx == *answer {
                                    continue;
                                }
                                let distractor_exp = crate::explain::explain_command(&catalog, choice);
                                assert!(
                                    !distractor_exp.warnings.iter().any(|w| w.risk == *risk),
                                    "spot_danger `{id}` safe distractor `{choice}` unexpectedly emitted risk {:?}",
                                    risk
                                );
                            }
                        }
                        checked += 1;
                    }
                }
            }
        }

        assert!(checked >= 20, "expected at least 20 SpotDanger questions across Units 1-11 (got {checked})");
    }

    #[test]
    fn embedded_practice_labs_are_valid_and_cover_all_checks() {
        let units = load_embedded().unwrap();
        let mut lab_count = 0;
        let mut seen_kinds = HashSet::new();

        for unit in &units {
            if let Some(lab) = &unit.practice {
                practice::validate_practice(lab)
                    .unwrap_or_else(|e| panic!("unit `{}` practice `{}` failed validation: {e}", unit.id, lab.id));
                lab_count += 1;
                for check in &lab.checks {
                    let kind = match check {
                        Check::DirExists { .. } => "dir_exists",
                        Check::FileExists { .. } => "file_exists",
                        Check::NotExists { .. } => "not_exists",
                        Check::FileContains { .. } => "file_contains",
                        Check::FileEquals { .. } => "file_equals",
                        Check::FileModeHas { .. } => "file_mode_has",
                        Check::FileModeEq { .. } => "file_mode_eq",
                        Check::IsSymlink { .. } => "is_symlink",
                        Check::RecordedOutput { .. } => "recorded_output",
                    };
                    seen_kinds.insert(kind);
                }
            }
        }

        assert_eq!(lab_count, 9, "expected 9 units with practice labs (got {lab_count})");
        for expected in [
            "dir_exists",
            "file_exists",
            "not_exists",
            "file_contains",
            "file_equals",
            "file_mode_has",
            "file_mode_eq",
            "is_symlink",
            "recorded_output",
        ] {
            assert!(seen_kinds.contains(expected), "embedded practice labs missing check kind `{expected}`");
        }
    }

    #[test]
    fn find_unit_and_question_lookup() {
        let units = load_embedded().unwrap();

        assert_eq!(find_unit(&units, "1").map(|u| u.id.as_str()), Some("finding-your-way"));
        assert_eq!(find_unit(&units, "  2  ").map(|u| u.id.as_str()), Some("files-and-folders"));
        assert_eq!(find_unit(&units, "3").map(|u| u.id.as_str()), Some("searching-and-pipes"));
        assert_eq!(find_unit(&units, "4").map(|u| u.id.as_str()), Some("reading-permissions"));
        assert_eq!(find_unit(&units, "5").map(|u| u.id.as_str()), Some("processes-and-disk"));
        assert_eq!(find_unit(&units, "6").map(|u| u.id.as_str()), Some("packages-and-manuals"));
        assert_eq!(find_unit(&units, "7").map(|u| u.id.as_str()), Some("archives-and-links"));
        assert_eq!(find_unit(&units, "8").map(|u| u.id.as_str()), Some("networking-and-stay-safe"));
        assert_eq!(find_unit(&units, "9").map(|u| u.id.as_str()), Some("apps-and-games"));
        assert_eq!(find_unit(&units, "10").map(|u| u.id.as_str()), Some("media-and-everyday"));
        assert_eq!(find_unit(&units, "11").map(|u| u.id.as_str()), Some("paint-your-pet"));
        assert_eq!(find_unit(&units, "finding-your-way").map(|u| u.number), Some(1));
        assert_eq!(find_unit(&units, "FILES-AND-FOLDERS").map(|u| u.number), Some(2));
        assert_eq!(find_unit(&units, "searching").map(|u| u.number), Some(3));
        assert_eq!(find_unit(&units, "permissions").map(|u| u.number), Some(4));
        assert_eq!(find_unit(&units, "disk").map(|u| u.number), Some(5));
        assert_eq!(find_unit(&units, "packages").map(|u| u.number), Some(6));
        assert_eq!(find_unit(&units, "archives").map(|u| u.number), Some(7));
        assert_eq!(find_unit(&units, "networking").map(|u| u.number), Some(8));
        assert_eq!(find_unit(&units, "apps").map(|u| u.number), Some(9));
        assert_eq!(find_unit(&units, "media").map(|u| u.number), Some(10));
        assert_eq!(find_unit(&units, "paint").map(|u| u.number), Some(11));
        assert!(find_unit(&units, "").is_none());
        assert!(find_unit(&units, "99").is_none());
        assert!(find_unit(&units, "quantum-computing").is_none());

        let (unit, lesson, q) = find_question(&units, "u1-l2-q4").expect("u1-l2-q4 should exist");
        assert_eq!(unit.id, "finding-your-way");
        assert_eq!(lesson.id, "u1-hidden-and-details");
        assert_eq!(q.kind_label(), "Type the command");
        assert!(!q.is_spot_danger());
        assert_ne!(q.hint_at(0), q.hint_at(1));
        assert_eq!(q.hint_at(1), q.hint_at(99));

        let (u8, boss, danger) = find_question(&units, "u8-l3-q5").expect("u8-l3-q5 should exist");
        assert_eq!(u8.id, "networking-and-stay-safe");
        assert_eq!(boss.id, "u8-boss-spot-danger");
        assert_eq!(danger.kind_label(), "Spot the danger");
        assert!(danger.is_spot_danger());
        assert!(!danger.hint_at(0).is_empty());

        let (u9, u9_l1, u9_q3) = find_question(&units, "u9-l1-q3").expect("u9-l1-q3 should exist");
        assert_eq!(u9.id, "apps-and-games");
        assert_eq!(u9_l1.id, "u9-portable-apps");
        assert_eq!(u9_q3.kind_label(), "Read the output");

        let (u10, u10_l3, u10_q5) = find_question(&units, "u10-l3-q5").expect("u10-l3-q5 should exist");
        assert_eq!(u10.id, "media-and-everyday");
        assert_eq!(u10_l3.id, "u10-ffmpeg-basics");
        assert!(u10_q5.is_spot_danger());

        let (u11, u11_l3, u11_q4) = find_question(&units, "u11-l3-q4").expect("u11-l3-q4 should exist");
        assert_eq!(u11.id, "paint-your-pet");
        assert_eq!(u11_l3.id, "u11-colors-and-backups");
        assert_eq!(u11_q4.kind_label(), "Read the output");

        assert!(find_question(&units, "no-such-question").is_none());
        assert!(find_question(&units, "").is_none());
    }

    #[test]
    fn validate_rejects_broken_curricula() {
        assert!(validate(&[]).is_err());

        let base = load_embedded().unwrap();

        let mut dup_num = base.clone();
        dup_num[1].number = 1;
        assert!(validate(&dup_num).is_err());

        let mut dup_id = base.clone();
        dup_id[1].id = dup_id[0].id.clone();
        assert!(validate(&dup_id).is_err());

        let mut bad_species = base.clone();
        bad_species[0].unlocks_species = Some("dragon".to_string());
        assert!(validate(&bad_species).is_err());

        let mut bad_badge = base.clone();
        bad_badge[0].badge = "not_a_real_badge".to_string();
        assert!(validate(&bad_badge).is_err());

        let mut dup_practice = base.clone();
        dup_practice[1].practice = dup_practice[0].practice.clone();
        assert!(validate(&dup_practice).is_err());

        let mut unresolved_pkg = base.clone();
        unresolved_pkg[5].summary = "Try {pkg_unknown:foo}".to_string();
        assert!(validate(&unresolved_pkg).is_err());

        let mut bad_answer = base.clone();
        if let Question::ExplainChoice { answer, .. } = &mut bad_answer[0].lessons[0].questions[0] {
            *answer = 99;
        }
        assert!(validate(&bad_answer).is_err());

        let mut too_few_choices = base.clone();
        if let Question::ExplainChoice { choices, .. } = &mut too_few_choices[0].lessons[0].questions[0] {
            choices.truncate(2);
        }
        assert!(validate(&too_few_choices).is_err());

        let mut empty_accepted = base.clone();
        if let Question::TypeCommand { accepted, .. } = &mut empty_accepted[0].lessons[0].questions[1] {
            accepted.clear();
        }
        assert!(validate(&empty_accepted).is_err());

        let mut empty_dep = base.clone();
        empty_dep[1].unlock_after = Some("   ".to_string());
        assert!(validate(&empty_dep).is_err());

        let mut self_dep = base.clone();
        self_dep[1].unlock_after = Some(self_dep[1].id.clone());
        assert!(validate(&self_dep).is_err());

        let mut unknown_dep = base.clone();
        unknown_dep[1].unlock_after = Some("no-such-unit".to_string());
        assert!(validate(&unknown_dep).is_err());

        let mut cycle_dep = base.clone();
        cycle_dep[1].unlock_after = Some(cycle_dep[2].id.clone());
        cycle_dep[2].unlock_after = Some(cycle_dep[1].id.clone());
        assert!(validate(&cycle_dep).is_err());

        let mut valid_branch = base.clone();
        valid_branch[2].unlock_after = Some(valid_branch[0].id.clone());
        assert!(!valid_branch[0].is_side_quest());
        assert!(valid_branch[2].is_side_quest());
        assert!(validate(&valid_branch).is_ok());
    }

    #[test]
    fn real_world_labs_and_type_commands_work() {
        let units = load_embedded().unwrap();
        for unit in units.iter().filter(|u| u.number >= 9) {
            for lesson in &unit.lessons {
                for q in &lesson.questions {
                    if let Question::TypeCommand { id, accepted, .. } = q {
                        for cmd in accepted {
                            assert!(
                                crate::learn::grade_command(cmd, accepted),
                                "TypeCommand `{id}` accepted answer `{cmd}` did not self-grade"
                            );
                        }
                    }
                }
            }
        }

        let root = std::env::temp_dir().join(format!("huh-lessons-real-world-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        // Unit 9 practice: u9-game-night-setup
        let u9_lab = units[8].practice.as_ref().expect("unit 9 should have a practice lab");
        assert_eq!(u9_lab.id, "u9-game-night-setup");
        let sb9 = practice::Sandbox::prepare(&root, u9_lab).unwrap();
        assert!(!practice::evaluate(sb9.dir(), u9_lab, None, &[]).unwrap().all_passed());
        std::fs::create_dir(sb9.dir().join("games")).unwrap();
        std::fs::rename(sb9.dir().join("downloads/SpaceQuest.AppImage"), sb9.dir().join("games/SpaceQuest.AppImage"))
            .unwrap();
        std::fs::rename(sb9.dir().join("downloads/saves.txt"), sb9.dir().join("games/saves.txt")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                sb9.dir().join("games/SpaceQuest.AppImage"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
        assert!(practice::evaluate(sb9.dir(), u9_lab, None, &[]).unwrap().all_passed());
        sb9.cleanup().unwrap();

        // Unit 10 practice: u10-downloads-cleanup
        let u10_lab = units[9].practice.as_ref().expect("unit 10 should have a practice lab");
        assert_eq!(u10_lab.id, "u10-downloads-cleanup");
        let sb10 = practice::Sandbox::prepare(&root, u10_lab).unwrap();
        assert!(!practice::evaluate(sb10.dir(), u10_lab, None, &[]).unwrap().all_passed());
        for dir in ["videos", "music", "pictures"] {
            std::fs::create_dir(sb10.dir().join(dir)).unwrap();
        }
        std::fs::rename(sb10.dir().join("inbox/trailer.mp4"), sb10.dir().join("videos/trailer.mp4")).unwrap();
        std::fs::rename(sb10.dir().join("inbox/song.mp3"), sb10.dir().join("music/song.mp3")).unwrap();
        std::fs::rename(sb10.dir().join("inbox/poster.png"), sb10.dir().join("pictures/poster.png")).unwrap();
        std::fs::remove_file(sb10.dir().join("inbox/broken.tmp")).unwrap();
        assert!(practice::evaluate(sb10.dir(), u10_lab, None, &[]).unwrap().all_passed());
        sb10.cleanup().unwrap();

        // Unit 11 practice: u11-sprite-studio
        let u11_lab = units[10].practice.as_ref().expect("unit 11 should have a practice lab");
        assert_eq!(u11_lab.id, "u11-sprite-studio");
        let sb11 = practice::Sandbox::prepare(&root, u11_lab).unwrap();
        assert!(!practice::evaluate(sb11.dir(), u11_lab, None, &[]).unwrap().all_passed());
        std::fs::create_dir(sb11.dir().join("sprites")).unwrap();
        std::fs::copy(sb11.dir().join("templates/penguin.toml"), sb11.dir().join("sprites/penguin.toml")).unwrap();
        std::fs::copy(sb11.dir().join("sprites/penguin.toml"), sb11.dir().join("sprites/penguin.toml.bak")).unwrap();
        let original = std::fs::read_to_string(sb11.dir().join("sprites/penguin.toml")).unwrap();
        std::fs::write(sb11.dir().join("sprites/penguin.toml"), original.replace("#89cff0", "accent")).unwrap();
        assert!(practice::evaluate(sb11.dir(), u11_lab, None, &[]).unwrap().all_passed());
        sb11.cleanup().unwrap();

        let _ = std::fs::remove_dir_all(&root);
    }
}
