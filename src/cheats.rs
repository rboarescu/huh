//! Hidden easter-egg cheat codes tucked inside `lessons/*.toml` and practice sandboxes.
//!
//! Cheat codes never appear in `--help`, `/help`, `/` completions, or docs, and never execute
//! external commands — they only update local companion/course state (`pet.toml`, `progress.toml`).

use crate::date;
use crate::pet::{self, Pet, Species};
use crate::progress::{self, Progress};
use crate::style::{Role, paint};
use std::fmt::Write as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cheat {
    UnlockPet(Species),
    PebbleVault,
    ManualWizard,
    Konami,
    ZeldaRelic,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheatOutcome {
    pub text: String,
    pub pet_changed: bool,
    pub progress_changed: bool,
}

/// Canonical strings as written inside `lessons/*.toml` (used by tests to verify every egg is
/// genuinely discoverable in the embedded curriculum).
#[cfg_attr(not(test), allow(dead_code))]
pub const CANONICAL_CODES: &[(&str, Cheat)] = &[
    ("pspsps-dotfile", Cheat::UnlockPet(Species::Cat)),
    ("bobr-builds-dams", Cheat::UnlockPet(Species::Beaver)),
    ("grep-for-shells", Cheat::UnlockPet(Species::Turtle)),
    ("vector-hibernates", Cheat::UnlockPet(Species::Bear)),
    ("symlink-hiss", Cheat::UnlockPet(Species::Snake)),
    ("hoot-on-port-443", Cheat::UnlockPet(Species::Owl)),
    ("catch-the-wave-mpv", Cheat::UnlockPet(Species::Dolphin)),
    ("honeycomb-palette", Cheat::UnlockPet(Species::Bee)),
    ("open-pebble-vault", Cheat::PebbleVault),
    ("rtfm-wizard", Cheat::ManualWizard),
    ("up up down down left right left right b a", Cheat::Konami),
    ("it's dangerous to go alone", Cheat::ZeldaRelic),
];

fn normalize(input: &str) -> String {
    let mut cleaned = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '-' | '_' => cleaned.push(' '),
            '\'' | '"' | '!' | '?' | '.' | ',' | ':' | ';' | '`' => {}
            other => cleaned.push(other.to_ascii_lowercase()),
        }
    }
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Matches an interactive query or `/pet <code...>` phrase against the hidden cheat table.
pub fn lookup(input: &str) -> Option<Cheat> {
    match normalize(input).as_str() {
        "pspsps dotfile" => Some(Cheat::UnlockPet(Species::Cat)),
        "bobr builds dams" => Some(Cheat::UnlockPet(Species::Beaver)),
        "grep for shells" => Some(Cheat::UnlockPet(Species::Turtle)),
        "vector hibernates" => Some(Cheat::UnlockPet(Species::Bear)),
        "symlink hiss" => Some(Cheat::UnlockPet(Species::Snake)),
        "hoot on port 443" => Some(Cheat::UnlockPet(Species::Owl)),
        "catch the wave mpv" => Some(Cheat::UnlockPet(Species::Dolphin)),
        "honeycomb palette" => Some(Cheat::UnlockPet(Species::Bee)),
        "open pebble vault" => Some(Cheat::PebbleVault),
        "rtfm wizard" => Some(Cheat::ManualWizard),
        "up up down down left right left right b a" | "uuddlrlrba" => Some(Cheat::Konami),
        "its dangerous to go alone" => Some(Cheat::ZeldaRelic),
        _ => None,
    }
}

/// Applies a discovered cheat code directly to `pet` and `progress` and renders a celebration note.
pub fn apply(cheat: Cheat, pet: &mut Pet, progress: &mut Progress, now_epoch: u64) -> CheatOutcome {
    let today = date::from_epoch(now_epoch);
    let mut out = String::new();
    match cheat {
        Cheat::UnlockPet(species) => {
            let newly_unlocked = pet.unlock_species(species);
            let _ = pet.set_species(species);
            let banner = if newly_unlocked {
                format!("SECRET PET UNLOCKED: {} ({})!", species.label(), pet.name)
            } else {
                format!("Secret whistle recognized! Switched to {} ({}).", species.label(), pet.name)
            };
            let _ = writeln!(out, "{}", paint(Role::Success, &banner));
            let _ = writeln!(out, "  {}", paint(Role::Dim, "Check your companion anytime with `huh pet` or `/pet`."));
            CheatOutcome { text: out, pet_changed: true, progress_changed: false }
        }
        Cheat::PebbleVault => {
            pet.pamper(now_epoch);
            let _ = writeln!(
                out,
                "{}",
                paint(
                    Role::Success,
                    &format!("Vault unlocked! {} is pampered to 100% fullness & happiness.", pet.name)
                )
            );
            if let Some(def) = pet.grant_secret_pebble("secret_vault", today) {
                let _ = writeln!(out, "  {}", pet::format_pebble_gift(&pet.name, def));
            } else {
                let _ = writeln!(
                    out,
                    "  {}",
                    paint(Role::Dim, "Vault Moonstone is already tucked safely in your pebble pouch.")
                );
            }
            CheatOutcome { text: out, pet_changed: true, progress_changed: false }
        }
        Cheat::ManualWizard => {
            pet.pamper(now_epoch);
            let freezes_before = progress.freezes;
            progress.freezes = progress::MAX_FREEZES;
            let _ = writeln!(
                out,
                "{}",
                paint(
                    Role::Success,
                    &format!(
                        "Manual Wizardry! Streak freezes topped up ({} -> {}) and {} is energized.",
                        freezes_before, progress.freezes, pet.name
                    )
                )
            );
            if let Some(def) = pet.grant_secret_pebble("secret_manual", today) {
                let _ = writeln!(out, "  {}", pet::format_pebble_gift(&pet.name, def));
            } else {
                let _ = writeln!(
                    out,
                    "  {}",
                    paint(Role::Dim, "Manual Bookmark is already in your collection (`huh pet --pebbles`).")
                );
            }
            CheatOutcome { text: out, pet_changed: true, progress_changed: true }
        }
        Cheat::Konami => {
            let newly = pet.unlock_all_species();
            pet.pamper(now_epoch);
            let headline = if newly.is_empty() {
                format!("30 LIVES! All 9 companions already unlocked — {} is hyped at 100%!", pet.name)
            } else {
                let names: Vec<&str> = newly.iter().map(|sp| sp.label()).collect();
                format!(
                    "+30 LIVES! All 9 companions unlocked (new: {}) and {} is pampered!",
                    names.join(", "),
                    pet.name
                )
            };
            let _ = writeln!(out, "{}", paint(Role::Success, &headline));
            if let Some(def) = pet.grant_secret_pebble("secret_konami", today) {
                let _ = writeln!(out, "  {}", pet::format_pebble_gift(&pet.name, def));
            } else {
                let _ =
                    writeln!(out, "  {}", paint(Role::Dim, "8-Bit Star is already shining in `huh pet --pebbles`."));
            }
            CheatOutcome { text: out, pet_changed: true, progress_changed: false }
        }
        Cheat::ZeldaRelic => {
            pet.happiness = 100;
            pet.last_fed_epoch = now_epoch;
            let _ = writeln!(out, "{}", paint(Role::Accent, "    /\\   *   *   *   /\\"));
            let _ = writeln!(out, "{}", paint(Role::Header, "   (o )  \"It's dangerous to go alone — take huh!\""));
            let _ = writeln!(out, "{}", paint(Role::Accent, "   /#!\\  *  [?] -> [!]  *"));
            let _ = writeln!(out, "{}", paint(Role::Dim, "   ====  Pebble's happiness restored to 100%."));
            if let Some(def) = pet.grant_secret_pebble("secret_relic", today) {
                let _ = writeln!(out, "  {}", pet::format_pebble_gift(&pet.name, def));
            } else {
                let _ = writeln!(
                    out,
                    "  {}",
                    paint(Role::Dim, "Ancient Cave Pebble is already glowing in `huh pet --pebbles`.")
                );
            }
            CheatOutcome { text: out, pet_changed: true, progress_changed: false }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lessons::{self, Question};

    const EPOCH: u64 = 1_795_046_400;

    #[test]
    fn lookup_handles_case_punctuation_and_aliases() {
        assert_eq!(lookup("pspsps-dotfile"), Some(Cheat::UnlockPet(Species::Cat)));
        assert_eq!(lookup("  PSPSPS_DOTFILE! "), Some(Cheat::UnlockPet(Species::Cat)));
        assert_eq!(lookup("pspsps dotfile"), Some(Cheat::UnlockPet(Species::Cat)));
        assert_eq!(lookup("bobr-builds-dams"), Some(Cheat::UnlockPet(Species::Beaver)));
        assert_eq!(lookup("grep-for-shells"), Some(Cheat::UnlockPet(Species::Turtle)));
        assert_eq!(lookup("vector-hibernates"), Some(Cheat::UnlockPet(Species::Bear)));
        assert_eq!(lookup("symlink-hiss"), Some(Cheat::UnlockPet(Species::Snake)));
        assert_eq!(lookup("hoot-on-port-443"), Some(Cheat::UnlockPet(Species::Owl)));
        assert_eq!(lookup("catch-the-wave-mpv"), Some(Cheat::UnlockPet(Species::Dolphin)));
        assert_eq!(lookup("honeycomb-palette"), Some(Cheat::UnlockPet(Species::Bee)));
        assert_eq!(lookup("open-pebble-vault"), Some(Cheat::PebbleVault));
        assert_eq!(lookup("RTFM-WIZARD"), Some(Cheat::ManualWizard));
        assert_eq!(lookup("up up down down left right left right b a"), Some(Cheat::Konami));
        assert_eq!(lookup("UUDDLRLRBA!"), Some(Cheat::Konami));
        assert_eq!(lookup("It's dangerous to go alone!"), Some(Cheat::ZeldaRelic));
        assert_eq!(lookup("its dangerous to go alone"), Some(Cheat::ZeldaRelic));

        assert_eq!(lookup("list files in current folder"), None);
        assert_eq!(lookup("ls -la"), None);
        assert_eq!(lookup(""), None);
    }

    #[test]
    fn apply_updates_pet_and_progress_idempotently() {
        let mut pet = Pet::default();
        let mut prog = Progress::default();

        // Pet unlock switches species & updates default mascot name, preserves custom nicknames.
        let res = apply(Cheat::UnlockPet(Species::Beaver), &mut pet, &mut prog, EPOCH);
        assert!(res.pet_changed && !res.progress_changed);
        assert!(pet.is_unlocked(Species::Beaver));
        assert_eq!(pet.species, Species::Beaver);
        assert_eq!(pet.name, "Bóbr");
        assert!(res.text.contains("SECRET PET UNLOCKED"), "{}", res.text);

        pet.rename("Mochi").unwrap();
        let repeat = apply(Cheat::UnlockPet(Species::Owl), &mut pet, &mut prog, EPOCH + 5);
        assert!(pet.is_unlocked(Species::Owl));
        assert_eq!(pet.species, Species::Owl);
        assert_eq!(pet.name, "Mochi");
        assert!(repeat.text.contains("SECRET PET UNLOCKED"), "{}", repeat.text);

        let already = apply(Cheat::UnlockPet(Species::Owl), &mut pet, &mut prog, EPOCH + 10);
        assert!(already.text.contains("Secret whistle recognized"), "{}", already.text);

        // ManualWizard tops up freezes and grants Manual Bookmark once.
        prog.freezes = 0;
        let wiz = apply(Cheat::ManualWizard, &mut pet, &mut prog, EPOCH + 20);
        assert!(wiz.pet_changed && wiz.progress_changed);
        assert_eq!(prog.freezes, progress::MAX_FREEZES);
        assert!(pet.has_pebble("secret_manual"));
        assert_eq!(pet.fullness, 100);
        assert_eq!(pet.happiness, 100);
        let wiz2 = apply(Cheat::ManualWizard, &mut pet, &mut prog, EPOCH + 30);
        assert!(wiz2.text.contains("already in your collection"), "{}", wiz2.text);

        // Vault & ZeldaRelic grant their secret pebbles.
        let vault = apply(Cheat::PebbleVault, &mut pet, &mut prog, EPOCH + 40);
        assert!(vault.text.contains("Vault Moonstone"), "{}", vault.text);
        assert!(pet.has_pebble("secret_vault"));

        let zelda = apply(Cheat::ZeldaRelic, &mut pet, &mut prog, EPOCH + 50);
        assert!(zelda.text.contains("dangerous to go alone"), "{}", zelda.text);
        assert!(zelda.text.contains("Ancient Cave Pebble"), "{}", zelda.text);
        assert!(pet.has_pebble("secret_relic"));

        // Konami unlocks every remaining species and grants 8-Bit Star.
        let konami = apply(Cheat::Konami, &mut pet, &mut prog, EPOCH + 60);
        assert!(konami.text.contains("8-Bit Star"), "{}", konami.text);
        for sp in Species::ALL {
            assert!(pet.is_unlocked(sp), "{sp:?}");
        }
        assert_eq!(pet.secret_pebbles_collected(), 4);
        assert_eq!(pet.pebbles_collected(), 0);
    }

    #[test]
    fn every_unlockable_species_has_a_cheat_code() {
        for sp in Species::ALL {
            if sp == Species::Penguin {
                continue;
            }
            assert!(
                CANONICAL_CODES.iter().any(|(_, cheat)| *cheat == Cheat::UnlockPet(sp)),
                "missing unlock cheat code for {sp:?}"
            );
        }
    }

    #[test]
    fn every_cheat_code_is_hidden_in_embedded_lessons() {
        let units = lessons::load_embedded_for(&["arch".into(), "omarchy".into()]).unwrap();
        let mut corpus = String::new();
        for unit in &units {
            for lesson in &unit.lessons {
                corpus.push_str(&lesson.intro);
                corpus.push('\n');
                for q in &lesson.questions {
                    corpus.push_str(q.prompt());
                    corpus.push('\n');
                    corpus.push_str(q.note());
                    corpus.push('\n');
                    match q {
                        Question::ExplainChoice { command, choices, hint, .. } => {
                            corpus.push_str(command);
                            corpus.push('\n');
                            for c in choices {
                                corpus.push_str(c);
                                corpus.push('\n');
                            }
                            corpus.push_str(hint);
                            corpus.push('\n');
                        }
                        Question::TypeCommand { accepted, hints, .. } => {
                            for a in accepted {
                                corpus.push_str(a);
                                corpus.push('\n');
                            }
                            for h in hints {
                                corpus.push_str(h);
                                corpus.push('\n');
                            }
                        }
                        Question::ReadOutput { command, output, choices, hint, .. } => {
                            corpus.push_str(command);
                            corpus.push('\n');
                            corpus.push_str(output);
                            corpus.push('\n');
                            for c in choices {
                                corpus.push_str(c);
                                corpus.push('\n');
                            }
                            corpus.push_str(hint);
                            corpus.push('\n');
                        }
                        Question::SpotDanger { command, choices, hint, .. } => {
                            if let Some(cmd) = command {
                                corpus.push_str(cmd);
                                corpus.push('\n');
                            }
                            for c in choices {
                                corpus.push_str(c);
                                corpus.push('\n');
                            }
                            corpus.push_str(hint);
                            corpus.push('\n');
                        }
                    }
                }
            }
            if let Some(practice) = &unit.practice {
                for file in &practice.setup.files {
                    corpus.push_str(&file.content);
                    corpus.push('\n');
                }
            }
        }
        let lower = corpus.to_ascii_lowercase();
        for &(code, _) in CANONICAL_CODES {
            assert!(lower.contains(&code.to_ascii_lowercase()), "cheat code `{code}` not hidden in any lesson");
        }
    }

    #[test]
    fn cheat_codes_are_not_shell_commands() {
        let known = |word: &str| crate::COMMON_PROGRAMS.contains(&word);
        for &(code, _) in CANONICAL_CODES {
            assert!(
                crate::guard::command_line_request(code, &known).is_none(),
                "cheat `{code}` falsely detected as a shell command"
            );
        }
    }
}
