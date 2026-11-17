#!/bin/sh
# Builds Jaynes Input into ~/.local/share/jaynes-input: a fixed place, since macOS keeps the Accessibility permission
# for this one build (an ad hoc signature is the build's own); a rebuild asks for it again.
set -e
here=$(cd "$(dirname "$0")" && pwd)
app="$HOME/.local/share/jaynes-input/Jaynes Input.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
swiftc -O "$here/main.swift" -o "$app/Contents/MacOS/JaynesInput"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>dev.jaynes.input</string>
<key>CFBundleName</key><string>Jaynes Input</string>
<key>CFBundleExecutable</key><string>JaynesInput</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>LSUIElement</key><true/>
</dict></plist>
PLIST
codesign --force -s - "$app"
echo "$app"
