#!/bin/bash
# Makes the Mac download: DAWmate.app in a .zip, with the CLAP plugin (Contents/PlugIns/DAWmate.clap) and the Bitwig
# extension (Contents/Resources/extensions) inside it, which the app puts in by itself as it does on Windows
# (mo-ui's daw_links). From builds made with
#   cargo build --locked --profile dist --target <arch>-apple-darwin -p mo-ui --bin music-organiser
#   cargo build --locked --profile dist --target <arch>-apple-darwin -p mo-plugin
# for each of aarch64 (Apple silicon) and x86_64 (Intel): both go in one universal file.
#
#   installer/mac/package.sh <out folder> <DAWmate.bwextension>
#
# The bundles are signed "ad hoc" (no Apple Developer ID): Apple silicon runs nothing unsigned, and a bundle's
# Info.plist must be sealed by a signature. Not notarised, so the first open needs right-click, Open (docs/installer.md).
# Plain ASCII, and only what a Mac has (bash 3.2, plutil, lipo, sips, iconutil, codesign, ditto).
set -euo pipefail

out=${1:?"the out folder"}
extension=${2:?"the built Bitwig extension"}
[ -f "$extension" ] || { echo "no $extension"; exit 1; }
extension=$(cd "$(dirname "$extension")" && pwd)/$(basename "$extension")
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
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/extensions" "$app/Contents/PlugIns"
cp "$extension" "$app/Contents/Resources/extensions/$name.bwextension"
lipo -create -output "$app/Contents/MacOS/music-organiser" "$arm/music-organiser" "$intel/music-organiser"

# The logo, as a Mac icon, from the same PNG the window uses (256 px: Finder scales it for the bigger sizes).
icons="$out/logo.iconset"
rm -rf "$icons"
mkdir -p "$icons"
# Each slot an iconset has, with its size in pixels (the @2x ones are twice their name).
for slot in 16x16:16 16x16@2x:32 32x32:32 32x32@2x:64 128x128:128 128x128@2x:256 256x256:256; do
    px=${slot#*:}
    sips -z "$px" "$px" assets/logo/logo.png --out "$icons/icon_${slot%%:*}.png" > /dev/null
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
    <!-- Sending files to the Trash goes through Finder (so Finder's Put Back works): macOS asks once, with these words. -->
    <key>NSAppleEventsUsageDescription</key><string>$name asks Finder to move files you remove to the Trash, so Put Back works.</string>
    <!-- Recording a take from the sound card's input: without these words macOS closes the app when it opens one. -->
    <key>NSMicrophoneUsageDescription</key><string>$name records a take from your sound card's input when you ask it to.</string>
</dict>
</plist>
PLIST

# The CLAP plugin: on a Mac a .clap is a bundle folder, its program inside. It goes in the app's PlugIns; the app copies
# it into ~/Library/Audio/Plug-Ins/CLAP itself.
clap="$app/Contents/PlugIns/$name.clap"
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
3. Half a minute after it starts, $name puts its Bitwig extension, its FL Studio script and its CLAP
   plugin (in ~/Library/Audio/Plug-Ins/CLAP) in by itself, and says so. Restart your DAW to see them.
TEXT

zip="$out/$name-$version-mac.zip"
rm -f "$zip"
# ditto keeps what a Mac bundle needs (links, signatures, the program's x bit) where zip may not.
(cd "$stage" && ditto -c -k --sequesterRsrc . "../$(basename "$zip")")
echo "$zip"
