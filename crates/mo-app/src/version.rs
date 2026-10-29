//! Which build this is, for the session log's header and F12 notes.

/// "0.1.0 (abc1234)": the app's version and the commit it was built from.
pub fn version() -> String {
    format!("{} ({})", env!("CARGO_PKG_VERSION"), env!("MO_COMMIT"))
}

/// The session log's first lines, like the C# app's.
pub fn log_header(start: chrono::DateTime<chrono::Local>) -> String {
    let build = if cfg!(debug_assertions) { "Debug" } else { "Release" };
    let watched = if std::env::var_os("CARGO_WATCH_COMMAND").is_some() || std::env::var_os("MO_WATCH").is_some() {
        "cargo watch (rebuilds and restarts on every change)"
    } else {
        "nothing"
    };
    let folder =
        std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.display().to_string())).unwrap_or_default();
    format!(
        "Music Organiser started {}\n  Version:    {}\n  Build:      {build}\n  Watched by: {watched}\n  System:     {} {}\n  Folder:     {folder}",
        start.format("%Y-%m-%d %H:%M:%S %:z"),
        version(),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}
