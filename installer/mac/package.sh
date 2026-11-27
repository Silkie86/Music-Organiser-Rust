#!/bin/bash
# Makes the Mac download: DAWmate.app (the app) and DAWmate.clap (the CLAP plugin) in one .zip, from builds made with
#   cargo build --locked --profile dist --target <arch>-apple-darwin -p mo-ui --bin music-organiser
#   cargo build --locked --profile dist --target <arch>-apple-darwin -p mo-plugin
# for each of aarch64 (Apple silicon) and x86_64 (Intel): both go in one universal file.
#
#   installer/mac/package.sh <out folder>
#
# The bundles are signed "ad hoc" (no Apple Developer ID): Apple silicon runs nothing unsigned, and a bundle's
# Info.plist must be sealed by a signature. Not notarised, so the first open needs right-click, Open (docs/installer.md).
# Plain ASCII, and only what a Mac has (bash 3.2, plutil, lipo, sips, iconutil, codesign, ditto).
set -euo pipefail

out=${1:?"the out folder"}
top=$(cd "$(dirname "$0")/../.." && pwd)
cd "$top"

name=$(sed -n 's/^name = "\(.*\)"/\1/p' product.toml | head -1)
publisher=$(sed -n 's/^publisher = "\(.*\)"/\1/p' product.toml | head -1)
version=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)
[ -n "$name" ] && [ -n "$version" ] || { echo "product.toml or Cargo.toml has no name or version"; exit 1; }
# CFBundleVersion takes numbers and dots only: 0.9.0-beta.2 is 0.9.0 there, and whole in the short version.
numbers=${version%%-*}
# The oldest macOS the builds are made for (MACOSX_DEPLOYMENT_TARGET in the workflow): the same in Info.plist.
minimum=${MACOSX_DEPLOYMENT_TARGET:-11.0}

arm=target/aarch64-apple-darwin/dist
intel=target/x86_64-apple-darwin/dist
for f in "$arm/music-organiser" "$intel/music-organiser" "$arm/libmo_plugin.dylib" "$intel/libmo_plugin.dylib"; do
    [ -f "$f" ] || { echo "no $f: build both targets first"; exit 1; }
done

stage="$out/stage"
rm -rf "$stage"
mkdir -p "$stage"

# The app. Its program keeps the name it has everywhere (music-organiser); people see the bundle's name.
app="$stage/$name.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
lipo -create -output "$app/Contents/MacOS/music-organiser" "$arm/music-organiser" "$intel/music-organiser"

# The logo, as a Mac icon, from the same PNG the window uses (256 px: Finder scales it for the bigger sizes).
icons="$out/logo.iconset"
rm -rf "$icons"
mkdir -p "$icons"
for size in 16 32 128 256; do
    sips -z "$size" "$size" assets/logo/logo.png --out "$icons/icon_${size}x${size}.png" > /dev/null
    half=$((size / 2))
    if [ "$half" -ge 16 ]; then cp "$icons/icon_${size}x${size}.png" "$icons/icon_${half}x${half}@2x.png"; fi
done
iconutil -c icns "$icons" -o "$app/Contents/Resources/logo.icns"
rm -rf "$icons"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>en</string>
    <key>CFBundleDisplayName</key><string>$name</string>
    <key>CFBundleExecutable</key><string>music-organiser</string>
    <key>CFBundleIconFile</key><string>logo</string>
    <key>CFBundleIdentifier</key><string>com.silkie86.music-organiser</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>CFBundleName</key><string>$name</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$numbers</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.music</string>
    <key>LSMinimumSystemVersion</key><string>$minimum</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSHumanReadableCopyright</key><string>$publisher</string>
    <!-- Recording a take from the sound card's input: without these words macOS closes the app when it opens one. -->
    <key>NSMicrophoneUsageDescription</key><string>$name records a take from your sound card's input when you ask it to.</string>
</dict>
</plist>
PLIST

# The CLAP plugin: on a Mac a .clap is a bundle folder, its program inside.
clap="$stage/$name.clap"
mkdir -p "$clap/Contents/MacOS"
lipo -create -output "$clap/Contents/MacOS/$name" "$arm/libmo_plugin.dylib" "$intel/libmo_plugin.dylib"
cat > "$clap/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>en</string>
    <key>CFBundleExecutable</key><string>$name</string>
    <key>CFBundleIdentifier</key><string>com.silkie86.music-organiser.clap</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>CFBundleName</key><string>$name</string>
    <key>CFBundlePackageType</key><string>BNDL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$numbers</string>
    <key>LSMinimumSystemVersion</key><string>$minimum</string>
</dict>
</plist>
PLIST

# Ad hoc signatures, inside out.
codesign --force --sign - "$clap"
codesign --force --sign - "$app"

cat > "$stage/Read me.txt" <<TEXT
$name $version for Mac (Apple silicon and Intel, macOS $minimum or later)

1. Drag $name.app into Applications.
2. The first time, right-click it and pick Open, then Open again: this test build isn't from an
   identified developer yet, so a plain double-click is refused.
3. The CLAP plugin: copy $name.clap into your Library/Audio/Plug-Ins/CLAP folder (in Finder: Go, Go to
   Folder, ~/Library/Audio/Plug-Ins/CLAP; make the CLAP folder if it isn't there), then rescan plugins
   in your DAW.
TEXT

zip="$out/$name-$version-mac.zip"
rm -f "$zip"
# ditto keeps what a Mac bundle needs (links, signatures, the program's x bit) where zip may not.
(cd "$stage" && ditto -c -k --sequesterRsrc . "../$(basename "$zip")")
echo "$zip"
