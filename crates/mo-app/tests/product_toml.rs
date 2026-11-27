//! product.toml, the one place the app's name is kept, read the way the build scripts read it.

#[path = "../product_toml.rs"]
#[allow(dead_code)]
mod product_toml;

use std::path::Path;

#[test]
fn the_real_file_has_every_key_and_its_logos() {
    let file = product_toml::path(Path::new(env!("CARGO_MANIFEST_DIR")));
    let product = product_toml::read(&file);
    let top = file.parent().unwrap();
    assert!(top.join(&product["logo_png"]).is_file());
    assert!(top.join(&product["logo_ico"]).is_file());
    // The Mac's icon: no build reads it yet (the Mac app isn't bundled), so it's checked here, not by the build.
    assert!(top.join(product.get("logo_icns").expect("product.toml names the Mac's icon")).is_file());
    // The installer's pictures, beside the .ico (installer/setup.iss names them).
    let pictures = top.join(&product["logo_ico"]).parent().unwrap().to_path_buf();
    for scale in ["100", "150", "200"] {
        assert!(pictures.join(format!("setup-side-{scale}.bmp")).is_file(), "setup-side-{scale}.bmp");
        assert!(pictures.join(format!("setup-small-{scale}.bmp")).is_file(), "setup-small-{scale}.bmp");
    }
    // Windows knows the installed app by this, so it must stay a GUID (and never change).
    let id = &product["installer_id"];
    assert_eq!(id.len(), 36, "{id}");
    assert!(id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'), "{id}");
    assert_eq!(product["name"], mo_app::product::NAME);
}

#[test]
fn reads_words_and_skips_comments() {
    let found = product_toml::parse("# a note\n\nname = \"New Name\"\n  website = \"https://x\"  \n").unwrap();
    assert_eq!(found["name"], "New Name");
    assert_eq!(found["website"], "https://x");
    assert_eq!(found.len(), 2);
}

#[test]
fn turns_away_what_it_cannot_pass_on_as_it_is() {
    for bad in [
        "name = Music",               // not in quotes
        "name = \"a \\\" b\"",        // a quote inside
        "name = \"C:\\\\x\"",         // a backslash
        "name = \"\"",                // nothing
        "Name = \"x\"",               // not a lower-case key
        "name \"x\"",                 // no =
        "name = \"a\"\nname = \"b\"", // twice
    ] {
        assert!(product_toml::parse(bad).is_err(), "{bad}");
    }
}
