// Puts the commit the app was built from into its version line, so a log or an F12 note says exactly which build ran,
// and the app's name and the words around it from product.toml (mo_app::product).

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
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/heads");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");

    let here = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let file = product_toml::path(&here);
    let product = product_toml::read(&file);
    for key in ["name", "description", "publisher", "website"] {
        println!("cargo:rustc-env=MO_PRODUCT_{}={}", key.to_ascii_uppercase(), product[key]);
    }
    println!("cargo:rerun-if-changed={}", file.display());
    println!("cargo:rerun-if-changed=product_toml.rs");
}
