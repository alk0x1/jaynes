#!/bin/sh
# Puts in OUT what every copy of Jaynes carries: LICENSE (Jaynes's own), NOTICE.md (the code copied into the app,
# and its fonts) with each license file it names at the path it names under app/, and THIRD-PARTY-NOTICES.md (the
# crates, by third-party.sh) when given. bundle.sh puts it in the app, release.yml beside the program.
#
#   licenses/gather.sh OUT [THIRD-PARTY-NOTICES.md]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
out=${1:?where to put the licenses}
mkdir -p "$out/app"
cp "$root/LICENSE" "$out/LICENSE"
cp "$root/app/NOTICE.md" "$out/NOTICE.md"
for f in $(sh "$here/named.sh"); do
  [ -s "$root/app/$f" ] || { echo "gather.sh: NOTICE.md names app/$f, which is not there" >&2; exit 1; }
  mkdir -p "$out/app/$(dirname "$f")"
  cp "$root/app/$f" "$out/app/$f"
done
[ -z "${2:-}" ] || cp "$2" "$out/THIRD-PARTY-NOTICES.md"
