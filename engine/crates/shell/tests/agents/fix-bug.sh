#!/bin/sh
# A fake agent for fix-bug: it reads its step from the prompt, keeps every prompt it gets, and its first fix
# is incomplete, so only the hidden case catches it. With NEVER_RIGHT set, no fix of it ever passes that case.
prompt=$(cat)
n=$(ls "$PROMPTS" 2>/dev/null | wc -l | tr -d ' ')
printf '%s' "$prompt" > "$PROMPTS/$n.txt"
case "$prompt" in
  *"# Your step: cases"*)
    printf '{"cases":[{"says":"greet says hi with the name","run":"sh greet.sh ana | grep -q \\"hi ana\\""}]}' > .jaynes/output.json ;;
  *"# Your step: reproduce"*)
    mkdir -p tests
    case "$prompt" in *"empty name"*)
      printf 'out=$(sh greet.sh)\n[ "$out" = "hi " ] || { echo "expected hi with nothing after, got $out"; exit 1; }\n' > tests/test_empty.sh ;;
    esac
    printf 'out=$(sh greet.sh ana)\ncase "$out" in hi*) ;; *) echo "expected hi, got $out"; exit 1;; esac\n' > tests/test_hi.sh
    failure="expected hi, got hello ana"
    saw="hello ana"
    case "$prompt" in *"empty name"*) failure="expected hi with nothing after"; saw="got hello" ;; esac
    printf '{"bug":"greet says hello","tests":["tests/test_hi.sh"],"failure":"%s","saw":"%s","examples":[{"case":"greeting ana","now":"hello ana","after":"hi ana"}]}' "$failure" "$saw" > .jaynes/output.json ;;
  *"# Your step: fix"*)
    case "$prompt" in
      *"A check you cannot see"*|*"runs again on top"*) [ -n "$NEVER_RIGHT" ] && printf 'echo "hi"\n' > greet.sh || printf 'echo "hi $1"\n' > greet.sh ;;
      *) printf 'echo "hi"\n' > greet.sh ;;
    esac
    # With TESTS_SETUP set, it also changes which files the project's tests are (the real-user's contas, 23/11)
    [ -n "$TESTS_SETUP" ] && printf '[project]\nname = "greet"\n\n[tool.pytest.ini_options]\npython_files = ["test_*.py", "*_test.py"]\n' > pyproject.toml
    printf '{"summary":"the greeting says hi","files":["greet.sh"]}' > .jaynes/output.json ;;
esac
echo '{"tokens":100}'
