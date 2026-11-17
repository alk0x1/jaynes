#!/bin/sh
# Makes the Jaynes app's icon from its source, AppIcon.icon (an Icon Composer file: the giraffe whose neck is the J,
# one layer for a light Mac and one for a dark one, the tile's colours in its icon.json). It writes Assets.car, from
# which macOS 26 shows the light or the dark icon as the person's appearance is set, AppIcon.icns for older systems,
# and mark.png, the top bar's mark, from mark.svg.
#
#   ./make-icon.sh
#
# It needs Xcode (its actool compiles the icon) and Google Chrome (to draw mark.svg); bundle.sh only copies what
# this leaves here, so building the app needs neither.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
chrome=${CHROME:-"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

DEVELOPER_DIR=${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer} xcrun actool "$here/AppIcon.icon" --compile "$work" \
  --platform macosx --minimum-deployment-target 13.0 --app-icon AppIcon --output-partial-info-plist "$work/partial.plist" \
  --errors --output-format human-readable-text
cp "$work/Assets.car" "$work/AppIcon.icns" "$here/"

# the mark, at four times the 22 px it is shown at, on a transparent background
printf '<html><body style="margin:0;background:transparent"><img src="file://%s" width="67" height="88"></body></html>' "$here/mark.svg" > "$work/mark.html"
"$chrome" --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=1 --default-background-color=00000000 \
  --window-size=67,88 --screenshot="$here/mark.png" "file://$work/mark.html" 2>/dev/null
echo "made Assets.car, AppIcon.icns and mark.png in $here"
