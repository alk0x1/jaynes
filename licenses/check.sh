#!/bin/sh
# Refuses a release whose licenses are not all there: in DIR (the folder beside the program in the release, or
# Jaynes.app/Contents/Resources/Licenses), LICENSE, NOTICE.md and THIRD-PARTY-NOTICES.md, the last naming crates of
# both the program and the app, and every license file NOTICE.md names, at its path under app/.
#
#   licenses/check.sh DIR
set -eu
here=$(cd "$(dirname "$0")" && pwd)
dir=${1:?which folder}
fail() { echo "check.sh: $dir: $1" >&2; exit 1; }
for f in LICENSE NOTICE.md THIRD-PARTY-NOTICES.md; do [ -s "$dir/$f" ] || fail "no $f"; done
grep -q '^## The program, jaynes' "$dir/THIRD-PARTY-NOTICES.md" || fail "THIRD-PARTY-NOTICES.md lists no crate of the program"
grep -q '^## The app, Jaynes.app' "$dir/THIRD-PARTY-NOTICES.md" || fail "THIRD-PARTY-NOTICES.md lists no crate of the app"
for crate in serde_json rusqlite gpui-pre; do grep -q "^- $crate " "$dir/THIRD-PARTY-NOTICES.md" || fail "THIRD-PARTY-NOTICES.md does not list $crate"; done
for f in $(sh "$here/named.sh"); do [ -s "$dir/app/$f" ] || fail "no app/$f, which NOTICE.md names"; done
echo "check.sh: $dir carries every license"
