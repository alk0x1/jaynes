#!/bin/sh
# Writes THIRD-PARTY-NOTICES.md: every crate of others that the program and the app are built from, by their
# Cargo.lock files, with the text of each license, for a release to carry beside LICENSE and app/NOTICE.md (the code
# copied into the app). Apache-2.0 4(a) and MIT ask a copy of the license to go with each copy of the work.
#
#   CARGO_ABOUT=path/to/cargo-about licenses/third-party.sh OUT
#
# Needs cargo-about 0.9.3: cargo install --locked cargo-about --version 0.9.3 --features cli
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
out=${1:?where to write the notices}
about=${CARGO_ABOUT:-cargo-about}
"$about" --version | grep -qx 'cargo-about 0.9.3' || { echo "third-party.sh: cargo-about 0.9.3 is needed, not $("$about" --version)" >&2; exit 1; }
part() {
  printf '\n## %s (%s)\n' "$1" "$2"
  "$about" generate --locked --fail -c "$here/about.toml" --manifest-path "$root/$2" "$here/about.hbs"
}
{
  printf '# Third-party notices\n\nThe Jaynes program and app are Apache-2.0 (LICENSE). They are built from the crates below, each under its\nlicense, whose text follows its list. Code copied into the app, and its fonts, are in NOTICE.md.\n'
  part "The program, jaynes" engine/Cargo.toml
  part "The app, Jaynes.app" app/Cargo.toml
} > "$out.tmp"
mv "$out.tmp" "$out"
