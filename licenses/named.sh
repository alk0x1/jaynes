#!/bin/sh
# The license files app/NOTICE.md names in its Licence column, as paths under app/, one a line: the one list
# gather.sh copies and check.sh asks for. A name with * is every file it matches.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root/app"
# the last cell of each row of the table, then each `path` in it that names a license file
sed -n 's/^|.*|\([^|]*\)|[[:space:]]*$/\1/p' NOTICE.md | grep -o '`[^`]*`' | tr -d '`' | grep -i 'licen[cs]e\|ofl' | while read -r name; do
  case "$name" in
    *\**) for f in $name; do echo "$f"; done ;;
    # the GPUI row's "each folder's LICENSE-APACHE", read below
    LICENSE-APACHE) ;;
    *) echo "$name" ;;
  esac
done
# the GPUI row says "each folder's LICENSE-APACHE": its folders, from its first cell
sed -n 's/^| GPUI snapshot: \([^|]*\)|.*/\1/p' NOTICE.md | grep -o '`vendor/[^`]*`' | tr -d '`' | sed 's|$|/LICENSE-APACHE|'
