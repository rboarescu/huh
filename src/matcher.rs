//! The deterministic matcher: ranks catalog commands for a request, then decides
//! each shortlisted command's fit, flags and slot values.
//!
//! Uses multi-field BM25F (with a precomputed inverted postings index) over
//! normalized stems and canonical Linux/desktop concepts, structural intent
//! disambiguators, domain-coherence fit checks, and syntactic slot-role binding.

use crate::candidates::{Candidates, EntryKind, number_word};
use crate::catalog::{Command, Flag, Kind, Slot, SlotKind};
use crate::platform;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

const FIELD_COUNT: usize = 6;
const FIELD_WEIGHTS: [f64; FIELD_COUNT] = [
    3.0, // name
    2.5, // description
    2.2, // keywords (hook for cataloger's Command.keywords)
    1.4, // slots (what + choice options)
    1.2, // flags (question + id)
    1.0, // template literals
];
const FIELD_B: [f64; FIELD_COUNT] = [0.5, 0.6, 0.5, 0.5, 0.5, 0.4];
const BM25_K1: f64 = 1.2;

/// Entries that fit one specific wording (see `passes_domain_coherence`): they take no part in the
/// ranking when the request does not fit them.
const SPECIFIC_ENTRIES: &[&str] = &[
    "git cherry-pick",
    "git diff refs",
    "git stash push",
    "git show",
    "git revert",
    "lsof",
    "ps top",
    "grep ip addresses",
    "du per folder",
    "find delete dirs",
    "journalctl vacuum time",
    "journalctl vacuum size",
    "docker cp",
    "docker cp from",
    "ssh remote tunnel",
    "ssh-keygen",
    "mount",
    "passwd user",
    "fsck",
    "fsck repair",
    "crontab list",
    "crontab list user",
    "crontab edit",
    "sed replace",
    "ip addr add",
    "ip addr del",
    "nohup",
    "docker compose restart",
    "awk column",
    "ls permissions",
    "dosbox",
    "appimage run",
    "flatpak install",
    "flatpak run",
    "flatpak search",
    "flatpak list",
    "flatpak uninstall",
    "mpv",
];

const STOPWORDS: &[&str] = &[
    "a", "an", "the", "i", "me", "my", "we", "you", "your", "it", "its", "this", "that", "these", "those", "is", "are",
    "was", "were", "be", "been", "being", "do", "does", "did", "can", "could", "would", "should", "will", "shall",
    "may", "might", "want", "need", "like", "please", "to", "of", "in", "on", "at", "for", "from", "with", "and", "or",
    "as", "by", "so", "there", "have", "has", "had", "some", "just", "am", "what", "which", "where", "how", "who",
    "when", "why", "if", "then", "than", "into", "about", "get", "give", "let", "tell",
];

#[derive(Clone, Debug)]
struct IndexedCommand {
    command: Command,
    kind: Kind,
    installed: bool,
    name_lower: String,
    name_words: Vec<String>,
    fields: [HashMap<String, f64>; FIELD_COUNT],
    field_lens: [f64; FIELD_COUNT],
    all_terms: HashSet<String>,
    flag_terms: HashMap<String, HashMap<String, f64>>,
    choice_opt_terms: HashMap<(String, String), HashMap<String, f64>>,
}

pub struct LocalMatcher {
    entries: Vec<IndexedCommand>,
    by_name: BTreeMap<String, usize>,
    vocab: HashMap<String, usize>,
    postings: Vec<Vec<(usize, f64)>>,
}

/// What the matcher decided about one command; `assemble::build` turns it into a line.
#[derive(Debug, Clone, PartialEq)]
pub struct Details {
    /// How well the command fits the request, 0..1.
    pub fit: f64,
    /// How likely each of the command's flags is wanted, 0..1, in catalog order.
    pub flags: Vec<f64>,
    /// What fills each of the command's slots, in catalog order.
    pub slots: Vec<SlotPick>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SlotPick {
    /// A `choice` slot: the index of the picked option.
    Option(usize),
    /// A path or text slot filled from the request; `named` (0..1) is how sure
    /// the matcher is that the request names this value for this slot.
    Value { named: f64, value: String },
    /// A path or text slot the request does not fill.
    Unnamed,
}

impl LocalMatcher {
    pub fn new(catalog: &[Command]) -> Self {
        let mut entries = Vec::with_capacity(catalog.len());
        let mut by_name = BTreeMap::new();
        let mut df: HashMap<String, usize> = HashMap::new();
        let mut total_lens = [0.0f64; FIELD_COUNT];

        for (idx, cmd) in catalog.iter().enumerate() {
            let indexed = index_command(cmd);
            let mut seen = HashSet::new();
            for (f_idx, field) in indexed.fields.iter().enumerate() {
                total_lens[f_idx] += indexed.field_lens[f_idx];
                for term in field.keys() {
                    if seen.insert(term.clone()) {
                        *df.entry(term.clone()).or_insert(0) += 1;
                    }
                }
            }
            by_name.insert(cmd.name.clone(), idx);
            entries.push(indexed);
        }

        let n = catalog.len().max(1) as f64;
        let mut avg_field_lens = [1.0f64; FIELD_COUNT];
        for i in 0..FIELD_COUNT {
            avg_field_lens[i] = (total_lens[i] / n).max(1.0);
        }

        let mut vocab: HashMap<String, usize> = HashMap::with_capacity(df.len());
        let mut postings: Vec<Vec<(usize, f64)>> = Vec::with_capacity(df.len());

        for (term, count) in df {
            let idf = (((n - count as f64 + 0.5) / (count as f64 + 0.5) + 1.0).ln()).max(0.1);
            let tid = postings.len();
            vocab.insert(term.clone(), tid);
            let mut list = Vec::with_capacity(count);

            for (cmd_idx, entry) in entries.iter().enumerate() {
                let mut tf_comb = 0.0f64;
                for f_idx in 0..FIELD_COUNT {
                    if let Some(&tf) = entry.fields[f_idx].get(&term) {
                        let norm =
                            1.0 - FIELD_B[f_idx] + FIELD_B[f_idx] * (entry.field_lens[f_idx] / avg_field_lens[f_idx]);
                        tf_comb += FIELD_WEIGHTS[f_idx] * (tf / norm);
                    }
                }
                if tf_comb > 0.0 {
                    let sat = (tf_comb * (BM25_K1 + 1.0)) / (tf_comb + BM25_K1);
                    list.push((cmd_idx, idf * sat));
                }
            }
            postings.push(list);
        }

        Self { entries, by_name, vocab, postings }
    }

    /// Whether the request asks about the state of the system ("is ssh running", "what's my
    /// volume", "sound isnt working") rather than ordering a change.
    pub fn asks_about_state(&self, request: &str) -> bool {
        let norm = normalize_text(request);
        request_mood(&norm, &raw_lower_words(&norm)) == Mood::Question
    }

    /// Whether the catalog entry of this name changes the system (an action) rather than only
    /// looking at it.
    pub fn is_action(&self, name: &str) -> bool {
        self.by_name.get(name).is_some_and(|&i| self.entries[i].kind == Kind::Action)
    }

    /// Every command of the catalog this matcher was made from.
    pub fn commands(&self) -> impl Iterator<Item = &Command> {
        self.entries.iter().map(|entry| &entry.command)
    }

    /// Whether some catalog command runs a program of this name.
    pub fn knows_program(&self, name: &str) -> bool {
        self.entries.iter().any(|entry| entry.command.program() == name)
    }

    /// Every command in the catalog with its probability, best first (ties by name).
    pub fn rank(&self, request: &str) -> Vec<(&Command, f64)> {
        let q_unquoted = strip_quoted_for_ranking(request);
        let q_terms = extract_weighted_terms(&q_unquoted);
        let q_norm = normalize_text(&q_unquoted);
        let q_words = raw_lower_words(&q_norm);
        let q_ctx = QueryContext::new(request, &q_norm, &q_words, &q_terms);

        // Precompute BM25F scores across all indexed commands in one fast pass.
        let mut bm25_scores = vec![0.0f64; self.entries.len()];
        for (term, &q_weight) in &q_terms {
            if let Some(&tid) = self.vocab.get(term) {
                for &(cmd_idx, contrib) in &self.postings[tid] {
                    bm25_scores[cmd_idx] += contrib * q_weight * 0.55;
                }
            }
        }

        let mut scored: Vec<(&Command, f64)> = self
            .entries
            .iter()
            .zip(&bm25_scores)
            .map(|(entry, bm25)| {
                let intent_bonus =
                    command_intent_bonus(&entry.command, entry.name_lower.as_str(), &entry.name_words, &q_ctx);
                let tie_break = if entry.installed { 0.12 } else { 0.0 };
                // The sysadmin entries answer one specific wording each; their keywords must not lift
                // them above the general command when the request is about something else.
                let gate = if SPECIFIC_ENTRIES.contains(&entry.command.name.as_str())
                    && !passes_domain_coherence(&entry.command, &q_ctx)
                {
                    -50.0
                } else {
                    0.0
                };
                (&entry.command, bm25 + intent_bonus + tie_break + intent_adjust(entry, &q_ctx) + gate)
            })
            .collect();
        // An instruction is not answered with a bare query when an action fits as well.
        if q_ctx.mood == Mood::Instruction && !q_ctx.asks_for_information() {
            let best_action = scored
                .iter()
                .zip(&self.entries)
                .filter(|(_, e)| e.kind == Kind::Action)
                .map(|((_, s), _)| *s)
                .fold(f64::MIN, f64::max);
            for ((_, score), entry) in scored.iter_mut().zip(&self.entries) {
                if entry.kind == Kind::Query && *score <= best_action + INSTRUCTION_MARGIN {
                    *score -= INSTRUCTION_QUERY_PENALTY;
                }
            }
        }
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.name.cmp(&b.0.name)));

        // Softmax over the top 12; the rest share a sliver of probability.
        let top_k = scored.len().min(12);
        let temperature = 2.6;
        let max_s = scored.first().map_or(0.0, |(_, s)| *s);
        let exp_scores: Vec<f64> = scored.iter().take(top_k).map(|(_, s)| ((*s - max_s) / temperature).exp()).collect();
        let sum_exp: f64 = exp_scores.iter().sum();
        let tail_count = scored.len() - top_k;
        let tail_mass = if tail_count > 0 { 0.005 } else { 0.0 };
        let head_scale = (1.0 - tail_mass) / sum_exp.max(1e-9);
        let tail_prob = if tail_count > 0 { tail_mass / tail_count as f64 } else { 0.0 };
        for (i, (_, score)) in scored.iter_mut().enumerate() {
            *score = if i < top_k { exp_scores[i] * head_scale } else { tail_prob };
        }
        // Equal probabilities (the tail) are listed by name.
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.name.cmp(&b.0.name)));
        scored
    }

    /// Fit, flags and slot values of one command for this request. `None` when
    /// the command is not in this matcher's catalog.
    pub fn details(&self, command: &Command, request: &str, candidates: &Candidates) -> Option<Details> {
        let idx = *self.by_name.get(&command.name)?;
        let entry = &self.entries[idx];
        let command = &entry.command;
        let q_norm = normalize_text(request);
        let q_words = raw_lower_words(&q_norm);
        let q_terms = extract_weighted_terms(request);
        let q = QueryContext::new(request, &q_norm, &q_words, &q_terms);

        let fit = self.score_fit(idx, entry, &q);
        let flags: Vec<f64> =
            command.flags.iter().map(|flag| score_flag(&command.name, flag, &entry.flag_terms[&flag.id], &q)).collect();

        // Choice slots first: which values `tar` takes depends on its action.
        let mut slots = vec![SlotPick::Unnamed; command.slots.len()];
        let mut choice_picks: HashMap<String, String> = HashMap::new();
        for (i, slot) in command.slots.iter().enumerate().filter(|(_, s)| s.kind == SlotKind::Choice) {
            let picked = score_choice_slot(&command.name, slot, entry, &q);
            choice_picks.insert(slot.id.clone(), slot.options[picked].name.clone());
            slots[i] = SlotPick::Option(picked);
        }

        let offered: HashMap<String, BTreeSet<&str>> = command
            .slots
            .iter()
            .filter(|s| s.kind != SlotKind::Choice)
            .map(|s| (s.id.clone(), candidates.for_slot(s.kind)))
            .filter(|(_, values)| !values.is_empty())
            .collect();
        if !offered.is_empty() {
            for (slot_id, (named, value)) in bind_slots(command, &offered, &choice_picks, &q, candidates) {
                if let Some(i) = command.slots.iter().position(|s| s.id == slot_id)
                    && !value.is_empty()
                {
                    slots[i] = SlotPick::Value { named, value };
                }
            }
        }
        // Values read from the wording of the request replace whatever the binding picked.
        for (slot, pick) in command.slots.iter().zip(slots.iter_mut()) {
            if let Some(reader) = &slot.extract {
                *pick = match crate::extract::extract(reader, request) {
                    Some(value) => SlotPick::Value { named: 0.95, value },
                    // A choice slot with a reader takes a typed value ("chmod 755") when there
                    // is one, else the option it scored.
                    None if slot.kind == SlotKind::Choice => pick.clone(),
                    None => SlotPick::Unnamed,
                };
            } else if let SlotPick::Value { value, .. } = pick
                && is_port_number(value, &q)
                && slot.what.as_deref().is_some_and(|what| what.contains("process ID"))
            {
                // A number after "port" is a port, never a process ID.
                *pick = SlotPick::Unnamed;
            }
        }
        // A common word that the request does not present as a name ("slow", "synced", "PATH",
        // the command's own name) is not a value: leave the placeholder.
        for (slot, pick) in command.slots.iter().zip(slots.iter_mut()) {
            if slot.extract.is_none()
                && let SlotPick::Value { value, .. } = pick
                && is_stray_value(command, slot, value, candidates, &q)
            {
                *pick = SlotPick::Unnamed;
            }
        }
        let mut flags: Vec<f64> = flags;
        adjust_flags_for_values(command, &slots, candidates, &q, &mut flags);
        // Opening needs something to open: a file, link or folder named in the request or
        // found in the current folder ("open my resume"), not an application name ("open firefox").
        let opens_an_entry = slots.iter().any(|pick| {
            matches!(pick, SlotPick::Value { value, .. }
                if candidates.entry_kind(value.trim_end_matches('/')) != EntryKind::NotEntry)
        });
        let fit = if command.name == "xdg-open" && !names_something_to_open(&q) && !opens_an_entry {
            fit.min(0.14)
        } else {
            fit
        };
        // A value the request spells out for the command ("777") is evidence beyond generic words.
        let typed = command
            .slots
            .iter()
            .zip(&slots)
            .any(|(slot, pick)| slot.kind == SlotKind::Choice && matches!(pick, SlotPick::Value { .. }));
        let fit = if !typed && self.rests_on_generic_words(entry, &q) { fit.min(0.14) } else { fit };
        Some(Details { fit, flags, slots })
    }

    /// The minimum-evidence floor: a match that rests only on generic words of the request
    /// ("order" for `sort`, "make" for `cp`, "last" for `tail`) or on the name of a program that
    /// many entries share ("git" for `git push`) says nothing about what the person wants.
    fn rests_on_generic_words(&self, entry: &IndexedCommand, q: &QueryContext<'_>) -> bool {
        if q.content_terms.len() < 2 {
            return false;
        }
        let shared_program = |term: &String| {
            let term = term.strip_prefix("c_").unwrap_or(term);
            entry.name_words.len() >= 2
                && entry.name_words.first().is_some_and(|w| stem(w) == term && entry.command.program() == w.as_str())
                && self.vocab.get(term).is_some_and(|&id| self.postings[id].len() >= 10)
        };
        // Concepts a generic word brings along ("order" is "sort") are as generic as the word,
        // unless another word or a phrase of the request names the same concept.
        let concepts_of = |generic: bool| -> HashSet<&'static str> {
            q.words
                .iter()
                .filter(|w| GENERIC_WORDS.contains(&stem(w).as_str()) == generic)
                .flat_map(|w| stem_concepts(&stem(w)).into_iter().chain(stem_concepts(w)).flatten().copied())
                .collect()
        };
        let (generic_concepts, other_concepts) = (concepts_of(true), concepts_of(false));
        let from_phrase =
            |term: &str| PHRASE_CONCEPTS.iter().any(|(phrase, c)| *c == term && contains_word_phrase(q.norm, phrase));
        let matched: Vec<&String> = q.term_weights.keys().filter(|t| entry.all_terms.contains(*t)).collect();
        !matched.is_empty()
            && matched.iter().all(|t| {
                GENERIC_WORDS.contains(&t.as_str())
                    || shared_program(t)
                    || (generic_concepts.contains(t.as_str())
                        && !other_concepts.contains(t.as_str())
                        && !from_phrase(t))
            })
    }

    fn score_fit(&self, idx: usize, entry: &IndexedCommand, q: &QueryContext<'_>) -> f64 {
        if is_off_topic_unmodeled(&entry.command, q) {
            return 0.08;
        }
        if !passes_domain_coherence(&entry.command, q) {
            return 0.14;
        }

        let mut matched_terms = 0usize;
        let mut bm25 = 0.0f64;
        for (term, &q_weight) in q.term_weights {
            if entry.all_terms.contains(term) {
                matched_terms += 1;
            }
            if let Some(&tid) = self.vocab.get(term)
                && let Some(&contrib) = self.postings[tid].iter().find(|(ci, _)| *ci == idx).map(|(_, c)| c)
            {
                bm25 += contrib * q_weight * 0.55;
            }
        }
        let bonus = command_intent_bonus(&entry.command, &entry.name_lower, &entry.name_words, q);
        let raw_score = bm25 + bonus + intent_adjust(entry, q) + if entry.installed { 0.12 } else { 0.0 };

        if raw_score >= 5.5 {
            0.95
        } else if raw_score >= 2.8 && matched_terms >= 1 {
            0.92
        } else if raw_score >= 1.5 && matched_terms >= 1 {
            0.84
        } else if raw_score >= 0.9 && matched_terms >= 1 {
            0.65
        } else {
            0.15
        }
    }
}

// ---------------------------------------------------------------------------
// Query Analysis & Normalization
// ---------------------------------------------------------------------------

struct QueryContext<'a> {
    raw: &'a str,
    norm: &'a str,
    words: &'a [String],
    word_set: HashSet<&'a str>,
    stem_set: HashSet<String>,
    term_weights: &'a HashMap<String, f64>,
    content_terms: Vec<String>,
    raw_tokens: Vec<RawToken>,
    mood: Mood,
}

#[derive(Clone, Debug)]
struct RawToken {
    text: String,
    lower: String,
    quoted: bool,
    word_pos: usize,
    prev_word: String,
    next_word: String,
}

impl<'a> QueryContext<'a> {
    fn new(raw: &'a str, norm: &'a str, words: &'a [String], term_weights: &'a HashMap<String, f64>) -> Self {
        let mut word_set = HashSet::with_capacity(words.len());
        let mut stem_set = HashSet::with_capacity(words.len());
        let mut content_terms = Vec::new();
        for w in words {
            word_set.insert(w.as_str());
            let st = stem(w);
            if !STOPWORDS.contains(&w.as_str()) && w.len() >= 2 && !is_generic_filler(w) && !content_terms.contains(&st)
            {
                content_terms.push(st.clone());
            }
            stem_set.insert(st);
        }
        let raw_tokens = parse_raw_tokens(raw);
        let mood = request_mood(norm, words);
        Self { raw, norm, words, word_set, stem_set, term_weights, content_terms, raw_tokens, mood }
    }

    /// "show", "list", "tell me", "find": the request wants something looked up, so a query is
    /// what it asks for even though it is phrased as an instruction.
    fn asks_for_information(&self) -> bool {
        const VERBS: &[&str] = &[
            "show", "list", "display", "print", "tell", "view", "find", "check", "see", "look", "read", "give", "get",
            "which", "what", "who", "where", "how", "whether", "describe", "explain", "count", "compare", "search",
        ];
        self.words.iter().any(|w| VERBS.contains(&w.as_str()))
    }

    /// The request says this word (or phrase) and does not negate it: "put it to sleep" names
    /// sleep, "stop it from going to sleep" does not.
    fn names_unnegated(&self, item: &str) -> bool {
        const NEGATORS: &[&str] =
            &["not", "no", "never", "without", "stop", "prevent", "avoid", "from", "disable", "off", "cancel", "block"];
        if item.contains(' ') {
            return contains_word_phrase(self.norm, item);
        }
        let wanted = stem(item);
        self.words.iter().enumerate().any(|(i, w)| {
            stem(w) == wanted && !self.words[i.saturating_sub(6)..i].iter().any(|b| NEGATORS.contains(&b.as_str()))
        })
    }

    fn has_exact_word(&self, w: &str) -> bool {
        self.word_set.contains(w)
    }

    fn has_word(&self, w: &str) -> bool {
        self.word_set.contains(w) || self.term_weights.contains_key(w) || self.stem_set.contains(&stem(w))
    }

    fn has_phrase(&self, phrase: &str) -> bool {
        if !phrase.contains(' ') {
            return self.has_word(phrase);
        }
        contains_word_phrase(self.norm, phrase)
    }

    fn has_any(&self, list: &[&str]) -> bool {
        list.iter().any(|item| self.has_phrase(item))
    }

    /// "remove htop", "uninstall docker": a removal verb whose only object is a bare word that is
    /// a program or package, not a file in the current folder.
    fn removes_program(&self) -> bool {
        const VERBS: &[&str] = &["remove", "uninstall", "delete", "erase", "purge"];
        const FILLER: &[&str] = &["package", "program", "app", "application", "software", "tool", "command"];
        if !self.words.iter().any(|w| VERBS.contains(&w.as_str())) && !self.has_phrase("get rid of") {
            return false;
        }
        let mut object = self.raw_tokens.iter().filter(|t| {
            !STOPWORDS.contains(&t.lower.as_str())
                && !VERBS.contains(&t.lower.as_str())
                && !FILLER.contains(&t.lower.as_str())
                && !matches!(t.lower.as_str(), "get" | "rid")
                && number_word(&t.lower).is_none()
        });
        let (Some(word), None) = (object.next(), object.next()) else { return false };
        let bare = !word.quoted && word.lower.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        bare && !std::path::Path::new(&word.text).exists()
            && (platform::is_known_program(&word.lower)
                || matches!(platform::locate(&word.lower), platform::Found::OnPath(_)))
    }

    /// "which package owns /usr/bin/ls": asks for the owner of a file, not to search or install.
    fn asks_package_owner(&self) -> bool {
        self.has_word("package")
            && (self.has_any(&[
                "owns",
                "owned by",
                "belongs to",
                "belong to",
                "come from",
                "comes from",
                "installed this file",
                "provided by",
                "owner of",
            ]) || (self.has_absolute_path() && self.has_any(&["which package", "what package"])))
    }

    /// A word that is part of a path ("/usr/bin/ls", "~/notes", "./run.sh").
    fn has_absolute_path(&self) -> bool {
        self.raw_tokens.iter().any(|t| t.text.starts_with('/') || t.text.starts_with("~/") || t.text.starts_with("./"))
    }

    /// A port number is named ("port 3000", "listening on 5432", "localhost:8080").
    fn names_port(&self) -> bool {
        crate::extract::extract("port", self.raw).is_some()
    }

    /// The request is about powering off or restarting this computer (not a service, VM or
    /// device): it names the computer, or says the bare verb ("power off", "reboot").
    fn is_power_request(&self, restart: bool) -> bool {
        const MACHINE: &[&str] = &["computer", "machine", "pc", "laptop", "system", "everything", "workstation"];
        const OTHER: &[&str] = &[
            "vm",
            "virtual",
            "container",
            "service",
            "services",
            "daemon",
            "wifi",
            "bluetooth",
            "screen",
            "display",
            "monitor",
            "touchpad",
            "trackpad",
            "app",
            "program",
            "docker",
            "ollama",
            "tmux",
            "network",
            "audio",
            "sound",
            "light",
            "lights",
            "printer",
            "router",
            "modem",
            "phone",
        ];
        if self.has_any(OTHER) {
            return false;
        }
        let named = self.has_any(MACHINE);
        let bare: &[&str] = if restart {
            &["reboot"]
        } else {
            &["shutdown", "poweroff", "power off", "power down", "shut down", "halt"]
        };
        let verbs: &[&str] = if restart { &["restart"] } else { &["turn off", "switch off"] };
        if self.has_any(bare) {
            // "shut down nginx": a bare verb followed by a name that is not the computer.
            return named || !self.verb_has_object(bare);
        }
        named && self.has_any(verbs)
    }

    /// Whether one of these verbs is followed by a word that is an object, not a time or filler.
    fn verb_has_object(&self, verbs: &[&str]) -> bool {
        const FILLER: &[&str] = &[
            "now",
            "the",
            "my",
            "in",
            "at",
            "by",
            "this",
            "please",
            "immediately",
            "tonight",
            "after",
            "it",
            "down",
            "off",
            "for",
            "on",
            "later",
            "soon",
            "today",
            "tomorrow",
            "and",
        ];
        let words: Vec<&str> = self.words.iter().map(String::as_str).collect();
        for verb in verbs {
            let parts: Vec<&str> = verb.split(' ').collect();
            for (i, w) in words.windows(parts.len()).enumerate() {
                if w == parts.as_slice()
                    && let Some(next) = words.get(i + parts.len())
                    && !FILLER.contains(next)
                    && !is_digits(next)
                {
                    return true;
                }
            }
        }
        false
    }

    /// The request is about getting something back (undo, recover, accidents).
    fn asks_recovery(&self) -> bool {
        self.has_any(&[
            "recover",
            "recovery",
            "undelete",
            "get back",
            "get it back",
            "bring back",
            "by accident",
            "accidentally",
            "by mistake",
            "undo",
            "restore",
            "retrieve",
            "oops",
        ])
    }

    fn has_log_file(&self) -> bool {
        self.raw_tokens.iter().any(|t| t.lower.ends_with(".log"))
    }

    fn has_remote_scp(&self) -> bool {
        self.raw_tokens.iter().any(|t| {
            (t.text.contains('@') && t.text.contains(':'))
                // backup-host:/data, 10.0.0.7:/srv/backups: a host and a path on it.
                || t.text.split_once(':').is_some_and(|(host, path)| {
                    host.len() > 1
                        && !host.contains('/')
                        && !host.eq_ignore_ascii_case("http")
                        && !host.eq_ignore_ascii_case("https")
                        && host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
                        && (path.starts_with('/') || path.starts_with('~'))
                        && !path.starts_with("//")
                })
        })
    }

    fn has_url(&self) -> bool {
        self.raw.contains("http://") || self.raw.contains("https://")
    }

    fn has_domain_token(&self) -> bool {
        self.raw_tokens.iter().any(|t| {
            !t.text.contains('@')
                && (t.lower.ends_with(".com")
                    || t.lower.ends_with(".org")
                    || t.lower.ends_with(".net")
                    || t.lower.ends_with(".io")
                    || t.lower.ends_with(".dev")
                    || t.lower.contains(".com/")
                    || t.lower.contains(".org/"))
        })
    }
}

/// What the request is: a question about the state of the system, or something to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mood {
    Question,
    Instruction,
}

/// Stems of words that appear in many descriptions without saying what the person wants.
const GENERIC_WORDS: &[&str] =
    &["order", "make", "set", "up", "last", "program", "not", "new", "thing", "time", "number", "one"];

/// How far a question about state pushes an action down, and how far an opposite pushes a command down.
const STATE_QUESTION_PENALTY: f64 = 4.0;
const OPPOSITE_PENALTY: f64 = 8.0;
/// An instruction takes a bare query down by this much when an action scores within `MARGIN` of it.
const INSTRUCTION_QUERY_PENALTY: f64 = 1.0;
const INSTRUCTION_MARGIN: f64 = 1.5;

/// Classifies the request by its opening words. Polite and how-to forms ("can you", "please",
/// "how do i", "i want to", "what command") are instructions, as is anything that goes on to ask
/// for a change ("which process uses port 80, and kill it"); "is/are/do i have/what/which/who/
/// where/how much/check if" are questions about state. Everything else is an instruction.
fn request_mood(norm: &str, words: &[String]) -> Mood {
    const FILLERS: &[&str] = &["so", "ok", "okay", "hey", "hi", "and", "also", "well", "um", "huh"];
    const INSTRUCTION_OPENERS: &[&str] = &[
        "can you",
        "could you",
        "would you",
        "will you",
        "can i",
        "could i",
        "can we",
        "please",
        "pls",
        "how do i",
        "how do you",
        "how do we",
        "how to",
        "how can i",
        "how can we",
        "how would i",
        "how should i",
        "how could i",
        "i want",
        "i need",
        "i would like",
        "i wanna",
        "i like to",
        "let us",
        "let me",
        "help me",
        "show me how",
        "tell me how",
        "give me",
        "what command",
        "which command",
        "what is the command",
        "what is the best way",
        "what is the way",
        "what do i type",
        "what do i run",
        "what do i use",
        "what should i run",
        "what should i type",
        "what should i use",
        "what can i use",
        "what can i run",
        "is there a command",
        "is there a way",
        "is there an easy way",
        "is it possible to",
        "what is the easiest way",
        "which tool",
        "what tool",
        "what program",
    ];
    const QUESTION_OPENERS: &[&str] = &[
        "is",
        "are",
        "am",
        "was",
        "were",
        "does",
        "did",
        "has",
        "have",
        "what",
        "which",
        "who",
        "whose",
        "where",
        "when",
        "why",
        "check if",
        "check whether",
        "see if",
        "see whether",
        "find out if",
        "find out what",
        "find out which",
        "tell me if",
        "tell me whether",
        "tell me what",
        "tell me which",
        "tell me who",
        "tell me where",
        "tell me how much",
        "tell me how many",
        "show me what",
        "show me which",
        "show me who",
        "show me where",
        "show me how much",
        "show me how many",
        "let me know",
        "do i",
        "do we",
        "do you",
        "do the",
        "do my",
        "do any",
        "do these",
        "do those",
        "do they",
        "how much",
        "how many",
        "how long",
        "how big",
        "how large",
        "how fast",
        "how hot",
        "how old",
        "how often",
        "how far",
        "how full",
        "how warm",
        "how loud",
        "how high",
        "how low",
        "how is",
        "how are",
        "how does",
    ];
    const CHANGE_VERBS: &[&str] = &[
        "kill",
        "delete",
        "remove",
        "stop",
        "restart",
        "start",
        "install",
        "uninstall",
        "fix",
        "free",
        "enable",
        "disable",
        "turn",
        "switch",
        "close",
        "clean",
        "reset",
        "change",
        "set",
        "make",
        "connect",
        "disconnect",
        "mount",
        "unmount",
        "reboot",
        "run",
        "open",
        "update",
        "upgrade",
        "create",
        "add",
        "move",
        "copy",
        "rename",
    ];
    let mut w: Vec<&str> = words.iter().map(String::as_str).collect();
    while w.len() > 1 && FILLERS.contains(&w[0]) {
        w.remove(0);
    }
    let starts = |phrase: &&str| {
        let p: Vec<&str> = phrase.split(' ').collect();
        w.len() >= p.len() && w[..p.len()] == p[..]
    };
    if INSTRUCTION_OPENERS.iter().any(starts) || w.contains(&"please") {
        return Mood::Instruction;
    }
    // "show permissions of x", "verify the archive", "count the files": an order to look, not to
    // change; ranked like a question so a command that only looks wins.
    const LOOK_OPENERS: &[&str] = &["show", "list", "display", "print", "view", "inspect", "verify", "count"];
    let first = w.first().copied().unwrap_or_default();
    let look_opener = LOOK_OPENERS.contains(&first) || (first == "check" && w.get(1) != Some(&"out"));
    if look_opener && !w.iter().any(|x| CHANGE_VERBS.contains(x)) {
        return Mood::Question;
    }
    if !QUESTION_OPENERS.iter().any(starts) {
        // "sound isnt working": a report of trouble describes the state; it does not order a change.
        if reports_trouble(&w) && !w.iter().any(|x| CHANGE_VERBS.contains(x)) {
            return Mood::Question;
        }
        return Mood::Instruction;
    }
    // A question that goes on to ask for a change is an instruction.
    let continues = |i: usize| w[i.saturating_sub(1)..i].iter().any(|b| matches!(*b, "and" | "then" | "so" | "to"));
    if (3..w.len()).any(|i| CHANGE_VERBS.contains(&w[i]) && continues(i))
        || norm.contains(", ") && w.len() > 5 && w.iter().skip(3).any(|x| CHANGE_VERBS.contains(x))
    {
        return Mood::Instruction;
    }
    Mood::Question
}

/// "my sound is not working", "wifi doesnt connect": a symptom, not a command.
fn reports_trouble(words: &[&str]) -> bool {
    const NEGATIONS: &[&str] = &["not", "isnt", "doesnt", "dont", "wont", "cant", "never", "stopped", "no"];
    const SYMPTOMS: &[&str] = &[
        "working",
        "work",
        "works",
        "responding",
        "loading",
        "connecting",
        "showing",
        "starting",
        "opening",
        "playing",
    ];
    words
        .iter()
        .enumerate()
        .any(|(i, w)| NEGATIONS.contains(w) && words.iter().skip(i + 1).take(2).any(|next| SYMPTOMS.contains(next)))
}

/// What the kind of the command, the mood of the request and the opposites do to the score:
/// a question about state strongly demotes actions, and a command (or option) whose opposite the
/// request names is ruled out.
fn intent_adjust(entry: &IndexedCommand, q: &QueryContext<'_>) -> f64 {
    let mut adjust = 0.0;
    if q.mood == Mood::Question && entry.kind == Kind::Action {
        adjust -= STATE_QUESTION_PENALTY;
    }
    if contradicted(entry, q) {
        adjust -= OPPOSITE_PENALTY;
    }
    adjust
}

/// The request names an opposite of this command, or of the option the matcher would pick.
fn contradicted(entry: &IndexedCommand, q: &QueryContext<'_>) -> bool {
    let command = &entry.command;
    if command.opposites.iter().any(|word| q.names_unnegated(word)) {
        return true;
    }
    command.slots.iter().filter(|slot| slot.kind == SlotKind::Choice).any(|slot| {
        slot.options.iter().any(|o| !o.opposites.is_empty()) && {
            let picked = &slot.options[score_choice_slot(&command.name, slot, entry, q)];
            picked.opposites.iter().any(|word| q.names_unnegated(word))
        }
    })
}

fn strip_quoted_for_ranking(request: &str) -> String {
    let lower = request.to_lowercase();
    let keep_quoted = !contains_word_phrase(&lower, "alias")
        && !contains_word_phrase(&lower, "shortcut")
        && !contains_word_phrase(&lower, "message");
    if keep_quoted || !request.contains(['"', '\'', '`']) {
        return request.to_string();
    }
    let mut out = String::with_capacity(request.len());
    let mut quote: Option<char> = None;
    for c in request.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if matches!(c, '"' | '\'' | '`') => quote = Some(c),
            None => out.push(c),
        }
    }
    out
}

fn contains_word_phrase(haystack: &str, needle: &str) -> bool {
    let mut start = 0usize;
    while let Some(pos) = haystack[start..].find(needle) {
        let abs = start + pos;
        let before_ok =
            abs == 0 || !haystack[..abs].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_');
        let after_idx = abs + needle.len();
        let after_ok = after_idx == haystack.len()
            || !haystack[after_idx..].chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_');
        if before_ok && after_ok {
            return true;
        }
        start = abs + needle.len().max(1);
    }
    false
}

fn is_generic_filler(w: &str) -> bool {
    matches!(w, "thing" | "things" | "stuff" | "something" | "anything" | "one" | "way" | "right" | "now")
}

fn parse_raw_tokens(request: &str) -> Vec<RawToken> {
    let mut items: Vec<(String, bool)> = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;

    for c in request.chars() {
        match quote {
            Some(q) if c == q => {
                let t = current.trim().to_string();
                if !t.is_empty() {
                    items.push((t, true));
                }
                current.clear();
                quote = None;
            }
            Some(_) => current.push(c),
            None if matches!(c, '"' | '\'' | '`') && current.is_empty() => quote = Some(c),
            None if c.is_whitespace() => {
                let t = current.trim_matches([',', ';', ':', '!', '?', '(', ')']).trim_end_matches('.');
                if !t.is_empty() {
                    items.push((t.to_string(), false));
                }
                current.clear();
            }
            None => current.push(c),
        }
    }
    let t = current.trim_matches([',', ';', ':', '!', '?', '(', ')']).trim_end_matches('.');
    if !t.is_empty() {
        items.push((t.to_string(), false));
    }

    let mut out = Vec::with_capacity(items.len());
    for (idx, (text, quoted)) in items.iter().enumerate() {
        let mut prev_idx = idx;
        let mut prev_word = String::new();
        while prev_idx > 0 {
            prev_idx -= 1;
            let cand = items[prev_idx].0.to_lowercase();
            if !matches!(cand.as_str(), "the" | "a" | "an" | "my") {
                prev_word = cand;
                break;
            }
        }
        let next_word = items.get(idx + 1).map(|(s, _)| s.to_lowercase()).unwrap_or_default();
        let lower = text.to_lowercase();
        out.push(RawToken {
            text: text.clone(),
            lower: lower.clone(),
            quoted: *quoted,
            word_pos: idx,
            prev_word: prev_word.clone(),
            next_word: next_word.clone(),
        });
        if !*quoted && let Some(num) = number_word(&lower) {
            out.push(RawToken { text: num.clone(), lower: num, quoted: false, word_pos: idx, prev_word, next_word });
        }
    }
    out
}

fn normalize_text(input: &str) -> Cow<'_, str> {
    let mut s = input.to_lowercase();
    if !s.contains(['\'', '-', '.'])
        && !s.contains("wi fi")
        && !s.contains("screen ")
        && !s.contains("record ")
        && !s.contains("nightlight")
        && !s.contains("readonly")
        && !s.contains("case ")
        && !s.contains("ghz")
    {
        return Cow::Owned(s);
    }
    if s.contains('\'') {
        for (from, to) in [
            ("what's", "what is"),
            ("don't", "do not"),
            ("doesn't", "does not"),
            ("isn't", "is not"),
            ("aren't", "are not"),
            ("can't", "cannot"),
            ("won't", "will not"),
            ("it's", "it is"),
            ("i'm", "i am"),
            ("let's", "let us"),
            ("today's", "today"),
        ] {
            if s.contains(from) {
                s = s.replace(from, to);
            }
        }
    }
    if s.contains('-') || s.contains("wi fi") {
        for (from, to) in [
            ("wi-fi", "wifi"),
            ("wi fi", "wifi"),
            ("do-not-disturb", "do not disturb"),
            ("night-light", "night light"),
            ("read-only", "readonly"),
            ("case-insensitive", "case insensitive"),
            ("screen-shot", "screenshot"),
            ("docker-compose", "docker compose"),
        ] {
            if s.contains(from) {
                s = s.replace(from, to);
            }
        }
    }
    for (from, to) in [
        ("nightlight", "night light"),
        ("night-time", "night"),
        ("readonly", "read only"),
        ("case insensitive", "ignore case"),
        ("screen recording", "screenrecording"),
        ("record video", "screenrecording"),
        ("record my screen", "screenrecording"),
        ("record screen", "screenrecording"),
        ("tar.gz", "targz"),
        ("pkg.tar.zst", "pkgtarzst"),
        ("2.4", "ghz"),
        ("5 ghz", "ghz"),
        ("6 ghz", "ghz"),
    ] {
        if s.contains(from) {
            s = s.replace(from, to);
        }
    }
    Cow::Owned(s)
}

fn raw_lower_words(norm: &str) -> Vec<String> {
    let mut words = Vec::new();
    for token in norm.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '/' && c != '.') {
        if token.contains('/') {
            continue;
        }
        let t = token.trim_matches(['-', '.']);
        if t.is_empty() || (t.contains('.') && !t.starts_with('.')) {
            continue;
        }
        words.push(t.to_string());
        if t.contains('-') {
            for part in t.split('-').filter(|p| !p.is_empty()) {
                words.push(part.to_string());
            }
        }
    }
    words
}

fn stem(word: &str) -> String {
    let w = word.to_lowercase();
    if let Some(irr) = match w.as_str() {
        "running" => Some("run"),
        "stopping" | "stopped" => Some("stop"),
        "frozen" | "hung" => Some("freeze"),
        "written" => Some("write"),
        "made" | "making" => Some("make"),
        "taken" | "taking" => Some("take"),
        "found" => Some("find"),
        "gone" | "going" => Some("go"),
        "directories" => Some("directory"),
        "entries" => Some("entry"),
        "copies" | "copied" | "copying" => Some("copy"),
        "dependencies" => Some("dependency"),
        "repositories" => Some("repository"),
        "processes" => Some("process"),
        "aliases" => Some("alias"),
        "addresses" => Some("address"),
        "branches" => Some("branch"),
        "switches" | "switching" | "switched" => Some("switch"),
        "watches" | "watching" | "watched" => Some("watch"),
        "searches" | "searching" | "searched" => Some("search"),
        "fixes" | "fixing" | "fixed" => Some("fix"),
        "pushes" | "pushing" | "pushed" => Some("push"),
        "creating" | "created" => Some("create"),
        "deleting" | "deleted" => Some("delete"),
        "removing" | "removed" => Some("remove"),
        "changing" | "changed" => Some("change"),
        "moving" | "moved" => Some("move"),
        "saving" | "saved" => Some("save"),
        "using" | "used" => Some("use"),
        "closing" | "closed" => Some("close"),
        "archiving" | "archived" => Some("archive"),
        "comparing" | "compared" => Some("compare"),
        "updating" | "updated" => Some("update"),
        "naming" | "named" => Some("name"),
        "renaming" | "renamed" => Some("rename"),
        "resizing" | "resized" => Some("resize"),
        "configuring" | "configured" => Some("configure"),
        "enabling" | "enabled" => Some("enable"),
        "disabling" | "disabled" => Some("disable"),
        "ignoring" | "ignored" => Some("ignore"),
        "living" => Some("live"),
        "cutting" => Some("cut"),
        "zipping" | "zipped" => Some("zip"),
        "unpacking" | "unpacked" => Some("unpack"),
        "packing" | "packed" => Some("pack"),
        "extracting" | "extracted" => Some("extract"),
        "decompressing" | "decompressed" => Some("decompress"),
        "compressing" | "compressed" => Some("compress"),
        "listening" => Some("listen"),
        "recording" | "recorded" => Some("record"),
        "modified" | "modifying" => Some("modify"),
        "alphabetically" => Some("alphabetical"),
        "numerically" => Some("numerical"),
        "recursively" => Some("recursive"),
        "permanently" => Some("permanent"),
        "automatically" => Some("automatic"),
        "recently" => Some("recent"),
        "locally" => Some("local"),
        "cleanly" => Some("clean"),
        "gracefully" => Some("graceful"),
        _ => None,
    } {
        return irr.to_string();
    }

    if w.len() <= 3 {
        return w;
    }
    if w.ends_with("ies") && w.len() > 4 {
        return format!("{}y", &w[..w.len() - 3]);
    }
    if w.ends_with("ied") && w.len() > 4 {
        return format!("{}y", &w[..w.len() - 3]);
    }
    if (w.ends_with("sses") || w.ends_with("shes") || w.ends_with("ches") || w.ends_with("xes") || w.ends_with("zes"))
        && w.len() > 4
    {
        return w[..w.len() - 2].to_string();
    }
    if w.ends_with("ing") && w.len() >= 6 {
        let base = &w[..w.len() - 3];
        let bytes = base.as_bytes();
        if bytes.len() >= 2
            && bytes[bytes.len() - 1] == bytes[bytes.len() - 2]
            && matches!(bytes[bytes.len() - 1], b'b' | b'g' | b'm' | b'n' | b'p' | b'r' | b't')
        {
            return base[..base.len() - 1].to_string();
        }
        return base.to_string();
    }
    if w.ends_with("ed") && w.len() >= 5 && !matches!(w.as_str(), "need" | "feed" | "seed" | "speed") {
        let base = &w[..w.len() - 2];
        let bytes = base.as_bytes();
        if bytes.len() >= 2
            && bytes[bytes.len() - 1] == bytes[bytes.len() - 2]
            && (matches!(bytes[bytes.len() - 1], b'b' | b'g' | b'm' | b'n' | b'p' | b'r' | b't')
                || (bytes[bytes.len() - 1] == b'd' && base != "add"))
        {
            return base[..base.len() - 1].to_string();
        }
        return base.to_string();
    }
    if w.ends_with("ly") && w.len() >= 5 {
        return w[..w.len() - 2].to_string();
    }
    if w.ends_with('s')
        && !w.ends_with("ss")
        && !w.ends_with("us")
        && !w.ends_with("is")
        && !w.ends_with("os")
        && !w.ends_with("as")
    {
        return w[..w.len() - 1].to_string();
    }
    w
}

/// Maps stems and multi-word phrases into canonical concept tags for matching.
fn extract_weighted_terms(text: &str) -> HashMap<String, f64> {
    let norm = normalize_text(text);
    let words = raw_lower_words(&norm);
    let mut map: HashMap<String, f64> = HashMap::new();

    let mut add = |term: &str, weight: f64| {
        let entry = map.entry(term.to_string()).or_insert(0.0);
        if weight > *entry {
            *entry = weight;
        }
    };

    for w in &words {
        if STOPWORDS.contains(&w.as_str()) {
            continue;
        }
        let st = stem(w);
        if st.len() >= 2 || matches!(st.as_str(), "r" | "x" | "n" | "f" | "a" | "l") {
            add(&st, 1.0);
        }
        if let Some(concepts) = stem_concepts(&st) {
            for &c in concepts {
                add(c, 0.35);
            }
        }
        if let Some(concepts) = stem_concepts(w) {
            for &c in concepts {
                add(c, 0.35);
            }
        }
    }

    for &(phrase, concept) in PHRASE_CONCEPTS {
        let first_word = phrase.split(' ').next().unwrap_or(phrase);
        if norm.contains(first_word) && contains_word_phrase(&norm, phrase) {
            add(concept, 0.55);
        }
    }
    if text.contains(".tar.gz") || text.contains(".tgz") || text.contains(".tar.") {
        add("c_tar", 0.90);
    }
    if text.contains(".zip") && !text.contains("http://") && !text.contains("https://") {
        add("c_zip", 0.90);
    }
    if text.contains(".rar") {
        add("c_rar", 0.90);
    }
    if text.contains(".pkg.tar.zst") {
        add("c_pkg_file", 0.95);
    }
    if text.contains(".csv") {
        add("c_column", 0.60);
    }
    if text.contains('@') && text.contains(':') {
        add("c_remote_copy", 0.90);
    }
    if text.contains("/dev/") {
        add("c_disk_device", 0.85);
    }
    if text.contains("http://") || text.contains("https://") {
        add("c_url", 0.70);
    }
    if contains_word_phrase(&norm, "log in")
        || contains_word_phrase(&norm, "log into")
        || contains_word_phrase(&norm, "log out")
        || contains_word_phrase(&norm, "logged in")
    {
        // "log" in "log in / log into / log out / logged in" is authentication/session, not a system log file.
        if words.iter().filter(|w| *w == "log" || *w == "logs").count() <= 1 {
            map.remove("log");
            map.remove("c_log");
        }
    }

    map
}

const PHRASE_CONCEPTS: &[(&str, &str)] = &[
    ("disk space", "c_storage"),
    ("free space", "c_storage"),
    ("space left", "c_storage"),
    ("disk full", "c_storage"),
    ("most space", "c_largest_items"),
    ("taking up", "c_largest_items"),
    ("get rid of", "c_delete"),
    ("all in one go", "c_parents"),
    ("do not disturb", "c_dnd"),
    ("night light", "c_nightlight"),
    ("stay awake", "c_awake"),
    ("keep awake", "c_awake"),
    ("power saver", "c_power_profile"),
    ("power profile", "c_power_profile"),
    ("top bar", "c_desktop_shell"),
    ("qr code", "c_qr"),
    ("line number", "c_line_numbers"),
    ("ignore case", "c_ignore_case"),
    ("ignoring case", "c_ignore_case"),
    ("any capitalisation", "c_ignore_case"),
    ("any capitalization", "c_ignore_case"),
    ("upper and lower", "c_ignore_case"),
    ("symbolic link", "c_symlink"),
    ("points to", "c_symlink"),
    ("last modified", "c_stat"),
    ("last changed", "c_stat"),
    ("link speed", "c_link_speed"),
    ("kernel module", "c_kernel_module"),
    ("virtual machine", "c_vm"),
    ("virtual network", "c_vm_net"),
    ("key login", "c_ssh_key"),
    ("without password", "c_ssh_key"),
    ("without typing a password", "c_ssh_key"),
    ("passwordless ssh", "c_ssh_key"),
    ("default gateway", "c_gateway"),
    ("local network", "c_lan"),
    ("open port", "c_port"),
    ("environment variable", "c_env_var"),
    ("startup file", "c_shell_rc"),
    ("shell config", "c_shell_rc"),
    ("tokens per second", "c_llm_speed"),
    ("how fast", "c_speed"),
    ("how often", "c_frequency"),
    ("most common", "c_frequency"),
    ("most frequent", "c_frequency"),
    ("duplicate line", "c_uniq"),
    ("repeated line", "c_uniq"),
    ("web app", "c_webapp"),
    ("hidden file", "c_hidden"),
    ("user service", "c_user_service"),
    ("own user service", "c_user_service"),
    ("at boot", "c_boot"),
    ("when the computer boots", "c_boot"),
    ("how do i use", "c_manual"),
    ("manual page", "c_manual"),
    ("saved network connection", "c_saved_conn"),
    ("system snapshot", "c_snapshot"),
    ("force close", "c_force_kill"),
    ("up to date", "c_update"),
    ("plugged in", "c_usb"),
    ("back up", "c_copy"),
];

fn stem_concepts(s: &str) -> Option<&'static [&'static str]> {
    Some(match s {
        "folder" | "directory" | "dir" | "subdir" | "subdirectory" => &["c_folder"],
        "create" | "make" => &["c_create"],
        "delete" | "remove" | "erase" | "rm" | "unlink" | "rid" | "wipe" | "purge" => &["c_delete"],
        "unpack" | "extract" | "decompress" | "untar" | "uncompress" => &["c_unpack"],
        "pack" | "compress" | "archive" | "bundle" | "tarball" => &["c_pack"],
        "storage" | "space" | "capacity" => &["c_storage"],
        "ram" | "memory" | "mem" => &["c_memory"],
        "wallpaper" => &["c_wallpaper"],
        "wifi" | "wireless" | "wlan" | "ssid" => &["c_wifi"],
        "port" | "listen" | "socket" => &["c_port"],
        "kill" | "terminate" | "stop" | "end" | "close" | "halt" => &["c_kill"],
        "freeze" | "hung" | "stuck" | "unresponsive" => &["c_freeze"],
        "search" | "find" | "look" | "grep" | "mention" | "contain" => &["c_search"],
        "rename" | "move" | "mv" | "relocate" => &["c_move"],
        "duplicate" => &["c_copy", "c_uniq"],
        "copy" | "cp" | "clone" => &["c_copy"],
        "runnable" | "executable" | "execute" | "chmod" => &["c_executable", "c_permission"],
        "permission" | "protect" | "private" | "readonly" => &["c_permission"],
        "snapshot" | "snapper" => &["c_snapshot"],
        "screenrecording" | "screenrecord" => &["c_screenrecord"],
        "screenshot" | "snip" => &["c_screenshot"],
        "mute" | "unmute" | "silence" => &["c_mute"],
        "volume" | "sound" | "audio" | "loud" | "louder" | "quiet" | "quieter" | "speaker" | "headphone" => {
            &["c_volume"]
        }
        "brightness" | "dim" | "dimmer" | "bright" | "brighter" | "backlight" => &["c_brightness"],
        "shortcut" | "symlink" | "link" => &["c_symlink"],
        "checksum" | "hash" | "sha256" | "sha256sum" | "verify" | "intact" => &["c_checksum"],
        "process" | "program" => &["c_process"],
        "service" | "daemon" | "unit" | "systemd" => &["c_service"],
        "journald" => &["c_log", "c_service"],
        "log" | "logs" | "journal" | "journalctl" | "output" => &["c_log"],
        "install" => &["c_install"],
        "uninstall" => &["c_uninstall"],
        "upgrade" | "update" => &["c_update"],
        "aur" | "yay" => &["c_aur"],
        "package" | "pkg" => &["c_package"],
        "git" | "repo" | "repository" | "commit" | "branch" | "stage" => &["c_git"],
        "docker" | "container" | "compose" => &["c_docker"],
        "vm" | "virsh" | "libvirt" => &["c_vm"],
        "tmux" | "reattach" | "attach" => &["c_tmux"],
        "tailscale" | "vpn" | "tailnet" => &["c_tailscale"],
        "ollama" | "llm" | "ai" => &["c_ollama"],
        "chat" | "talk" => &["c_chat"],
        "rust" | "cargo" | "crate" | "compile" | "dependency" => &["c_rust"],
        "bashrc" | "zshrc" | "profile" | "reload" | "source" => &["c_shell_rc"],
        "export" | "printenv" | "env" | "variable" => &["c_env_var"],
        "alias" => &["c_alias"],
        "dns" | "cloudflare" | "nameserver" => &["c_dns"],
        "speedtest" | "bandwidth" => &["c_speedtest"],
        "bluetooth" => &["c_bluetooth"],
        "touchpad" | "trackpad" => &["c_touchpad"],
        "awake" | "inhibit" | "idle" | "sleep" => &["c_awake"],
        "reminder" | "remind" | "notification" => &["c_reminder"],
        "webapp" => &["c_webapp"],
        "lock" => &["c_lock"],
        "logout" => &["c_logout"],
        "firmware" | "bios" | "uefi" => &["c_firmware"],
        "gateway" | "route" | "routing" => &["c_gateway"],
        "neigh" | "neighbor" | "neighbour" => &["c_lan"],
        "ping" | "reach" | "reachable" => &["c_ping"],
        "tracepath" | "trace" | "traceroute" => &["c_trace"],
        "download" | "fetch" | "curl" => &["c_download"],
        "ssh" => &["c_ssh"],
        "disk" | "drive" | "partition" | "nvme" | "ssd" | "lsblk" => &["c_disk"],
        "healthy" | "health" | "smart" | "smartctl" => &["c_disk_health"],
        "cpu" | "processor" | "core" | "lscpu" => &["c_cpu"],
        "usb" | "lsusb" => &["c_usb"],
        "kernel" | "dmesg" | "uname" | "driver" | "modprobe" => &["c_kernel"],
        "battery" | "charge" | "upower" => &["c_battery"],
        "calendar" | "cal" | "month" => &["c_calendar"],
        "date" | "today" | "day" => &["c_date"],
        "uptime" => &["c_uptime"],
        "whoami" | "username" => &["c_user"],
        "group" | "groups" | "usermod" => &["c_group"],
        "hostname" => &["c_hostname"],
        "pwd" => &["c_pwd"],
        "history" | "previous" => &["c_history"],
        "clear" => &["c_clear"],
        "passwd" | "password" => &["c_passwd"],
        "man" | "manual" => &["c_manual"],
        "which" => &["c_which"],
        "edit" | "nano" | "editor" => &["c_edit"],
        "scroll" | "less" | "pager" => &["c_scroll"],
        "diff" | "differ" | "different" | "difference" | "compare" => &["c_diff"],
        "sort" | "order" | "alphabetical" | "numerical" => &["c_sort"],
        "uniq" | "repeated" | "frequency" => &["c_uniq"],
        "column" | "field" | "cut" | "delimiter" => &["c_column"],
        "count" | "wc" => &["c_line_count"],
        "first" | "beginning" | "head" => &["c_head"],
        "last" | "tail" | "follow" | "grow" => &["c_tail"],
        "owner" | "chown" => &["c_owner"],
        "btrfs" | "subvolume" => &["c_btrfs"],
        "ethtool" => &["c_link_speed"],
        "ghz" | "band" | "channel" => &["c_wifi_band"],
        "github" | "gh" => &["c_github"],
        "mise" => &["c_mise"],
        "qr" | "barcode" | "zbarimg" => &["c_qr"],
        "theme" => &["c_theme"],
        "font" | "monospace" => &["c_font"],
        "open" | "viewer" => &["c_open_file"],
        "reboot" | "restart" => &["c_restart"],
        "shutdown" | "poweroff" => &["c_shutdown"],
        "print" | "show" | "read" | "write" | "written" | "display" | "content" => &["c_view"],
        "biggest" | "largest" => &["c_largest_items"],
        _ => return None,
    })
}

fn index_command(cmd: &Command) -> IndexedCommand {
    let mut fields: [HashMap<String, f64>; FIELD_COUNT] = Default::default();
    let mut field_lens = [0.0f64; FIELD_COUNT];
    let mut all_terms = HashSet::new();
    let mut flag_terms = HashMap::new();
    let mut choice_opt_terms = HashMap::new();

    let add_text = |f_idx: usize,
                    text: &str,
                    fields: &mut [HashMap<String, f64>; FIELD_COUNT],
                    lens: &mut [f64; FIELD_COUNT],
                    all: &mut HashSet<String>| {
        let map = extract_weighted_terms(text);
        for (term, w) in map {
            *fields[f_idx].entry(term.clone()).or_insert(0.0) += w;
            lens[f_idx] += w;
            all.insert(term);
        }
    };

    add_text(0, &cmd.name, &mut fields, &mut field_lens, &mut all_terms);
    add_text(1, &cmd.description, &mut fields, &mut field_lens, &mut all_terms);
    for kw in &cmd.keywords {
        add_text(2, kw, &mut fields, &mut field_lens, &mut all_terms);
    }
    for slot in &cmd.slots {
        if let Some(what) = &slot.what {
            add_text(3, what, &mut fields, &mut field_lens, &mut all_terms);
        }
        if let Some(q) = &slot.question {
            add_text(3, q, &mut fields, &mut field_lens, &mut all_terms);
        }
        for opt in &slot.options {
            let opt_text = format!("{} {} {}", opt.name.replace('_', " "), opt.description, opt.value);
            choice_opt_terms.insert((slot.id.clone(), opt.name.clone()), extract_weighted_terms(&opt_text));
            add_text(3, &opt.name.replace('_', " "), &mut fields, &mut field_lens, &mut all_terms);
            add_text(3, &opt.description, &mut fields, &mut field_lens, &mut all_terms);
        }
    }
    for flag in &cmd.flags {
        let flag_text = format!("{} {} {}", flag.question, flag.id.replace('_', " "), flag.value);
        flag_terms.insert(flag.id.clone(), extract_weighted_terms(&flag_text));
        add_text(4, &flag.id.replace('_', " "), &mut fields, &mut field_lens, &mut all_terms);
        add_text(4, &flag.question, &mut fields, &mut field_lens, &mut all_terms);
    }
    let tmpl_clean: String = cmd
        .template
        .split_whitespace()
        .filter(|w| !w.starts_with('{') && !w.contains('{'))
        .collect::<Vec<_>>()
        .join(" ");
    add_text(5, &tmpl_clean, &mut fields, &mut field_lens, &mut all_terms);

    let name_lower = cmd.name.to_lowercase();
    let name_words = raw_lower_words(&normalize_text(&cmd.name));

    IndexedCommand {
        command: cmd.clone(),
        kind: cmd.kind(),
        installed: cmd.available(),
        name_lower,
        name_words,
        fields,
        field_lens,
        all_terms,
        flag_terms,
        choice_opt_terms,
    }
}

// ---------------------------------------------------------------------------
// Ranking: Intent & Disambiguation Bonus
// ---------------------------------------------------------------------------

// Hand-tuned keyword tables read best one rule per line.
#[rustfmt::skip]
fn command_intent_bonus(_cmd: &Command, name_lower: &str, name_words: &[String], q: &QueryContext<'_>) -> f64 {
    let mut bonus = 0.0f64;
    let is_meta_man = q.has_any(&["how do i use", "manual page", "manual"]);
    let is_meta_which = (q.has_phrase("where is") && q.has_word("installed"))
        || q.has_phrase("path to")
        || (q.has_word("installed") && q.has_word("where"));
    let has_url = q.has_url() || q.has_domain_token();
    let has_upper_env = q.raw_tokens.iter().any(|t| is_upper_env_name(&t.text));
    let has_is_running = q.has_exact_word("is") && q.has_exact_word("running");

    if !is_meta_man && !is_meta_which && !name_words.is_empty() {
        let matched = name_words.iter().filter(|nw| q.has_exact_word(nw)).count();
        if matched == name_words.len() && name_words.len() >= 2 {
            bonus += 5.0;
        } else if matched == 1 && name_words.len() == 1 && name_words[0].len() >= 3
            && !(name_lower == "date" && q.has_any(&["dates", "up to date"]))
        {
            bonus += 3.0;
        }
    }

    let kills = q.has_any(&["kill", "stop", "terminate", "free", "free up", "release", "close", "end"]);
    let asks_cancel = q.has_any(&["cancel", "abort"]) && q.has_any(&["shutdown", "reboot", "power off", "restart", "shut down", "poweroff"]);
    let age_phrase = q.has_any(&["older than", "more than", "days old", "weeks old", "not modified", "not changed", "stale"])
        && q.has_any(&["day", "days", "week", "weeks", "month", "months"]);
    let delete_verb = q.has_any(&["delete", "remove", "clean", "clean up", "purge", "erase", "get rid of", "wipe"]);
    // "delete all .log files in logs": files matching a pattern inside a folder, never the folder itself.
    let pattern_delete = delete_verb && crate::extract::extract("file_glob", q.raw).is_some_and(|g| g.contains('*'))
        && q.has_any(&["files", "file"]);
    let installs = q.has_any(&["install", "uninstall", "package", "remove"]);
    let dns_lookup = !installs && q.has_any(&["dns lookup", "dns record", "dns records", "mx record", "mx records", "dig", "nameserver", "nameservers", "txt record", "resolve the domain", "look up the domain"])
        || (q.has_exact_word("mx") && q.has_domain_token());
    let who_else = q.has_any(&["who is logged in", "who's logged in", "users logged in", "logged in users", "who is online"]);
    let power_machine = q.is_power_request(true) || q.is_power_request(false);
    let port_check = q.names_port() && q.has_any(&["open", "reachable", "reach", "closed", "accessible", "responding", "check if"]) && crate::extract::extract("host", q.raw).is_some();
    let git_words = q.has_any(&["git", "repo", "repository", "branch", "commit", "commits", "staged"]);
    let docker_words = q.has_any(&["container", "containers", "image", "images", "compose"]);

    let compose = q.has_word("compose") || q.raw.contains("compose");
    let journal_vacuum = journal_vacuum_kind(q);
    let account_named = names_other_account(q);
    let interface_change = changes_interface_address(q);
    let shows_permissions = lists_permissions(q);

    match name_lower {
        // Sysadmin requests: the commands the generic words alone get wrong.
        "git cherry-pick" if q.has_any(&["cherry-pick", "cherry pick", "cherrypick"]) => bonus += 16.0,
        "git cherry-pick"
            if has_commit_hash(q)
                && q.has_word("commit")
                && q.has_any(&["apply", "copy", "bring", "port", "take", "pick"])
                && q.has_any(&["branch", "onto", "here", "current"]) =>
        {
            bonus += 14.0
        }
        "git show" if has_commit_hash(q) && q.has_word("commit") && q.has_any(&["show", "display", "see", "view", "inspect", "what"]) => bonus += 14.0,
        "git revert" if q.has_exact_word("revert") && (q.has_word("commit") || has_commit_hash(q)) => bonus += 14.0,
        "git diff refs" if names_two_refs(q) => bonus += 16.0,
        "git stash push" if stash_with_message(q) => bonus += 16.0,
        "git rebase" if asks_squash(q) => bonus += 14.0,
        "git reset" if undoes_a_commit(q) => bonus += 14.0,
        "lsof" if asks_open_files(q) => bonus += 16.0,
        "uptime" if (q.has_phrase("how long") && q.has_exact_word("up")) || q.has_any(&["been up", "system up", "server up", "machine up"]) => bonus += 10.0,
        "free" if q.has_any(&["memory", "ram"]) && q.has_any(&["human readable", "human-readable", "megabytes", "gigabytes", "in mb", "in gb"]) && !asks_process_ranking(q) => bonus += 12.0,
        "ps top" if asks_process_ranking(q) => bonus += 16.0,
        "grep ip addresses" if asks_ip_in_file(q) => bonus += 16.0,
        "du per folder" if asks_per_folder_sizes(q) => bonus += 16.0,
        "find delete dirs" if asks_delete_named_dirs(q) => bonus += 16.0,
        "journalctl vacuum time" if journal_vacuum == Some(true) => bonus += 17.0,
        "journalctl vacuum size" if journal_vacuum == Some(false) => bonus += 17.0,
        "docker cp" if copies_with_container(q) && !copies_out_of_container(q) => bonus += 17.0,
        "docker cp from" if copies_with_container(q) && copies_out_of_container(q) => bonus += 17.0,
        "ssh remote tunnel" if asks_reverse_tunnel(q) => bonus += 16.0,
        "ssh tunnel" if asks_reverse_tunnel(q) => bonus += 0.0,
        "ssh-keygen" if generates_ssh_key(q) => bonus += 16.0,
        "ssh-copy-id" if generates_ssh_key(q) => bonus += 0.0,
        "mount" if mounts_device_on_folder(q) => bonus += 16.0,
        "passwd user" if q.has_any(&["password", "passwd"]) && account_named && !q.has_any(&["wifi", "ssh", "key", "file", "zip", "archive", "pdf", "database"]) => bonus += 15.0,
        "passwd" if account_named => bonus += 0.0,
        "fsck" if asks_fsck(q) && !asks_fsck_repair(q) => bonus += 16.0,
        "fsck repair" if asks_fsck_repair(q) => bonus += 17.0,
        "crontab list user" if lists_cron(q) && account_named => bonus += 16.0,
        "crontab list" if lists_cron(q) && !account_named => bonus += 16.0,
        "crontab edit" if asks_schedule(q) => bonus += 16.0,
        "sed replace" if asks_replace_in_file(q) => bonus += 16.0,
        "ip addr add" if interface_change == Some(true) => bonus += 16.0,
        "ip addr del" if interface_change == Some(false) => bonus += 16.0,
        "nohup" if asks_nohup(q) => bonus += 16.0,
        "docker compose restart" if compose && q.has_any(&["restart", "reload", "bounce"]) => bonus += 16.0,
        "awk column" if asks_awk_column(q) => bonus += 14.0,
        "cut" if asks_awk_column(q) => bonus += 0.0,
        "chmod" if shows_permissions => bonus += 0.0,
        "ls permissions" if shows_permissions => bonus += 16.0,
        "ls" if shows_permissions => bonus += 0.0,
        "reboot" if q.is_power_request(true) && !asks_cancel => bonus += 8.0,
        "shutdown" if q.is_power_request(false) && !asks_cancel => bonus += 8.0,
        "shutdown cancel" if asks_cancel => bonus += 14.0,
        "systemctl failed" if q.has_any(&["failed", "failing", "broken", "crashed"]) && q.has_any(&["service", "services", "unit", "units", "daemon", "daemons"]) => bonus += 13.0,
        "systemctl status" if !power_machine && !git_words && !docker_words
            && ((q.has_word("status") && !q.has_any(&["tailscale", "battery", "wifi", "network", "omarchy", "vpn", "raid", "user service"]))
                || (has_is_running && q.has_any(&["service", "daemon", "unit"]))) => bonus += 9.5,
        "find delete" if delete_verb && (age_phrase || pattern_delete) => bonus += 13.0,
        "format drive" if (q.has_any(&["disk", "drive", "usb", "stick", "partition", "sd card", "ssd", "nvme", "hdd", "pendrive", "flash drive"]) || q.raw.contains("/dev/")) && q.has_any(&["format", "reformat", "mkfs"]) => bonus += 14.0,
        "wipe drive" if (q.has_any(&["disk", "drive", "usb", "stick", "partition", "sd card", "ssd", "nvme", "hdd", "pendrive", "flash drive"]) || q.raw.contains("/dev/")) && q.has_any(&["wipe", "erase", "clear", "wipefs"]) && !q.has_any(&["zero", "zeros", "securely", "secure"]) => bonus += 14.0,
        "zero drive" if (q.has_any(&["disk", "drive", "usb", "stick", "partition", "sd card", "ssd", "nvme", "hdd", "pendrive", "flash drive"]) || q.raw.contains("/dev/")) && q.has_any(&["zero", "zeros", "securely", "secure", "overwrite"]) => bonus += 14.0,
        // A mode for "all files in X" must not reach the folders: `chmod -R 644` makes them unopenable.
        "find chmod files" if (q.has_any(&["chmod", "permission", "permissions", "mode", "readable", "writable", "executable"])
                || crate::extract::extract("chmod_mode", q.raw).is_some())
            && q.has_any(&["all files", "every file", "each file", "files in", "files under", "files inside", "only files", "files only", "only the files", "regular files"]) => bonus += 14.0,
        "find chmod dirs" if (q.has_any(&["chmod", "permission", "permissions", "mode", "readable", "writable", "executable"])
                || crate::extract::extract("chmod_mode", q.raw).is_some())
            && q.has_any(&["all directories", "all folders", "every directory", "every folder", "directories in", "folders in", "directories under", "folders under", "only directories", "only folders", "directories only", "folders only", "subfolders", "subdirectories"]) => bonus += 14.0,
        "kill port" if kills && q.names_port() && !q.has_any(&["listening", "what", "which", "who"]) => bonus += 14.0,
        "kill port" if q.has_phrase("already in use") => bonus += 14.0,
        "ss port" if q.names_port() && !port_check && q.has_any(&["listening", "listens", "using", "uses", "used", "what", "which", "who", "program", "process"]) && !kills => bonus += 14.0,
        "nc" if port_check => bonus += 14.0,
        "git restore" if q.has_any(&["discard", "throw away", "revert", "undo", "drop", "restore"]) && q.has_any(&["changes", "edits", "modifications", "uncommitted", "local"]) && !q.has_any(&["commit", "commits", "stash", "all local", "everything"]) => bonus += 13.0,
        "git stash" if q.has_any(&["stash", "stashes", "stashed"]) => bonus += 13.0,
        "git reset" if q.has_exact_word("uncommit") || q.has_any(&["undo the last commit", "undo last commit", "reset hard", "hard reset", "reset to the last commit", "reset to last commit", "discard all local", "discard all changes", "throw away all", "undo the previous commit"]) => bonus += 13.0,
        "git clean" if q.has_word("untracked") && delete_verb => bonus += 13.0,
        "docker image prune" if q.has_any(&["dangling", "untagged", "unused", "prune"]) && q.has_any(&["image", "images"]) => bonus += 14.0,
        "docker container prune" if q.has_any(&["stopped", "exited", "unused", "prune", "dead"]) && q.has_any(&["container", "containers"]) && (delete_verb || q.has_word("prune")) => bonus += 14.0,
        "docker system df" if q.has_word("docker") && q.has_any(&["disk usage", "space used", "how much space", "storage usage", "size", "disk space used"]) && !delete_verb && !q.has_any(&["reclaim", "free", "prune"]) => bonus += 13.0,
        "gio trash empty" if q.has_any(&["trash", "recycle bin"]) && q.has_any(&["empty", "clear", "clean out", "purge", "wipe", "delete everything"]) => bonus += 14.0,
        "gio list trash" if q.asks_recovery() && q.has_any(&["delete", "deleted", "remove", "removed", "file", "files", "trash", "folder", "notes", "lost"]) => bonus += 14.0,
        "cd previous" if q.has_any(&["previous folder", "previous directory", "previous dir", "last folder", "last directory", "go back to", "back to where i was", "cd -"]) => bonus += 13.0,
        "who" if who_else => bonus += 14.0,
        "iostat" if q.has_any(&["io", "i/o", "iops", "iostat", "disk activity", "disk throughput", "disk performance", "disk speed", "io wait"]) => bonus += 13.0,
        "useradd" if q.has_any(&["create user", "add user", "new user", "create a user", "add a user", "make a user", "create an account", "add a new user"]) && !q.has_word("group") => bonus += 13.0,
        "tracepath" if q.has_any(&["trace route", "traceroute", "trace the route", "tracepath", "hops"]) => bonus += 14.0,
        "dig" if dns_lookup => bonus += 14.0,
        "curl headers" if q.has_any(&["headers", "header"]) && (has_url || q.has_domain_token()) => bonus += 14.0,
        "man" if is_meta_man => bonus += 9.5,
        "which" if is_meta_which => bonus += 9.5,
        "df" if (q.has_any(&["free", "out of", "left"]) && q.has_any(&["disk", "space", "storage", "full"])) || q.has_phrase("disk full") || (q.has_any(&["almost", "nearly"]) && q.has_word("full") && q.has_any(&["disk", "drive", "storage", "space"])) => bonus += 7.5,
        "stat" if q.mood == Mood::Question && !q.has_any(&["package", "packages"]) && q.has_any(&["permissions", "permission", "owner", "owns", "who owns"]) && q.raw_tokens.iter().any(|t| looks_like_regular_file(&t.text)) => bonus += 10.0,
        "echo shell" if q.has_exact_word("shell") && q.mood == Mood::Question && !q.has_any(&["reload", "config", "alias", "variable", "script", "change", "set", "switch", "bash", "zsh", "fish"]) => bonus += 12.0,
        "echo path" if q.has_exact_word("path") && !q.has_any(&["value of", "environment", "printenv", "file", "folder", "directory", "to", "add", "put", "append", "export", "set", "remove"]) => bonus += 12.0,
        "du" if q.has_any(&["how big", "how large", "space", "size"])
            && (q.has_any(&["folder", "directory", "file", "this folder"]) || q.raw_tokens.iter().any(|t| looks_like_regular_file(&t.text)))
            && !q.has_any(&["free", "most space", "taking up", "largest", "biggest", "out of", "permissions"]) => bonus += 7.0,
        "largest items" if q.has_any(&["most space", "taking up", "largest", "biggest"])
            && !q.has_any(&["biggest to smallest", "order", "sort"]) => bonus += 8.5,
        "free" if q.has_word("ram") || (q.has_word("memory") && q.has_any(&["free", "much", "use"]) && !q.has_any(&["docker", "container", "vm", "virtual", "model", "ollama"])) => bonus += 7.0,
        "smartctl" if q.raw.contains("/dev/") || (q.has_any(&["disk", "drive", "ssd", "nvme"]) && q.has_any(&["health", "healthy", "smart"])) => bonus += 7.5,
        "lsblk" if q.has_any(&["disk", "disks", "partition", "partitions", "block device", "block devices"]) && !q.has_any(&["health", "healthy", "free", "space"]) => bonus += 6.0,
        "stat" if q.has_any(&["when was", "last modified", "last changed", "facts about", "metadata"])
            || (q.has_word("size") && q.has_word("permissions") && q.raw_tokens.iter().any(|t| looks_like_regular_file(&t.text))) => bonus += 8.0,
        "file" if q.has_any(&["what type of file", "what kind of file", "type of file", "kind of file"]) => bonus += 8.0,
        "ls" if q.has_any(&["who owns", "owner of", "file owner", "owned by", "who is the owner"]) && !q.has_any(&["package", "pacman", "dpkg", "rpm"]) => bonus += 9.0,
        "wpctl set-volume" if crate::extract::extract("percent", q.raw).is_some() && q.has_any(&["volume", "sound", "audio"]) => bonus += 9.0,
        "omarchy brightness set" if crate::extract::extract("percent", q.raw).is_some() && q.has_any(&["brightness", "backlight", "bright", "dim"]) => bonus += 9.0,
        "userdel" if q.has_any(&["user", "account"]) && q.has_any(&["delete", "remove", "erase", "get rid of"]) && !q.has_any(&["group", "package", "file"]) => bonus += 16.0,
        "usermod lock" if q.has_any(&["user", "account"]) && q.has_any(&["lock", "block"]) && !q.has_any(&["unlock", "screen"]) => bonus += 9.0,
        "git branch delete" if q.has_any(&["delete", "remove", "drop", "get rid of"]) && q.has_any(&["branch", "branches"]) => bonus += 9.0,
        "git rebase" if q.has_any(&["rebase"]) => bonus += 9.0,
        "ssh tunnel" if q.has_any(&["tunnel", "port forward", "forward a port", "forward local port", "forward port"]) => bonus += 9.0,
        "last" if q.has_any(&["logged in recently", "recent logins", "login history", "last logins", "who logged in", "previous logins"]) && !q.has_any(&["currently", "right now", "who is logged in", "who's logged in"]) => bonus += 9.0,
        "wifi current" if q.has_any(&["wifi name", "name of the wifi", "wifi i am on", "wifi am i on", "wifi network name", "network name"]) => bonus += 9.0,
        "package list" if q.has_any(&["what is installed", "what is installed on", "everything installed", "all installed"]) => bonus += 9.0,
        "umount" if q.has_any(&["unmount", "umount"]) && q.raw_tokens.iter().any(|t| ["/mnt", "/media", "/run/media", "~/", "/srv", "/home"].iter().any(|p| t.text.starts_with(p))) => bonus += 9.0,
        "package installed" if q.has_exact_word("installed") && !q.has_exact_word("where") && !q.has_any(&["packages", "everything", "all", "list"]) => bonus += 8.0,
        "bluetoothctl info" if q.has_any(&["connected"]) && q.has_any(&["is", "are", "am"]) && q.has_any(&["bluetooth", "headphones", "headset", "earbuds", "airpods", "speaker"]) => bonus += 9.0,
        "run script" if q.has_any(&["run", "execute", "launch", "start"])
            && q.raw_tokens.iter().any(|t| [".sh", ".py", ".bash", ".pl", ".rb"].iter().any(|ext| t.lower.ends_with(ext))) => bonus += 9.0,
        "wifi current" if q.has_any(&["connected to", "am i on", "am i connected", "current wifi", "which wifi", "what wifi", "which network", "what network", "my ssid", "current ssid"]) => bonus += 8.0,
        "bluetooth status" if q.has_any(&["is bluetooth on", "is bluetooth enabled", "is bluetooth working", "bluetooth powered", "bluetooth status", "bluetooth state"]) => bonus += 12.0,
        "ls" if (q.has_any(&["list", "contents", "what is in", "what files are in", "files in", "hidden", "dotfile", "dotfiles", "sizes and dates", "changed most recently"])
            || (q.has_word("show") && q.has_any(&["dotfiles", "hidden", "permissions"])))
            && !q.has_any(&["container", "image", "branch", "snapshot", "subvolume", "tmux", "vm", "virtual", "theme", "font", "wifi", "disk", "partition", "usb", "ollama", "tar", "archive", "dump"])
            && !q.raw_tokens.iter().any(|t| looks_like_regular_file(&t.text)) => bonus += 8.0,
        "find" if q.has_any(&["show", "list", "which", "what"]) && q.has_any(&["file", "files"]) && crate::extract::extract("file_age", q.raw).is_some() && !q.has_any(&["permission", "permissions", "chmod", "protect"]) => bonus += 7.0,
        "find" if q.has_any(&["find", "locate", "where is", "look for"])
            && q.has_any(&["file", "files", "folder", "folders", "directory", "directories", "named", "bigger", "changed", "modified"])
            && !is_meta_which && !q.has_any(&["inside", "mention", "contain", "search inside", "look through", "ignoring case", "install"]) && !q.raw.contains('"') => bonus += 6.0,
        "grep" if (q.has_any(&["search", "which line", "which lines", "every line", "mention", "contain", "look through", "find the word"]) || q.raw.contains('"'))
            && !q.has_any(&["service", "package", "aur", "alias", "shortcut", "commit", "flatpak", "flatpaks", "flathub"])
            && (!q.has_any(&["log", "logs"]) || q.has_log_file()) => bonus += 9.5,
        "search service log" if q.has_any(&["search", "look for", "grep"]) && q.has_any(&["log", "logs"]) && !q.has_log_file() => bonus += 9.5,
        "journalctl" if q.has_any(&["log", "logs", "log lines", "journald", "journal"])
            && !q.has_log_file()
            && !q.has_any(&["search", "look for", "for failed", "in lines", "log in", "log into", "logged in", "log out", "log me out", "logout", "logged out"])
            && !((q.has_word("container") || q.has_phrase("docker logs")) && !q.has_any(&["journald", "journal", "systemd"])) => bonus += 8.5,
        "docker logs" if q.has_any(&["log", "logs"]) && (q.has_word("container") || q.has_phrase("docker logs")) && !q.has_any(&["journald", "journal", "systemd"]) => bonus += 10.5,
        "head" if q.has_any(&["first", "beginning", "top"]) && q.has_any(&["line", "lines", "of"]) => bonus += 7.5,
        "tail" if (q.has_any(&["last", "keep watching", "as new lines", "growing", "follow"]) || q.has_log_file())
            && !q.has_any(&["service", "sshd", "system log", "container", "journald", "journal", "docker", "grep", "search", "which line", "every line", "in lines"]) => bonus += 6.5,
        "less" if q.has_any(&["scroll", "huge", "page through"]) => bonus += 7.0,
        "cat" if (q.has_any(&["written in", "whole contents", "line numbers", "print", "dump"])
            || (q.has_any(&["contents", "inside"]) && q.raw_tokens.iter().any(|t| looks_like_regular_file(&t.text))))
            && !q.has_any(&["first", "last", "column", "scroll", "find", "search", "tar", "zip", "archive"]) => bonus += 9.0,
        "nano" if (q.has_any(&["edit", "editor", "nano"]) || (q.has_word("change") && q.raw_tokens.iter().any(|t| looks_like_regular_file(&t.text))))
            && !q.has_any(&["chmod", "modified", "stop", "prevent", "protect"]) && !has_upper_env => bonus += 6.5,
        "wc" if q.has_any(&["how many", "count", "number of", "how long"]) && q.has_any(&["lines", "words", "characters"]) && !q.has_any(&["how often", "occurs", "each line"]) => bonus += 8.5,
        "uniq" if q.has_any(&["duplicate", "repeated", "how often", "how many times", "occurs", "appear", "most common", "frequency"]) => bonus += 8.5,
        "sort" if q.has_any(&["sort", "order", "alphabetical", "alphabetically", "numerical", "numerically", "biggest to smallest"]) && !q.has_any(&["duplicate", "repeated", "how often", "most space"]) => bonus += 8.0,
        "cut" if q.has_any(&["column", "columns", "field", "fields", "delimiter"]) => bonus += 8.5,
        "diff" if q.has_any(&["different", "difference", "diff", "compare"]) => bonus += 7.5,
        // "how big is x.tar.gz" asks for a size: that is du, not tar.
        "tar" if (q.raw.contains(".tar") || q.raw.contains("tar.gz") || q.raw.contains(".tgz") || q.has_word("tarball")) && !q.has_remote_scp()
            && !q.has_any(&["how big", "how large", "size of", "file size", "how much space"]) => bonus += 9.0,
        "unzip" if q.raw.contains(".zip") && !has_url && !q.has_remote_scp() && q.has_any(&["extract", "unpack", "unzip", "decompress", "open"]) => bonus += 9.5,
        "zip" if !has_url && !q.has_remote_scp() && q.has_any(&["zip up", "into", "pack", "compress", "create", "make"]) && (q.raw.contains(".zip") || q.has_word("zip")) => bonus += 9.5,
        "unrar" if q.raw.contains(".rar") => bonus += 9.5,
        "package install" if q.has_exact_word("install")
            && !q.has_any(&["flatpak", "flatpaks", "flathub"])
            && q.raw_tokens.iter().any(|t| !t.quoted && t.lower != "install" && platform::is_known_program(&t.lower)) => bonus += 12.0,
        "package remove" if q.removes_program() && !q.has_any(&["flatpak", "flatpaks", "flathub"]) => bonus += 9.0,
        "package owns file" if q.asks_package_owner() => bonus += 9.0,
        "package provides program" if q.has_word("package")
            && q.has_any(&["provides", "provide", "contains", "has the command", "for the command"])
            && !q.has_absolute_path() => bonus += 5.0,
        "gunzip" if q.raw.contains(".gz") && !q.raw.contains(".tar.gz") && !has_url && q.has_any(&["decompress", "unpack", "extract", "gunzip"]) => bonus += 9.5,
        "gzip" if q.has_any(&["compress", "gzip"]) && !q.raw.contains(".zip") && !q.raw.contains(".tar") => bonus += 8.0,
        "sha256sum" if q.has_any(&["checksum", "sha256", "hash", "verify", "intact"]) => bonus += 9.0,
        "xdg-open" if q.has_any(&["open", "viewer", "default app"]) && !q.has_any(&["port", "shell", "web app", "webapp", "nobody", "change", "terminal", "every time"]) => bonus += 6.0,
        "zbarimg" if q.has_any(&["qr", "barcode"]) => bonus += 9.5,
        "chmod" if (q.has_any(&["sticky", "executable", "runnable", "as a program", "permission", "permissions", "chmod", "nobody but me", "protect", "read only", "modified"])
            || (q.has_any(&["stop", "prevent"]) && q.has_word("editing")) || crate::extract::extract("chmod_mode", q.raw).is_some())
            && !q.has_any(&["rust", "docker", "container", "ollama", "frozen", "dotfiles"])
            && !(q.has_any(&["show", "list", "which", "what"]) && crate::extract::extract("file_age", q.raw).is_some()) => bonus += 9.0,
        "chown" if q.has_any(&["owner", "ownership", "chown"]) => bonus += 8.5,
        "ln" if q.has_any(&["shortcut", "symlink", "symbolic link", "points to"]) && !q.has_word("alias") && !q.raw.contains('"') => bonus += 8.0,
        "cp" if q.has_any(&["copy", "duplicate", "back up", "backup"]) && !q.has_any(&["clipboard", "screenshot", "remote", "server", "over the network", "snapshot"]) && !q.has_remote_scp() => bonus += 7.0,
        "mv" if q.has_any(&["move", "rename", "put"]) && q.has_any(&["to", "into"]) && !q.has_any(&["branch", "theme", "new directory", "trash", "recycle bin"]) && !q.raw.contains(".zip") && !q.raw.contains(".tar") => bonus += 9.0,
        "gio trash" if q.has_any(&["trash", "recycle bin", "discard"]) && !q.has_any(&["permanently", "permanent", "empty"]) && !q.asks_recovery() => bonus += 9.5,
        "rm" if q.has_any(&["delete", "remove", "erase", "get rid of"]) && !q.has_any(&["container", "image", "model", "duplicate", "uninstall", "package", "branch", "vm", "trash", "recycle bin"]) && !age_phrase && !pattern_delete && !q.asks_recovery() => bonus += 6.5,
        "mkdir" if q.has_any(&["create", "make", "new"]) && q.has_any(&["folder", "folders", "directory", "directories"]) && !q.has_any(&["shortcut", "archive", "zip", "copy"]) => bonus += 8.0,
        "touch" if q.has_any(&["empty file", "new file", "create an empty file"]) => bonus += 8.5,
        "pwd" if q.has_any(&["which directory", "which folder", "where am i", "current directory", "current folder"]) || (q.has_word("what") && q.has_any(&["folder", "directory"]) && q.has_word("this")) || q.has_any(&["folder am i in", "directory am i in", "folder i am in", "directory i am in"]) => bonus += 9.0,
        "cd" if q.has_any(&["go to", "change directory", "change into", "enter folder"]) => bonus += 8.5,
        "ps" if q.has_any(&["programs are running", "running processes", "what programs"]) && !q.has_any(&["cpu", "memory", "live"]) => bonus += 7.0,
        "top" if q.has_any(&["eating my cpu", "most processor", "most memory", "cpu right now"]) => bonus += 9.5,
        "pgrep" if has_is_running && !q.has_any(&["service", "container", "tmux", "out of"]) => bonus += 7.5,
        "kill" if q.has_any(&["kill", "stop", "terminate"]) && q.raw_tokens.iter().any(|t| is_digits(&t.text)) && !q.has_word("tmux") && !q.names_port() => bonus += 8.5,
        "pkill" if q.has_any(&["frozen", "force close", "kill", "close", "refuses to stop"])
            && !q.raw_tokens.iter().any(|t| is_digits(&t.text))
            && !q.has_any(&["tmux", "container", "vm", "window"]) => bonus += 8.0,
        "systemctl --user" if q.has_phrase("user service") || q.has_phrase("--user") => bonus += 9.5,
        "systemctl" if (q.has_any(&["service", "boot", "enable", "disable"]) || has_is_running)
            && !q.has_any(&["user service", "log", "logs", "vm", "virtual", "failed"]) => bonus += 7.0,
        "systemctl" if q.has_any(&["restart", "stop", "start"]) && !q.has_any(&["user service", "log", "logs", "vm", "virtual", "container", "docker", "compose", "tmux", "wifi", "bluetooth", "audio", "sound", "shell", "terminal", "trackpad", "touchpad", "bar", "hyprland", "ollama", "frozen", "process"])
            && !power_machine && q.raw_tokens.iter().any(|t| looks_like_service_name(&t.lower)) => bonus += 9.0,
        "systemd-inhibit" if q.has_any(&["until", "finishes", "command"]) && q.has_any(&["sleep", "awake"]) => bonus += 8.0,
        "omarchy toggle idle" if q.has_any(&["awake", "idle", "sleep"]) && !q.has_any(&["until", "finishes"]) => bonus += 8.5,
        "omarchy bluetooth power" if q.has_word("bluetooth") && q.has_any(&["turn", "on", "off", "power"]) && !q.has_word("boot") && !q.has_word("restart") => bonus += 8.5,
        "omarchy restart" if q.has_any(&["restart", "acting up", "stopped working", "fix it", "misbehaving"]) && !q.has_word("service")
            && q.has_any(&["wifi", "sound", "audio", "bluetooth", "top bar", "shell", "bar", "terminal", "trackpad", "touchpad"])
            && !q.has_word("service") => bonus += 7.5,
        "omarchy theme bg next" if q.has_word("wallpaper") || (q.has_word("background") && !q.has_phrase("in the background") && !q.has_any(&["container", "image", "service"])) => bonus += 9.0,
        "omarchy theme list" if q.has_word("theme") && q.has_any(&["which", "list", "choose", "available", "what"]) => bonus += 9.0,
        "omarchy theme set" if q.has_word("theme") && q.has_any(&["switch", "change", "set", "use", "apply"]) => bonus += 8.5,
        "omarchy font list" if q.has_word("font") && q.has_any(&["what", "which", "list", "available"]) => bonus += 9.0,
        "omarchy font set" if q.has_word("font") && q.has_any(&["use", "set", "change", "switch"]) => bonus += 8.5,
        "omarchy capture screenshot" if q.has_any(&["screenshot", "screen shot"]) => bonus += 9.5,
        "omarchy capture screenrecording" if q.has_any(&["screenrecording", "screen record", "record my screen", "record screen"]) => bonus += 9.5,
        "omarchy snapshot create" if q.has_word("snapshot") && q.has_any(&["take", "create", "make"]) => bonus += 9.0,
        "snapper list" if q.has_any(&["snapshot", "snapshots"]) && q.has_any(&["what", "list", "exist"]) => bonus += 8.5,
        "btrfs subvolume list" if q.has_any(&["btrfs", "subvolume", "subvolumes"]) => bonus += 9.5,
        "omarchy update firmware" if q.has_any(&["firmware", "bios", "uefi"]) => bonus += 9.5,
        "omarchy update" if q.has_word("omarchy") && q.has_any(&["update", "up to date"]) && !q.has_any(&["firmware", "bios"]) => bonus += 9.0,
        "omarchy version" if q.has_word("omarchy") && q.has_word("version") => bonus += 9.5,
        "omarchy network speedtest" if q.has_any(&["speedtest", "speed test", "bandwidth", "mbps", "how fast", "upload", "download speed"]) => bonus += 9.0,
        "omarchy dns" if q.has_any(&["dns", "cloudflare", "1.1.1.1", "8.8.8.8"]) && !dns_lookup && !q.has_any(&["trace route", "traceroute"]) => bonus += 9.0,
        "omarchy audio output volume" if q.has_any(&["volume", "louder", "quieter", "mute", "unmute"]) => bonus += 9.0,
        "omarchy brightness display" if q.has_any(&["brightness", "dim", "dimmer", "brighter"]) => bonus += 9.0,
        "omarchy powerprofiles set" if q.has_any(&["power saver", "power profile", "balanced", "performance mode"]) => bonus += 9.5,
        "omarchy reminder" if q.has_any(&["remind", "reminder"]) => bonus += 9.5,
        "omarchy launch webapp" if q.has_any(&["web app", "webapp"]) => bonus += 9.5,
        "omarchy toggle nightlight" if q.has_phrase("night light") || q.has_word("nightlight") => bonus += 9.5,
        "omarchy toggle notification silencing" if q.has_phrase("do not disturb") || q.has_word("notifications") => bonus += 9.5,
        "omarchy toggle touchpad" if q.has_any(&["touchpad", "trackpad"]) => bonus += 9.5,
        "omarchy system lock" if q.has_word("lock") && q.has_any(&["screen", "computer", "laptop", "pc"]) => bonus += 9.5,
        "omarchy system logout" if q.has_any(&["log out", "logout", "log me out"]) => bonus += 9.5,
        "omarchy pkg aur add" if q.has_word("omarchy") && q.has_word("aur") && q.has_any(&["install", "add", "get"]) => bonus += 12.0,
        "package install" if q.has_word("omarchy") && q.has_any(&["install", "add", "get"]) && !q.has_word("aur") => bonus += 10.5,
        "package remove" if q.has_word("omarchy") && q.has_any(&["uninstall", "remove", "drop"]) => bonus += 10.5,
        "pacman install file" if q.raw.contains(".pkg.tar.zst") => bonus += 9.5,
        "package install" if q.has_word("install") && !q.has_any(&["aur", "omarchy", "where is", ".pkg.tar.zst", "look for", "is there", "flatpak", "flatpaks", "flathub"]) => bonus += 6.0,
        "package remove" if q.has_any(&["uninstall", "remove the package", "remove package"]) && !q.has_word("omarchy") && !q.has_any(&["flatpak", "flatpaks", "flathub"]) => bonus += 7.5,
        "system update" if q.has_any(&["update all", "upgrade", "all my packages", "update system", "up to date"]) && !q.has_word("omarchy") => bonus += 8.0,
        "package search" if (q.has_any(&["is there a package", "package for", "available to install"]) || (q.has_any(&["look for", "search for"]) && q.has_any(&["package", "install", "reader", "program", "app"]))) && !q.has_word("aur") && !q.has_any(&["flatpak", "flatpaks", "flathub"]) => bonus += 10.5,
        "yay install" if q.has_word("aur") && q.has_any(&["install", "get", "add"]) && !q.has_word("omarchy") => bonus += 9.5,
        "yay search" if q.has_word("aur") && q.has_any(&["search", "find", "look"]) => bonus += 9.5,
        "flatpak install" if q.has_any(&["flatpak", "flathub"]) && q.has_any(&["install", "add", "get"]) => bonus += 12.0,
        "flatpak search" if q.has_any(&["flatpak", "flathub"]) && q.has_any(&["search", "find", "look"]) => bonus += 11.5,
        "flatpak run" if q.has_any(&["flatpak", "flathub"]) && q.has_any(&["run", "launch", "start", "open"]) => bonus += 11.0,
        "flatpak list" if q.has_any(&["flatpak", "flatpaks"]) && q.has_any(&["list", "installed", "show"]) => bonus += 11.0,
        "flatpak uninstall" if q.has_any(&["flatpak", "flathub"]) && q.has_any(&["uninstall", "remove", "delete"]) => bonus += 11.5,
        "add shell alias" if q.has_word("alias") || (q.has_word("shortcut") && q.raw.contains('"')) => bonus += 11.5,
        "add to PATH" if q.has_any(&["to my path", "to path"]) || (q.raw.contains("PATH") && q.has_word("add")) => bonus += 9.5,
        "set shell variable" if q.has_any(&["export", "permanently", "every time", "environment variable"])
            || (q.has_any(&["set", "point"]) && has_upper_env) => bonus += 9.5,
        "printenv" if q.has_any(&["value of", "environment variable", "environment variables"]) && !q.has_any(&["set", "export", "permanently", "point"]) => bonus += 8.0,
        "reload shell config" if q.has_any(&["reload", "source"]) && q.has_any(&["bashrc", "zshrc", "shell", "config", "profile"]) => bonus += 9.5,
        "add user to group" if q.has_word("group") && q.has_any(&["add", "join", "be in"]) => bonus += 9.5,
        "groups" if q.has_any(&["which groups", "what groups", "groups am i"]) => bonus += 9.0,
        "ssh-copy-id" if q.has_any(&["without typing a password", "without password", "passwordless", "key login", "ssh key"]) => bonus += 10.0,
        "ssh" if (q.has_any(&["connect to the server", "log in", "log into", "ssh"]) || (q.raw.contains('@') && !q.has_remote_scp()))
            && !q.has_any(&["without", "passwordless", "copy", "key", "service", "github"])
            && !q.has_exact_word("logs")
            && !q.has_log_file() => bonus += 10.0,
        "scp" if q.has_remote_scp() && !q.has_any(&["sync", "rsync", "mirror", "incremental"]) => bonus += 12.0,
        "rsync" if !is_meta_man && !is_meta_which && q.has_any(&["sync", "rsync", "mirror", "incremental"]) => bonus += 13.5,
        "battery status" if q.has_word("battery") && !q.has_any(&["power saver", "save battery"]) => bonus += 9.0,
        "wifi band" if q.has_any(&["ghz", "band", "channel"]) && q.has_word("wifi") => bonus += 9.5,
        "ip neigh" if q.has_any(&["devices on my network", "local network", "arp", "neighbors"]) => bonus += 9.0,
        "ip route get" if !q.has_any(&["trace route", "traceroute", "trace the route", "hops"]) && (q.raw_tokens.iter().any(|t| looks_like_ip(&t.text)) || q.has_any(&["which interface", "to reach"])) && q.has_any(&["route", "interface", "path"]) => bonus += 9.5,
        "ip route" if q.has_any(&["gateway", "routed", "routes", "routing table", "network routes"]) && !q.raw_tokens.iter().any(|t| looks_like_ip(&t.text)) => bonus += 9.5,
        "ip" if q.has_any(&["network interfaces", "ip address", "my ip"]) && !q.has_any(&["tailscale", "vm", "gateway", "route", "routes"]) => bonus += 7.5,
        "ss" if q.has_any(&["open", "listening"]) && q.has_any(&["port", "ports"]) => bonus += 9.0,
        "ethtool" if q.has_phrase("link speed") => bonus += 9.5,
        "modprobe" if q.has_any(&["kernel module", "modprobe"]) => bonus += 9.5,
        "mise upgrade" if q.has_word("mise") => bonus += 9.5,
        "gh auth login" if (q.has_word("github") || q.has_word("gh")) && q.has_any(&["log in", "login", "auth", "authenticate"]) => bonus += 9.5,
        "ping" if q.has_any(&["reachable", "ping", "connected to the internet", "internet connection", "am i online", "internet working"]) && !q.has_word("tailscale") => bonus += 8.0,
        "tailscale ping" if q.has_word("tailscale") && q.has_any(&["reach", "ping"]) => bonus += 9.5,
        "tailscale ip" if q.has_word("tailscale") && q.has_word("ip") => bonus += 9.5,
        "tailscale up" if q.has_word("tailscale") && q.has_any(&["connect", "up", "start"]) => bonus += 9.0,
        "tailscale down" if q.has_any(&["tailscale", "tailnet"]) && q.has_any(&["disconnect", "down", "stop", "off"]) => bonus += 9.5,
        "tailscale status" if q.has_any(&["tailscale", "tailnet"]) && q.has_any(&["online", "devices", "status", "show", "network"]) => bonus += 9.0,
        "nmcli wifi list" if q.has_any(&["wifi", "wireless"]) && q.has_any(&["nearby", "available", "list", "what", "scan"]) => bonus += 9.0,
        "nmcli wifi connect" if q.has_any(&["wifi", "wireless"]) && q.has_any(&["connect", "join"]) => bonus += 9.0,
        "nmcli connection show" if q.has_phrase("saved") && q.has_word("connection") => bonus += 9.5,
        "git status" if q.has_any(&["what files did i change", "git status", "changed in this repo"]) => bonus += 8.0,
        "git add" if q.has_exact_word("stage") => bonus += 10.0,
        "git commit" if (q.has_exact_word("commit") || q.has_exact_word("staged")) && !q.has_exact_word("stage") && !q.has_any(&["push", "history", "log", "since", "merge", "undo", "uncommit", "reset", "show", "see", "view", "diff", "last", "list"]) => bonus += 8.0,
        "git push" if q.has_word("push") && q.has_any(&["commit", "commits", "git", "origin", "upstream", "branch", "force", "remote"]) => bonus += 8.5,
        "git pull" if q.has_any(&["latest changes", "pull"]) && q.has_any(&["remote", "git", "pushed"]) => bonus += 9.0,
        "git clone" if (q.has_word("clone") || q.has_word("repo")) && (has_url || q.has_word("repo")) => bonus += 9.0,
        "git log" if q.has_any(&["commit history", "git log"])
            || (q.has_any(&["commits", "commit"]) && q.has_any(&["show", "list", "last", "recent", "latest", "see", "view"]) && !q.has_any(&["undo", "uncommit", "revert", "reset", "discard", "amend", "push"])) => bonus += 9.0,
        "git diff" if q.has_any(&["uncommitted", "git diff"])
            || (q.has_any(&["staged", "unstaged"]) && q.has_any(&["show", "see", "view", "diff", "look", "what", "list", "display"])) => bonus += 8.5,
        "git switch" if (q.has_word("branch") && q.has_any(&["create", "new", "switch", "checkout"]))
            || (q.has_any(&["switch to", "checkout", "go to"]) && q.has_any(&["main", "master", "develop", "dev", "trunk"])) => bonus += 8.5,
        "git branch" if q.has_any(&["list all branches", "branches", "list branches"]) => bonus += 8.5,
        "docker ps" if q.has_any(&["container", "containers"]) && q.has_any(&["list", "running", "all", "stopped", "up"]) && !q.has_any(&["stats", "memory", "terminal", "shell", "open"]) => bonus += 8.5,
        "docker images" if q.has_word("docker") && q.has_any(&["image", "images"]) && q.has_any(&["what", "list", "have"]) => bonus += 8.0,
        "docker compose up" if (q.has_word("compose") || q.raw.contains("compose")) && q.has_any(&["up", "bring up", "start"]) => bonus += 10.5,
        "docker compose down" if (q.has_word("compose") || q.raw.contains("compose")) && q.has_any(&["down", "tear down", "stop"]) => bonus += 10.5,
        "docker exec" if q.has_word("container") && q.has_any(&["shell", "terminal"]) && q.has_any(&["inside", "exec", "running"]) => bonus += 9.5,
        "docker system prune" if q.has_word("docker") && q.has_any(&["unused", "prune", "free space", "clean up", "reclaim"]) => bonus += 9.5,
        "docker stats" if q.has_word("container") && q.has_any(&["memory", "cpu", "stats"]) => bonus += 9.0,
        "docker run" if q.has_any(&["image", "container"]) && (q.has_exact_word("run") || q.has_any(&["launch", "start a new", "create a new"])) && !q.has_any(&["running container", "running containers", "in the running", "already running", "compose"]) && !q.raw.contains("compose") => bonus += 9.0,
        "docker rmi" if q.has_word("image") && q.has_any(&["remove", "delete"]) && !q.has_any(&["dangling", "untagged", "unused", "prune"]) => bonus += 8.5,
        "docker rm" if q.has_word("container") && q.has_any(&["remove", "delete"]) => bonus += 8.5,
        "docker pull" if q.has_word("image") && q.has_any(&["pull", "download", "fetch"]) => bonus += 9.0,
        "docker start" if q.has_word("container") && q.has_word("start") && !q.has_word("restart") && !q.has_word("compose") && !q.raw.contains("compose") => bonus += 8.0,
        "docker stop" if q.has_word("container") && q.has_any(&["stop", "halt"]) && !q.has_exact_word("stopped") => bonus += 8.5,
        "docker restart" if q.has_word("container") && q.has_word("restart") => bonus += 8.5,
        "virsh net-list" if q.has_any(&["virtual network", "virtual networks"]) && q.has_any(&["show", "list"]) => bonus += 9.5,
        "virsh net-start" if q.has_any(&["virtual network", "virtual networks"]) && q.has_word("start") => bonus += 9.5,
        "virsh list" if q.has_any(&["virtual machine", "virtual machines", "vm", "vms"]) && q.has_any(&["list", "show all"]) => bonus += 9.0,
        "virsh domiflist" if q.has_any(&["network card", "network cards", "interfaces"]) && q.has_any(&["vm", "virtual machine"]) => bonus += 9.5,
        "virsh domifaddr" if q.has_word("ip") && q.has_any(&["vm", "virtual machine"]) => bonus += 9.5,
        "virsh dominfo" if q.has_any(&["memory", "cpu", "cpus", "details"]) && q.has_any(&["vm", "virtual machine"]) => bonus += 9.5,
        "virsh autostart" if q.has_any(&["vm", "virtual machine"]) && (q.has_any(&["autostart", "host boots", "at boot"]) || q.has_exact_word("boots")) => bonus += 9.5,
        "virsh destroy" if q.has_any(&["vm", "virtual machine"]) && q.has_any(&["force", "hard", "kill", "hung", "destroy", "pull"]) => bonus += 10.5,
        "virsh undefine" if q.has_any(&["vm", "virtual machine"]) && q.has_any(&["delete", "undefine", "definition"]) => bonus += 9.5,
        "virsh console" if q.has_any(&["vm", "virtual machine"]) && q.has_word("console") => bonus += 9.5,
        "virsh shutdown" if q.has_any(&["vm", "virtual machine"]) && q.has_any(&["shut down", "shutdown", "gracefully", "turn off", "cleanly"]) && !q.has_any(&["force", "hard", "kill", "hung", "destroy"]) => bonus += 9.5,
        "virsh reboot" if q.has_any(&["vm", "virtual machine"]) && q.has_word("reboot") => bonus += 9.5,
        "virsh start" if q.has_any(&["vm", "virtual machine"]) && q.has_any(&["boot", "start", "power on"]) => bonus += 9.0,
        "ollama run" if (q.has_any(&["ollama", "model", "chat", "talk"]) || q.has_phrase("tokens per second")) && q.has_any(&["chat", "talk", "run", "fast", "tokens per second"]) => bonus += 9.0,
        "ollama stop" if q.has_any(&["unload", "stop", "free"]) && q.has_any(&["model", "memory", "ollama", "unload"]) => bonus += 9.0,
        "ollama pull" if (q.has_word("ollama") || q.has_word("model")) && q.has_any(&["download", "pull", "fetch"]) => bonus += 9.0,
        "ollama rm" if (q.has_word("ollama") || q.has_word("model")) && q.has_any(&["delete", "remove"]) => bonus += 8.5,
        "ollama ps" if q.has_word("ollama") && q.has_any(&["loaded", "running", "right now"]) => bonus += 8.5,
        "ollama list" if q.has_word("ollama") && q.has_any(&["which", "list", "have"]) => bonus += 8.0,
        "ollama serve" if q.has_word("ollama") && q.has_any(&["server", "serve", "start"]) => bonus += 8.5,
        "tmux new" if q.has_word("tmux") && q.has_any(&["new", "open", "start"]) && !q.has_any(&["running", "which", "list"]) => bonus += 8.5,
        "tmux attach" if q.has_word("tmux") && q.has_any(&["attach", "reattach"]) => bonus += 9.0,
        "tmux ls" if q.has_word("tmux") && q.has_any(&["list", "ls", "running", "which"]) && !q.has_any(&["new", "attach", "reattach", "kill", "end"]) => bonus += 9.0,
        "tmux kill-session" if q.has_word("tmux") && q.has_any(&["kill", "end"]) => bonus += 9.0,
        "cargo build" if q.has_any(&["rust", "cargo"]) && q.has_any(&["compile", "build"]) && !q.has_word("run") => bonus += 8.5,
        "cargo run" if q.has_any(&["rust", "cargo"]) && q.has_word("run") && !q.has_word("test") => bonus += 8.0,
        "cargo test" if q.has_any(&["rust", "cargo"]) && q.has_any(&["test", "tests"]) => bonus += 9.0,
        "cargo new" if q.has_any(&["rust", "cargo"]) && q.has_any(&["new", "create"]) => bonus += 8.5,
        "cargo add" if q.has_any(&["rust", "cargo", "dependency", "crate"]) && q.has_any(&["add", "crate", "dependency"]) => bonus += 9.0,
        "curl" if (has_url || q.has_any(&["web address", "page at", "curl"])) && !q.has_any(&["clone", "git", "web app", "webapp", "model", "ollama", "is curl", "image"]) => bonus += 7.5,
        "history" if q.has_any(&["previous commands", "command history", "typed earlier", "what did i type", "typed before", "commands i ran", "commands i typed"]) => bonus += 8.5,
        "clear" if q.has_any(&["clear", "clean up"]) && q.has_word("screen") && !q.has_any(&["lock", "screenshot"]) => bonus += 7.5,
        "passwd" if q.has_word("password") && q.has_any(&["change", "my"]) && !q.has_any(&["without", "passwordless", "ssh", "find", "grep"]) => bonus += 8.0,
        "hostname" if q.has_any(&["machine called", "computer called", "hostname", "computer's name"]) => bonus += 8.5,
        "whoami" if q.has_any(&["logged in", "which user", "who am i", "whoami", "username"]) && !who_else => bonus += 10.5,
        "uptime" if q.has_any(&["uptime", "been on", "running since"]) || (q.has_phrase("how long") && q.has_any(&["reboot", "boot", "on", "running"])) => bonus += 9.0,
        "date" if q.has_any(&["date", "today", "what day"]) && !q.has_any(&["dates", "sizes and dates", "update", "up to date"]) => bonus += 8.0,
        "cal" if q.has_word("calendar") => bonus += 9.0,
        _ => {}
    }
    bonus
}

// ---------------------------------------------------------------------------
// Details: Fit Check
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Sysadmin requests: predicates shared by the bonus table and the coherence table
// ---------------------------------------------------------------------------

/// A git commit hash as people type it: 7 to 40 hex digits with at least one digit and one letter.
fn is_commit_hash(s: &str) -> bool {
    (7..=40).contains(&s.len())
        && s.chars().all(|c| c.is_ascii_hexdigit())
        && s.chars().any(|c| c.is_ascii_digit())
        && s.chars().any(|c| c.is_ascii_lowercase() || c.is_ascii_uppercase())
}

fn has_commit_hash(q: &QueryContext<'_>) -> bool {
    q.raw_tokens.iter().any(|t| is_commit_hash(&t.text))
}

/// "stash my work with the message wip-auth": a stash that gets a name.
fn stash_with_message(q: &QueryContext<'_>) -> bool {
    q.has_any(&["stash", "stashes", "stashed"])
        && q.has_any(&["message", "msg", "named", "called", "label", "labelled", "labeled", "description", "comment"])
        && !q.has_any(&["pop", "apply", "list", "drop"])
}

/// A branch, tag or commit name as opposed to a file.
fn looks_like_git_ref(s: &str) -> bool {
    let lower = s.to_lowercase();
    if matches!(
        lower.as_str(),
        "main"
            | "master"
            | "develop"
            | "dev"
            | "trunk"
            | "head"
            | "staging"
            | "prod"
            | "production"
            | "release"
            | "stable"
    ) || lower.starts_with("origin/")
        || lower.starts_with("head~")
        || lower.starts_with("head^")
        || lower.starts_with("release")
        || lower.starts_with("feature/")
        || lower.starts_with("hotfix")
    {
        return true;
    }
    // A version tag: v1.0, v2.3.1.
    let mut chars = lower.chars();
    (chars.next() == Some('v') && chars.next().is_some_and(|c| c.is_ascii_digit())) || is_commit_hash(s)
}

/// "diff between main and develop": two git names, not two files.
fn names_two_refs(q: &QueryContext<'_>) -> bool {
    // "show commits between a and b" lists commits (git log), it does not diff.
    if q.has_any(&["commits", "log", "history"])
        && !q.has_any(&["diff", "difference", "compare", "changes", "changed", "differ"])
    {
        return false;
    }
    let git_words = q.has_any(&["git", "branch", "branches", "commit", "commits", "tag", "tags", "repo", "repository"]);
    crate::readers::ref_pair(q.raw).is_some_and(|(a, b)| {
        let plain = |s: &str| !looks_like_regular_file(s) || looks_like_git_ref(s);
        (looks_like_git_ref(&a) && looks_like_git_ref(&b)) || (git_words && plain(&a) && plain(&b))
    })
}

/// "list open files of pid 1234", "what process has app.log open".
fn asks_open_files(q: &QueryContext<'_>) -> bool {
    q.has_any(&["open files", "open file", "files open", "files opened", "lsof"])
        || (q.has_exact_word("open")
            && q.has_any(&["process", "processes", "program", "programs", "who", "what"])
            && q.has_any(&["has", "have", "using", "uses", "holding", "holds"])
            && q.raw_tokens.iter().any(|t| looks_like_regular_file(&t.text) || t.text.contains('/')))
}

/// "top 10 processes by memory": a fixed list of the heaviest programs, not a live monitor.
fn asks_process_ranking(q: &QueryContext<'_>) -> bool {
    q.has_any(&["process", "processes", "program", "programs"])
        && q.has_any(&["memory", "cpu", "ram", "processor"])
        && (crate::extract::extract("count", q.raw).is_some()
            || q.has_any(&["by memory", "by cpu", "by ram", "by memory usage", "by cpu usage"]))
        && q.has_any(&["top", "most", "biggest", "highest", "heaviest", "hungry", "hogs", "largest"])
        && !q.has_any(&["live", "watch", "monitor", "real time", "interactive"])
}

/// "unique ip addresses in app.log": addresses pulled out of a file.
fn asks_ip_in_file(q: &QueryContext<'_>) -> bool {
    q.has_any(&["ip addresses", "ip address", "ips", "ip adresses"])
        && q.has_any(&["in", "from", "inside", "of"])
        && q.has_any(&["show", "list", "extract", "find", "unique", "distinct", "get", "pull", "all", "print"])
        && q.raw_tokens.iter().any(|t| looks_like_regular_file(&t.text))
}

/// "size of each folder in /var": one line per subfolder.
fn asks_per_folder_sizes(q: &QueryContext<'_>) -> bool {
    q.has_any(&["each", "every", "per"])
        && q.has_any(&[
            "folder",
            "folders",
            "directory",
            "directories",
            "subfolder",
            "subfolders",
            "subdirectory",
            "subdirectories",
            "dir",
            "dirs",
        ])
        && q.has_any(&["size", "sizes", "space", "usage", "disk", "big", "bigger", "large", "much", "takes"])
        && !q.has_any(&["delete", "remove", "clean"])
}

/// A folder name that people delete everywhere at once.
fn is_throwaway_dir_name(lower: &str) -> bool {
    matches!(
        lower,
        "node_modules"
            | "__pycache__"
            | "pycache"
            | ".cache"
            | "build"
            | "dist"
            | "target"
            | ".venv"
            | "venv"
            | ".pytest_cache"
            | ".gradle"
            | "vendor"
            | ".next"
            | ".tox"
    )
}

/// "remove node_modules folders recursively": delete every folder of one name below here.
fn asks_delete_named_dirs(q: &QueryContext<'_>) -> bool {
    q.has_any(&["delete", "remove", "clean", "clean up", "purge", "erase", "get rid of", "wipe"])
        && (q.has_any(&["folders", "directories", "dirs", "folder", "directory"])
            || q.raw_tokens.iter().any(|t| matches!(t.lower.as_str(), "node_modules" | "__pycache__" | "pycache")))
        && q.has_any(&[
            "recursively",
            "everywhere",
            "all",
            "every",
            "anywhere",
            "below",
            "under",
            "nested",
            "inside all",
        ])
        && q.raw_tokens.iter().any(|t| is_throwaway_dir_name(&t.lower))
        && (["folders", "directories", "dirs"].iter().any(|w| q.has_exact_word(w))
            || q.raw_tokens.iter().any(|t| matches!(t.lower.as_str(), "node_modules" | "__pycache__" | "pycache"))
            || q.has_any(&["all", "every", "everywhere", "anywhere"]))
        && !q.has_any(&["empty"])
}

/// The journal-trimming request: `Some(true)` by age, `Some(false)` by size.
fn journal_vacuum_kind(q: &QueryContext<'_>) -> Option<bool> {
    if !(q.has_any(&["journal", "journalctl", "system log", "system logs", "systemd log", "systemd logs"])
        && q.has_any(&[
            "vacuum", "clear", "clean", "delete", "remove", "purge", "trim", "shrink", "limit", "cap", "rotate",
            "free", "prune", "reduce", "keep",
        ]))
    {
        return None;
    }
    let by_size = q.raw_tokens.iter().any(|t| is_size_spec(&t.text))
        || q.has_any(&["size", "megabytes", "gigabytes", "mb", "gb", "bigger", "larger", "max"]);
    let by_age = crate::extract::extract("file_age", q.raw).is_some_and(|age| age.starts_with('+'))
        || q.has_any(&["older", "days", "weeks", "months"]);
    match (by_age, by_size) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        (true, true) => Some(!q.raw_tokens.iter().any(|t| is_size_spec(&t.text))),
        (false, false) => Some(true),
    }
}

/// "copy x out of container web": the file leaves the container.
fn copies_out_of_container(q: &QueryContext<'_>) -> bool {
    q.has_any(&["from container", "from the container", "out of container", "out of the container", "from docker"])
}

/// "copy app.log into container web01": a file crossing into or out of a container.
fn copies_with_container(q: &QueryContext<'_>) -> bool {
    q.has_any(&["container", "containers"])
        && !crate::readers::cp_paths(q.raw).is_empty()
        && q.has_any(&["copy", "cp", "put", "upload", "send", "push", "get", "download", "fetch", "grab", "pull"])
        && !q.has_any(&[
            "image", "images", "logs", "log", "restart", "stop", "start", "remove", "shell", "exec", "terminal",
        ])
}

/// "ssh reverse tunnel": the remote end listens and leads back here (`ssh -R`).
fn asks_reverse_tunnel(q: &QueryContext<'_>) -> bool {
    q.has_any(&[
        "reverse tunnel",
        "reverse ssh",
        "reverse port",
        "remote forward",
        "remote port forward",
        "remote forwarding",
        "expose my local",
        "expose local",
        "forward remote port",
        "forward a remote port",
        "ssh -r",
    ])
}

/// "generate ed25519 ssh key": a new key pair, not installing one on a server.
fn generates_ssh_key(q: &QueryContext<'_>) -> bool {
    q.has_any(&["generate", "create", "new", "make", "keygen", "ssh-keygen"])
        && q.has_any(&["ssh key", "ssh keys", "key pair", "keypair", "ed25519", "ssh keypair", "rsa key", "ssh-keygen"])
        && !q.has_any(&["copy", "install", "send", "authorize", "authorise", "push", "server"])
}

/// A mount point in the request: an absolute path that is not a device.
fn has_mount_point(q: &QueryContext<'_>) -> bool {
    q.raw_tokens.iter().any(|t| t.text.starts_with('/') && !t.text.starts_with("/dev/") && t.text.len() > 1)
}

/// "mount /dev/sdb1 on /mnt/data": a device and where to put it.
fn mounts_device_on_folder(q: &QueryContext<'_>) -> bool {
    q.has_exact_word("mount")
        && crate::extract::extract("device", q.raw).is_some()
        && has_mount_point(q)
        && !q.has_any(&["unmount", "umount"])
}

/// "password for bob": another account than the person's own.
fn names_other_account(q: &QueryContext<'_>) -> bool {
    const SELF: &[&str] =
        &["my", "me", "myself", "the", "this", "that", "a", "an", "wifi", "wi-fi", "ssh", "root", "your"];
    q.raw_tokens.iter().any(|t| {
        matches!(t.lower.as_str(), "for" | "of") && {
            let next = t.next_word.trim_matches(|c: char| !c.is_alphanumeric());
            !next.is_empty()
                && !SELF.contains(&next)
                && next.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        }
    }) || crate::extract::extract("user_name", q.raw).is_some()
}

fn mentions_cron(q: &QueryContext<'_>) -> bool {
    q.has_any(&["cron", "crontab", "cron jobs", "cron job", "scheduled jobs", "scheduled tasks"])
}

/// "show cron jobs": a look at the schedule, not a change.
fn lists_cron(q: &QueryContext<'_>) -> bool {
    mentions_cron(q)
        && q.has_any(&["show", "list", "display", "see", "view", "what", "which", "print", "check", "my"])
        && !q.has_any(&["add", "create", "edit", "schedule", "remove", "delete"])
}

/// "run backup.sh every day at 3am": a timetable for a command.
fn asks_schedule(q: &QueryContext<'_>) -> bool {
    if lists_cron(q) {
        return false;
    }
    let every_unit = q.words.windows(2).any(|w| {
        w[0] == "every"
            && matches!(
                w[1].trim_end_matches('s'),
                "day"
                    | "hour"
                    | "minute"
                    | "night"
                    | "morning"
                    | "evening"
                    | "week"
                    | "month"
                    | "monday"
                    | "tuesday"
                    | "wednesday"
                    | "thursday"
                    | "friday"
                    | "saturday"
                    | "sunday"
                    | "weekday"
                    | "weekend"
                    | "sec"
                    | "min"
            )
    }) || q.words.windows(3).any(|w| {
        w[0] == "every"
            && is_digits(&w[1])
            && matches!(w[2].trim_end_matches('s'), "minute" | "hour" | "day" | "min" | "week")
    });
    let named_period =
        ["nightly", "hourly", "daily", "weekly", "monthly", "schedule"].iter().any(|w| q.has_exact_word(w))
            || q.has_any(&["on a schedule", "cron job"]);
    (every_unit || named_period)
        && !q.has_any(&[
            "remind",
            "reminder",
            "shutdown",
            "reboot",
            "shut down",
            "power off",
            "poweroff",
            "backup folder",
            "show",
            "list",
        ])
        && !(q.has_exact_word("schedule") && q.has_any(&["timetable", "calendar", "meeting"]))
}

/// "replace foo with bar in nginx.conf": a substitution inside a file.
fn asks_replace_in_file(q: &QueryContext<'_>) -> bool {
    q.has_any(&["replace", "substitute", "swap"])
        && q.has_exact_word("with")
        && q.raw_tokens.iter().any(|t| looks_like_regular_file(&t.text))
        && !q.has_any(&["package", "packages", "pacman", "battery", "disk", "drive"])
}

/// "add ip 10.0.0.5/24 to eth0" (`Some(true)`) or "remove ip ... from eth0" (`Some(false)`).
fn changes_interface_address(q: &QueryContext<'_>) -> Option<bool> {
    let address = q.raw_tokens.iter().any(|t| {
        let ip = t.text.split('/').next().unwrap_or_default();
        looks_like_ip(ip)
    });
    let interface = q.raw_tokens.iter().any(|t| crate::readers::is_interface_name(&t.text))
        || q.has_any(&["interface", "nic", "network card"]);
    if !(address && interface) {
        return None;
    }
    if q.has_any(&["remove", "delete", "drop", "unassign", "detach", "del"]) {
        Some(false)
    } else if q.has_any(&["add", "assign", "give", "set", "attach", "put"]) {
        Some(true)
    } else {
        None
    }
}

/// "print second column of access.log": whitespace-separated columns (awk), not a delimited file (cut).
fn asks_awk_column(q: &QueryContext<'_>) -> bool {
    q.has_any(&["column", "columns", "field", "fields"])
        && q.raw_tokens.iter().any(|t| looks_like_regular_file(&t.text))
        && !q.has_any(&["delimiter", "comma", "separated", "colon", "semicolon", "tab", "pipe", "csv", "tsv"])
        && !q.raw_tokens.iter().any(|t| {
            t.lower.ends_with(".csv")
                || t.lower.ends_with(".tsv")
                || t.lower == "/etc/passwd"
                || t.lower == "/etc/group"
        })
}

/// "check filesystem on /dev/sdc1".
fn asks_fsck(q: &QueryContext<'_>) -> bool {
    (q.has_any(&["fsck", "filesystem", "file system"])
        && q.has_any(&["check", "repair", "fix", "scan", "errors", "fsck", "verify"]))
        && crate::extract::extract("device", q.raw).is_some()
}

/// "repair the filesystem on /dev/sdc1": the repairing fsck, not the read-only check.
fn asks_fsck_repair(q: &QueryContext<'_>) -> bool {
    asks_fsck(q) && q.has_any(&["repair", "repairs", "repairing", "fix", "fixes", "fixing"])
}

/// "make it run in the background with nohup".
fn asks_nohup(q: &QueryContext<'_>) -> bool {
    q.has_word("nohup")
        || (q.has_any(&["background"])
            && q.has_any(&["keep running", "after logout", "after i log out", "survive", "even after", "detached"]))
}

/// "list files with permissions", "show file permissions in /etc": a look at the modes, not a change.
fn lists_permissions(q: &QueryContext<'_>) -> bool {
    q.has_any(&["permission", "permissions"])
        && q.has_any(&["list", "show", "display", "see", "view", "with", "what"])
        && !q.has_any(&[
            "change",
            "set",
            "make",
            "give",
            "grant",
            "fix",
            "restrict",
            "remove",
            "add",
            "only",
            "everyone",
            "nobody",
            "readable",
            "writable",
            "executable",
            "chmod",
            "sticky",
            "protect",
            "revoke",
            "allow",
            "deny",
        ])
        && crate::extract::extract("chmod_mode", q.raw).is_none()
        && !q.has_any(&["hidden", "dotfiles", "dotfile", "recent", "recently", "newest", "sorted", "sort"])
        && !q.raw_tokens.iter().any(|t| looks_like_regular_file(&t.text))
}

/// "squash the last 3 commits".
fn asks_squash(q: &QueryContext<'_>) -> bool {
    q.has_any(&["squash", "squashing", "squashed"])
        || (q.has_any(&["combine", "merge", "fold", "collapse", "join"])
            && q.has_any(&["commits"])
            && q.has_any(&["last", "recent", "previous"]))
}

/// "undo git commit", "undo my last commit": take the commit back.
fn undoes_a_commit(q: &QueryContext<'_>) -> bool {
    q.has_any(&[
        "undo",
        "uncommit",
        "take back",
        "revert my last",
        "undo my",
        "undo the",
        "undo a",
        "undo git",
        "undo last",
    ]) && q.has_any(&["commit", "commits"])
        && !has_commit_hash(q)
        && !q.has_any(&["push", "pushed"])
}

fn is_off_topic_unmodeled(_cmd: &Command, q: &QueryContext<'_>) -> bool {
    let pure_offtopic = [
        "weather",
        "forecast",
        "joke",
        "poem",
        "haiku",
        "story",
        "essay",
        "summarize",
        "summary",
        "translate",
        "flight",
        "hotel",
        "recipe",
        "who won",
        "world cup",
        "percent of",
        "capital of",
        "meaning of",
    ];
    if pure_offtopic.iter().any(|p| q.has_phrase(p)) {
        return true;
    }

    if q.has_phrase("time in") || q.has_phrase("timezone") || q.has_phrase("what time is it in") {
        return true;
    }

    // Hardware and media that huh has no command for.
    if q.has_any(&["printer", "printers", "scanner", "scanners", "playlist", "playlists"]) {
        return true;
    }
    if q.has_word("print")
        && (q.has_any(&["document", "page", "pages", "resume"])
            || q.raw_tokens.iter().any(|t| {
                matches!(
                    t.lower.rsplit('.').next(),
                    Some("pdf" | "doc" | "docx" | "odt" | "xls" | "xlsx" | "ppt" | "pptx" | "jpg" | "jpeg" | "png")
                ) && t.lower.contains('.')
            }))
    {
        return true;
    }

    if q.has_word("alarm") && !q.has_any(&["remind", "reminder"]) {
        return true;
    }

    if q.has_word("play") && q.has_any(&["music", "song", "songs", "spotify", "mp3", "podcast", "radio"]) {
        return true;
    }

    let unmodeled_checks: &[(&[&str], &[&str])] = &[
        (
            &["resize", "crop", "rotate", "watermark", "blur"],
            &["image", "photo", "picture", "png", "jpg", "jpeg", "pixels", "wide"],
        ),
        (&["convert", "transcode"], &["video", "gif", "mp4", "mp3", "wav", "webp", "heic", "pdf", "audio"]),
        (&["shrink", "compress"], &["video", "videos", "webm", "mp4", "mkv", "avi", "mov"]),
        (&["burn", "flash"], &["iso", "usb", "sd card", "drive"]),
        (&["merge", "split", "combine"], &["pdf"]),
        (&["send", "compose"], &["email", "mail", "tweet", "slack"]),
    ];
    for (verbs, nouns) in unmodeled_checks {
        if verbs.iter().any(|v| q.has_word(v)) && nouns.iter().any(|n| q.has_phrase(n)) {
            return true;
        }
    }

    q.has_any(&["homebrew", "brew", "winget", "chocolatey", "powershell", "regedit"])
}

// Hand-tuned keyword tables read best one rule per line.
#[rustfmt::skip]
fn passes_domain_coherence(cmd: &Command, q: &QueryContext<'_>) -> bool {
    let name = cmd.name.as_str();
    let has = |words: &[&str]| q.has_any(words);

    // Asking to get something back is never answered with a command that deletes or overwrites.
    if q.asks_recovery() && (cmd.destructive || matches!(name, "gio trash" | "cp" | "mv" | "rsync")) && name != "git restore" {
        return false;
    }

    // New-style entries first: the words that say the request is about their subject at all.
    let percent = crate::extract::extract("percent", q.raw).is_some();
    // Sysadmin requests: each new entry says what its request is, and the entries it competes with step aside.
    let named_account = names_other_account(q);
    let compose = q.has_word("compose") || q.raw.contains("compose");
    match name {
        "git cherry-pick" => return has(&["cherry-pick", "cherry pick", "cherrypick"]) || (has_commit_hash(q) && q.has_word("commit")),
        "git diff refs" => return names_two_refs(q),
        "diff" if names_two_refs(q) => return false,
        "git show" => return has_commit_hash(q) && q.has_word("commit"),
        "git revert" => return q.has_exact_word("revert") && (q.has_word("commit") || has_commit_hash(q)),
        "git stash push" => return stash_with_message(q),
        "git stash" if stash_with_message(q) => return false,
        "git rebase" if asks_squash(q) => return true,
        "git reset" if undoes_a_commit(q) => return true,
        "lsof" => return asks_open_files(q),
        "xdg-open" | "ls" | "tail" | "cat" if asks_open_files(q) => return false,
        "ps top" => return asks_process_ranking(q),
        "top" | "ps" if asks_process_ranking(q) => return false,
        "grep ip addresses" => return asks_ip_in_file(q),
        "ip" | "tail" | "grep" if asks_ip_in_file(q) => return false,
        "du per folder" => return asks_per_folder_sizes(q),
        "du" | "largest items" if asks_per_folder_sizes(q) => return false,
        "find delete dirs" => return asks_delete_named_dirs(q),
        "rm" | "find delete" | "find" if asks_delete_named_dirs(q) => return false,
        "journalctl vacuum time" => return journal_vacuum_kind(q) == Some(true),
        "journalctl vacuum size" => return journal_vacuum_kind(q) == Some(false),
        "journalctl" | "search service log" | "rm" | "find delete" | "tail" if journal_vacuum_kind(q).is_some() => return false,
        "docker cp" => return copies_with_container(q) && !copies_out_of_container(q),
        "docker cp from" => return copies_with_container(q) && copies_out_of_container(q),
        "cp" | "mv" | "scp" | "rsync" if copies_with_container(q) => return false,
        "cp" | "mv" if q.has_remote_scp() => return false,
        "ssh remote tunnel" => return asks_reverse_tunnel(q),
        "ssh tunnel" if asks_reverse_tunnel(q) => return false,
        "ssh-keygen" => return generates_ssh_key(q),
        "ssh-copy-id" | "ssh" if generates_ssh_key(q) => return false,
        "mount" => return mounts_device_on_folder(q),
        "udisksctl mount" | "findmnt" if mounts_device_on_folder(q) => return false,
        "passwd user" => return has(&["password", "passwd"]) && named_account && !has(&["wifi", "ssh", "key", "file", "zip", "archive", "pdf", "database"]),
        "passwd" if named_account && !has(&["wifi", "ssh", "key", "file", "zip", "archive", "pdf", "database"]) => return false,
        "fsck" => return asks_fsck(q) && !asks_fsck_repair(q),
        "fsck repair" => return asks_fsck_repair(q),
        "smartctl" | "lsblk" | "findmnt" if asks_fsck(q) => return false,
        "crontab list" => return lists_cron(q) && !named_account,
        "crontab list user" => return lists_cron(q) && named_account,
        "crontab edit" => return asks_schedule(q),
        "run script" | "omarchy reminder" | "systemctl" | "ls" if asks_schedule(q) && mentions_cron(q) => return false,
        "sed replace" => return asks_replace_in_file(q),
        "nano" | "cp" | "mv" | "sort" if asks_replace_in_file(q) => return false,
        "ip addr add" => return changes_interface_address(q) == Some(true),
        "ip addr del" => return changes_interface_address(q) == Some(false),
        "rm" | "ip" | "ip route" | "ip neigh" if changes_interface_address(q).is_some() => return false,
        "nohup" => return asks_nohup(q),
        "docker compose restart" => return compose && has(&["restart", "reload", "bounce"]),
        "docker restart" | "systemctl" if compose && has(&["restart", "reload", "bounce"]) => return false,
        "awk column" => return asks_awk_column(q),
        "cut" if asks_awk_column(q) => return false,
        "chmod" if lists_permissions(q) => return false,
        "ls permissions" => return lists_permissions(q),
        "ls" | "stat" if lists_permissions(q) => return false,
        "wifi current" => return has(&["wifi", "wireless", "ssid"]) && !has(&["nearby", "available", "scan", "list", "password"]),
        "package installed" => return q.has_exact_word("installed") && !has(&["packages", "everything", "all"]),
        "package list" => return q.has_exact_word("installed") && has(&["packages", "programs", "software", "apps", "everything", "all", "list", "what is installed"]),
        "package cache clean" => return has(&["cache", "old packages", "unused packages", "autoremove", "leftover", "leftovers", "orphans"])
            && has(&["clean", "clear", "free", "remove", "delete", "purge", "empty", "up"]),
        "package install" | "package remove" if q.has_exact_word("installed") && !q.has_exact_word("install") => return false,
        "package owns file" => return has(&["package", "packages", "pacman", "dpkg", "rpm", "installed"]),
        "git branch" => return !has(&["delete", "remove", "drop", "get rid of"]) && has(&["branch", "branches", "git"]),
        "suspend" => return has(&["sleep", "suspend"]),
        "hibernate" => return has(&["hibernate", "hibernation"]),
        "bluetoothctl connect" | "bluetoothctl disconnect" | "bluetoothctl devices" | "bluetoothctl info" | "bluetooth status" =>
            return has(&["bluetooth", "headphones", "headset", "earbuds", "airpods"]),
        "udisksctl unmount" if q.raw_tokens.iter().any(|t| ["/mnt", "/media", "/run/media", "~/", "/srv", "/home"].iter().any(|p| t.text.starts_with(p))) => return false,
        "udisksctl mount" | "udisksctl unmount" | "udisksctl power-off" =>
            return has(&["usb", "drive", "disk", "stick", "mount", "unmount", "eject", "partition", "sd card", "flash", "device"]) && !has(&["docker", "container", "image"]),
        "umount" => return has(&["unmount", "umount"]) || (has(&["mount point"]) && has(&["unmount", "release"])),
        "sensors" => return has(&["temperature", "temperatures", "temp", "temps", "hot", "overheating", "overheat", "thermal", "fan", "fans", "sensors", "heat"]),
        "last" => return has(&["login", "logins", "logged in", "log ins"]) && !has(&["who is logged in", "who's logged in", "currently"]),
        "graphics card" => return has(&["graphics", "gpu", "video card", "vga"]),
        "program version" => return has(&["version", "versions"]) && (!has(&["omarchy", "kernel", "linux", "distro", "os", "git"]) || has(&["version of git", "git version", "version git"])),
        "run script" => return has(&["script", "run", "execute"])
            && (has(&["script"]) || q.raw_tokens.iter().any(|t| [".sh", ".py", ".bash", ".pl", ".rb"].iter().any(|ext| t.lower.ends_with(ext)))),
        "userdel" => return has(&["user", "account"]) && has(&["delete", "remove", "erase", "get rid of"]) && !has(&["group", "package"]),
        "usermod lock" => return has(&["user", "account"]) && has(&["lock", "block", "disable"]) && !has(&["unlock", "screen"]),
        "docker build" => return has(&["docker", "container", "dockerfile", "image"]) && has(&["build", "dockerfile", "tag"]),
        "git branch delete" => return has(&["delete", "remove", "drop", "get rid of"]) && has(&["branch", "branches"]),
        "git rebase" => return has(&["rebase", "squash", "reorder"]),
        "ssh tunnel" => return has(&["tunnel", "forward", "forwarding", "port forward"]),
        "curl post json" => return has(&["post"]) && has(&["json", "curl", "api", "body", "data"]),
        "wl-copy" => return has(&["clipboard"]) && !has(&["paste", "screenshot"]),
        "wl-paste" => return has(&["clipboard", "paste"]) && has(&["paste", "show", "print", "contents", "what"]),
        "wpctl get-volume" => return has(&["volume", "sound", "audio", "muted", "loud"]) && !has(&["microphone", "mic", "input", "working", "broken", "stopped", "isnt", "not"]),
        "wpctl status" => return has(&["audio", "output", "outputs", "speaker", "speakers", "sound", "sink", "headphones"]) && !has(&["working", "fix", "broken"]),
        "brightnessctl info" => return has(&["brightness", "bright", "backlight"]),
        "xdg-settings get" => return has(&["default"]) && has(&["browser", "web browser"]),
        "timedatectl" => return has(&["clock", "time", "ntp", "timezone", "time zone", "synced", "synchronized", "synchronised"]) && !has(&["time in"]),
        "echo path" => return has(&["path"]) && !has(&["file", "folder", "directory", "to"]) || has(&["my path", "the path variable", "path variable"]),
        "echo shell" => return has(&["shell"]) && has(&["which", "what", "current", "using", "my", "login"]) && !has(&["reload", "config", "alias", "variable", "script"]),
        "id" => return has(&["root", "admin", "sudo", "uid", "user id"]) && has(&["am i", "do i have", "my", "i am", "i'm"]) && !has(&["become", "run", "install"]),
        "getent group" => return has(&["group", "groups"]) && has(&["who", "members", "member", "users", "belongs", "belong", "list", "show"]) && !has(&["add", "my groups", "which groups"]),
        "findmnt" => return has(&["mounted", "mount", "mounts", "mountpoints", "mount points"]) && !has(&["unmount", "umount", "eject"]),
        "systemctl running" => return has(&["service", "services", "daemons", "daemon"]) && has(&["running", "active", "list", "started"]) && !has(&["failed", "failing", "restart", "start", "stop", "enable", "disable"]),
        "wpctl set-volume" => return percent && has(&["volume", "sound", "audio"]),
        "omarchy brightness set" => return percent && has(&["brightness", "backlight", "bright", "dim", "screen"]),
        "omarchy audio output volume" if percent => return false,
        "omarchy brightness display" if percent => return false,
        "dosbox" => return has(&["dosbox", "dos", "ms-dos", "msdos"]),
        "appimage run" => return has(&["appimage", "fuse", "libfuse"]) || q.raw_tokens.iter().any(|t| t.lower.ends_with(".appimage")),
        "flatpak install" | "flatpak run" | "flatpak search" | "flatpak list" | "flatpak uninstall" => return has(&["flatpak", "flatpaks", "flathub"]),
        "mpv" => return asks_mpv(q),
        _ => {}
    }

    match name {
        "reboot" => return q.is_power_request(true),
        "shutdown" => return q.is_power_request(false),
        "shutdown cancel" => return has(&["cancel", "abort"]) && has(&["shutdown", "reboot", "power off", "restart", "shut down", "poweroff"]),
        "systemctl" | "systemctl --user" => return !(has(&["wifi", "wireless", "bluetooth", "headphones"]) && !has(&["service", "daemon", "networkmanager", "boot", "startup", "unit", "automatically"])),
        "nmcli wifi list" => return has(&["wifi", "wireless", "nmcli"]) && !(has(&["connected to", "am i on", "am i connected", "current wifi", "current ssid", "my ssid"]) && !has(&["nearby", "available", "around", "range", "scan", "list"])),
        "systemctl status" | "systemctl failed" => return !has(&["container", "containers"]),
        "find delete" => return has(&["delete", "remove", "clean", "clean up", "purge", "erase", "get rid of", "wipe"]),
        // Formatting or wiping needs a drive named: "wipe everything here" is about files.
        "format drive" => return (q.has_any(&["disk", "drive", "usb", "stick", "partition", "sd card", "ssd", "nvme", "hdd", "pendrive", "flash drive"]) || q.raw.contains("/dev/")) && has(&["format", "reformat", "mkfs", "filesystem"]),
        "wipe drive" => return (q.has_any(&["disk", "drive", "usb", "stick", "partition", "sd card", "ssd", "nvme", "hdd", "pendrive", "flash drive"]) || q.raw.contains("/dev/")) && has(&["wipe", "erase", "clear", "wipefs", "partition table", "remove all partitions"]),
        "zero drive" => return (q.has_any(&["disk", "drive", "usb", "stick", "partition", "sd card", "ssd", "nvme", "hdd", "pendrive", "flash drive"]) || q.raw.contains("/dev/")) && has(&["zero", "zeros", "securely", "secure", "overwrite"]),
        "kill port" | "ss port" => return q.names_port() || q.has_phrase("already in use"),
        "nc" => return q.names_port() && crate::extract::extract("host", q.raw).is_some(),
        "gio trash empty" => return has(&["trash", "recycle bin"]) && has(&["empty", "clear", "clean out", "purge", "wipe", "delete everything"]),
        "gio list trash" => return q.asks_recovery() || (has(&["trash", "recycle bin"]) && has(&["list", "show", "what", "look"])),
        "cd previous" => return has(&["previous", "last", "go back", "back to where i was", "cd -"]) && has(&["folder", "directory", "dir", "go back", "back to where i was", "cd -"]),
        "who" => return has(&["who is logged in", "who's logged in", "users logged in", "logged in users", "who is online"]),
        "iostat" => return has(&["io", "i/o", "iops", "iostat", "disk activity", "disk throughput", "disk performance", "disk speed", "io wait"]),
        "useradd" => return has(&["user", "account"]) && has(&["create", "add", "new", "make"]) && !has(&["group"]),
        "dig" => return has(&["dns", "dig", "mx", "nameserver", "nameservers", "lookup", "look up", "resolve", "record", "records"]) && !has(&["install", "uninstall", "package", "remove"]) && crate::extract::extract("domain", q.raw).is_some(),
        "curl headers" => return has(&["headers", "header"]) && (q.has_url() || q.has_domain_token()),
        "docker image prune" => return has(&["dangling", "untagged", "unused", "prune"]) && has(&["image", "images"]),
        "docker container prune" => return has(&["stopped", "exited", "unused", "prune", "dead"]) && has(&["container", "containers"]),
        "docker system df" => return has(&["docker"]) && has(&["disk usage", "space", "storage", "size", "usage"]),
        "git restore" => return has(&["discard", "throw away", "revert", "undo", "restore", "drop"]) && has(&["changes", "edits", "modifications", "uncommitted", "local"]),
        "git stash" => return has(&["stash", "stashes", "stashed"]),
        "git reset" => return has(&["commit", "commits", "changes", "uncommitted", "repo", "git", "working tree"]) && (q.has_exact_word("uncommit") || has(&["undo", "reset", "discard", "throw away", "hard", "soft", "go back"])),
        "git clean" => return has(&["untracked"]) || (has(&["clean"]) && has(&["repo", "repository", "git"])),
        _ => {}
    }

    if name.starts_with("git ") {
        return has(&["git", "repo", "repository", "branch", "branches", "commit", "commits", "stage", "staged", "remote", "clone", "diff", "uncommitted", "push", "pull", "teammates", "stash", "untracked", "origin", "upstream"])
            || (name == "git switch" && has(&["main", "master", "develop", "trunk"]) && has(&["switch", "checkout", "go to"]));
    }
    if name.starts_with("docker ") {
        return has(&["docker", "container", "containers", "compose", "stack"])
            || (has(&["image", "images"]) && !has(&["photo", "jpg", "png", "qr", "barcode", "viewer", "resize", "iso"]));
    }
    if name.starts_with("virsh ") {
        return has(&["vm", "vms", "virsh", "libvirt", "virtual machine", "virtual machines", "virtual network", "virtual networks"]);
    }
    if name.starts_with("cargo ") {
        return has(&["cargo", "rust", "crate", "crates", "dependency", "dependencies"]);
    }
    if name.starts_with("ollama ") {
        return match name {
            "ollama run" => has(&["ollama", "llm", "model", "models", "chat", "talk", "tokens per second"]),
            "ollama stop" => has(&["ollama", "llm", "model", "models", "unload"]),
            _ => has(&["ollama", "llm", "ai model", "ai models", "local ai"])
                || (has(&["model", "models"]) && has(&["download", "fetch", "loaded", "delete", "remove", "list"])),
        };
    }
    if name.starts_with("tmux ") {
        return has(&["tmux"]) || (has(&["session", "sessions"]) && has(&["reattach", "attach", "terminal", "kill", "new", "list"]));
    }
    if name.starts_with("tailscale ") {
        return has(&["tailscale", "vpn", "tailnet"]);
    }
    if name.starts_with("nmcli ") {
        return has(&["wifi", "wireless", "nmcli", "saved network connection", "network connections"]);
    }
    if name.starts_with("omarchy ") {
        return match name {
            "omarchy theme set" | "omarchy theme list" => has(&["theme", "themes", "colour", "color"]),
            "omarchy theme bg next" => has(&["wallpaper"]) || (has(&["background"]) && !has(&["in the background"]) && !has(&["container", "image", "service"])),
            "omarchy font set" | "omarchy font list" => has(&["font", "fonts", "monospace"]),
            "omarchy capture screenshot" => has(&["screenshot", "capture"]),
            "omarchy capture screenrecording" => has(&["screenrecording", "screen record", "record my screen", "record screen"]),
            "omarchy audio output volume" => has(&["volume", "sound", "audio", "louder", "quieter", "mute", "unmute"]),
            "omarchy brightness display" => has(&["brightness", "brighter", "dim", "dimmer"]),
            "omarchy bluetooth power" => has(&["bluetooth"]),
            "omarchy dns" => has(&["dns", "cloudflare", "1.1.1.1", "8.8.8.8"]),
            "omarchy network speedtest" => has(&["speedtest", "speed test", "speed", "how fast", "bandwidth", "mbps", "upload", "download speed"]) && !has(&["model", "tokens per second"]),
            "omarchy powerprofiles set" => has(&["power saver", "power profile", "balanced", "performance"]),
            "omarchy reminder" => has(&["remind", "reminder"]),
            "omarchy launch webapp" => has(&["webapp", "web app"]),
            "omarchy toggle nightlight" => has(&["night light", "nightlight", "warm"]),
            "omarchy toggle notification silencing" => has(&["do not disturb", "notification", "notifications", "silence"]),
            "omarchy toggle touchpad" => has(&["touchpad", "trackpad"]),
            "omarchy toggle idle" => has(&["awake", "idle", "sleep"]),
            "omarchy system lock" => has(&["lock"]) && has(&["screen", "computer", "laptop", "pc"]),
            "omarchy system logout" => has(&["logout", "log out", "log me out"]),
            "omarchy snapshot create" => has(&["snapshot"]),
            "omarchy update firmware" => has(&["firmware", "bios", "uefi"]),
            "omarchy update" | "omarchy version" => has(&["omarchy", "up to date"]),
            "omarchy refresh" => {
                has(&["shell", "top bar", "bar", "hyprland", "tmux", "pacman", "hyprsunset", "chromium", "night light", "waybar", "config", "configuration", "defaults"])
                    && has(&["reset", "restore", "refresh", "default", "defaults", "broken", "fix"])
                    && !has(&["router", "modem"])
            }
            "omarchy restart" => has(&["restart", "acting up", "stopped working", "fix", "misbehaving"])
                && has(&["wifi", "wireless", "sound", "audio", "speaker", "speakers", "headphones", "microphone", "bluetooth", "bar", "shell", "menu", "menus", "waybar", "terminal", "trackpad", "touchpad", "hyprland", "hyprsunset", "night light", "desktop"]),
            "omarchy pkg aur add" => has(&["aur", "arch user repository"]) && has(&["install", "add", "get", "package", "omarchy"]),
            _ => true,
        };
    }

    match name {
        "gio trash" => has(&["trash", "recycle bin", "discard"]),
        "package search" | "package install" | "package remove" => !q.asks_package_owner(),
        "rm" => (!has(&["trash", "recycle bin"]) || has(&["permanently", "permanent"])) && !q.removes_program()
            && !(has(&["user", "account", "accounts"]) && !has(&["file", "files"]) || has(&["user account", "user accounts"])),
        "ls" => !q.raw_tokens.iter().any(|t| looks_like_regular_file(&t.text))
            || q.raw_tokens.iter().any(|t| t.text.ends_with('/') || std::path::Path::new(&t.text).is_dir()),
        "battery status" => has(&["battery", "charge", "upower"]),
        "reload shell config" => has(&["reload", "source", "re-read", "apply changes"]) && has(&["bashrc", "zshrc", "shell", "config", "profile", "startup"]),
        "pacman install file" => q.raw.contains(".pkg.tar.zst") || (has(&["downloaded", "package file"]) && has(&["install"])),
        "tar" => q.raw.contains(".tar") || q.raw.contains("tar.gz") || q.raw.contains(".tgz") || has(&["tar", "tarball"])
            || (has(&["archive"]) && !has(&["archive folder", "archive directory"])),
        "zip" => q.raw.contains(".zip") || has(&["zip"]),
        "unzip" => q.raw.contains(".zip") || has(&["unzip"]),
        "find" => !has(&["look through", "search inside", "every line"]),
        "largest items" => !has(&["biggest to smallest", "sort", "order", "lines"])
            && has(&["space", "biggest", "largest", "taking up", "most"]),
        "lscpu" => has(&["cpu", "processor", "core", "cores"]) && !has(&["eating", "right now", "most", "using the most", "hot", "temperature", "temp", "temps", "overheating"]),
        "tail" => !has(&["journald", "journal", "systemd"]) && !(has(&["docker", "service"]) && !q.has_log_file()),
        "docker logs" => !has(&["journald", "journal", "systemd"]),
        "journalctl" | "search service log" => !q.has_log_file() && !has(&["log in", "log into", "logged in", "in lines"]) && !(q.has_phrase("docker logs") && !has(&["journald", "journal", "systemd"])),
        "ip route get" => q.raw_tokens.iter().any(|t| looks_like_ip(&t.text) || looks_like_domain_host(&t.text))
            || has(&["to reach", "reach an", "route to", "interface used"]),
        "cd" => has(&["cd", "go to", "change directory", "change into", "enter folder"]),
        "curl" => (q.has_url() || q.has_domain_token() || has(&["curl", "web", "website", "url", "web address", "page at"]))
            && !has(&["image", "model", "ollama", "docker", "clone"]),
        "ping" => has(&["ping", "reach", "reachable", "connected to the internet", "internet", "online", "latency"]),
        "ssh" => has(&["ssh", "log in", "log into", "connect to the server", "server"]) || q.raw.contains('@'),
        "btrfs subvolume list" => has(&["btrfs", "subvolume", "subvolumes"]),
        "snapper list" => has(&["snapshot", "snapshots", "snapper"]),
        "mise upgrade" => has(&["mise"]),
        "modprobe" => has(&["kernel module", "modprobe", "driver"]),
        "ethtool" => has(&["link speed", "ethtool", "ethernet"]),
        "zbarimg" => has(&["qr", "barcode", "zbarimg"]),
        "smartctl" => has(&["smart", "smartctl", "healthy", "health"]) && (q.raw.contains("/dev/") || has(&["disk", "drive", "ssd", "nvme"])),
        "diff" => has(&["diff", "differ", "different", "difference", "compare"]),
        "uniq" => has(&["uniq", "duplicate", "repeated", "how often", "how many times", "occurs", "appear", "most common", "frequency"]),
        "cut" => has(&["cut", "column", "columns", "field", "fields", "delimiter"]),
        "sort" => has(&["sort", "order", "alphabetical", "alphabetically", "numerical", "numerically"]),
        "wc" => has(&["wc", "count", "how many", "number of", "how long"]) && has(&["line", "lines", "word", "words", "character", "characters"]),
        "man" => has(&["man", "manual", "how do i use"]),
        "which" => has(&["which", "where is", "where", "installed", "location"]),
        "sha256sum" => has(&["checksum", "sha256", "sha256sum", "hash", "verify", "intact"]),
        "chmod" => (has(&["chmod", "sticky", "executable", "runnable", "as a program", "permission", "permissions", "read only", "nobody but me", "protect", "modified"])
            || (has(&["stop", "prevent"]) && has(&["editing"])) || crate::extract::extract("chmod_mode", q.raw).is_some())
            && !has(&["dotfiles"])
            && !(has(&["show", "list", "which", "what"]) && crate::extract::extract("file_age", q.raw).is_some()),
        "chown" => has(&["chown", "owner", "ownership"]),
        "ln" => has(&["ln", "symlink", "symbolic link", "points to"]) || (has(&["shortcut"]) && !has(&["alias"]) && !q.raw.contains('"')),
        "date" => has(&["date", "today", "what day", "time"]) && !q.has_exact_word("dates") && !has(&["up to date"]),
        "cal" => has(&["cal", "calendar"]),
        "uptime" => has(&["uptime", "been on", "running since", "been up"]) || (has(&["how long"]) && has(&["reboot", "boot", "on", "running", "up"])),
        "hostname" => has(&["hostname", "machine called", "computer called", "computer's name"]),
        "whoami" => has(&["whoami", "which user", "logged in", "username", "who am i"]),
        "passwd" => has(&["passwd", "password"]) && !has(&["passwordless"]),
        "clear" => has(&["clear", "clean up"]) && has(&["screen", "terminal"]),
        "history" => has(&["history", "previous command", "previous commands", "typed earlier", "what did i type", "typed before", "commands i ran", "commands i typed"]),
        "nano" => (has(&["nano", "edit", "editor"]) || (has(&["change"]) && q.raw_tokens.iter().any(|t| looks_like_regular_file(&t.text))))
            && !has(&["stop", "prevent", "protect", "nobody", "modified"])
            && !q.raw_tokens.iter().any(|t| is_upper_env_name(&t.text)),
        "stat" => !has(&["dotfiles", "hidden"]),
        "pwd" => has(&["pwd", "working directory", "where am i", "current folder", "current directory", "which folder", "which directory", "what folder", "what directory"]),
        "gh auth login" => has(&["gh", "github"]) && has(&["log in", "login", "auth", "authenticate"]),
        // Whether there is something to open is checked in `details`, once values are bound.
        "xdg-open" => has(&["open", "viewer", "xdg-open"]) && !has(&["resize", "convert", "port", "shell", "terminal", "every time", "change it"]),
        "free" => has(&["ram", "memory", "swap"]) && !has(&["model", "ollama", "docker", "container"]),
        _ => true,
    }
}

/// Play media only when the user names `mpv` or asks to play or watch a media URL, file, or noun.
fn asks_mpv(q: &QueryContext<'_>) -> bool {
    const MEDIA_EXTS: &[&str] =
        &[".mp4", ".mkv", ".avi", ".webm", ".mov", ".mp3", ".flac", ".wav", ".ogg", ".opus", ".m4a", ".m3u", ".m3u8"];
    const MEDIA_NOUNS: &[&str] = &[
        "video", "movie", "film", "audio", "media", "mp4", "mkv", "avi", "webm", "mov", "mp3", "flac", "wav", "ogg",
        "opus", "m4a",
    ];
    if q.has_word("mpv") {
        return true;
    }
    if !q.has_any(&["play", "watch", "listen", "loop", "player"]) || q.has_any(&["recommend", "good movie"]) {
        return false;
    }
    q.has_url()
        || q.raw_tokens.iter().any(|t| {
            t.lower.starts_with("http://")
                || t.lower.starts_with("https://")
                || MEDIA_EXTS.iter().any(|ext| t.lower.ends_with(ext))
        })
        || q.has_any(MEDIA_NOUNS)
}

// ---------------------------------------------------------------------------
// Details: Flags & Choice Slots
// ---------------------------------------------------------------------------

/// `-r` follows what the bound value is: a folder needs it, a plain file must not get it, whatever
/// else the request says about folders ("copy notes.txt to my projects folder").
fn adjust_flags_for_values(
    command: &Command,
    slots: &[SlotPick],
    candidates: &Candidates,
    q: &QueryContext<'_>,
    flags: &mut [f64],
) {
    let bound = |slot_id: &str| -> Option<&str> {
        let i = command.slots.iter().position(|s| s.id == slot_id)?;
        match &slots[i] {
            SlotPick::Value { value, .. } => Some(value.as_str()),
            _ => None,
        }
    };
    // Options typed as written: `-ri` asks for both `-r` and `-i`.
    let typed: Vec<char> = q
        .raw_tokens
        .iter()
        .filter(|t| t.text.len() > 1 && t.text.starts_with('-') && !t.text.starts_with("--"))
        .filter(|t| t.text[1..].chars().all(|c| c.is_ascii_alphabetic()))
        .flat_map(|t| t.text[1..].chars().collect::<Vec<_>>())
        .collect();
    for (flag, wanted) in command.flags.iter().zip(flags.iter_mut()) {
        let mut chars = flag.value.chars();
        if let (Some('-'), Some(letter), None) = (chars.next(), chars.next(), chars.next())
            && typed.contains(&letter)
        {
            *wanted = wanted.max(0.94);
        }
    }
    let (slot_id, rule) = match command.name.as_str() {
        "cp" | "rm" => (if command.name == "cp" { "source" } else { "target" }, "recursive"),
        "grep" => ("where", "recursive"),
        _ => return,
    };
    let Some(i) = command.flags.iter().position(|f| f.id == rule) else { return };
    let Some(value) = bound(slot_id) else { return };
    let kind = candidates.entry_kind(value.trim_end_matches('/'));
    let is_folder = kind == EntryKind::Dir || value == "." || value == "~" || value.ends_with('/');
    if is_folder {
        flags[i] = flags[i].max(0.94);
    } else if kind == EntryKind::File && value.contains('.') && command.name != "grep" {
        flags[i] = flags[i].min(0.08);
    }
}

/// The request names a file, link or folder by its shape or by a word for one.
fn names_something_to_open(q: &QueryContext<'_>) -> bool {
    q.raw_tokens.iter().any(|t| looks_like_regular_file(&t.text) || looks_like_url(&t.text) || t.text.contains('/'))
        || q.has_domain_token()
        || q.has_any(&[
            "file",
            "folder",
            "directory",
            "document",
            "photo",
            "picture",
            "image",
            "pdf",
            "link",
            "website",
            "page",
            "url",
            "viewer",
            "default app",
        ])
}

/// How likely `flag` is wanted; `terms` are the flag's indexed words.
// Hand-tuned keyword tables read best one rule per line.
#[rustfmt::skip]
fn score_flag(cmd_name: &str, flag: &Flag, terms: &HashMap<String, f64>, q: &QueryContext<'_>) -> f64 {
    let flag_id = flag.id.as_str();
    let has = |terms: &[&str]| q.has_any(terms);
    let has_path_slash = q.raw_tokens.iter().any(|t| t.text.contains('/'));

    // The flag itself is in the request ("tail -f app.log").
    if flag.value.starts_with('-') && q.raw_tokens.iter().any(|t| t.lower == flag.value) {
        return 0.97;
    }

    let explicit = match (cmd_name, flag_id) {
        ("docker image prune", "all") => Some(has(&["all unused", "all images", "every unused", "not just dangling", "not only dangling", "-a"])),
        ("docker system prune", "volumes") => Some(has(&["volume", "volumes", "--volumes"])),
        ("git push", "upstream") => Some(has(&["first time", "upstream", "-u", "track", "new branch", "for the first time"])),
        ("git push", "force") => Some(has(&["force", "--force", "overwrite", "-f"])),
        ("git diff", "staged") => Some(has(&["staged", "--staged", "cached", "about to commit"])),
        ("docker rm", "force") => Some(has(&["force", "forcefully", "-f", "even if", "still running", "running container", "kill"])),
        ("userdel", "home") => Some(has(&["home", "home folder", "home directory", "homedir", "files too", "everything of"])),
        ("git rebase", "interactive") => Some(has(&["interactive", "interactively", "squash", "combine", "fold", "collapse", "reorder", "edit", "-i", "fixup", "reword"])),
        ("ls", "long") => Some(has(&["long", "detail", "details", "size", "sizes", "owner", "owns", "owned by", "permission", "permissions", "date", "dates", "-l"])),
        ("ls", "all") => Some(has(&["hidden", "dotfile", "dotfiles", "including hidden", "-a", "-la", "-al"])),
        ("ls", "by_time") => Some(has(&["recent", "recently", "newest", "latest", "last modified", "-t"])),
        ("chmod" | "chown", "recursive") => Some(has(&[
            "everything in",
            "everything inside",
            "all files in",
            "all files inside",
            "recursively",
            "recursive",
            "inside",
            "-r",
        ])),
        ("cp" | "rm", "recursive") => {
            if has(&[
                "folder",
                "folders",
                "directory",
                "directories",
                "everything in",
                "recursively",
                "recursive",
                "completely",
                "-r",
            ]) {
                Some(true)
            } else {
                Some(false)
            }
        }
        ("cat", "numbers") => Some(has(&["line number", "line numbers", "numbered", "with numbers", "-n"])),
        ("curl", "save") => {
            Some(has(&["save", "saved", "download", "-o", "into a file", "as a file"]) && !has(&["page at"]))
        }
        ("docker logs" | "journalctl" | "tail", "follow") => Some(has(&[
            "follow",
            "watch",
            "watching",
            "live",
            "stream",
            "as new lines",
            "as they appear",
            "as it grows",
            "-f",
        ])),
        ("docker ps", "all") => Some(has(&["all", "stopped", "exited", "including stopped", "-a"])),
        ("docker run", "detached") => Some(has(&["background", "detached", "daemon", "-d"])),
        ("docker run", "interactive") => Some(has(&["interactive", "shell", "bash", "sh", "terminal", "-it"])),
        ("git switch", "create") => Some(has(&["new", "create", "make", "-c", "-b"])),
        ("grep", "ignore_case") => Some(has(&[
            "ignore case",
            "ignoring case",
            "case insensitive",
            "any capitalisation",
            "any capitalization",
            "upper and lower",
            "-i",
        ])),
        ("grep", "recursive") => Some(has(&[
            "all files",
            "every file",
            "everything under",
            "folder",
            "directory",
            "recursively",
            "recursive",
            "-r",
        ])),
        ("grep", "line_numbers") => {
            Some(has(&["line number", "line numbers", "which line", "what line", "tell me which line", "-n"]))
        }
        ("journalctl", "boot") => Some(has(&[
            "this boot",
            "current boot",
            "since boot",
            "last started",
            "since startup",
            "last boot",
            "previous boot",
            "from boot",
            "-b",
        ])),
        ("git stash" | "git stash push", "untracked") => Some(has(&["untracked", "new files", "not tracked", "-u"])),
        ("rsync", "delete") => Some(has(&["delete", "deleting", "deleted", "--delete", "remove extra", "exact mirror", "exactly mirror", "extra files"]) && !has(&["do not delete", "without deleting", "dont delete"])),
        ("grep ip addresses", "unique") => Some(has(&["unique", "distinct", "once", "deduplicated", "dedupe", "no duplicates", "without duplicates"])),
        ("sort", "unique") => Some(has(&["dedupe", "dedup", "unique", "duplicate", "duplicates", "distinct", "-u"])),
        ("curl", "resume") => Some(has(&["resume", "continue", "partial", "unfinished", "interrupted", "-c"])),
        ("kill" | "pkill", "force") => Some(has(&[
            "force",
            "frozen",
            "hung",
            "stuck",
            "refuses to stop",
            "forcefully",
            "forcibly",
            "forced",
            "force kill",
            "unresponsive",
            "not responding",
            "-9",
            "sigkill",
        ])),
        ("mkdir", "parents") => Some(
            has(&["nested", "parent", "parents", "all in one go", "may not exist", "inside other", "-p"])
                || has_path_slash,
        ),
        ("ollama run", "verbose") => Some(has(&[
            "speed",
            "tokens per second",
            "tok/s",
            "stats",
            "statistics",
            "verbose",
            "how fast",
            "--verbose",
        ])),
        ("omarchy capture screenrecording", "desktop_audio") => Some(
            (has(&["desktop", "system", "computer", "internal", "own"]) && has(&["audio", "sound"]))
                || has(&["speaker"])
                || (has(&["audio", "sound"]) && !has(&["microphone", "mic", "voice"])),
        ),
        ("omarchy capture screenrecording", "microphone") => Some(has(&["microphone", "mic", "voice", "narration"])),
        ("sort", "numeric") => Some(has(&[
            "number",
            "numbers",
            "numeric",
            "numerical",
            "numerically",
            "biggest",
            "smallest",
            "largest",
            "-n",
        ])),
        ("sort", "reverse") => Some(has(&[
            "reverse",
            "reversed",
            "descending",
            "biggest to smallest",
            "largest first",
            "highest first",
            "last first",
            "-r",
        ])),
        ("top", "by_memory") => Some(has(&["memory", "ram", "mem"])),
        ("uniq", "count") => Some(has(&[
            "count",
            "how many",
            "how many times",
            "how often",
            "occurrences",
            "frequency",
            "times each",
            "-c",
        ])),
        ("uniq", "ranked") => Some(has(&["most common", "most frequent", "highest first", "descending", "ranked"])),
        ("virsh autostart", "disable") => {
            Some(has(&["disable", "off", "stop", "do not", "no longer", "prevent", "turn off", "--disable"]))
        }
        ("cargo build", "release") => Some(has(&["release", "optimised", "optimized", "production", "--release"])),
        _ => None,
    };

    if let Some(wanted) = explicit {
        return if wanted { 0.94 } else { 0.08 };
    }

    let hits = q.content_terms.iter().filter(|t| terms.contains_key(*t)).count();
    if hits >= 2 {
        0.85
    } else if hits == 1 {
        0.50
    } else {
        0.12
    }
}

/// The index of the best option of a `choice` slot (ties go to the option name first in order).
// Hand-tuned keyword tables read best one rule per line.
#[rustfmt::skip]
fn score_choice_slot(cmd_name: &str, slot: &Slot, entry: &IndexedCommand, q: &QueryContext<'_>) -> usize {
    let has = |terms: &[&str]| q.has_any(terms);
    let slot_id = slot.id.as_str();
    let mut best: Option<(usize, f64)> = None;

    for (index, option) in slot.options.iter().enumerate() {
        let opt_name = &option.name;
        let mut s = 0.0f64;

        s += match (cmd_name, slot_id, opt_name.as_str()) {
            ("chmod", "mode", "sticky_bit") => if has(&["sticky", "sticky bit"]) || q.raw.split_whitespace().any(|w| w == "+t") { 9.0 } else { 0.0 },
            ("chmod", "mode", "make_executable") => if has(&["executable", "runnable", "execute", "as a program", "+x"]) { 6.0 } else { 0.0 },
            ("chmod", "mode", "private_to_owner") => if has(&["private", "only me", "nobody else", "nobody but me", "only the owner", "600"]) { 6.5 } else { 0.0 },
            ("chmod", "mode", "read_only") => if has(&["read only", "protect", "modified", "nobody should be able to change", "a-w"]) || (has(&["stop", "prevent"]) && has(&["editing"])) { 6.5 } else { 0.0 },
            ("chmod", "mode", "readable_by_everyone") => if has(&["everyone", "anyone", "world readable", "644"]) && !(has(&["stop", "prevent"]) && has(&["editing"])) { 6.0 } else { 0.0 },

            ("cut", "delimiter", "comma") => if has(&["comma", "commas", "csv", "-d,"]) || q.raw.contains(".csv") { 6.0 } else { 0.0 },
            ("cut", "delimiter", "colon") => if has(&["colon", "colons", "-d:"]) || q.raw.contains("/etc/passwd") || q.raw.contains("/etc/group") { 6.0 } else { 0.0 },
            ("cut", "delimiter", "space") => if has(&["space", "spaces", "whitespace"]) { 5.5 } else { 0.0 },
            ("cut", "delimiter", "tab_or_unknown") => 0.8,

            ("find", "type", "folders") => {
                let looking_for_folders = has(&["all folders", "folders named", "directories named", "directories called", "find folders", "find directories", "locate folders"])
                    || (has(&["folder", "folders", "directory", "directories"]) && !has(&["file", "files"]) && !q.raw.contains("*."));
                if looking_for_folders { 6.0 } else { 0.0 }
            }
            ("find", "type", "files") => if has(&["file", "files"]) || q.raw.contains("*.") { 5.0 } else { 0.0 },
            ("find", "type", "either") => 0.8,

            ("omarchy default", "kind", "agent") => if has(&["agent", "coding agent", "ai agent"]) { 6.5 } else { 0.0 },
            ("omarchy default", "kind", "browser") => if has(&["browser", "web browser"]) { 6.5 } else { 0.0 },
            ("omarchy default", "kind", "editor") => if has(&["editor", "text editor"]) { 6.5 } else { 0.0 },
            ("omarchy default", "kind", "terminal") => if has(&["terminal", "terminal emulator"]) { 6.5 } else { 0.0 },

            ("omarchy audio output volume", "change", "louder") => if has(&["up", "louder", "raise", "increase", "higher", "turn up"]) { 6.0 } else { 0.0 },
            ("omarchy audio output volume", "change", "quieter") => if has(&["down", "quieter", "lower", "decrease", "softer", "turn down"]) { 6.0 } else { 0.0 },
            ("omarchy audio output volume", "change", "mute") => if has(&["mute", "unmute", "silence"]) { 6.5 } else { 0.0 },

            ("omarchy bluetooth power", "state", "on") => if has(&["on", "enable", "start", "activate"]) && !has(&["off", "disable"]) { 6.0 } else { 0.0 },
            ("omarchy bluetooth power", "state", "off") => if has(&["off", "disable", "stop", "deactivate"]) { 6.0 } else { 0.0 },

            ("omarchy brightness display", "change", "brighter") => if has(&["up", "brighter", "increase", "raise"]) { 6.0 } else { 0.0 },
            ("omarchy brightness display", "change", "dimmer") => if has(&["down", "dim", "dimmer", "lower", "decrease", "darker"]) { 6.0 } else { 0.0 },

            ("omarchy capture screenshot", "area", "region") => if has(&["region", "area", "selection", "select", "part of", "snip"]) { 6.0 } else { 0.0 },
            ("omarchy capture screenshot", "area", "window") => if has(&["window", "windows"]) { 6.0 } else { 0.0 },
            ("omarchy capture screenshot", "area", "fullscreen") => if has(&["fullscreen", "full screen", "whole screen", "entire screen"]) { 6.0 } else { 0.0 },
            ("omarchy capture screenshot", "area", "unspecified") => 0.8,

            ("omarchy capture screenshot", "output", "copy_to_clipboard") => if has(&["copy", "clipboard", "paste"]) { 6.5 } else { 0.0 },
            ("omarchy capture screenshot", "output", "save_as_file") => if has(&["save", "file"]) { 5.0 } else { 0.8 },

            ("omarchy dns", "provider", "cloudflare") => if has(&["cloudflare", "1.1.1.1"]) { 7.0 } else { 0.0 },
            ("omarchy dns", "provider", "google") => if has(&["google", "8.8.8.8"]) { 7.0 } else { 0.0 },
            ("omarchy dns", "provider", "from_router") => if has(&["dhcp", "router", "automatic", "default", "reset"]) { 6.0 } else { 0.5 },

            ("omarchy network speedtest", "direction", "upload") => if has(&["upload", "sending"]) { 6.5 } else { 0.0 },
            ("omarchy network speedtest", "direction", "download") => if has(&["download"]) { 6.0 } else { 0.8 },

            ("omarchy powerprofiles set", "profile", "power_saver") => if has(&["power saver", "saver", "battery", "low power"]) { 6.5 } else { 0.0 },
            ("omarchy powerprofiles set", "profile", "balanced") => if has(&["balanced", "middle", "normal"]) { 6.0 } else { 0.0 },
            ("omarchy powerprofiles set", "profile", "performance") => if has(&["performance", "maximum", "max", "fast"]) { 6.0 } else { 0.0 },

            ("omarchy restart", "part", "wifi") => if has(&["wifi", "wireless", "wlan"]) { 6.5 } else { 0.0 },
            ("omarchy restart", "part", "audio") => if has(&["audio", "sound", "speaker", "speakers", "headphone", "microphone"]) { 6.5 } else { 0.0 },
            ("omarchy restart", "part", "bluetooth") => if has(&["bluetooth"]) { 6.5 } else { 0.0 },
            ("omarchy restart", "part", "shell") => if has(&["shell", "top bar", "bar", "waybar", "menu", "menus"]) { 6.5 } else { 0.0 },

            ("omarchy toggle idle", "mode", "stay_awake") => if has(&["awake", "stay awake", "keep", "prevent", "off", "disable", "do not let", "from sleeping", "from going to sleep"]) { 6.5 } else { 0.0 },
            ("omarchy toggle idle", "mode", "allow_idle") => if has(&["allow idle", "normal", "normally", "sleep again", "enable idle"]) { 6.5 } else { 0.0 },

            ("omarchy toggle touchpad", "state", "on") => if has(&["on", "enable", "activate"]) && !has(&["off", "disable"]) { 6.0 } else { 0.0 },
            ("omarchy toggle touchpad", "state", "off") => if has(&["off", "disable", "deactivate"]) { 6.0 } else { 0.0 },

            ("systemctl" | "systemctl --user", "action", "enable") => if has(&["enable", "boot", "automatically", "at login", "startup", "autostart"]) && !has(&["disable", "stop"]) { 7.0 } else { 0.0 },
            ("systemctl" | "systemctl --user", "action", "disable") => if has(&["disable", "stop the service from starting", "stop starting", "don't start"]) { 7.0 } else { 0.0 },
            ("systemctl" | "systemctl --user", "action", "restart") => if has(&["restart", "reload"]) { 7.0 } else { 0.0 },
            ("systemctl" | "systemctl --user", "action", "stop") => if has(&["stop", "halt"]) && !has(&["restart", "disable"]) { 6.0 } else { 0.0 },
            ("systemctl" | "systemctl --user", "action", "start") => if has(&["start"]) && !has(&["restart", "boot", "automatically", "enable", "disable"]) { 6.0 } else { 0.0 },
            ("systemctl" | "systemctl --user", "action", "status") => if has(&["status", "running", "check", "error", "errors"]) { 5.5 } else { 0.6 },

            ("git stash", "action", "save") => if has(&["pop", "apply", "list", "show", "restore", "bring back"]) || q.has_exact_word("stashes") { 0.0 } else { 5.0 },
            ("git stash", "action", "pop") => if has(&["pop", "apply", "bring back", "restore", "unstash", "get back"]) { 6.5 } else { 0.0 },
            ("git stash", "action", "list") => if has(&["list", "show", "what is stashed"]) || q.has_exact_word("stashes") { 6.5 } else { 0.0 },

            ("git reset", "mode", "keep_changes") => if has(&["keep", "soft", "staged", "without losing"]) { 6.5 } else { 0.0 },
            ("git reset", "mode", "discard_all_changes") => if has(&["hard", "discard", "throw away", "everything", "all local", "all changes", "lose"]) && !has(&["keep"]) { 6.5 } else { 0.0 },
            ("git reset", "mode", "drop_last_commit") => if has(&["undo", "remove", "drop", "delete", "uncommit"]) && has(&["commit", "commits"]) && has(&["discard", "throw away", "hard", "lose", "delete"]) && !has(&["keep"]) { 7.5 } else { 0.0 },

            ("dig", "type", "address") => 1.0,
            ("dig", "type", "mail") => if has(&["mx", "mail"]) { 6.5 } else { 0.0 },
            ("dig", "type", "text") => if has(&["txt", "text record", "text records"]) { 6.5 } else { 0.0 },
            ("dig", "type", "nameservers") => if has(&["ns", "nameserver", "nameservers", "name server", "name servers"]) { 6.5 } else { 0.0 },

            ("tar", "action", "list_contents") => if has(&["without extracting", "without unpacking", "what is inside", "what is in", "what's in", "inside", "list", "contents", "peek", "-t"]) { 7.5 } else { 0.0 },
            ("tar", "action", "create") => if (has(&["pack", "create", "compress", "tar up", "bundle", "into", "-c", "excluding", "exclude", "except"]) || (has(&["targz", "tarball"]) && has(&["of", "from"]))) && !has(&["unpack", "extract", "without"]) { 6.5 } else { 0.0 },
            ("tar", "action", "extract") => if has(&["unpack", "extract", "untar", "decompress", "-x"]) && !has(&["without extracting", "without unpacking"]) { 6.5 } else { 0.4 },

            ("nmcli radio wifi", "state", "on") => if has(&["on", "enable", "activate", "start"]) && !has(&["off", "disable"]) { 6.0 } else { 0.0 },
            ("nmcli radio wifi", "state", "off") => if has(&["off", "disable", "deactivate", "stop"]) { 6.0 } else { 0.0 },

            ("git branch delete", "mode", "merged") => 1.0,
            ("git branch delete", "mode", "force") => if has(&["force", "unmerged", "even if", "anyway", "-d"]) { 6.5 } else { 0.0 },

            ("wc", "unit", "lines") => if has(&["line", "lines", "-l"]) { 6.5 } else { 0.5 },
            ("wc", "unit", "words") => if has(&["word", "words", "-w"]) { 6.5 } else { 0.0 },
            ("wc", "unit", "characters") if has(&["character", "characters", "char", "chars", "byte", "-m"]) => { 6.5 },

            _ => 0.0,
        };

        if option.description.to_lowercase().contains("does not say") || opt_name == "unspecified" || opt_name == "status" {
            s += 0.4;
        }
        let opt_terms = &entry.choice_opt_terms[&(slot_id.to_string(), opt_name.clone())];
        for (t, w) in q.term_weights {
            if let Some(&ow) = opt_terms.get(t) {
                s += 0.8 * w * ow;
            }
        }

        // A question (or an order to look) takes the option that only looks: `tar -t`, not `tar -x`.
        if q.mood == Mood::Question && option.read_only {
            s += 6.0;
        }
        let better = match best {
            None => true,
            Some((b, bs)) => s > bs || (s == bs && *opt_name < slot.options[b].name),
        };
        if better {
            best = Some((index, s));
        }
    }
    best.map_or(0, |(index, _)| index)
}

// ---------------------------------------------------------------------------
// Details: Path / Text Slot Role Disambiguation & Joint Binding
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotRole {
    SshUser,
    PortNumber,
    PingCount,
    TarOutDir,
    TarExclude,
    TarMember,
    ShellName,
    ContainerShell,
    SinceTime,
    UserName,
    SourcePath,
    DestPath,
    LinkTarget,
    LinkName,
    FirstPath,
    SecondPath,
    ArchiveFile,
    ArchiveInputs,
    Pattern,
    NamePattern,
    FindWhere,
    SearchWhere,
    LineCount,
    FieldNumber,
    DaysNumber,
    SizeSpec,
    MinutesNumber,
    PidNumber,
    ServiceUnit,
    Url,
    HostOrIp,
    PackageOrTerm,
    ProgramName,
    ContainerOrImage,
    VmOrNetwork,
    AiModel,
    ThemeOrFont,
    AppName,
    EnvVarName,
    EnvVarValue,
    AliasName,
    AliasCommand,
    MessageText,
    OwnerUser,
    GroupName,
    DiskDevice,
    NetInterface,
    KernelModule,
    BranchName,
    SessionName,
    WifiSsid,
    GenericPath,
    GenericText,
}

fn infer_slot_role(cmd_name: &str, slot_id: &str, kind: SlotKind) -> SlotRole {
    // `find delete` reads its folder and name pattern the way `find` does.
    let cmd_name = if cmd_name == "find delete" { "find" } else { cmd_name };
    match (cmd_name, slot_id) {
        ("tar", "member") => SlotRole::TarMember,
        ("useradd", "shell") => SlotRole::ShellName,
        ("docker exec", "shell") => SlotRole::ContainerShell,
        ("useradd" | "pkill" | "docker exec", "user" | "owner") => SlotRole::UserName,
        ("docker logs", "lines") => SlotRole::LineCount,
        (_, "exclude") => SlotRole::TarExclude,
        ("cp" | "mv" | "scp" | "rsync", "source") => SlotRole::SourcePath,
        ("cp" | "mv" | "scp" | "rsync", "dest") => SlotRole::DestPath,
        ("ln", "source") => SlotRole::LinkTarget,
        ("ln", "link") => SlotRole::LinkName,
        ("diff", "first") => SlotRole::FirstPath,
        ("diff", "second") => SlotRole::SecondPath,
        ("tar" | "zip" | "unzip" | "unrar", "archive") => SlotRole::ArchiveFile,
        ("tar" | "zip", "inputs") => SlotRole::ArchiveInputs,
        ("grep", "pattern") | ("search service log", "word") => SlotRole::Pattern,
        ("grep", "where") => SlotRole::SearchWhere,
        ("find", "where") => SlotRole::FindWhere,
        ("find", "name") => SlotRole::NamePattern,
        ("find", "changed") => SlotRole::DaysNumber,
        ("find", "size") => SlotRole::SizeSpec,
        ("head" | "tail", "count") | ("journalctl", "lines") | ("git log", "count") => SlotRole::LineCount,
        ("ssh", "user") => SlotRole::SshUser,
        (_, "port") => SlotRole::PortNumber,
        ("ping", "count") => SlotRole::PingCount,
        ("tar" | "unzip" | "curl", "outdir") => SlotRole::TarOutDir,
        ("journalctl", "since") => SlotRole::SinceTime,
        ("add user to group", "user") => SlotRole::UserName,
        ("cut", "field") => SlotRole::FieldNumber,
        ("omarchy reminder", "minutes") => SlotRole::MinutesNumber,
        ("omarchy reminder", "message") | ("git commit", "message") | ("append to system file", "line") => {
            SlotRole::MessageText
        }
        ("kill", "pid") => SlotRole::PidNumber,
        ("systemctl" | "systemctl --user", "service") | ("journalctl" | "search service log", "unit") => {
            SlotRole::ServiceUnit
        }
        (_, "url") => SlotRole::Url,
        (_, "host" | "address") => SlotRole::HostOrIp,
        (_, "package" | "term" | "crate") => SlotRole::PackageOrTerm,
        ("pkill" | "pgrep" | "which" | "man", "name") | ("systemd-inhibit", "command") => SlotRole::ProgramName,
        (_, "container" | "image") if cmd_name.starts_with("docker ") => SlotRole::ContainerOrImage,
        (_, "vm" | "network") if cmd_name.starts_with("virsh ") => SlotRole::VmOrNetwork,
        (_, "model") if cmd_name.starts_with("ollama ") => SlotRole::AiModel,
        (_, "theme" | "font") => SlotRole::ThemeOrFont,
        ("omarchy default", "app") => SlotRole::AppName,
        ("set shell variable" | "printenv", "name") => SlotRole::EnvVarName,
        ("set shell variable", "value") => SlotRole::EnvVarValue,
        ("add shell alias", "name") => SlotRole::AliasName,
        ("add shell alias", "command") => SlotRole::AliasCommand,
        ("chown", "owner") => SlotRole::OwnerUser,
        ("add user to group", "group") => SlotRole::GroupName,
        ("smartctl", "disk") => SlotRole::DiskDevice,
        ("ethtool", "interface") => SlotRole::NetInterface,
        ("modprobe", "module") => SlotRole::KernelModule,
        ("git switch", "branch") => SlotRole::BranchName,
        (_, "name") if cmd_name.starts_with("tmux ") => SlotRole::SessionName,
        (_, "ssid") => SlotRole::WifiSsid,
        _ => match kind {
            SlotKind::Path => SlotRole::GenericPath,
            SlotKind::Text | SlotKind::Choice => SlotRole::GenericText,
        },
    }
}

fn is_upper_env_name(s: &str) -> bool {
    s.len() >= 2
        && s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && s.chars().any(|c| c.is_ascii_uppercase())
}

fn looks_like_path_or_file(s: &str) -> bool {
    if s.contains('/')
        || s.starts_with('~')
        || s.contains('*')
        || s.contains('?')
        || (s.starts_with('.') && s.len() >= 2)
    {
        return true;
    }
    if let Some((stem_part, ext)) = s.rsplit_once('.') {
        return !stem_part.is_empty()
            && !ext.is_empty()
            && ext.len() <= 8
            && ext.chars().all(|c| c.is_ascii_alphanumeric())
            && !s.chars().all(|c| c.is_ascii_digit() || c == '.');
    }
    false
}

fn looks_like_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

fn looks_like_domain_host(s: &str) -> bool {
    let lower = s.to_lowercase();
    !s.contains('@')
        && !s.contains('/')
        && (lower.ends_with(".com")
            || lower.ends_with(".org")
            || lower.ends_with(".net")
            || lower.ends_with(".io")
            || lower.ends_with(".dev")
            || lower.ends_with(".edu")
            || lower.ends_with(".gov")
            || lower.ends_with(".local"))
}

fn looks_like_regular_file(s: &str) -> bool {
    if s.ends_with('/') || looks_like_ip(s) || looks_like_url(s) || looks_like_domain_host(s) {
        return false;
    }
    let path = std::path::Path::new(s);
    if path.is_file() {
        return true;
    }
    if path.is_dir() {
        return false;
    }
    let basename = s.rsplit('/').next().unwrap_or(s);
    if basename.starts_with('.') && basename.len() >= 2 && !basename.contains('/') {
        return matches!(
            basename.to_ascii_lowercase().as_str(),
            ".bashrc" | ".zshrc" | ".profile" | ".gitignore" | ".env" | ".vimrc" | ".tmux.conf"
        );
    }
    if let Some((stem_part, ext)) = basename.rsplit_once('.') {
        return !stem_part.is_empty()
            && !ext.is_empty()
            && ext.len() <= 8
            && ext.chars().all(|c| c.is_ascii_alphanumeric())
            && !basename.chars().all(|c| c.is_ascii_digit() || c == '.');
    }
    false
}

fn is_size_spec(s: &str) -> bool {
    let Some(unit) = s.chars().last() else { return false };
    let num = &s[..s.len() - unit.len_utf8()];
    !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) && matches!(unit, 'k' | 'M' | 'G')
}

fn looks_like_ip(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// Whether the request says `value` is a port ("port 3000", "localhost:3000").
fn is_port_number(value: &str, q: &QueryContext<'_>) -> bool {
    let lower = q.raw.to_lowercase();
    lower.contains(&format!("port {value}"))
        || lower.contains(&format!("port:{value}"))
        || lower.contains(&format!(":{value}"))
}

/// Names that are plainly a background service: well-known daemons, or anything ending in `d`
/// that is not an English word ("sshd", "crond").
fn looks_like_service_name(word: &str) -> bool {
    const KNOWN: &[&str] = &[
        "nginx",
        "apache",
        "apache2",
        "httpd",
        "sshd",
        "ssh",
        "docker",
        "postgresql",
        "postgres",
        "mysql",
        "mariadb",
        "redis",
        "mongodb",
        "cron",
        "crond",
        "cups",
        "bluetooth",
        "networkmanager",
        "sddm",
        "gdm",
        "lightdm",
        "ufw",
        "firewalld",
        "fail2ban",
        "tailscaled",
        "libvirtd",
        "smb",
        "samba",
        "nfs",
        "avahi-daemon",
        "pipewire",
        "wireplumber",
        "systemd-resolved",
        "systemd-networkd",
        "chronyd",
        "ntpd",
        "rsyslog",
        "containerd",
        "kubelet",
        "grafana",
        "prometheus",
        "jellyfin",
        "plex",
        "syncthing",
        "caddy",
        "traefik",
        "haproxy",
        "squid",
        "dnsmasq",
        "unbound",
        "openvpn",
        "wireguard",
        "elasticsearch",
        "rabbitmq",
        "memcached",
        "php-fpm",
        "mosquitto",
    ];
    KNOWN.contains(&word)
        || (word.len() >= 4
            && word.ends_with('d')
            && !["ed", "ld", "nd", "rd"].iter().any(|end| word.ends_with(end))
            && word.chars().all(|c| c.is_ascii_lowercase()))
}

/// Words that say the next value is to be left out.
const EXCLUSION_WORDS: &[&str] =
    &["excluding", "exclude", "except", "without", "skipping", "ignoring", "omitting", "minus"];

/// Whether the request names `cand` right after an exclusion word ("excluding node_modules",
/// "but not .git", "other than logs").
fn is_excluded_value(cand: &str, q: &QueryContext<'_>) -> bool {
    let Some(token) = find_raw_token(cand, q) else { return false };
    // "ignoring case" is a way of searching, not something to leave out.
    if matches!(
        token.lower.as_str(),
        "case" | "capitalization" | "capitalisation" | "whitespace" | "errors" | "warnings"
    ) {
        return false;
    }
    if EXCLUSION_WORDS.contains(&token.prev_word.as_str()) {
        return true;
    }
    let earlier = |back: usize| {
        token
            .word_pos
            .checked_sub(back)
            .and_then(|pos| q.raw_tokens.iter().find(|t| t.word_pos == pos))
            .map(|t| t.lower.as_str())
    };
    (token.prev_word == "not" && earlier(2) == Some("but"))
        || (token.prev_word == "than" && earlier(2) == Some("other"))
}

/// Files that hold no searchable text, by extension.
fn is_binary_name(name: &str) -> bool {
    const BINARY: &[&str] = &[
        "xlsx", "xls", "docx", "doc", "pptx", "ppt", "odt", "ods", "odp", "pdf", "zip", "gz", "tgz", "bz2", "xz", "7z",
        "rar", "tar", "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "heic", "mp3", "mp4", "mkv", "avi", "mov",
        "wav", "flac", "ogg", "webm", "iso", "img", "exe", "bin", "so", "o", "db", "sqlite", "epub",
    ];
    name.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && BINARY.contains(&ext.to_lowercase().as_str()))
}

/// Adjectives, participles and verbs that show up as "values" by accident: "show the slow log",
/// "is my clock synced", "restore my backup".
const STRAY_WORDS: &[&str] = &[
    "slow",
    "fast",
    "quick",
    "big",
    "small",
    "large",
    "old",
    "new",
    "high",
    "low",
    "long",
    "short",
    "hot",
    "cold",
    "full",
    "empty",
    "busy",
    "free",
    "clock",
    "time",
    "restore",
    "recover",
    "shorten",
    "lengthen",
    "sync",
    "synced",
    "not",
    "path",
    "everything",
    "anything",
    "something",
    "nothing",
    "all",
    "it",
    "them",
    "this",
    "that",
    "these",
    "those",
    "stuff",
    "things",
    "thing",
    "here",
    "there",
    "right",
    "wrong",
    "good",
    "bad",
    "ok",
    "okay",
    "correct",
    "enough",
    "almost",
    "again",
    "still",
    "really",
    "very",
    "much",
    "many",
    "more",
    "most",
    "less",
    "least",
];

/// A word taken as a name that the request does not present as one. Entries of the current
/// folder, paths, quoted text, numbers and words with a label ("folder logs", "called x") are
/// never stray.
fn is_stray_value(command: &Command, slot: &Slot, value: &str, candidates: &Candidates, q: &QueryContext<'_>) -> bool {
    let bare = value.trim_end_matches('/');
    if candidates.entry_kind(bare) != EntryKind::NotEntry
        || bare.is_empty()
        || value.starts_with('<')
        || bare.contains(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        || is_digits(bare)
    {
        return false;
    }
    let lower = bare.to_lowercase();
    let Some(token) = find_raw_token(bare, q) else { return false };
    if token.quoted || matches!(token.prev_word.as_str(), "called" | "named" | "group" | "user" | "service" | "unit") {
        return false;
    }
    if matches!(token.next_word.as_str(), "folder" | "directory" | "dir" | "file" | "service" | "group" | "user") {
        return false;
    }
    let first_program_word = command.name.split_whitespace().next().unwrap_or_default().to_lowercase();
    // Search words and the like may be any word; names of files, services and users may not be
    // adjectives, participles or the command's own name.
    let names_a_thing = slot.kind == SlotKind::Path || slot.value_type.is_some() || slot.id == "unit";
    if !names_a_thing {
        return false;
    }
    let participle =
        slot.kind == SlotKind::Path && lower.len() > 4 && (lower.ends_with("ed") || lower.ends_with("ing"));
    STRAY_WORDS.contains(&lower.as_str())
        || participle
        || lower == command.program()
        || lower == first_program_word
        || (slot.kind == SlotKind::Path && is_upper_env_name(bare))
}

/// Programs that are also ordinary topic words ("docker", "git"): they are a name to act on only
/// when the request points at them ("is docker running", "enable docker at boot").
fn is_product_target(lower: &str, prev: &str, next: &str) -> bool {
    matches!(lower, "docker" | "git" | "tmux" | "ollama" | "tailscale" | "virsh")
        && (matches!(
            prev,
            "is" | "are"
                | "enable"
                | "disable"
                | "start"
                | "stop"
                | "restart"
                | "reload"
                | "status"
                | "named"
                | "called"
                | "kill"
        ) || matches!(next, "running" | "service" | "daemon" | "process" | "processes" | "at" | "on" | "is" | "up"))
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

fn is_non_value_english_word(lower: &str) -> bool {
    STOPWORDS.contains(&lower)
        || matches!(
            lower,
            "make"
                | "list"
                | "show"
                | "get"
                | "set"
                | "add"
                | "remove"
                | "delete"
                | "erase"
                | "rid"
                | "start"
                | "stop"
                | "restart"
                | "follow"
                | "watch"
                | "close"
                | "run"
                | "open"
                | "read"
                | "write"
                | "written"
                | "connect"
                | "disconnect"
                | "launch"
                | "halt"
                | "scan"
                | "join"
                | "fetch"
                | "talk"
                | "chat"
                | "call"
                | "called"
                | "named"
                | "create"
                | "build"
                | "compile"
                | "copy"
                | "duplicate"
                | "move"
                | "rename"
                | "put"
                | "unpack"
                | "pack"
                | "extract"
                | "compress"
                | "decompress"
                | "check"
                | "verify"
                | "find"
                | "locate"
                | "search"
                | "look"
                | "print"
                | "count"
                | "sort"
                | "order"
                | "compare"
                | "edit"
                | "modify"
                | "modified"
                | "change"
                | "changed"
                | "protect"
                | "enable"
                | "disable"
                | "turn"
                | "switch"
                | "apply"
                | "use"
                | "used"
                | "install"
                | "uninstall"
                | "update"
                | "upgrade"
                | "unload"
                | "load"
                | "boot"
                | "reboot"
                | "shutdown"
                | "power"
                | "lock"
                | "logout"
                | "remind"
                | "save"
                | "download"
                | "upload"
                | "sync"
                | "kill"
                | "terminate"
                | "force"
                | "pull"
                | "push"
                | "stage"
                | "commit"
                | "clone"
                | "attach"
                | "reattach"
                | "keep"
                | "allow"
                | "prevent"
                | "take"
                | "bring"
                | "free"
                | "appear"
                | "occurs"
                | "mention"
                | "contain"
                | "point"
                | "points"
                | "belong"
                | "leave"
                | "clear"
                | "clean"
                | "file"
                | "files"
                | "folder"
                | "folders"
                | "directory"
                | "directories"
                | "dir"
                | "contents"
                | "content"
                | "line"
                | "lines"
                | "word"
                | "words"
                | "character"
                | "characters"
                | "program"
                | "programs"
                | "process"
                | "processes"
                | "service"
                | "services"
                | "container"
                | "containers"
                | "image"
                | "images"
                | "vm"
                | "vms"
                | "virtual"
                | "machine"
                | "archive"
                | "checksum"
                | "permissions"
                | "permission"
                | "executable"
                | "runnable"
                | "frozen"
                | "hung"
                | "running"
                | "stopped"
                | "installed"
                | "available"
                | "alphabetically"
                | "numerically"
                | "recursively"
                | "permanently"
                | "gracefully"
                | "cleanly"
                | "automatically"
                | "completely"
                | "locally"
                | "live"
                | "fast"
                | "hidden"
                | "recent"
                | "recently"
                | "most"
                | "biggest"
                | "smallest"
                | "largest"
                | "command"
                | "commands"
                | "alias"
                | "variable"
                | "variables"
                | "value"
                | "column"
                | "columns"
                | "field"
                | "fields"
                | "minute"
                | "minutes"
                | "day"
                | "days"
                | "megabyte"
                | "megabytes"
                | "gigabyte"
                | "gigabytes"
                | "rust"
                | "crate"
                | "project"
                | "branch"
                | "branches"
                | "session"
                | "sessions"
                | "theme"
                | "themes"
                | "font"
                | "fonts"
                | "network"
                | "wifi"
                | "wireless"
                | "disk"
                | "disks"
                | "healthy"
                | "health"
                | "speed"
                | "link"
                | "shortcut"
                | "owner"
                | "group"
                | "groups"
                | "module"
                | "kernel"
                | "server"
                | "computer"
                | "laptop"
                | "terminal"
                | "local"
                | "remote"
                | "uncommitted"
                | "changes"
                | "empty"
                | "new"
                | "old"
                | "all"
                | "only"
                | "every"
                | "any"
                | "long"
                | "huge"
                | "much"
                | "many"
                | "background"
                | "again"
                | "inside"
                | "outside"
                | "under"
                | "within"
                | "console"
                | "definition"
                | "cards"
                | "card"
                | "manual"
                | "package"
                | "packages"
                | "aur"
                | "tmux"
                | "ollama"
                | "docker"
                | "git"
                | "virsh"
                | "tailscale"
                | "tailnet"
                | "past"
                | "last"
                | "first"
                | "top"
                | "bottom"
                | "beginning"
                | "end"
                | "up"
                | "down"
                | "off"
                | "out"
                | "back"
                | "here"
                | "there"
                | "today"
                | "time"
                | "date"
                | "everything"
                | "nothing"
                | "something"
                | "anything"
                | "everyone"
                | "nobody"
                | "anyone"
                | "someone"
                | "sleep"
                | "awake"
                | "dependency"
                | "memory"
                | "space"
                | "storage"
                | "cpu"
                | "ram"
                | "model"
                | "models"
                | "logs"
                | "log"
                | "reader"
                | "teammates"
                | "export"
                | "shell"
                | "route"
                | "routes"
                | "routing"
                | "table"
                | "gateway"
                | "internet"
                | "connected"
                | "online"
                | "journald"
                | "browser"
                | "editor"
                | "application"
                | "omarchy"
                | "using"
                | "working"
                | "port"
                | "myself"
                | "ago"
                | "since"
                | "until"
                | "excluding"
                | "except"
                | "respond"
                | "responding"
                | "responds"
                | "wont"
                | "won't"
                | "doesnt"
                | "see"
                | "quit"
                | "exit"
                | "each"
                | "previous"
                | "go"
                | "going"
                | "usb"
                | "drive"
                | "stick"
                | "pendrive"
                | "sizes"
                | "size"
                | "taking"
                | "tell"
                | "know"
                | "display"
                | "view"
                | "work"
                | "works"
                | "happen"
                | "happening"
                | "happened"
                | "sorted"
                | "times"
                | "twice"
                | "once"
                | "please"
                | "now"
                | "also"
                | "both"
                | "same"
                | "other"
                | "another"
                | "own"
                | "way"
                | "ways"
                | "kind"
                | "type"
                | "types"
                | "thing"
                | "things"
                | "stuff"
        )
}

fn find_phrase_match<'a>(cand: &str, q: &'a QueryContext<'a>) -> Option<(usize, &'a str, &'a str)> {
    let parts: Vec<String> = cand.split_whitespace().map(|p| p.to_lowercase()).collect();
    if parts.len() < 2 {
        return None;
    }
    // Deduplicate raw_tokens entries that share the same word_pos (number_word duplicates).
    let mut seq: Vec<&RawToken> = Vec::new();
    for rt in &q.raw_tokens {
        if seq.last().is_none_or(|prev| prev.word_pos != rt.word_pos) {
            seq.push(rt);
        }
    }
    for win in seq.windows(parts.len()) {
        if win.iter().zip(&parts).all(|(rt, p)| rt.lower == *p) {
            let first = win[0];
            let last = win[win.len() - 1];
            return Some((first.word_pos, first.prev_word.as_str(), last.next_word.as_str()));
        }
    }
    None
}

fn indirect_file_match_score(cand: &str, q: &QueryContext<'_>) -> f64 {
    if q.raw_tokens.iter().any(|rt| rt.text == cand || rt.lower == cand.to_lowercase()) {
        return 0.0;
    }
    if find_phrase_match(cand, q).is_some() {
        return 12.5;
    }
    let lower_cand = cand.to_lowercase();
    let (stem_part, ext_part) = lower_cand.rsplit_once('.').unwrap_or((&lower_cand, ""));
    let stem_part = stem_part.strip_suffix(".tar").unwrap_or(stem_part);
    // Only the distinctive words of a name count: never "my", "the" and the like.
    let stem_words: Vec<&str> = stem_part
        .split(['-', '_', ' ', '.'])
        .filter(|p| p.len() >= 2 && !STOPWORDS.contains(p) && !matches!(*p, "file" | "files" | "folder" | "copy"))
        .collect();
    if stem_words.is_empty() {
        return 0.0;
    }
    // A name the request only half spells ("projects" for projects-download.zip) is not that entry.
    if !stem_words.iter().all(|sw| q.words.iter().any(|w| w == sw)) {
        return 0.0;
    }
    // "a file named notes" is a name to look for, not the entry notes.txt.
    if stem_words.iter().any(|sw| {
        q.raw_tokens.iter().any(|rt| rt.lower == *sw && matches!(rt.prev_word.as_str(), "named" | "called" | "name"))
    }) {
        return 0.0;
    }

    let mut score = 5.5 * stem_words.len() as f64;

    match ext_part {
        "sh" | "bash" | "py" | "rb" | "pl" if q.has_any(&["script", "runnable", "executable"]) => score += 2.5,
        "toml" | "yaml" | "yml" | "json" | "conf" | "ini" if q.has_any(&["config", "settings"]) => score += 2.0,
        "log" if q.has_word("log") => score += 2.0,
        "md" | "txt" if q.has_any(&["readme", "notes", "doc", "text", "file"]) => score += 1.5,
        "" if q.has_any(&["folder", "directory", "dir"]) => score += 2.0,
        _ => {}
    }
    score
}

fn find_raw_token<'a>(cand: &str, q: &'a QueryContext<'a>) -> Option<&'a RawToken> {
    // "~" stands for "my home folder".
    let cand = if cand == "~" { "home" } else { cand };
    q.raw_tokens
        .iter()
        .find(|rt| rt.text == cand)
        .or_else(|| q.raw_tokens.iter().find(|rt| rt.lower == cand.to_lowercase()))
}

/// Assigns the offered values to the command's path and text slots, each value
/// to at most one slot. Returns, per slot, how sure the binding is and the value
/// ("" when nothing fits).
fn bind_slots(
    cmd: &Command,
    slot_picks: &HashMap<String, BTreeSet<&str>>,
    choice_picks: &HashMap<String, String>,
    q: &QueryContext<'_>,
    candidates: &Candidates,
) -> HashMap<String, (f64, String)> {
    let cmd_name = cmd.name.as_str();

    let ordered_slots: Vec<(String, SlotKind)> = cmd
        .slots
        .iter()
        .filter(|s| s.kind != SlotKind::Choice && slot_picks.contains_key(&s.id))
        .map(|s| (s.id.clone(), s.kind))
        .collect();

    let tar_skip_inputs = cmd_name == "tar" && choice_picks.get("action").is_some_and(|a| a != "create");
    let tar_extracts = cmd_name == "tar" && choice_picks.get("action").is_some_and(|a| a == "extract");
    let typed = TypedValues::new(cmd, q);

    let mut slot_candidates: Vec<(String, Vec<(String, f64)>)> = Vec::new();
    let to_split_pos = find_preposition_pos(q, &["to", "into", "as"]);

    for (slot_id, kind) in &ordered_slots {
        if (tar_skip_inputs && matches!(slot_id.as_str(), "inputs" | "exclude"))
            || (cmd_name == "tar" && !tar_extracts && matches!(slot_id.as_str(), "outdir" | "member"))
        {
            slot_candidates.push((slot_id.clone(), vec![]));
            continue;
        }
        let role = infer_slot_role(cmd_name, slot_id, *kind);
        let mut cands_scored = Vec::new();

        for &cand in &slot_picks[slot_id] {
            let entry = if *kind == SlotKind::Path { candidates.entry_kind(cand) } else { EntryKind::NotEntry };
            let s = score_candidate_for_role(cmd_name, cand, role, *kind, entry, to_split_pos, q);
            let s = typed.adjust(s, cand, slot_id, role, q);
            // Unpacking: the archive is the file with an archive name; "to /opt/app" is the folder.
            if tar_extracts && role == SlotRole::ArchiveFile && s < 9.0 {
                continue;
            }
            if s > 0.5 {
                cands_scored.push((cand.to_string(), s));
            }
        }
        cands_scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        slot_candidates.push((slot_id.clone(), cands_scored));
    }

    let mut used: HashSet<String> = HashSet::new();
    let mut result: HashMap<String, (f64, String)> = HashMap::new();

    let mut order: Vec<usize> = (0..slot_candidates.len()).collect();
    order.sort_by_key(|&i| {
        let slot_id = &slot_candidates[i].0;
        let kind = ordered_slots[i].1;
        let role = infer_slot_role(cmd_name, slot_id, kind);
        role_priority(role)
    });

    for idx in order {
        let (slot_id, list) = &slot_candidates[idx];
        let pick = list.iter().find(|(c, _)| !used.contains(c));
        if let Some((cand, score)) = pick {
            used.insert(cand.clone());
            let named_prob = if *score >= 4.0 {
                0.94
            } else if *score >= 2.2 {
                0.82
            } else {
                0.50
            };
            let role = infer_slot_role(cmd_name, slot_id, ordered_slots[idx].1);
            result.insert(slot_id.clone(), (named_prob, shape_value(role, cand, candidates, q)));
        } else {
            result.insert(slot_id.clone(), (0.05, String::new()));
        }
    }

    result
}

/// A label in the request: "group docker" says `docker` is a group. The label word marks the value
/// beside it and is never a value itself.
#[derive(Debug, Clone)]
struct Label {
    kind: &'static str,
    label_pos: usize,
    value_pos: usize,
}

fn label_kind(word: &str) -> Option<&'static str> {
    match word {
        "group" => Some("group"),
        "unit" | "service" | "daemon" => Some("service"),
        "user" | "username" => Some("user"),
        "container" => Some("container"),
        "port" => Some("port"),
        "branch" => Some("branch"),
        "shell" => Some("shell"),
        _ => None,
    }
}

/// Whether `word` can be the thing a label names (not a filler, verb or another label).
fn can_be_labelled(word: &str) -> bool {
    const SKIP: &[&str] = &[
        "exec",
        "execute",
        "since",
        "until",
        "after",
        "before",
        "status",
        "running",
        "logs",
        "log",
        "journal",
        "check",
        "display",
        "view",
        "tell",
        "give",
        "find",
        "kill",
        "reload",
        "enable",
        "disable",
        "errors",
        "error",
        "older",
        "newer",
        "recursively",
        "called",
        "named",
        "name",
        "names",
        "details",
        "info",
        "all",
        "every",
        "any",
        "up",
        "down",
        "on",
        "off",
        "not",
        "failed",
        "crashed",
        "stopped",
        "started",
        "again",
        "permissions",
        "access",
        "rights",
        "membership",
        "members",
        "owner",
        "owned",
        "that",
        "dir",
        "home",
        "directory",
        "with",
        "without",
        "owns",
        "inside",
        "into",
        "onto",
        "under",
        "within",
        "through",
        "over",
        "about",
        "around",
        "near",
        "between",
        "via",
        "using",
        "per",
        "make",
        "list",
        "show",
        "get",
        "set",
        "add",
        "remove",
        "delete",
        "erase",
        "start",
        "restart",
        "stop",
        "follow",
        "watch",
        "close",
        "run",
        "open",
        "read",
        "write",
        "connect",
        "launch",
        "create",
        "new",
        "my",
        "your",
        "other",
        "same",
    ];
    !word.is_empty() && !STOPWORDS.contains(&word) && !SKIP.contains(&word) && label_kind(word).is_none()
}

/// The labels the request writes: the value is the word after the label ("group docker"), or when
/// that is no value, the word before it ("docker group", "bash shell").
fn find_labels(q: &QueryContext<'_>) -> Vec<Label> {
    let mut seq: Vec<&RawToken> = Vec::new();
    for rt in &q.raw_tokens {
        if seq.last().is_none_or(|prev| prev.word_pos != rt.word_pos) {
            seq.push(rt);
        }
    }
    let mut out: Vec<Label> = Vec::new();
    for (i, rt) in seq.iter().enumerate() {
        let Some(kind) = (!rt.quoted).then(|| label_kind(&rt.lower)).flatten() else { continue };
        let taken = |pos: usize, out: &[Label]| out.iter().any(|l| l.value_pos == pos || l.label_pos == pos);
        // After the label, past "the", "a" and "called".
        let mut j = i + 1;
        while seq.get(j).is_some_and(|n| matches!(n.lower.as_str(), "the" | "a" | "an" | "called" | "named")) {
            j += 1;
        }
        let after = seq.get(j).filter(|n| (n.quoted || can_be_labelled(&n.lower)) && !taken(n.word_pos, &out));
        // Before the label, past "the" and "a".
        let mut k = i;
        let mut before = None;
        while k > 0 {
            k -= 1;
            if matches!(seq[k].lower.as_str(), "the" | "a" | "an" | "my") {
                continue;
            }
            before = Some(seq[k]).filter(|n| can_be_labelled(&n.lower) && !taken(n.word_pos, &out));
            break;
        }
        // "user", "port" and "branch" lead into their value; "docker service", "bash shell" and
        // "docker group" put it first. A word after the label wins when both are possible.
        let trailing = matches!(kind, "service" | "group" | "shell" | "container");
        let found = if trailing { after.copied().or(before) } else { after.copied() };
        if let Some(value) = found {
            out.push(Label { kind, label_pos: rt.word_pos, value_pos: value.word_pos });
        }
    }
    out
}

/// The (label, value) pairs a request writes: ("group", "docker"), ("branch", "main"). Used to check
/// that a typed name did not vanish from the line.
pub fn labelled_values(request: &str) -> Vec<(String, String)> {
    let norm = normalize_text(request);
    let words = raw_lower_words(&norm);
    let terms = extract_weighted_terms(request);
    let q = QueryContext::new(request, &norm, &words, &terms);
    find_labels(&q)
        .into_iter()
        .filter(|l| l.kind != "port")
        .filter_map(|l| {
            let value = q.raw_tokens.iter().find(|t| t.word_pos == l.value_pos)?.text.clone();
            let label = q.raw_tokens.iter().find(|t| t.word_pos == l.label_pos)?.text.clone();
            Some((label, value))
        })
        .collect()
}

/// What kind of name a role takes, for slots whose catalog entry has no `type`.
fn role_type(role: SlotRole) -> Option<&'static str> {
    match role {
        SlotRole::ServiceUnit => Some("service"),
        SlotRole::UserName | SlotRole::OwnerUser | SlotRole::SshUser => Some("user"),
        SlotRole::GroupName => Some("group"),
        SlotRole::ShellName => Some("shell"),
        SlotRole::ContainerOrImage => Some("container"),
        SlotRole::BranchName => Some("branch"),
        SlotRole::PortNumber => Some("port"),
        _ => None,
    }
}

/// Labels and system names, used to tell which word fills a typed slot.
struct TypedValues {
    labels: Vec<Label>,
    /// Slot id to its declared `type`.
    declared: HashMap<String, String>,
    facts: Option<std::rc::Rc<crate::sysinfo::Facts>>,
    /// The program the command runs: its own name is never a name to act on.
    program: String,
}

impl TypedValues {
    fn new(cmd: &Command, q: &QueryContext<'_>) -> Self {
        let declared: HashMap<String, String> =
            cmd.slots.iter().filter_map(|s| s.value_type.clone().map(|t| (s.id.clone(), t))).collect();
        let facts = declared.values().any(|t| crate::sysinfo::has_list(t)).then(crate::sysinfo::facts);
        Self { labels: find_labels(q), declared, facts, program: cmd.program().to_lowercase() }
    }

    /// Rescores `cand` for a slot: a label word is never a value, a labelled value goes to the slot
    /// of its kind only, and a typed slot checks names against what the system lists.
    fn adjust(&self, score: f64, cand: &str, slot_id: &str, role: SlotRole, q: &QueryContext<'_>) -> f64 {
        if self.labels.is_empty() && self.facts.is_none() {
            return score;
        }
        let declared = self.declared.get(slot_id).map(String::as_str);
        // A slot named after a label kind ("branch", "user", "group") takes that kind of value,
        // so entries need no explicit `type` for labels to reach them.
        let slot_type = declared.or_else(|| role_type(role)).or_else(|| label_kind(slot_id));
        let label_word_at = |pos: usize| self.labels.iter().any(|l| l.label_pos == pos);
        let Some(token) = find_raw_token(cand, q) else {
            // A phrase that swallowed a label ("group docker") is not a value.
            let swallowed = cand.contains(' ')
                && cand
                    .split_whitespace()
                    .any(|w| q.raw_tokens.iter().any(|t| t.lower == w.to_lowercase() && label_word_at(t.word_pos)));
            return if swallowed { 0.0 } else { score };
        };
        if label_word_at(token.word_pos) {
            return 0.0;
        }
        if let Some(label) = self.labels.iter().find(|l| l.value_pos == token.word_pos) {
            return if slot_type == Some(label.kind) { 9.9 } else { 0.0 };
        }
        let Some(slot_type) = slot_type else { return score };
        if matches!(slot_type, "service" | "user" | "group") && cand.eq_ignore_ascii_case(&self.program) {
            return 0.0;
        }
        if self.labels.iter().any(|l| l.kind == slot_type) {
            // The request labels this slot's value; no other word is it.
            return 0.0;
        }
        let Some(facts) = self.facts.as_ref().filter(|_| declared.is_some_and(crate::sysinfo::has_list)) else {
            return score;
        };
        let lower = cand.to_lowercase();
        if facts.knows(slot_type, cand) {
            let pointed_at = is_product_target(&lower, &token.prev_word, &token.next_word);
            return if (is_non_value_english_word(&lower) && !pointed_at) || is_digits(cand) {
                score
            } else {
                score.max(8.8)
            };
        }
        // The name of a login shell is not a user, a service or a group.
        if slot_type != "shell" && facts.knows("shell", cand) {
            return 0.0;
        }
        match slot_type {
            "shell" => 0.0,
            "service" if score < 8.5 && !looks_like_service_name(&lower) => 0.0,
            "user" | "group" if score < 8.5 => score.min(3.0),
            _ => score,
        }
    }
}

/// The text that goes into the line for a picked value: a folder that receives a copy ends in `/`,
/// and a bare name to look for ("a file named notes") becomes a pattern that also finds `notes.txt`.
fn shape_value(role: SlotRole, cand: &str, candidates: &Candidates, q: &QueryContext<'_>) -> String {
    match role {
        SlotRole::DestPath if candidates.entry_kind(cand) == EntryKind::Dir => format!("{cand}/"),
        // "bash" is written as the path /etc/shells lists for it.
        SlotRole::ShellName => crate::sysinfo::facts().shell_path(cand).unwrap_or_else(|| cand.to_string()),
        SlotRole::NamePattern
            if cand.chars().all(|c| c.is_alphabetic())
                && find_raw_token(cand, q)
                    .is_some_and(|rt| matches!(rt.prev_word.as_str(), "named" | "called" | "name")) =>
        {
            format!("{cand}*")
        }
        _ => cand.to_string(),
    }
}

fn role_priority(role: SlotRole) -> u8 {
    match role {
        SlotRole::ArchiveFile | SlotRole::Url | SlotRole::DiskDevice => 1,
        SlotRole::PortNumber | SlotRole::PingCount => 1,
        SlotRole::SshUser | SlotRole::UserName | SlotRole::TarExclude | SlotRole::SinceTime => 3,
        SlotRole::TarOutDir => 4,
        SlotRole::PidNumber
        | SlotRole::FieldNumber
        | SlotRole::DaysNumber
        | SlotRole::SizeSpec
        | SlotRole::MinutesNumber
        | SlotRole::LineCount => 2,
        SlotRole::NamePattern | SlotRole::EnvVarName | SlotRole::AliasName | SlotRole::LinkName | SlotRole::Pattern => {
            3
        }
        SlotRole::ServiceUnit | SlotRole::OwnerUser | SlotRole::DestPath | SlotRole::LinkTarget => 4,
        SlotRole::FindWhere | SlotRole::SearchWhere | SlotRole::SourcePath => 5,
        _ => 6,
    }
}

fn find_preposition_pos(q: &QueryContext<'_>, preps: &[&str]) -> Option<usize> {
    for rt in &q.raw_tokens {
        if !rt.quoted && preps.contains(&rt.lower.as_str()) {
            if rt.lower == "to"
                && matches!(
                    rt.prev_word.as_str(),
                    "want" | "need" | "like" | "how" | "going" | "used" | "trying" | "able" | "refuses" | "force"
                )
            {
                continue;
            }
            return Some(rt.word_pos);
        }
    }
    None
}

fn is_adjective_or_determiner(lower: &str) -> bool {
    STOPWORDS.contains(&lower)
        || matches!(
            lower,
            "new"
                | "empty"
                | "hidden"
                | "all"
                | "every"
                | "any"
                | "other"
                | "same"
                | "current"
                | "most"
                | "first"
                | "last"
                | "biggest"
                | "largest"
                | "smallest"
                | "huge"
                | "long"
                | "old"
                | "local"
                | "remote"
                | "virtual"
                | "whole"
                | "entire"
                | "each"
                | "see"
                | "view"
                | "show"
                | "display"
                | "list"
                | "find"
                | "open"
                | "go"
                | "previous"
                | "next"
                | "another"
                | "both"
                | "sorted"
        )
}

/// A lowercase word with nothing around it that says it is the value (no "called", no quotes, no
/// path shape) is a guess unless it is a program the system knows: it scores low enough that the
/// line carries a "not sure" note, or asks.
fn bare_word(lower: &str, strong: f64) -> f64 {
    if platform::is_known_program(lower) { strong } else { 2.0 }
}

/// How well `cand` fills a slot with this role, adjusted for what the current folder says about it:
/// an entry named exactly beats one that merely shares a word, and "folder" asks for a folder.
fn score_candidate_for_role(
    cmd_name: &str,
    cand: &str,
    role: SlotRole,
    kind: SlotKind,
    entry: EntryKind,
    to_split_pos: Option<usize>,
    q: &QueryContext<'_>,
) -> f64 {
    // "excluding node_modules": the value named after an exclusion word is never a destination or target.
    if role != SlotRole::TarExclude && is_excluded_value(cand, q) {
        return 0.0;
    }
    // "-9" in "kill -9 java" is an option of the command, not a name.
    if cand.starts_with('-') && !matches!(role, SlotRole::DaysNumber | SlotRole::PidNumber | SlotRole::LineCount) {
        return 0.0;
    }
    // A spreadsheet or picture is not searched for text unless the request names that file.
    if matches!(role, SlotRole::SearchWhere) && is_binary_name(cand) && !q.raw_tokens.iter().any(|t| t.text == cand) {
        return 0.0;
    }
    let mut score = score_by_role(cmd_name, cand, role, kind, entry, to_split_pos, q);
    if score <= 0.0 || entry == EntryKind::NotEntry {
        return score;
    }
    let token = find_raw_token(cand, q);
    let next = token.map_or("", |t| t.next_word.as_str());
    if token.is_some() {
        score += 6.0;
    } else if q.raw_tokens.iter().any(|t| !t.quoted && t.text.contains('/') && t.text.len() > 1) {
        // A path written out in the request beats an entry that merely shares a word with it.
        score = score.min(8.0);
    }
    if matches!(next, "folder" | "directory" | "dir" | "folders") {
        if entry == EntryKind::Dir {
            score += 1.5;
        } else if cand.contains('.') {
            score = 0.0;
        }
    }
    score
}

// Hand-tuned keyword tables read best one rule per line.
#[rustfmt::skip]
fn score_by_role(
    cmd_name: &str,
    cand: &str,
    role: SlotRole,
    kind: SlotKind,
    entry: EntryKind,
    to_split_pos: Option<usize>,
    q: &QueryContext<'_>,
) -> f64 {
    let is_from_files = entry != EntryKind::NotEntry;
    if cand == "." {
        return if role == SlotRole::SearchWhere { 3.0 } else { 0.0 };
    }
    // The command's own name at the start of the request ("chown -R ...") is not a value.
    if role != SlotRole::PackageOrTerm
        && q.raw_tokens.first().is_some_and(|first| first.lower == cand.to_lowercase())
        && cmd_name.split_whitespace().next().is_some_and(|name| name.eq_ignore_ascii_case(cand))
    {
        return 0.0;
    }
    // `-ri` and `-R` are options the person typed, not values.
    if cand.len() > 1 && cand.starts_with('-') && !cand.chars().skip(1).all(|c| c.is_ascii_digit()) {
        return 0.0;
    }
    let rt_opt = find_raw_token(cand, q);
    let phrase_match = if rt_opt.is_none() && cand.contains(' ') {
        find_phrase_match(cand, q)
    } else {
        None
    };
    let is_ext_glob_cand = role == SlotRole::NamePattern
        && cand.starts_with("*.")
        && q.raw_tokens.iter().any(|rt| {
            let ext = cand.trim_start_matches("*.").to_lowercase();
            rt.lower == format!(".{ext}") || rt.lower == ext
        });
    let is_size_cand = role == SlotRole::SizeSpec && is_size_spec(cand);
    let indirect = if kind == SlotKind::Path { indirect_file_match_score(cand, q) } else { 0.0 };

    if rt_opt.is_none() && phrase_match.is_none() && !is_ext_glob_cand && !is_size_cand && indirect == 0.0 {
        return 0.0;
    }

    let lower = cand.to_lowercase();
    let is_multi_phrase = cand.contains(' ');
    // "all files" and "every folder" quantify things; they are not the name of one.
    if kind == SlotKind::Path
        && is_multi_phrase
        && !is_from_files
        && lower.split_whitespace().next().is_some_and(|w| matches!(w, "all" | "every" | "each" | "any" | "the" | "my"))
        && lower.split_whitespace().skip(1).all(|w| {
            matches!(w, "files" | "file" | "folders" | "folder" | "directories" | "directory" | "things" | "items" | "stuff")
        })
    {
        return 0.0;
    }
    let quoted = rt_opt.is_some_and(|r| r.quoted);
    let pos = rt_opt
        .map(|r| r.word_pos)
        .or_else(|| phrase_match.map(|(p, _, _)| p))
        .unwrap_or(usize::MAX / 2);
    let prev = rt_opt
        .map(|r| r.prev_word.as_str())
        .or_else(|| phrase_match.map(|(_, pr, _)| pr))
        .unwrap_or("");
    let next = rt_opt
        .map(|r| r.next_word.as_str())
        .or_else(|| phrase_match.map(|(_, _, nx)| nx))
        .unwrap_or("");

    let is_named_dir_or_file = kind == SlotKind::Path
        && (matches!(next, "folder" | "directory" | "dir" | "file")
            // "the directory build": the label comes first.
            || matches!(prev, "folder" | "directory" | "dir")
                && rt_opt.is_some_and(|r| {
                    r.word_pos >= 2 && matches!(q.words.get(r.word_pos - 2).map(String::as_str), Some("the" | "a" | "my"))
                }))
        && (!is_adjective_or_determiner(&lower) || is_from_files);
    let is_path = looks_like_path_or_file(cand) || indirect > 0.0 || is_from_files || is_named_dir_or_file;
    let explicitly_labeled = matches!(prev, "called" | "named")
        || matches!(next, "service" | "group" | "container" | "image" | "vm" | "network" | "model" | "branch" | "session")
        || (role == SlotRole::ServiceUnit && matches!(next, "log" | "logs" | "unit" | "daemon"));

    // "docker", "make" and "tree" are ordinary words elsewhere but real packages here.
    let known_package = role == SlotRole::PackageOrTerm && crate::platform::is_known_program(&lower);
    if !quoted && !is_path && !is_digits(cand) && !explicitly_labeled && !known_package && is_non_value_english_word(&lower)
        && !is_multi_phrase
            && !is_upper_env_name(cand)
            && !(matches!(role, SlotRole::ServiceUnit | SlotRole::ProgramName) && (prev == "for" || is_product_target(&lower, prev, next)))
            // "delete .log files in logs": a folder named like a thing.
            && !(role == SlotRole::FindWhere && matches!(lower.as_str(), "logs" | "log") && matches!(prev, "in" | "under" | "inside" | "within"))
        {
            return 0.0;
        }

    match role {
        SlotRole::SshUser | SlotRole::UserName => {
            let cue = matches!(prev, "as" | "user" | "username" | "login" | "account")
                || (role == SlotRole::UserName && matches!(prev, "by" | "owner"))
                // "named" and "called" name a user only where a user is being made.
                || (role == SlotRole::UserName && matches!(prev, "called" | "named") && cmd_name != "pkill")
                || (role == SlotRole::UserName && prev == "add" && next != "group");
            if cue && !is_path && !is_digits(cand) && !is_non_value_english_word(&lower) { 9.0 } else { 0.0 }
        }
        SlotRole::PortNumber => {
            if is_digits(cand) && (prev == "port" || next == "port") { 9.5 } else { 0.0 }
        }
        SlotRole::PingCount => {
            if is_digits(cand) && matches!(next, "times" | "time" | "packets" | "pings") { 9.5 } else { 0.0 }
        }
        SlotRole::TarOutDir => {
            if matches!(prev, "to" | "into" | "in") && (is_path || is_named_dir_or_file) && !is_digits(cand) { 9.0 } else { 0.0 }
        }
        SlotRole::ShellName => 0.0,
        SlotRole::ContainerShell => {
            // The shell program to start inside a container; the container's own list, not this machine's.
            if matches!(lower.as_str(), "bash" | "zsh" | "fish" | "ash" | "dash" | "ksh" | "csh" | "tcsh" | "sh") { 9.0 } else { 0.0 }
        }
        SlotRole::TarMember => {
            let archive = lower.ends_with(".tar.gz") || lower.ends_with(".tgz") || lower.ends_with(".tar") || lower.ends_with(".zip");
            if !archive && is_path && !is_digits(cand) && (matches!(next, "from" | "out" | "inside" | "in") || matches!(prev, "only" | "just" | "member" | "extract" | "unpack")) {
                9.0
            } else {
                0.0
            }
        }
        SlotRole::TarExclude => {
            if is_excluded_value(cand, q) && !is_digits(cand) { 9.0 } else { 0.0 }
        }
        SlotRole::SinceTime => {
            if prev != "since" || (is_digits(cand) && !is_multi_phrase) { 0.0 } else if is_multi_phrase { 9.5 } else { 9.0 }
        }
        SlotRole::PidNumber => {
            if is_digits(cand) { 9.0 } else { 0.0 }
        }
        SlotRole::FieldNumber => {
            if is_digits(cand) && (prev == "column" || prev == "field" || next == "column" || next == "field" || prev == "col") {
                9.5
            } else if is_digits(cand) {
                7.0
            } else {
                0.0
            }
        }
        SlotRole::DaysNumber => {
            if is_digits(cand) && matches!(next, "day" | "days") { 9.5 } else { 0.0 }
        }
        SlotRole::SizeSpec => {
            if is_size_spec(cand) { 9.5 } else { 0.0 }
        }
        SlotRole::MinutesNumber => {
            if is_digits(cand) && matches!(next, "minute" | "minutes" | "min" | "mins") {
                9.5
            } else if is_digits(cand) {
                7.0
            } else {
                0.0
            }
        }
        SlotRole::LineCount => {
            if is_digits(cand) && (matches!(next, "line" | "lines" | "log" | "most") || matches!(prev, "first" | "last" | "top" | "bottom" | "--tail" | "tail")) {
                9.5
            } else {
                0.0
            }
        }
        SlotRole::Url => {
            if looks_like_url(cand) {
                9.5
            } else if looks_like_path_or_file(cand) && cand.contains('.') {
                6.0
            } else {
                0.0
            }
        }
        SlotRole::DiskDevice => {
            if cand.starts_with("/dev/") { 9.5 } else if lower.starts_with("nvme") || lower.starts_with("sd") { 6.0 } else { 0.0 }
        }
        SlotRole::ArchiveFile => {
            if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") || lower.ends_with(".tar.bz2")
                || lower.ends_with(".tar.xz") || lower.ends_with(".tar.zst") || lower.ends_with(".tar")
                || lower.ends_with(".zip") || lower.ends_with(".rar")
            {
                9.5
            } else if is_path
                && matches!(prev, "into" | "to")
                && cand.contains('.')
                && !matches!(lower.as_str(), "zip" | "tar" | "rar" | "gz" | "archive" | "tarball" | "file" | "folder" | "directory")
            {
                5.0
            } else {
                0.0
            }
        }
        SlotRole::ArchiveInputs => {
            if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") || lower.ends_with(".zip") || lower.ends_with(".rar") {
                return 0.0;
            }
            if is_path || indirect > 0.0 || matches!(next, "folder" | "directory" | "file" | "files") {
                7.5 + indirect
            } else if matches!(prev, "pack" | "zip" | "compress" | "archive" | "up") {
                5.0
            } else {
                0.0
            }
        }
        SlotRole::SourcePath => {
            let mut s = if is_path { 6.0 } else if matches!(next, "folder" | "directory" | "file") { 5.0 } else { 2.0 };
            s += indirect;
            if let Some(split) = to_split_pos {
                if pos < split || (indirect > 0.0 && !matches!(prev, "to" | "into" | "as")) {
                    s += 3.5;
                } else if pos > split {
                    s -= 4.0;
                }
            }
            s.max(0.0)
        }
        SlotRole::DestPath => {
            if cand.contains('@') && cand.contains(':') {
                return 9.5;
            }
            let mut s = if is_path || cand.contains('/') { 5.0 } else if !is_non_value_english_word(&lower) { 3.5 } else { 0.0 };
            s += indirect;
            if matches!(prev, "called" | "named") {
                s += 12.0;
            }
            if let Some(split) = to_split_pos {
                if pos > split || matches!(prev, "to" | "into" | "as") {
                    s += 4.5;
                } else if pos < split {
                    return 0.0;
                }
            }
            s.max(0.0)
        }
        SlotRole::LinkName => {
            if matches!(prev, "called" | "named") { 9.0 } else if let Some(split) = to_split_pos {
                if pos < split && !is_non_value_english_word(&lower) { 4.5 } else { 0.0 }
            } else { 0.0 }
        }
        SlotRole::LinkTarget => {
            if prev == "to" { 9.0 } else if !matches!(prev, "called" | "named") && (is_path || !is_non_value_english_word(&lower)) { 4.5 } else { 0.0 }
        }
        SlotRole::FirstPath => {
            if is_path { 8.0 - (pos as f64 * 0.1) } else { 0.0 }
        }
        SlotRole::SecondPath => {
            if is_path { 6.0 + (pos as f64 * 0.1) } else { 0.0 }
        }
        SlotRole::NamePattern => {
            if is_ext_glob_cand {
                if q.raw_tokens.iter().any(|rt| {
                    let ext = cand.trim_start_matches("*.").to_lowercase();
                    (rt.lower == format!(".{ext}") || rt.lower == ext) && matches!(rt.next_word.as_str(), "file" | "files")
                }) {
                    10.5
                } else {
                    9.5
                }
            } else if cand.contains('*') || cand.contains('?') {
                9.5
            } else if matches!(prev, "named" | "called" | "name") {
                9.0
            } else if is_path && !matches!(prev, "under" | "in" | "inside") {
                if cand.starts_with('.') && matches!(next, "file" | "files") { 3.0 } else { 6.0 }
            } else {
                0.0
            }
        }
        SlotRole::FindWhere => {
            if (matches!(prev, "under" | "inside" | "within") || (prev == "in" && !matches!(next, "day" | "days" | "megabytes" | "gigabytes")))
                && !is_digits(cand)
                && !cand.contains('*')
                && !matches!(prev, "named" | "called")
                && !matches!(lower.as_str(), "past" | "last")
            {
                return 8.5 + indirect;
            }
            if indirect > 0.0 && !matches!(prev, "named" | "called") {
                return 6.0 + indirect;
            }
            0.0
        }
        SlotRole::SearchWhere => {
            if !quoted && (is_path || indirect > 0.0 || matches!(prev, "in" | "under" | "inside")) {
                return 8.0 + indirect;
            }
            0.0
        }
        SlotRole::Pattern => {
            if quoted {
                9.5
            } else if (matches!(prev, "for" | "mention" | "containing" | "contain" | "matching" | "word" | "with")
                || (prev.starts_with('-') && prev.len() > 1 && cmd_name == "grep"))
                && !is_path
            {
                9.0
            } else if !is_path && !is_digits(cand) && !is_non_value_english_word(&lower) {
                bare_word(&lower, 4.0)
            } else {
                0.0
            }
        }
        SlotRole::OwnerUser => {
            if matches!(prev, "make" | "owner" | "by" | "to" | "user") && !is_path {
                8.5
            } else if !is_path && !is_non_value_english_word(&lower) {
                bare_word(&lower, 4.5)
            } else {
                0.0
            }
        }
        SlotRole::ServiceUnit => {
            if matches!(lower.as_str(), "system" | "boot" | "last" | "recent" | "log" | "logs" | "logged" | "tail" | "head" | "journald" | "journal" | "lines" | "status" | "running" | "since")
                || is_digits(cand)
            {
                return 0.0;
            }
            if matches!(next, "service" | "log" | "logs" | "unit" | "daemon")
                || (cmd_name != "search service log" && matches!(prev, "service" | "for" | "unit"))
                || is_product_target(&lower, prev, next)
            {
                return 9.0;
            }
            if !is_path && !is_non_value_english_word(&lower) && !matches!(lower.as_str(), "failed" | "error") {
                bare_word(&lower, 5.0)
            } else {
                0.0
            }
        }
        SlotRole::HostOrIp => {
            if cand.contains('@') || looks_like_ip(cand) || (cand.contains('.') && !cand.ends_with(".txt") && !cand.ends_with(".log")) {
                9.5
            } else if cmd_name == "ip route get" {
                0.0
            } else if matches!(prev, "to" | "into" | "at" | "over" | "reach" | "ping" | "for" | "on") && !is_non_value_english_word(&lower) {
                7.5
            } else if !is_non_value_english_word(&lower) && !is_digits(cand) {
                bare_word(&lower, 4.5)
            } else {
                0.0
            }
        }
        SlotRole::PackageOrTerm => {
            if cand.contains('.') && lower.ends_with(".pkg.tar.zst") {
                return 0.0;
            }
            if cmd_name.starts_with("flatpak ") && matches!(lower.as_str(), "flatpak" | "flatpaks" | "flathub") {
                return 0.0;
            }
            let value_word = known_package || !is_non_value_english_word(&lower);
            if matches!(prev, "install" | "uninstall" | "remove" | "for" | "add" | "drop" | "search" | "crate" | "package" | "get")
                && value_word
            {
                9.0
            } else if matches!(next, "as" | "from" | "crate" | "package") && value_word {
                8.5
            } else if value_word && !is_digits(cand) {
                bare_word(&lower, 5.0)
            } else {
                0.0
            }
        }
        SlotRole::ProgramName => {
            if quoted {
                8.5
            } else if matches!(prev, "called" | "named") && !is_digits(cand) {
                9.5
            } else if is_product_target(&lower, prev, next)
                || ((matches!(prev, "for" | "is" | "about") || matches!(next, "installed" | "command" | "processes"))
                    && !is_non_value_english_word(&lower))
            {
                9.0
            } else if !is_non_value_english_word(&lower) && !is_digits(cand) {
                bare_word(&lower, 7.0)
            } else {
                0.0
            }
        }
        SlotRole::GroupName => {
            if matches!(next, "group") || matches!(prev, "group") {
                9.0
            } else if !is_non_value_english_word(&lower) && !is_digits(cand) {
                bare_word(&lower, 5.0)
            } else {
                0.0
            }
        }
        SlotRole::ContainerOrImage | SlotRole::VmOrNetwork | SlotRole::AiModel | SlotRole::BranchName | SlotRole::NetInterface | SlotRole::KernelModule => {
            if (cand.contains(':') || cand.contains('-') || cand.chars().any(|c| c.is_ascii_digit()))
                && !is_digits(cand) && !is_non_value_english_word(&lower) {
                    return 9.2;
                }
            if matches!(next, "container" | "image" | "vm" | "network" | "model" | "branch" | "locally")
                && !is_non_value_english_word(&lower)
            {
                return 9.0;
            }
            if matches!(prev, "called" | "named" | "container" | "image" | "vm" | "network" | "model" | "branch" | "of" | "to" | "with" | "for" | "delete" | "stop" | "start" | "restart" | "pull" | "unload" | "load" | "boot" | "reboot" | "destroy" | "undefine" | "launch" | "halt" | "fetch" | "talk" | "in")
                && !is_non_value_english_word(&lower)
            {
                return 8.5;
            }
            if !is_non_value_english_word(&lower) && !is_digits(cand) { bare_word(&lower, 5.0) } else { 0.0 }
        }
        SlotRole::SessionName => {
            if matches!(prev, "called" | "named") {
                9.5
            } else if matches!(next, "session") && !is_non_value_english_word(&lower) {
                7.5
            } else {
                0.0
            }
        }
        SlotRole::WifiSsid => {
            if quoted || ((cand.chars().any(|c| c.is_uppercase()) || matches!(prev, "called" | "named" | "network" | "wifi" | "to")) && !is_non_value_english_word(&lower)) {
                9.0
            } else {
                0.0
            }
        }
        SlotRole::ThemeOrFont => {
            if is_multi_phrase && (quoted || phrase_match.is_some()) {
                10.5
            } else if is_multi_phrase {
                9.5
            } else if quoted || (cand.chars().next().is_some_and(|c| c.is_uppercase()) && !is_non_value_english_word(&lower)) {
                8.5
            } else if matches!(prev, "to" | "use" | "apply") && !is_non_value_english_word(&lower) {
                7.0
            } else {
                0.0
            }
        }
        SlotRole::AppName => {
            if is_digits(cand)
                || is_non_value_english_word(&lower)
                || matches!(lower.as_str(), "default" | "browser" | "editor" | "terminal" | "agent" | "coding" | "app" | "application" | "omarchy")
            {
                0.0
            } else if matches!(prev, "to" | "as" | "browser" | "editor" | "terminal" | "agent") {
                9.5
            } else {
                6.5
            }
        }
        SlotRole::EnvVarName => {
            if is_upper_env_name(cand) {
                9.5
            } else if matches!(prev, "of" | "variable" | "set" | "export" | "make") && !is_non_value_english_word(&lower) {
                6.0
            } else {
                0.0
            }
        }
        SlotRole::EnvVarValue => {
            if is_upper_env_name(cand) {
                0.0
            } else if matches!(prev, "to" | "=") && !is_non_value_english_word(&lower) {
                9.0
            } else {
                0.0
            }
        }
        SlotRole::AliasName => {
            if matches!(prev, "called" | "named" | "alias") && !quoted && !is_non_value_english_word(&lower) {
                9.5
            } else if matches!(next, "to" | "as") && !quoted && !is_non_value_english_word(&lower) {
                8.5
            } else {
                0.0
            }
        }
        SlotRole::AliasCommand => {
            if quoted {
                9.5
            } else if is_multi_phrase && matches!(prev, "for" | "to") {
                8.0
            } else {
                0.0
            }
        }
        SlotRole::MessageText => {
            if quoted {
                9.5
            } else if is_multi_phrase && !cand.chars().all(|c| c.is_uppercase() || c == ' ') {
                if phrase_match.is_some() { 9.5 } else { 8.5 }
            } else if matches!(prev, "message" | "about" | "saying") && !is_non_value_english_word(&lower) {
                6.5
            } else {
                0.0
            }
        }
        SlotRole::GenericPath => {
            if matches!(prev, "called" | "named") {
                10.0
            } else if is_from_files && is_multi_phrase && (indirect >= 12.0 || phrase_match.is_some()) {
                11.5 + indirect
            } else if is_path {
                9.0 + indirect
            } else if indirect > 0.0 || is_named_dir_or_file {
                8.5 + indirect
            } else if matches!(prev, "to" | "into" | "in" | "from" | "of")
                && !is_non_value_english_word(&lower)
                && !is_digits(cand)
                && !is_multi_phrase
            {
                7.0
            } else if matches!(prev, "on" | "for")
                && !is_non_value_english_word(&lower)
                && !is_digits(cand)
                && !is_multi_phrase
                && (cand.contains(['_', '-']) || matches!(cmd_name, "chmod" | "chown" | "stat" | "file"))
            {
                // "chmod 600 on id_rsa": the file the change is for.
                6.5
            } else if cand.chars().next().is_some_and(|c| c.is_uppercase()) && !is_non_value_english_word(&lower) && !is_multi_phrase {
                6.0
            } else {
                0.0
            }
        }
        SlotRole::GenericText => {
            if quoted {
                8.5
            } else if is_multi_phrase {
                7.5
            } else if matches!(prev, "called" | "named") {
                9.0
            } else if !is_non_value_english_word(&lower) && !is_digits(cand) {
                bare_word(&lower, 5.0)
            } else {
                0.0
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests & Eval Benchmarks
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{catalog, eval, suggest};
    use std::path::Path;

    fn load_catalog() -> Vec<Command> {
        let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/catalog"));
        catalog::load(dir).unwrap()
    }

    #[test]
    fn normalizes_stems_and_synonyms() {
        let terms = extract_weighted_terms("unpack compressed directories and check ram");
        assert!(terms.contains_key("c_unpack"));
        assert!(terms.contains_key("c_folder"));
        assert!(terms.contains_key("c_memory"));
    }

    #[test]
    fn rejects_off_topic_and_wrong_platform() {
        let all = load_catalog();
        let arch = catalog::usable(all.clone(), &["arch".into(), "omarchy".into()], false);
        let matcher = LocalMatcher::new(&arch);

        for req in [
            "write me a poem about the ocean",
            "set an alarm for 7am",
            "what time is it in Tokyo",
            "burn ubuntu.iso to a usb drive",
            "summarize this article",
            "play some music",
        ] {
            let out = suggest(&matcher, req, vec![], &mut |_| None).unwrap();
            assert!(out.suggestions.is_empty(), "expected rejection for `{req}`, got {:?}", out.suggestions);
        }

        let debian = catalog::usable(all, &["debian".into()], false);
        let deb_matcher = LocalMatcher::new(&debian);
        let out = suggest(&deb_matcher, "switch the desktop theme to Everforest", vec![], &mut |_| None).unwrap();
        assert!(out.suggestions.is_empty(), "expected rejection on debian, got {:?}", out.suggestions);
    }

    #[test]
    fn supports_indirect_pwd_and_slot_disambiguation() {
        let arch = catalog::usable(load_catalog(), &["arch".into(), "omarchy".into()], false);
        let matcher = LocalMatcher::new(&arch);

        let out = suggest(
            &matcher,
            "make the deploy script runnable",
            vec!["README.md".into(), "deploy.sh".into(), "src".into()],
            &mut |_| None,
        )
        .unwrap();
        assert_eq!(out.suggestions[0].line, "chmod +x deploy.sh");

        let out2 = suggest(&matcher, "print the third column of users.csv", vec![], &mut |_| None).unwrap();
        assert_eq!(out2.suggestions[0].line, "cut -d, -f3 users.csv");
    }

    #[test]
    fn handles_multi_word_values_safety_and_sysadmin_disambiguation() {
        let arch = catalog::usable(load_catalog(), &["arch".into(), "omarchy".into()], false);
        let matcher = LocalMatcher::new(&arch);

        let cases: &[(&str, Vec<&str>, &str)] = &[
            ("switch theme to Catppuccin Latte", vec![], "omarchy theme set 'Catppuccin Latte'"),
            ("change font to JetBrains Mono", vec![], "omarchy font set 'JetBrains Mono'"),
            ("remind me in 15 minutes to check oven", vec![], "omarchy reminder 15 'check oven'"),
            (
                "record screen with desktop sound and mic",
                vec![],
                "omarchy capture screenrecording --with-desktop-audio --with-microphone-audio",
            ),
            ("set default browser to firefox", vec![], "omarchy default browser firefox"),
            ("set default editor to nvim", vec![], "omarchy default editor nvim"),
            ("install firefox with omarchy", vec![], "omarchy pkg add firefox"),
            ("install aur package spotify using omarchy", vec![], "omarchy pkg aur add spotify"),
            ("put vacation.jpg in the trash", vec![], "gio trash vacation.jpg"),
            ("show me what is inside homework.txt", vec!["homework.txt"], "cat homework.txt"),
            ("make backup script.sh runnable", vec!["backup script.sh"], "chmod +x 'backup script.sh'"),
            ("find big files over 50MB", vec![], "find . -type f -size +50M"),
            ("find files larger than 50MB under /tmp", vec![], "find /tmp -type f -size +50M"),
            ("am i connected to the internet", vec![], "ping -c 4 1.1.1.1"),
            ("show network routes", vec![], "ip route"),
            ("tail last 50 lines of docker logs in journald", vec![], "journalctl -u docker -n 50 --no-pager"),
            ("find .rs files changed in the last 2 days", vec![], "find . -type f -name '*.rs' -mtime -2"),
            ("force shut down vm win11", vec![], "virsh destroy win11"),
            ("who am i logged in as", vec![], "whoami"),
            ("compress Downloads folder into a zip file", vec![], "zip -r <archive> Downloads"),
            ("sync ./build/ to deploy@web1:/var/www/app", vec![], "rsync -av ./build/ deploy@web1:/var/www/app"),
            ("set default coding agent to claude", vec![], "omarchy default agent claude"),
        ];

        for (req, files, expected) in cases {
            let files_vec: Vec<String> = files.iter().map(|s| (*s).to_string()).collect();
            let out = suggest(&matcher, req, files_vec, &mut |_| None).unwrap();
            assert_eq!(out.suggestions.first().map(|s| s.line.as_str()), Some(*expected), "request: `{req}`");
        }
    }

    #[test]
    fn catalog_keywords_find_a_command() {
        let cmds = vec![
            Command {
                name: "alpha".into(),
                description: "Run tool alpha".into(),
                template: "alpha".into(),
                destructive: false,
                warning: None,
                risk: None,
                explain_only: false,
                pkg_role: None,
                platform: None,
                package: None,
                keywords: vec!["custom-flux-widget".into()],
                slots: vec![],
                flags: vec![],
                kind: None,
                opposites: vec![],
            },
            Command {
                name: "beta".into(),
                description: "Run tool beta".into(),
                template: "beta".into(),
                destructive: false,
                warning: None,
                risk: None,
                explain_only: false,
                pkg_role: None,
                platform: None,
                package: None,
                keywords: vec![],
                slots: vec![],
                flags: vec![],
                kind: None,
                opposites: vec![],
            },
        ];
        let matcher = LocalMatcher::new(&cmds);
        let out = suggest(&matcher, "custom-flux-widget", vec![], &mut |_| None).unwrap();
        assert_eq!(out.suggestions[0].command, "alpha");
    }

    #[test]
    fn eval_cases_and_heldout_benchmark() {
        let all = load_catalog();
        let arch = LocalMatcher::new(&catalog::usable(all.clone(), &["arch".into(), "omarchy".into()], false));
        let debian = LocalMatcher::new(&catalog::usable(all, &["debian".into()], false));
        let evals = Path::new(env!("CARGO_MANIFEST_DIR")).join("evals");

        // (matcher, file, least top-1 %, least exact-line %)
        for (matcher, platform, file, min_top1, min_exact) in [
            (&arch, "arch", "cases", 100, 100),
            (&arch, "arch", "heldout", 100, 100),
            (&debian, "debian", "debian", 100, 100),
        ] {
            let platform = if platform == "arch" {
                vec!["arch".to_string(), "omarchy".to_string()]
            } else {
                vec![platform.to_string()]
            };
            let report = eval::score_for(matcher, &evals.join(format!("{file}.toml")), &platform).unwrap();
            for line in report.lines.iter().filter(|line| line.starts_with("MISS")) {
                println!("{line}");
            }
            println!("\n=== {file}.toml ===\n{report}");
            assert_eq!(
                report.rejected.hit, report.rejected.total,
                "all non-terminal cases in {file}.toml must be rejected"
            );
            assert!(report.top1.percent() >= min_top1, "{file} top-1 too low: {}", report.top1);
            assert!(report.exact.percent() >= min_exact, "{file} exact too low: {}", report.exact);
        }
    }

    /// Values land in the right slot or stay a placeholder (evals/slots.toml): every case must pass.
    #[test]
    fn slot_filling_cases_all_pass() {
        let all = load_catalog();
        let platform = vec!["arch".to_string(), "omarchy".to_string()];
        let matcher = LocalMatcher::new(&catalog::usable(all, &platform, false));
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("evals/slots.toml");
        let report = eval::score_for(&matcher, &file, &platform).unwrap();
        let misses: Vec<&String> = report.lines.iter().filter(|line| line.starts_with("MISS")).collect();
        assert!(report.exact.total >= 40, "slots.toml should hold the slot-filling cases");
        assert!(misses.is_empty(), "{misses:#?}");
    }

    fn labels_of(request: &str) -> Vec<(String, String)> {
        let norm = normalize_text(request);
        let words = raw_lower_words(&norm);
        let terms = extract_weighted_terms(request);
        let q = QueryContext::new(request, &norm, &words, &terms);
        find_labels(&q)
            .into_iter()
            .map(|l| {
                let value = q.raw_tokens.iter().find(|t| t.word_pos == l.value_pos).unwrap().text.clone();
                (l.kind.to_string(), value)
            })
            .collect()
    }

    #[test]
    fn labels_mark_the_value_beside_them() {
        let pair = |k: &str, v: &str| (k.to_string(), v.to_string());
        assert_eq!(labels_of("add bob to group docker"), vec![pair("group", "docker")]);
        assert_eq!(labels_of("follow journal for unit sshd"), vec![pair("service", "sshd")]);
        assert_eq!(labels_of("restart the nginx service"), vec![pair("service", "nginx")]);
        assert_eq!(labels_of("show logs for the docker service since yesterday"), vec![pair("service", "docker")]);
        assert_eq!(labels_of("create user carol with zsh shell"), vec![pair("user", "carol"), pair("shell", "zsh")]);
        assert_eq!(labels_of("switch to branch main"), vec![pair("branch", "main")]);
        // No value beside the label: nothing is labelled.
        assert!(labels_of("show service status").is_empty());
        assert!(labels_of("get a shell inside the box").is_empty());
    }

    #[test]
    fn exclusion_words_mark_the_next_value() {
        let request = "grep TODO in src excluding node_modules but not tests";
        let norm = normalize_text(request);
        let words = raw_lower_words(&norm);
        let terms = extract_weighted_terms(request);
        let q = QueryContext::new(request, &norm, &words, &terms);
        assert!(is_excluded_value("node_modules", &q));
        assert!(!is_excluded_value("src", &q));
        assert!(is_binary_name("budget.xlsx") && !is_binary_name("notes.txt") && !is_binary_name(".xlsx"));
    }

    /// Round-2 slot filling (evals/slots2.toml): labelled words, exclusions, system names, no silent drops.
    #[test]
    fn slot_filling_round_two_cases_all_pass() {
        let all = load_catalog();
        let platform = vec!["arch".to_string(), "omarchy".to_string()];
        let matcher = LocalMatcher::new(&catalog::usable(all, &platform, false));
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("evals/slots2.toml");
        let report = eval::score_for(&matcher, &file, &platform).unwrap();
        let misses: Vec<&String> = report.lines.iter().filter(|line| line.starts_with("MISS")).collect();
        println!("slots2: {} of {} exact lines", report.exact.hit, report.exact.total);
        assert!(report.exact.total >= 50, "slots2.toml should hold the round-2 cases");
        assert!(misses.is_empty(), "{} misses: {misses:#?}", misses.len());
    }

    /// Round-3 slot filling (evals/slots3.toml): named values, several values per sentence, roles.
    #[test]
    fn slot_filling_round_three_cases_all_pass() {
        let all = load_catalog();
        let platform = vec!["arch".to_string(), "omarchy".to_string()];
        let matcher = LocalMatcher::new(&catalog::usable(all, &platform, false));
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("evals/slots3.toml");
        let report = eval::score_for(&matcher, &file, &platform).unwrap();
        let misses: Vec<&String> = report.lines.iter().filter(|line| line.starts_with("MISS")).collect();
        for line in &misses {
            println!("{line}");
        }
        println!("slots3: {} of {} exact lines", report.exact.hit, report.exact.total);
        assert!(report.exact.total >= 50, "slots3.toml should hold the round-3 cases");
        assert!(misses.is_empty(), "{} misses", misses.len());
    }

    /// Safety evals (evals/safety.toml): every case must pass, including the WARNING expectations.
    #[test]
    fn safety_evals_all_pass() {
        let arch = vec!["arch".to_string(), "omarchy".to_string()];
        let matcher = LocalMatcher::new(&catalog::usable(load_catalog(), &arch, false));
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("evals/safety.toml");
        let report = eval::score_for(&matcher, &file, &arch).unwrap();
        let misses: Vec<&String> = report.lines.iter().filter(|line| line.starts_with("MISS")).collect();
        assert!(report.top1.total > 0, "no safety cases ran");
        assert!(misses.is_empty(), "safety evals must be at 100%: {misses:#?}");
    }

    /// Typed values never vanish silently (evals/values.toml): each shows in the line or a note.
    #[test]
    fn typed_values_evals_all_pass() {
        let arch = vec!["arch".to_string(), "omarchy".to_string()];
        let matcher = LocalMatcher::new(&catalog::usable(load_catalog(), &arch, false));
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("evals/values.toml");
        let report = eval::score_for(&matcher, &file, &arch).unwrap();
        let misses: Vec<&String> = report.lines.iter().filter(|line| line.starts_with("MISS")).collect();
        for line in &misses {
            println!("{line}");
        }
        assert!(report.lines.len() >= 50, "values.toml should hold 50+ cases");
        assert!(misses.is_empty(), "values evals must be at 100%: {} misses", misses.len());
    }

    /// Sysadmin round (evals/sysadmin.toml): git, process, file, ssh, cron, docker requests; every case must pass.
    #[test]
    fn sysadmin_evals_all_pass() {
        let arch = vec!["arch".to_string(), "omarchy".to_string()];
        let matcher = LocalMatcher::new(&catalog::usable(load_catalog(), &arch, false));
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("evals/sysadmin.toml");
        let report = eval::score_for(&matcher, &file, &arch).unwrap();
        let misses: Vec<&String> = report.lines.iter().filter(|line| line.starts_with("MISS")).collect();
        for line in &misses {
            println!("{line}");
        }
        println!("sysadmin: {} misses of {} cases", misses.len(), report.lines.len());
        assert!(report.lines.len() >= 100, "sysadmin.toml should hold the sysadmin cases");
        assert!(misses.is_empty(), "sysadmin evals must be at 100%: {} misses", misses.len());
    }

    /// Never mislead (evals/mislead.toml): questions about state never get actions, dangerous
    /// entries need the verb and the object named, invented values stay placeholders.
    #[test]
    fn mislead_evals_all_pass() {
        let arch = vec!["arch".to_string(), "omarchy".to_string()];
        let matcher = LocalMatcher::new(&catalog::usable(load_catalog(), &arch, false));
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("evals/mislead.toml");
        let report = eval::score_for(&matcher, &file, &arch).unwrap();
        let misses: Vec<&String> = report.lines.iter().filter(|line| line.starts_with("MISS")).collect();
        for line in &misses {
            println!("{line}");
        }
        println!("mislead: {} misses of {} cases", misses.len(), report.lines.len());
        assert!(report.lines.len() >= 100, "mislead.toml should hold the never-mislead cases");
        assert!(misses.is_empty(), "mislead evals must be at 100%: {} misses", misses.len());
    }

    /// Measurement helper, not a check: `HUH_DUMP=<file> cargo test dump_request_evals -- --ignored`
    /// writes every request case's outcome line, to compare before and after a change.
    #[test]
    #[ignore]
    fn dump_request_evals() {
        let Ok(out) = std::env::var("HUH_DUMP") else { return };
        let arch = vec!["arch".to_string(), "omarchy".to_string()];
        let matcher = LocalMatcher::new(&catalog::usable(load_catalog(), &arch, false));
        let mut text = String::new();
        for name in ["cases", "heldout", "safety", "slots", "slots2", "slots3", "intent", "mislead"] {
            let file = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("evals/{name}.toml"));
            for line in eval::score_for(&matcher, &file, &arch).unwrap().lines {
                text.push_str(&format!("{name}: {line}\n"));
            }
        }
        std::fs::write(out, text).unwrap();
    }

    #[test]
    fn reads_the_mood_of_a_request() {
        let mood = |request: &str| {
            let norm = normalize_text(request).into_owned();
            request_mood(&norm, &raw_lower_words(&norm))
        };
        for question in [
            "is vlc installed",
            "what wifi am i connected to",
            "do i have docker",
            "how much space is left",
            "check if nginx is running",
            "which process is using port 80?",
            "who owns this file",
            // An order to look ranks like a question.
            "show permissions of /etc/shadow",
            "verify the archive backup.tar.gz",
            "list running containers",
            "check the status of my user service pipewire",
        ] {
            assert_eq!(mood(question), Mood::Question, "{question}");
        }
        for instruction in [
            "can you show my disk space",
            "please restart nginx",
            "how do i run backup.sh",
            "i want to install vlc",
            "what command lists files",
            "install vlc",
            "which process uses port 80, and kill it",
            "is it possible to rename a file",
        ] {
            assert_eq!(mood(instruction), Mood::Instruction, "{instruction}");
        }
    }

    #[test]
    fn opposites_ignore_negated_words() {
        let check = |request: &str| {
            let norm = normalize_text(request).into_owned();
            let words = raw_lower_words(&norm);
            let terms = extract_weighted_terms(request);
            QueryContext::new(request, &norm, &words, &terms).names_unnegated("sleep")
        };
        assert!(check("put the computer to sleep"));
        assert!(!check("stop the computer from going to sleep"));
        assert!(!check("don't let the laptop go to sleep"));
    }

    /// Questions about state get queries, instructions get actions, opposites are never mixed up
    /// and off-topic requests get nothing (evals/intent.toml): every case must pass.
    #[test]
    fn intent_evals_all_pass() {
        let arch = vec!["arch".to_string(), "omarchy".to_string()];
        let matcher = LocalMatcher::new(&catalog::usable(load_catalog(), &arch, false));
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("evals/intent.toml");
        let report = eval::score_for(&matcher, &file, &arch).unwrap();
        let misses: Vec<&String> = report.lines.iter().filter(|line| line.starts_with("MISS")).collect();
        assert!(report.top1.total + report.rejected.total >= 60, "intent.toml should hold the intent cases");
        assert!(misses.is_empty(), "intent evals must be at 100%: {misses:#?}");
    }

    /// Every package manager family answers package requests with its own commands and package
    /// names (evals/platforms.toml): `huh eval --cases evals/platforms.toml --platform <family>`.
    #[test]
    fn package_requests_on_every_family() {
        let all = load_catalog();
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("evals/platforms.toml");
        for family in ["debian", "fedora", "suse", "alpine", "void", "gentoo", "nixos", "nix"] {
            let platform = vec![family.to_string()];
            let matcher = LocalMatcher::new(&catalog::usable(all.clone(), &platform, false));
            let report = eval::score_for(&matcher, &file, &platform).unwrap();
            let misses: Vec<&String> = report.lines.iter().filter(|line| line.starts_with("MISS")).collect();
            assert!(report.top1.total > 0, "{family}: no cases ran");
            assert!(misses.is_empty(), "{family}: {misses:#?}");
        }
    }
}
