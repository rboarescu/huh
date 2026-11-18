//! huh's own risk categories. Every warning (a request suggestion's `WARNING`, a `huh explain`
//! line under WARNINGS, a `huh why` note) names exactly one of them, so the same kind of danger
//! always reads the same way. Warnings inform; they never forbid.

use serde::Deserialize;

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Risk {
    #[serde(rename = "destroys data")]
    DestroysData,
    #[serde(rename = "rewrites history")]
    RewritesHistory,
    #[serde(rename = "runs code from the internet")]
    RunsInternetCode,
    #[serde(rename = "runs unread code")]
    RunsUnreadCode,
    #[serde(rename = "gives away access")]
    GivesAwayAccess,
    #[serde(rename = "weakens security")]
    WeakensSecurity,
    #[serde(rename = "exposes a service")]
    ExposesService,
    #[serde(rename = "leaks secrets")]
    LeaksSecrets,
    #[serde(rename = "hides tracks")]
    HidesTracks,
    #[serde(rename = "persists")]
    Persists,
    #[serde(rename = "affects other people or production")]
    AffectsOthers,
    #[serde(rename = "kills work")]
    KillsWork,
    #[serde(rename = "big system change")]
    BigSystemChange,
    #[serde(rename = "heads-up")]
    HeadsUp,
}

impl Risk {
    pub const ALL: [Risk; 14] = [
        Risk::DestroysData,
        Risk::RewritesHistory,
        Risk::RunsInternetCode,
        Risk::RunsUnreadCode,
        Risk::GivesAwayAccess,
        Risk::WeakensSecurity,
        Risk::ExposesService,
        Risk::LeaksSecrets,
        Risk::HidesTracks,
        Risk::Persists,
        Risk::AffectsOthers,
        Risk::KillsWork,
        Risk::BigSystemChange,
        Risk::HeadsUp,
    ];

    /// The short name shown in every warning.
    pub fn name(self) -> &'static str {
        match self {
            Risk::DestroysData => "destroys data",
            Risk::RewritesHistory => "rewrites history",
            Risk::RunsInternetCode => "runs code from the internet",
            Risk::RunsUnreadCode => "runs unread code",
            Risk::GivesAwayAccess => "gives away access",
            Risk::WeakensSecurity => "weakens security",
            Risk::ExposesService => "exposes a service",
            Risk::LeaksSecrets => "leaks secrets",
            Risk::HidesTracks => "hides tracks",
            Risk::Persists => "persists",
            Risk::AffectsOthers => "affects other people or production",
            Risk::KillsWork => "kills work",
            Risk::BigSystemChange => "big system change",
            Risk::HeadsUp => "heads-up",
        }
    }

    /// One plain sentence on what the category means.
    pub fn explanation(self) -> &'static str {
        match self {
            Risk::DestroysData => "deletes or overwrites data in a way you cannot undo",
            Risk::RewritesHistory => {
                "changes git history, so earlier commits can be lost or differ from what others have"
            }
            Risk::RunsInternetCode => "downloads code and runs it before you have read it",
            Risk::RunsUnreadCode => "runs code that huh cannot see into: eval, inline scripts, installer scripts",
            Risk::GivesAwayAccess => "hands someone or something more rights: permissions, groups, root, sudoers",
            Risk::WeakensSecurity => "turns off a check or protection: certificates, host keys, firewall, SELinux",
            Risk::ExposesService => "makes a service reachable from other computers or the internet",
            Risk::LeaksSecrets => "puts a password, token or private key where others or logs can see it",
            Risk::HidesTracks => "erases history or logs that would show what happened",
            Risk::Persists => "makes something run again later: cron, startup files, services at boot",
            Risk::AffectsOthers => "changes something shared: a shared branch, a registry, a cluster, production",
            Risk::KillsWork => "stops programs or the whole machine, losing unsaved work",
            Risk::BigSystemChange => "changes the whole system: packages, kernel, system settings, admin rights",
            Risk::HeadsUp => "does not end by itself or prints a lot: a note, not a danger",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_category_has_a_unique_name_and_an_explanation() {
        let mut names: Vec<&str> = Risk::ALL.iter().map(|r| r.name()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), Risk::ALL.len());
        assert!(Risk::ALL.iter().all(|r| !r.explanation().is_empty() && !r.explanation().ends_with('.')));
    }

    /// The README's "Warnings" table lists every category with its meaning.
    #[test]
    fn the_readme_lists_every_category() {
        let readme = include_str!("../README.md");
        for r in Risk::ALL {
            let row = format!("| {} | {} |", r.name(), r.explanation());
            assert!(readme.contains(&row), "README.md is missing the row: {row}");
        }
    }
}
