#!/bin/sh
# THROWAWAY. Both cases inside a Linux, with no person and no display: a virtual screen stands for the real one.
#   docker build -t jaynes-target-spike linux
#   docker run --rm -v "$PWD":/spike:ro jaynes-target-spike sh linux/run.sh
set -e
export SPIKE_RUNS=/tmp/runs
uname -sm; node --version; chromium --version
node case-a.mjs
node case-b.mjs cdp
node case-b.mjs screen pipe
Xvfb :99 -screen 0 800x600x24 -nolisten tcp >/dev/null 2>&1 &
export DISPLAY=:99
sleep 1
node case-b.mjs screen x11
