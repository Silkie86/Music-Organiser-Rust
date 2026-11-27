//! Makes the logo's pictures from the SVGs in `assets/logo` (tools/logo/draw.py draws those):
//!
//! - `logo.png`: 256 by 256, the window's icon (product.toml `logo_png`).
//! - `logo.ico`: Windows' icon for the .exe, the installer and the shortcuts (product.toml `logo_ico`). 16, 20, 24 and
//!   32 pixels come from their own hand-placed drawings (logo-16.svg ...), 40 to 256 from logo.svg.
//! - `logo.icns`: the Mac's icon, from logo-mac.svg (Apple's grid), 16 to 1024 pixels (product.toml `logo_icns`).
//! - `setup-side-*.bmp`, `setup-small-*.bmp`: the installer's pictures, one for each screen scale Inno Setup knows
//!   (installer/setup.iss picks them up). Plain 24-bit pictures, which every Inno Setup 6 reads.
//! - `wordmark-dark.png`, `wordmark-light.png`: the icon with the name, 128 pixels high, for pages and the README.
//!
//! Run from anywhere: `cargo run --release --manifest-path tools/logo/Cargo.toml`. It writes each file next to a
//! temporary copy first and then renames it over the old one, so a failed run never leaves half a picture.

use std::fs;
use std::path::{Path, PathBuf};

use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{Options, Tree};

/// The sizes Windows asks an .ico for: small icons at 100 to 200% screen scale, the taskbar, Explorer's views.
const ICO_SMALL: &[u32] = &[16, 20, 24, 32];
const ICO_LARGE: &[u32] = &[40, 48, 64, 96, 128, 256];

/// Inno Setup 6.6's picture sizes at 100, 150 and 200% screen scale (it picks the nearest and stretches it).
const SIDE: &[(&str, u32, u32)] = &[("100", 202, 386), ("150", 336, 643), ("200", 430, 824)];
const SMALL: &[(&str, u32)] = &[("100", 58), ("150", 97), ("200", 124)];

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("assets").join("logo");
    let dir = dir.canonicalize().unwrap_or_else(|e| panic!("can't find {}: {e}", dir.display()));

    let logo = load(&dir.join("logo.svg"));
    save(&dir.join("logo.png"), &png(&render(&logo, 256, 256)));

    let mut ico = ico::IconDir::new(ico::ResourceType::Icon);
    for &size in ICO_SMALL {
        let drawing = load(&dir.join(format!("logo-{size}.svg")));
        ico.add_entry(ico_entry(&render(&drawing, size, size)));
    }
    for &size in ICO_LARGE {
        ico.add_entry(ico_entry(&render(&logo, size, size)));
    }
    let mut bytes = Vec::new();
    ico.write(&mut bytes).expect("the .ico should write to memory");
    save(&dir.join("logo.ico"), &bytes);

    save(&dir.join("logo.icns"), &icns(&load(&dir.join("logo-mac.svg"))));

    let side = load(&dir.join("setup-side.svg"));
    for &(scale, w, h) in SIDE {
        save(&dir.join(format!("setup-side-{scale}.bmp")), &bmp(&render(&side, w, h)));
    }
    let small = load(&dir.join("setup-small.svg"));
    for &(scale, size) in SMALL {
        save(&dir.join(format!("setup-small-{scale}.bmp")), &bmp(&render(&small, size, size)));
    }

    for name in ["wordmark-dark", "wordmark-light"] {
        let tree = load(&dir.join(format!("{name}.svg")));
        let height = 128;
        // Rounded down, so the drawing at its own shape fills it (a sliver at most is cut from the right-hand gap).
        let width = (tree.size().width() * height as f32 / tree.size().height()).floor() as u32;
        save(&dir.join(format!("{name}.png")), &png(&render(&tree, width, height)));
    }
    println!("made the logo's pictures in {}", dir.display());
}

fn load(file: &Path) -> Tree {
    let text = fs::read(file).unwrap_or_else(|e| panic!("can't read {}: {e}", file.display()));
    Tree::from_data(&text, &Options::default())
        .unwrap_or_else(|e| panic!("{} isn't an SVG we can read: {e}", file.display()))
}

/// The drawing on `w` by `h` pixels at its own shape, never stretched: scaled to fill the picture and centred, so when
/// the shapes differ a little (Setup's sizes, a width rounded up to whole pixels) a sliver of the edge is cut off.
fn render(tree: &Tree, w: u32, h: u32) -> Pixmap {
    let mut pixmap = Pixmap::new(w, h).expect("a picture of at least one pixel");
    let size = tree.size();
    let scale = (w as f32 / size.width()).max(h as f32 / size.height());
    let (dx, dy) = ((w as f32 - size.width() * scale) / 2.0, (h as f32 - size.height() * scale) / 2.0);
    resvg::render(tree, Transform::from_scale(scale, scale).post_translate(dx, dy), &mut pixmap.as_mut());
    pixmap
}

/// The pixels as plain red, green, blue, see-through bytes (tiny-skia keeps them pre-multiplied).
fn rgba(pixmap: &Pixmap) -> Vec<u8> {
    pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}

fn png(pixmap: &Pixmap) -> Vec<u8> {
    pixmap.encode_png().expect("the picture should save as PNG")
}

/// Small sizes as BMP (every Windows reads those), 256 as PNG (Windows' own rule for the biggest size).
fn ico_entry(pixmap: &Pixmap) -> ico::IconDirEntry {
    let image = ico::IconImage::from_rgba_data(pixmap.width(), pixmap.height(), rgba(pixmap));
    if pixmap.width() >= 256 {
        ico::IconDirEntry::encode_as_png(&image)
    } else {
        ico::IconDirEntry::encode_as_bmp(&image)
    }
    .expect("the icon size should encode")
}

/// The Mac's sizes: 16 to 512 at 1x and 2x (1024 is 512 at 2x), each a PNG inside the .icns.
fn icns(tree: &Tree) -> Vec<u8> {
    use icns::IconType::*;
    let mut family = icns::IconFamily::new();
    for (kind, size) in [
        (RGBA32_16x16, 16),
        (RGBA32_16x16_2x, 32),
        (RGBA32_32x32, 32),
        (RGBA32_32x32_2x, 64),
        (RGBA32_128x128, 128),
        (RGBA32_128x128_2x, 256),
        (RGBA32_256x256, 256),
        (RGBA32_256x256_2x, 512),
        (RGBA32_512x512, 512),
        (RGBA32_512x512_2x, 1024),
    ] {
        let image = icns::Image::from_data(icns::PixelFormat::RGBA, size, size, rgba(&render(tree, size, size)))
            .expect("the picture's bytes should be the right length");
        family.add_icon_with_type(&image, kind).expect("the icon size should encode");
    }
    let mut bytes = Vec::new();
    family.write(&mut bytes).expect("the .icns should write to memory");
    bytes
}

/// A plain 24-bit BMP. The installer's pictures have no see-through parts: anything see-through is laid on white.
fn bmp(pixmap: &Pixmap) -> Vec<u8> {
    let rgb: Vec<u8> = rgba(pixmap)
        .chunks_exact(4)
        .flat_map(|p| {
            let a = u16::from(p[3]);
            let on_white = |c: u8| ((u16::from(c) * a + 255 * (255 - a)) / 255) as u8;
            [on_white(p[0]), on_white(p[1]), on_white(p[2])]
        })
        .collect();
    let image = image::RgbImage::from_raw(pixmap.width(), pixmap.height(), rgb).expect("the right number of pixels");
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Bmp).expect("the picture should save as BMP");
    bytes.into_inner()
}

/// Writes beside the file first, then renames over it: copy, check, rename.
fn save(file: &Path, bytes: &[u8]) {
    let mut part = PathBuf::from(file);
    part.set_extension(format!("{}.part", file.extension().and_then(|e| e.to_str()).unwrap_or("")));
    fs::write(&part, bytes).unwrap_or_else(|e| panic!("can't write {}: {e}", part.display()));
    assert_eq!(fs::read(&part).map(|b| b.len()).ok(), Some(bytes.len()), "{} didn't write in full", part.display());
    fs::rename(&part, file).unwrap_or_else(|e| panic!("can't put {} in place: {e}", file.display()));
}
