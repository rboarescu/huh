//! Values the matcher may pick for path and text slots. It never invents text,
//! so every value has to be offered: words from the request, plus entries in the
//! current folder for paths.

use crate::catalog::SlotKind;
use std::collections::BTreeSet;

const MAX_TOKENS: usize = 30;
pub const MAX_FILES: usize = 40;

const STOPWORDS: &[&str] = &[
    "a", "an", "the", "i", "me", "my", "we", "you", "it", "its", "this", "that", "these", "those", "is", "are", "be",
    "do", "does", "can", "could", "would", "should", "want", "need", "like", "please", "how", "what", "which", "where",
    "to", "of", "in", "on", "at", "for", "from", "into", "with", "and", "or", "as", "by", "so", "there", "have", "has",
    "some", "just",
];

pub struct Candidates {
    pub tokens: Vec<String>,
    pub files: Vec<String>,
    pub phrases: Vec<String>,
}

impl Candidates {
    pub fn new(request: &str, files: Vec<String>) -> Self {
        let tokens = tokens(request);
        let phrases = prepositional_phrases(request, &tokens);
        Self { tokens, files, phrases }
    }

    pub fn for_slot(&self, kind: SlotKind) -> BTreeSet<&str> {
        let mut set: BTreeSet<&str> = self.tokens.iter().map(String::as_str).collect();
        set.extend(self.phrases.iter().map(String::as_str));
        if kind == SlotKind::Path {
            set.extend(self.files.iter().map(String::as_str));
        }
        set
    }
}

/// Converts English number words and ordinal numbers into canonical digit strings.
pub fn number_word(word: &str) -> Option<String> {
    let lower = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '-').to_lowercase();
    if lower.is_empty() {
        return None;
    }
    if let Some(prefix) = lower
        .strip_suffix("st")
        .or_else(|| lower.strip_suffix("nd"))
        .or_else(|| lower.strip_suffix("rd"))
        .or_else(|| lower.strip_suffix("th"))
        && !prefix.is_empty()
        && prefix.chars().all(|c| c.is_ascii_digit())
    {
        return Some(prefix.to_string());
    }
    if let Some((tens_part, unit_part)) = lower.split_once('-') {
        let tens = match tens_part {
            "twenty" => 20,
            "thirty" => 30,
            "forty" => 40,
            "fifty" => 50,
            "sixty" => 60,
            "seventy" => 70,
            "eighty" => 80,
            "ninety" => 90,
            _ => return None,
        };
        let unit = match unit_part {
            "one" | "first" => 1,
            "two" | "second" => 2,
            "three" | "third" => 3,
            "four" | "fourth" => 4,
            "five" | "fifth" => 5,
            "six" | "sixth" => 6,
            "seven" | "seventh" => 7,
            "eight" | "eighth" => 8,
            "nine" | "ninth" => 9,
            _ => return None,
        };
        return Some((tens + unit).to_string());
    }
    let val = match lower.as_str() {
        "zero" => "0",
        "one" | "first" => "1",
        "two" | "second" => "2",
        "three" | "third" => "3",
        "four" | "fourth" => "4",
        "five" | "fifth" => "5",
        "six" | "sixth" => "6",
        "seven" | "seventh" => "7",
        "eight" | "eighth" => "8",
        "nine" | "ninth" => "9",
        "ten" | "tenth" => "10",
        "eleven" | "eleventh" => "11",
        "twelve" | "twelfth" => "12",
        "thirteen" | "thirteenth" => "13",
        "fourteen" | "fourteenth" => "14",
        "fifteen" | "fifteenth" => "15",
        "sixteen" | "sixteenth" => "16",
        "seventeen" | "seventeenth" => "17",
        "eighteen" | "eighteenth" => "18",
        "nineteen" | "nineteenth" => "19",
        "twenty" | "twentieth" => "20",
        "thirty" | "thirtieth" => "30",
        "forty" | "fortieth" => "40",
        "fifty" | "fiftieth" => "50",
        "sixty" | "sixtieth" => "60",
        "seventy" | "seventieth" => "70",
        "eighty" | "eightieth" => "80",
        "ninety" | "ninetieth" => "90",
        "hundred" | "hundredth" => "100",
        _ => return None,
    };
    Some(val.to_string())
}

const SENTENCE_VERBS: &[&str] = &[
    "switch", "change", "set", "use", "open", "install", "find", "show", "list", "delete", "remove", "copy", "move",
    "make", "create", "check", "print", "count", "sort", "run", "start", "stop", "restart", "add", "remind", "search",
    "take", "record", "turn", "give", "put", "edit", "read", "write", "unpack", "pack", "extract", "compress",
    "compare",
];

/// Extracts runs of 2..=4 consecutive capitalized words (e.g. "Tokyo Night", "JetBrains Mono").
fn capitalized_phrases(request: &str) -> Vec<String> {
    let mut phrases = Vec::new();
    let raw_words: Vec<&str> = request.split_whitespace().collect();
    let mut run: Vec<&str> = Vec::new();

    let flush = |run: &mut Vec<&str>, phrases: &mut Vec<String>| {
        if run.len() >= 2 && run.len() <= 4 {
            phrases.push(run.join(" "));
        }
        run.clear();
    };

    for (idx, raw) in raw_words.iter().enumerate() {
        let trimmed = raw.trim_matches([',', ';', ':', '!', '?', '(', ')', '"', '\'', '`']).trim_end_matches('.');
        let starts_upper = trimmed.chars().next().is_some_and(|c| c.is_uppercase());
        let is_stop = STOPWORDS.contains(&trimmed.to_lowercase().as_str());
        let is_start_verb = idx == 0 && SENTENCE_VERBS.contains(&trimmed.to_lowercase().as_str());
        if starts_upper && !is_stop && !is_start_verb && !trimmed.contains('/') && !trimmed.contains('.') {
            run.push(trimmed);
        } else {
            flush(&mut run, &mut phrases);
        }
    }
    flush(&mut run, &mut phrases);
    phrases
}

/// Extracts short multi-word value phrases after prepositions/markers (e.g. "to stretch my legs").
fn prepositional_phrases(request: &str, existing: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let words: Vec<&str> = request.split_whitespace().collect();
    let triggers = ["to", "about", "for", "called", "named", "message", "saying", "use"];
    let stops = [
        "in",
        "on",
        "from",
        "under",
        "inside",
        "within",
        "with",
        "into",
        "to",
        "as",
        "of",
        "permanently",
        "theme",
        "font",
        "alias",
        "command",
        "file",
        "folder",
        "minutes",
        "minute",
    ];
    let articles = ["the", "a", "an", "my"];

    let mut i = 0;
    while i < words.len() {
        let w = words[i].trim_matches([',', ';', ':', '!', '?', '(', ')']).to_lowercase();
        let prev = i.checked_sub(1).and_then(|p| words.get(p)).map(|s| s.to_lowercase()).unwrap_or_default();
        if triggers.contains(&w.as_str()) && !(w == "to" && matches!(prev.as_str(), "want" | "need" | "like" | "how")) {
            let mut j = i + 1;
            if let Some(first) = words.get(j)
                && articles.contains(&first.to_lowercase().as_str())
                && w != "called"
                && w != "named"
            {
                j += 1;
            }
            let mut collected = Vec::new();
            while j < words.len() && collected.len() < 6 {
                let raw = words[j];
                if raw.starts_with('"') || raw.starts_with('\'') || raw.starts_with('`') {
                    collected.clear();
                    break;
                }
                let cleaned = raw.trim_matches([',', ';', ':', '!', '?', '(', ')']).trim_end_matches('.');
                let lower = cleaned.to_lowercase();
                if cleaned.is_empty()
                    || (w != "use" && stops.contains(&lower.as_str()))
                    || (w == "use" && lower == "as")
                {
                    break;
                }
                if w == "use" && !words[j..].iter().any(|x| x.eq_ignore_ascii_case("as")) {
                    collected.clear();
                    break;
                }
                collected.push(cleaned);
                if raw.ends_with([',', ';', '.', '!', '?']) {
                    break;
                }
                j += 1;
            }
            if (2..=6).contains(&collected.len()) {
                let phrase = collected.join(" ");
                let has_path = collected.iter().any(|c| c.contains('/') || c.contains('.'));
                if !has_path && !existing.iter().any(|e| e == &phrase) && !out.iter().any(|e| e == &phrase) {
                    out.push(phrase);
                }
            }
        }
        i += 1;
    }
    out
}

/// Normalizes bare extension tokens like `.rs` or `.log` into filename globs (`*.rs`, `*.log`),
/// leaving literal dotfiles like `.git` and `.env` untouched.
fn bare_extension_glob(text: &str) -> Option<String> {
    let ext = text.strip_prefix('.')?;
    if !(1..=5).contains(&ext.len()) || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    if matches!(
        ext.to_ascii_lowercase().as_str(),
        "git" | "env" | "bashrc" | "zshrc" | "profile" | "vimrc" | "ssh" | "gnupg" | "cache" | "local" | "config"
    ) {
        return None;
    }
    Some(format!("*.{ext}"))
}

/// Normalizes size expressions like `50MB`, `100 megabytes`, or `a gigabyte` into `find -size` units (`50M`, `100M`, `1G`).
fn extract_size_tokens(request: &str) -> Vec<String> {
    let raw_words: Vec<&str> = request.split_whitespace().collect();
    let mut out = Vec::new();
    let unit_suffix = |u: &str| -> Option<&'static str> {
        match u {
            "gb" | "g" | "gigabyte" | "gigabytes" => Some("G"),
            "mb" | "m" | "megabyte" | "megabytes" => Some("M"),
            "kb" | "k" | "kilobyte" | "kilobytes" => Some("k"),
            _ => None,
        }
    };
    for (i, raw) in raw_words.iter().enumerate() {
        let w = raw.trim_matches([',', ';', ':', '!', '?', '(', ')']).trim_end_matches('.').to_lowercase();
        if w.is_empty() {
            continue;
        }
        for (sfx, canon) in [("gb", "G"), ("mb", "M"), ("kb", "k"), ("g", "G"), ("m", "M"), ("k", "k")] {
            if let Some(num) = w.strip_suffix(sfx)
                && !num.is_empty()
                && num.chars().all(|c| c.is_ascii_digit())
            {
                let formatted = format!("{num}{canon}");
                if !out.contains(&formatted) {
                    out.push(formatted);
                }
                break;
            }
        }
        if let Some(canon) = unit_suffix(&w)
            && i > 0
        {
            let prev =
                raw_words[i - 1].trim_matches([',', ';', ':', '!', '?', '(', ')']).trim_end_matches('.').to_lowercase();
            let num = if prev.chars().all(|c| c.is_ascii_digit()) && !prev.is_empty() {
                Some(prev)
            } else if matches!(prev.as_str(), "a" | "an") {
                Some("1".to_string())
            } else {
                number_word(&prev)
            };
            if let Some(n) = num {
                let formatted = format!("{n}{canon}");
                if !out.contains(&formatted) {
                    out.push(formatted);
                }
            }
        }
    }
    out
}

/// Words and quoted phrases from the request, minus filler words.
pub fn tokens(request: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;

    let mut push_raw = |text: &str| {
        if !text.is_empty() && !out.iter().any(|seen| seen == text) && out.len() < MAX_TOKENS {
            out.push(text.to_string());
        }
    };

    let mut push = |text: &str, quoted: bool| {
        let text = if quoted {
            text.trim()
        } else {
            text.trim_matches([',', ';', ':', '!', '?', '(', ')']).trim_end_matches('.')
        };
        if text.is_empty() {
            return;
        }
        if !quoted && STOPWORDS.contains(&text.to_lowercase().as_str()) {
            return;
        }
        push_raw(text);
        if !quoted {
            if let Some(num) = number_word(text) {
                push_raw(&num);
            }
            if let Some(glob) = bare_extension_glob(text) {
                push_raw(&glob);
            }
        }
    };

    for c in request.chars() {
        match quote {
            Some(q) if c == q => {
                push(&current, true);
                current.clear();
                quote = None;
            }
            Some(_) => current.push(c),
            None if matches!(c, '"' | '\'' | '`') && current.is_empty() => quote = Some(c),
            None if c.is_whitespace() => {
                push(&current, false);
                current.clear();
            }
            None => current.push(c),
        }
    }
    push(&current, false);
    for size in extract_size_tokens(request) {
        push_raw(&size);
    }
    for phrase in capitalized_phrases(request) {
        push_raw(&phrase);
    }
    out
}

/// Visible entries of the current folder, sorted, capped.
pub fn current_folder_entries() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(".")
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                .filter(|name| !name.starts_with('.'))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names.truncate(MAX_FILES);
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_words_and_drops_filler() {
        assert_eq!(tokens("I want to make deploy.sh executable."), ["make", "deploy.sh", "executable"]);
    }

    #[test]
    fn keeps_quoted_phrases_whole() {
        assert_eq!(tokens("search for \"hello world\" in notes.txt"), ["search", "hello world", "notes.txt"]);
    }

    #[test]
    fn apostrophes_inside_words_are_not_quotes() {
        assert_eq!(tokens("what's in src/"), ["what's", "src/"]);
    }

    #[test]
    fn normalizes_number_words_alongside_tokens() {
        let got = tokens("print the third column of users.csv");
        assert!(got.contains(&"third".to_string()));
        assert!(got.contains(&"3".to_string()));
        assert!(got.contains(&"users.csv".to_string()));

        let ord = tokens("show the twenty-first line and first five rows");
        assert!(ord.contains(&"21".to_string()));
        assert!(ord.contains(&"1".to_string()));
        assert!(ord.contains(&"5".to_string()));
    }

    #[test]
    fn extracts_capitalized_and_prepositional_phrases() {
        let theme = tokens("switch the desktop theme to Tokyo Night");
        assert!(theme.contains(&"Tokyo Night".to_string()));

        let font = tokens("Set JetBrains Mono as my terminal font");
        assert!(font.contains(&"JetBrains Mono".to_string()));

        let rem = Candidates::new("remind me in 15 minutes to stretch my legs", vec![]);
        let text_opts = rem.for_slot(SlotKind::Text);
        assert!(text_opts.contains("15"));
        assert!(text_opts.contains("stretch my legs"));

        let catppuccin = Candidates::new("switch theme to Catppuccin Latte", vec![]);
        assert!(catppuccin.for_slot(SlotKind::Text).contains("Catppuccin Latte"));

        let oven = Candidates::new("remind me in 15 minutes to check oven", vec![]);
        assert!(oven.for_slot(SlotKind::Text).contains("check oven"));

        let ext = tokens("find .rs files changed in the last 2 days");
        assert!(ext.contains(&"*.rs".to_string()));
        assert!(!tokens("look for .git under code").contains(&"*.git".to_string()));
    }
}
