// Puts the commit the app was built from into its version line, so a log or an F12 note says exactly which build ran.
fn main() {
    let commit = std::env::var("GITHUB_SHA")
        .ok()
        .map(|s| s.chars().take(7).collect())
        .or_else(|| {
            let out = std::process::Command::new("git").args(["rev-parse", "--short=7", "HEAD"]).output().ok()?;
            out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        })
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=MO_COMMIT={commit}");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
}
