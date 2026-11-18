#!/bin/sh
# Installs Jaynes on a Mac with Apple silicon:
#
#   curl -fsSL https://jaynes.dev/install.sh | sh
#
# The app into /Applications/Jaynes.app (~/Applications when that is not yours to write), the program into ~/.local/bin/jaynes, herdr-core (the terminals the app keeps
# alive) with Herdr's own installer when there is none yet, and the plugin into Claude Code. Running it again updates
# them; `jaynes update` does that for you.
#
# The app is signed ad hoc, not by an Apple developer account: macOS opens it because it came through curl, and asks
# you once after each update before the app reads its key in your keychain. The source is open: you can read it and
# build it yourself, https://github.com/alk0x1/jaynes.
set -eu

releases=${JAYNES_RELEASES:-https://github.com/alk0x1/jaynes/releases/latest/download}
bin="$HOME/.local/bin"
# /Applications, where Finder and Launchpad look, is open to an admin without sudo; ~/Applications otherwise
apps=/Applications
[ -w "$apps" ] || apps="$HOME/Applications"

say() { printf '%s\n' "$*"; }
fail() { say "jaynes: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || fail "needs $1"; }
need curl
need tar

source_build="cargo install --locked --git https://github.com/alk0x1/jaynes jaynes-shell --root ~/.local"
[ "$(uname -s)" = Darwin ] || fail "the app is for macOS; the program alone builds from source with: $source_build"
arch=$(uname -m)
# Rosetta reports x86_64 on Apple silicon
if [ "$arch" = arm64 ] || [ "$(sysctl -n hw.optional.arm64 2>/dev/null || true)" = 1 ]; then
  target=macos-aarch64
else
  fail "there is no build for Intel Macs; the program builds from source with: $source_build"
fi

sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }

# --- installed note: begin
# What this installer put there that may be someone else's too: herdr-core, with the fingerprint it had, so that
# `jaynes uninstall` removes it only while it is the one Jaynes installed (Herdr may update it, or the person may
# install their own). One note a file, in the engine's folder beside the record: installed/herdr-core.json.
note_herdr() {
  case "$1" in *"
"*) say "herdr-core's path has a line break; no note is kept, so jaynes uninstall will leave it"; return 0 ;; esac
  notes=${JAYNES_ENGINE_HOME:-$HOME/.jaynes/engine}/installed
  mkdir -p "$notes"
  path=$(printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g')
  printf '{"path": "%s", "sha256": "%s"}\n' "$path" "$(sha256 "$1")" > "$notes/herdr-core.json.new"
  mv -f "$notes/herdr-core.json.new" "$notes/herdr-core.json"
}
# --- installed note: end

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

curl -fsSL "$releases/latest.json" -o "$tmp/latest.json" || fail "cannot reach $releases"
# read without jq: the manifest is flat enough for sed once spaces are gone
flat=$(tr -d '\n ' < "$tmp/latest.json")
entry=$(printf '%s' "$flat" | sed -n "s/.*\"$target\":{\([^}]*\)}.*/\1/p")
url=$(printf '%s' "$entry" | sed -n 's/.*"url":"\([^"]*\)".*/\1/p')
want=$(printf '%s' "$entry" | sed -n 's/.*"sha256":"\([^"]*\)".*/\1/p')
version=$(printf '%s' "$flat" | sed -n 's/^{"version":"\([^"]*\)".*/\1/p')
[ -n "$url" ] && [ -n "$want" ] || fail "no build for $target yet"

say "Installing Jaynes ${version} for $target..."
curl -fsSL "$url" -o "$tmp/jaynes.tar.gz" || fail "download failed"
[ "$(sha256 "$tmp/jaynes.tar.gz")" = "$want" ] || fail "the download is damaged (checksum mismatch); try again"
tar -xzf "$tmp/jaynes.tar.gz" -C "$tmp"
[ -x "$tmp/jaynes" ] && [ -d "$tmp/Jaynes.app" ] || fail "the release is missing the program or the app"

mkdir -p "$bin" "$apps"
# moved in whole, so a run or the app that uses the old program never sees half of the new one
mv -f "$tmp/jaynes" "$bin/jaynes.new"
mv -f "$bin/jaynes.new" "$bin/jaynes"
xattr -dr com.apple.quarantine "$tmp/Jaynes.app" 2>/dev/null || true
rm -rf "$apps/Jaynes.app"
mv "$tmp/Jaynes.app" "$apps/Jaynes.app"
# a copy an earlier install left in ~/Applications goes, so there is one Jaynes to open
[ "$apps" = "$HOME/Applications" ] || rm -rf "$HOME/Applications/Jaynes.app"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$apps/Jaynes.app" 2>/dev/null || true

# herdr-core: installed only when there is none; one already there (yours, or Herdr's) is left alone
if [ ! -e "$bin/herdr-core" ] && ! command -v herdr-core >/dev/null 2>&1; then
  if curl -fsSL https://releases.herdr.dev/core/install.sh | HC_INSTALL_DIR="$bin" sh && [ -f "$bin/herdr-core" ]; then
    note_herdr "$bin/herdr-core"
  else
    say "herdr-core did not install; install it from https://github.com/herdrdev/herdr-core before opening the app."
  fi
fi

# the plugin, through Claude Code's own commands
if [ -z "${JAYNES_NO_PLUGIN:-}" ] && command -v claude >/dev/null 2>&1; then
  claude plugin marketplace add alk0x1/jaynes >/dev/null 2>&1 || true
  if claude plugin install jaynes@jaynes >/dev/null 2>&1; then
    say "The Jaynes plugin is in Claude Code."
  else
    say "The plugin did not install; in Claude Code, type: /plugin install jaynes --marketplace alk0x1/jaynes"
  fi
elif [ -z "${JAYNES_NO_PLUGIN:-}" ]; then
  say "Claude Code is not on your PATH; once it is, type in it: /plugin install jaynes --marketplace alk0x1/jaynes"
fi

say "Jaynes ${version} is installed in $apps. Open it from your apps."
case ":$PATH:" in
  *":$bin:"*) ;;
  *) say "Add $bin to your PATH to use jaynes from a terminal." ;;
esac
