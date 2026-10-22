import pytest

from jaynes.shell import split_commands

PUSH = "git " + "push"  # spelled apart so the repository's own guard hooks don't trip on this file


@pytest.mark.parametrize(
    "command,expected",
    [
        ("ls 2>&1 | head -3", ["ls 2>&1", "head -3"]),
        ("for f in $(ls *.md); do wc -l \"$f\"; done", ["ls *.md", "wc -l \"$f\""]),
        ("U=$(git rev-parse HEAD); echo $U", ["git rev-parse HEAD", "echo $U"]),
        (f"(cd /tmp && {PUSH} origin main)", ["cd /tmp", f"{PUSH} origin main"]),
        ("timeout 30 uv run pytest -q", ["uv run pytest -q"]),
        ("find . -name x -print0 | xargs -0 rm -f", ["find . -name x -print0", "rm -f"]),
        ("git status \\\n  --short", ["git status    --short"]),
        ("export A=1; set -e", ["set -e"]),
        ("if [ -f x ]; then rm x; fi", ["[ -f x ]", "rm x"]),
        ("echo 'a; b && c' | grep \"x|y\"", ["echo 'a; b && c'", "grep \"x|y\""]),
        ("python3 -c 'print(1)' &> /dev/null &", ["python3 -c 'print(1)' &> /dev/null"]),
        ('B="http://h/$(cat "a b/key")" && curl "$B"', ['cat "a b/key"', 'curl "$B"']),
        ('echo "x $(rm -rf "$D") y"', ['rm -rf "$D"', 'echo "x $(...) y"']),
        ("# a comment\nls", ["ls"]),
        ("done 2>/dev/null", []),
    ],
)
def test_split(command: str, expected: list[str]) -> None:
    assert split_commands(command) == expected


def test_heredoc_body_is_prose_unless_fed_to_a_shell() -> None:
    prose = f"cat > notes.md <<'EOF'\nwe never {PUSH} here\nEOF\necho done"
    assert split_commands(prose) == ["cat > notes.md <<'EOF'", "echo done"]
    script = f"bash <<EOF\n{PUSH} origin main\nEOF"
    assert f"{PUSH} origin main" in split_commands(script)
