// Puts the commit the app was built from into its version line, so a log or an F12 note says exactly which build ran,
// and the Rust that built it, and the app's name and the words around it from product.toml (mo_app::product).

#[path = "product_toml.rs"]
mod product_toml;

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
    // The build's number: how many commits lead to it, so each build Solomon tests counts up from the last (25 Nov:
    // "all these new builds have the same beta number"). The beta number itself (Cargo.toml) moves on with each
    // release, which self-update compares. A clone without the whole history (shallow) can't count: no number then.
    // Release builds only (the installer from GitHub's Release action, the launcher's Release build): Solomon, 25 Nov,
    // "only for installers built from the action on github or ... the release builder on the desktop". Cargo says
    // "release" for the release profile and for the ones built on it (the installer's dist).
    let release = std::env::var("PROFILE").is_ok_and(|p| p == "release");
    let build = std::env::var("MO_BUILD").ok().filter(|b| !b.is_empty()).or_else(|| {
        if !release {
            return None;
        }
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git").args(args).output().ok()?;
            out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        };
        (git(&["rev-parse", "--is-shallow-repository"])? == "false")
            .then(|| git(&["rev-list", "--count", "HEAD"]))
            .flatten()
    });
    println!("cargo:rustc-env=MO_BUILD={}", build.unwrap_or_default());
    println!("cargo:rerun-if-env-changed=MO_BUILD");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");

    // The Rust that built it, for the log's first lines ("Rust:", a line of its own: the self-check reads "Build:"):
    // how the standard library looks at files can change from one Rust to the next (Solomon's log of 25 Nov 2026:
    // every moved folder's link failed a check that had passed). A new Rust builds everything again, this with it.
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let rust = std::process::Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=MO_RUSTC={rust}");

    let here = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let file = product_toml::path(&here);
    let product = product_toml::read(&file);
    for key in ["name", "description", "publisher", "website"] {
        println!("cargo:rustc-env=MO_PRODUCT_{}={}", key.to_ascii_uppercase(), product[key]);
    }
    println!("cargo:rerun-if-changed={}", file.display());
    println!("cargo:rerun-if-changed=product_toml.rs");
}
