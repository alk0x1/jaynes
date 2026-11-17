#!/bin/zsh
# open-test.sh <dir>: opens app/dist/Jaynes.app as a test, in front (through LaunchServices, which may bring it there;
# an app cannot put itself in front) and zoomed, never touching the person's key in the Keychain; prints its pid and
# its window's x y. The window draws itself into <dir>/shot.png each second, takes test lines from <dir>/do.txt
# (see drive_from in app/src/island.rs) and logs to <dir>/app.log. Real keys and mouse then go through run.sh, at
# points from the window's x y; `open -b dev.herdr.app` (or the app that was in front) gives the screen back.
dir=$1; app=$(cd "$(dirname "$0")/../.." && pwd)
rm -f $dir/shot.png $dir/app.log; : > $dir/do.txt
before=($(pgrep -f "dist/Jaynes.app/Contents/MacOS/Jaynes"))
open -n --stdout $dir/app.log --stderr $dir/app.log --env JAYNES_APP_TEST_KEY=1 --env JAYNES_APP_SHOT=$dir/shot.png \
  --env JAYNES_APP_TEST_DO=$dir/do.txt --env JAYNES_APP_LOG=info $app/dist/Jaynes.app
sleep 8
pid=""
for p in $(pgrep -f "dist/Jaynes.app/Contents/MacOS/Jaynes"); do (( ! ${before[(Ie)$p]} )) && pid=$p; done
echo zoom > $dir/do.txt; sleep 1.5
print "window $pid" > $dir/in.txt; set -- $($app/tools/input/run.sh $dir/in.txt)
echo "$pid $2 $3"
