// A build script that stops just fails the build, with its message, so it may expect.
#![allow(clippy::expect_used)]

//! Compiles the Slint screens, with the app's name and logo from product.toml (the one place they're kept) as the
//! `Product` global the screens import from "@product". On Windows the logo, the name and the version also go into the
//! .exe itself, so Explorer, the taskbar and the file's Details tab show them.

#[path = "../mo-app/product_toml.rs"]
mod product_toml;

use std::path::{Path, PathBuf};

fn main() {
    let here = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo gives the crate's folder"));
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo gives a folder for made files"));
    let file = product_toml::path(&here);
    let product = product_toml::read(&file);
    let top = file.parent().expect("product.toml is in a folder").to_owned();
    let logo_png = top.join(&product["logo_png"]);
    let logo_ico = top.join(&product["logo_ico"]);
    for f in [&logo_png, &logo_ico] {
        assert!(f.is_file(), "the logo {} named in product.toml isn't there", f.display());
        println!("cargo:rerun-if-changed={}", f.display());
    }
    println!("cargo:rerun-if-changed={}", file.display());
    println!("cargo:rerun-if-changed=../mo-app/product_toml.rs");

    let slint = out.join("product.slint");
    std::fs::write(&slint, product_slint(&product["name"], &logo_png)).expect("the made Product global should save");
    let config = slint_build::CompilerConfiguration::new()
        .with_library_paths(std::iter::once(("product".to_owned(), slint)).collect());
    slint_build::compile_with_config("ui/app.slint", config).expect("the Slint screens should compile");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_details(&product, &logo_ico);
    }
}

/// The `Product` global: the name and the window's logo. product_toml has already turned away a name with `"` or `\`.
fn product_slint(name: &str, logo: &Path) -> String {
    // Slint takes `/` in a path on every system.
    let logo = logo.display().to_string().replace('\\', "/");
    format!(
        "// Made by mo-ui's build.rs from product.toml. Edit that, not this.\n\
         export global Product {{\n    \
             out property <string> name: \"{name}\";\n    \
             out property <image> logo: @image-url(\"{logo}\");\n\
         }}\n"
    )
}

/// The logo, name, description and version in the .exe. Only when building on Windows (where the resource compiler
/// is); a build for Windows from elsewhere, or one where the resource compiler can't be found, goes without and says
/// so: a missing icon is never worth a failed build.
fn windows_details(product: &std::collections::HashMap<String, String>, ico: &Path) {
    #[cfg(windows)]
    {
        let version = std::env::var("CARGO_PKG_VERSION").expect("cargo gives the version");
        let mut res = winresource::WindowsResource::new();
        res.set_icon(&ico.display().to_string())
            .set("ProductName", &product["name"])
            .set("FileDescription", &product["name"])
            .set("CompanyName", &product["publisher"])
            .set("Comments", &product["description"])
            .set("ProductVersion", &version)
            .set("FileVersion", &version);
        if let Err(e) = res.compile() {
            println!("cargo:warning=the .exe has no logo or details: {e}");
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (product, ico);
        println!("cargo:warning=built for Windows away from Windows: the .exe has no logo or details");
    }
}
