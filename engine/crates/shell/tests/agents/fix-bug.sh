#!/bin/sh
# A fake agent for fix-bug: it reads its step from the prompt, keeps every prompt it gets, and its first fix
# is incomplete, so only the hidden case catches it.
prompt=$(cat)
n=$(ls "$PROMPTS" 2>/dev/null | wc -l | tr -d ' ')
printf '%s' "$prompt" > "$PROMPTS/$n.txt"
case "$prompt" in
  *"# Your step: cases"*)
    printf '{"cases":[{"says":"greet says hi with the name","run":"sh greet.sh ana | grep -q \\"hi ana\\""}]}' > .jaynes/output.json ;;
  *"# Your step: reproduce"*)
    mkdir -p tests
    printf 'out=$(sh greet.sh ana)\ncase "$out" in hi*) ;; *) echo "expected hi, got $out"; exit 1;; esac\n' > tests/test_hi.sh
    printf '{"bug":"greet says hello","tests":["tests/test_hi.sh"],"failure":"expected hi, got hello ana","examples":[{"case":"greeting ana","now":"hello ana","after":"hi ana"}]}' > .jaynes/output.json ;;
  *"# Your step: fix"*)
    case "$prompt" in
      *"A check you cannot see"*) printf 'echo "hi $1"\n' > greet.sh ;;
      *) printf 'echo "hi"\n' > greet.sh ;;
    esac
    printf '{"summary":"the greeting says hi","files":["greet.sh"]}' > .jaynes/output.json ;;
esac
echo '{"tokens":100}'
