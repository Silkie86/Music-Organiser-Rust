#!/bin/bash
# Tries the Mac download the way a tester gets it, on a CI Mac (never on someone's own: it starts the app):
#
#   1. Both bundles are whole: Info.plist reads, the programs hold Apple silicon and Intel, the signatures check, and
#      the plugin offers clap_entry (what a DAW looks for).
#   2. The app starts, with a data folder of its own (HOME is a scratch folder, so nothing of the machine's is read
#      and the DAW links find no DAW), keeps running for a minute (past the DAW links' look at 30 s), writes its log
#      and no crash log, and closes when asked (--close-for-setup, as the uninstaller asks on Windows).
#
#   installer/mac/check.sh dist/DAWmate-0.9.0-mac.zip
#
# Ends with 1 if anything failed, after trying everything. Plain ASCII; bash 3.2.
set -uo pipefail
[ "${GITHUB_ACTIONS:-}" = "true" ] || { echo "installer/mac/check.sh runs only in CI: it starts the app"; exit 2; }

zip=${1:?"the .zip"}
top=$(cd "$(dirname "$0")/../.." && pwd)
name=$(sed -n 's/^name = "\(.*\)"/\1/p' "$top/product.toml" | head -1)
work=$(mktemp -d "${TMPDIR:-/tmp}/mac-check.XXXXXX")
failed=0

check() { # check <what> <command...>
    local what=$1
    shift
    if "$@" > "$work/last.txt" 2>&1; then
        echo "ok:   $what"
    else
        echo "FAIL: $what"
        sed 's/^/      /' "$work/last.txt"
        failed=$((failed + 1))
    fi
}

ditto -x -k "$zip" "$work/unzipped"
app="$work/unzipped/$name.app"
clap="$work/unzipped/$name.clap"
program="$app/Contents/MacOS/music-organiser"
plugin="$clap/Contents/MacOS/$name"

both_archs() { local a; a=$(lipo -archs "$1") && [[ " $a " == *" arm64 "* ]] && [[ " $a " == *" x86_64 "* ]]; }
check "the app's Info.plist reads" plutil -lint "$app/Contents/Info.plist"
check "the plugin's Info.plist reads" plutil -lint "$clap/Contents/Info.plist"
check "the app runs on Apple silicon and Intel" both_archs "$program"
check "the plugin runs on Apple silicon and Intel" both_archs "$plugin"
check "the app's signature checks" codesign --verify --deep --strict "$app"
check "the plugin's signature checks" codesign --verify --deep --strict "$clap"
check "the app has its icon" test -s "$app/Contents/Resources/logo.icns"
offers_entry() { nm -gU -arch arm64 "$plugin" | grep -q ' _clap_entry$' && nm -gU -arch x86_64 "$plugin" | grep -q ' _clap_entry$'; }
check "the plugin offers clap_entry" offers_entry
check "the Read me is there" test -s "$work/unzipped/Read me.txt"

# The app, started as a tester's first start would be.
home="$work/home"
mkdir -p "$home"
logs="$home/Library/Application Support/MusicOrganiser/logs"
HOME="$home" "$program" > "$work/app-output.txt" 2>&1 &
pid=$!
sleep 60
still_running() { kill -0 "$pid"; }
check "the app is still running after a minute" still_running
has_log() { ls "$logs"/rust-session-*.log > /dev/null 2>&1; }
check "the app wrote its log" has_log
no_crash() { ! ls "$logs"/crash-*.log > /dev/null 2>&1; }
check "no crash log" no_crash

HOME="$home" "$program" --close-for-setup > "$work/close-output.txt" 2>&1
ended() {
    local i
    for i in $(seq 1 60); do
        kill -0 "$pid" 2> /dev/null || return 0
        sleep 1
    done
    return 1
}
check "the app closes when asked" ended
closed_line() { grep -q "\[App\] Closed" "$logs"/rust-session-*.log; }
check "its log says how it closed" closed_line
check "still no crash log" no_crash
kill -9 "$pid" 2> /dev/null || true

echo
echo "--- the app's log ---"
cat "$logs"/rust-session-*.log 2> /dev/null | tail -n 200
for f in "$logs"/crash-*.log; do
    [ -f "$f" ] || continue
    echo "--- $(basename "$f") ---"
    cat "$f"
done
echo "--- what the app printed ---"
tail -n 50 "$work/app-output.txt"
tail -n 20 "$work/close-output.txt"

if [ "$failed" -gt 0 ]; then
    echo "$failed check(s) failed"
    exit 1
fi
echo "every check passed"
