#!/bin/sh
# Builds the Jaynes app as a macOS app bundle, Jaynes.app, in the folder given (default: dist/). Only a bundle gets the
# system's notifications, a place in the Dock and Finder, and a name of its own in the person's keychain.
#
#   ZIG=/opt/homebrew/opt/zig@0.16/bin/zig ./bundle.sh [folder]
#
# The bundle is signed ad hoc ("-"): macOS asks the person again for the keychain key after each new build. A
# release signs it with a lasting identity, which the keychain then trusts across builds.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out=${1:-$here/dist}
target=${CARGO_TARGET_DIR:-$here/target}
profile=${PROFILE:-debug}
if [ "$profile" = release ]; then cargo build --release --manifest-path "$here/Cargo.toml"; else cargo build --manifest-path "$here/Cargo.toml"; fi
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$here/Cargo.toml" | head -1)
app="$out/Jaynes.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$target/$profile/jaynes-app" "$app/Contents/MacOS/Jaynes"
# the icon, made by assets/make-icon.sh: Assets.car holds its light and dark looks (macOS 26), AppIcon.icns the light
# one for older systems
cp "$here/assets/Assets.car" "$here/assets/AppIcon.icns" "$app/Contents/Resources/"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>dev.jaynes.app</string>
  <key>CFBundleName</key><string>Jaynes</string>
  <key>CFBundleDisplayName</key><string>Jaynes</string>
  <key>CFBundleExecutable</key><string>Jaynes</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundleIconName</key><string>AppIcon</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSFaceIDUsageDescription</key><string>Jaynes asks for your touch before an answer that leads to something that cannot be undone.</string>
</dict>
</plist>
PLIST
codesign --force --sign - "$app"
echo "$app"
