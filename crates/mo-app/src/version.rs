//! Which build this is, for the session log's header and F12 notes.

/// "0.9.0-beta.1 build 812 (abc1234)": the app's version, the build's number (the commits that lead to it, so it
/// counts up with every build) and the commit it was built from. No number from a clone that can't count them.
pub fn version() -> String {
    label(env!("CARGO_PKG_VERSION"), env!("MO_BUILD"), env!("MO_COMMIT"))
}

fn label(version: &str, build: &str, commit: &str) -> String {
    if build.is_empty() { format!("{version} ({commit})") } else { format!("{version} build {build} ({commit})") }
}

/// The session log's first lines, like the C# app's.
pub fn log_header(start: chrono::DateTime<chrono::Local>) -> String {
    log_header_of(crate::product::NAME, start)
}

/// [`log_header`] for a run of `what` (the app's name, or its helper's: [`crate::product`]).
pub fn log_header_of(what: &str, start: chrono::DateTime<chrono::Local>) -> String {
    let build = if cfg!(debug_assertions) { "Debug" } else { "Release" };
    let watched = if std::env::var_os("MO_WATCH").is_some() {
        "the launcher (rebuilds and restarts on every push)"
    } else if std::env::var_os("CARGO_WATCH_COMMAND").is_some() {
        "cargo watch (rebuilds and restarts on every change)"
    } else {
        "nothing"
    };
    let folder =
        std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.display().to_string())).unwrap_or_default();
    format!(
        "{what} started {}\n  Version:    {}\n  Build:      {build}\n  Rust:       {}\n  Watched by: {watched}\n  System:     {} {}\n  Folder:     {folder}",
        start.format("%Y-%m-%d %H:%M:%S %:z"),
        version(),
        env!("MO_RUSTC"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

#[cfg(test)]
mod tests {
    use super::label;

    #[test]
    fn the_label_has_the_build_number_when_there_is_one() {
        assert_eq!(label("0.9.0-beta.1", "812", "abc1234"), "0.9.0-beta.1 build 812 (abc1234)");
        assert_eq!(label("0.9.0-beta.1", "", "abc1234"), "0.9.0-beta.1 (abc1234)");
    }
}
