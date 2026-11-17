#!/bin/sh
# run.sh <script>: runs a Jaynes Input script and prints what it wrote (open -W does not always wait, so this does).
out=$(mktemp); rm -f "$out"
open -W -n "$HOME/.local/share/jaynes-input/Jaynes Input.app" --args "$1" "$out" 2>/dev/null
for i in $(seq 1 100); do [ -f "$out" ] && break; sleep 0.1; done
cat "$out" 2>/dev/null; rm -f "$out"
