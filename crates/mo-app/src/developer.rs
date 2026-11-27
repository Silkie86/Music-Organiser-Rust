//! Whether this is Solomon's own copy of the app or a beta tester's (27 Nov 2026). Solomon's shows Settings ›
//! Developer (the beta checklist, the self-check, where the logs are, the made-up activity card); a tester's hides it,
//! along with the daily self-check that marks that checklist.
//!
//! A copy the launcher built (not put in by the installer) is always his: testers only ever use the installer. It
//! also keeps the `developer` setting, so his installed copy, which shares his data folder, shows the tab too.

use std::sync::OnceLock;

/// The setting that says this data folder is Solomon's: "1" once a launcher build has run on it.
pub const SETTING_KEY: &str = "developer";

static ON: OnceLock<bool> = OnceLock::new();

/// Whether the Developer parts show, from whether this copy was installed and the `developer` setting.
pub fn decide(installed: bool, saved: Option<&str>) -> bool {
    !installed || saved == Some("1")
}

/// Whether a run should keep the `developer` setting now: a launcher build on a data folder that hasn't got it yet.
pub fn keeps_setting(installed: bool, saved: Option<&str>) -> bool {
    !installed && saved != Some("1")
}

/// Fixes the answer for this run, once, at the start. Later calls change nothing.
pub fn set(on: bool) {
    let _ = ON.set(on);
}

/// Whether the Developer parts show. Before the start has decided (tests, the screenshots) they do.
pub fn on() -> bool {
    ON.get().copied().unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_installed_copy_without_the_setting_is_a_testers() {
        // A launcher build: always Solomon's, and it keeps the setting for his installed copy.
        assert!(decide(false, None));
        assert!(keeps_setting(false, None));
        assert!(!keeps_setting(false, Some("1")));
        // His installed copy, on the data folder a launcher build has marked.
        assert!(decide(true, Some("1")));
        assert!(!keeps_setting(true, Some("1")));
        // A tester's: installed, on a data folder no launcher build has run on.
        assert!(!decide(true, None));
        assert!(!decide(true, Some("0")));
        assert!(!keeps_setting(true, None));
    }

    #[test]
    fn developer_is_the_last_tab_so_the_others_keep_their_places() {
        use crate::settings::Tab;
        assert_eq!(Tab::ALL.last(), Some(&Tab::Developer));
        assert!(!Tab::ALL[..4].contains(&Tab::Developer));
    }
}
