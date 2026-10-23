"""The box that project commands run in: what agent-changed code may write, read, reach and touch."""

from __future__ import annotations

import http.server
import os
import signal
import subprocess
import sys
import threading
from pathlib import Path

import pytest

from jaynes import sandbox
from jaynes.sandbox import Box, Unavailable

boxed = pytest.mark.skipif(not sandbox.available(), reason="no box on this system")


def sh(script: str, box: Box) -> subprocess.CompletedProcess[str]:
    return subprocess.run(sandbox.command(script, box), capture_output=True, text=True, timeout=30)


@pytest.fixture
def home(tmp_path: Path) -> Path:
    h = tmp_path / "home"
    (h / ".ssh").mkdir(parents=True)
    (h / ".ssh" / "id_ed25519").write_text("secret")
    (h / "notes.txt").write_text("plain")
    return h


@boxed
def test_it_writes_only_inside_its_folders(tmp_path: Path, home: Path) -> None:
    copy = tmp_path / "copy"
    copy.mkdir()
    done = sh(f"echo a > {copy}/a; echo b > {home}/b; echo c > /dev/null", Box((copy,), home=home))
    assert (copy / "a").read_text() == "a\n"
    assert not (home / "b").exists() and "Operation not permitted" in done.stderr


@boxed
def test_it_cannot_read_secrets_or_hidden_folders_but_reads_the_rest(tmp_path: Path, home: Path) -> None:
    hidden = tmp_path / "jaynes-home"
    copy = hidden / "runs" / "r1" / "copy"
    copy.mkdir(parents=True)
    (hidden / "jaynes.sqlite3").write_text("log")
    (copy / "index.html").write_text("page")
    done = sh(f"cat {home}/notes.txt; cat {copy}/index.html; cat {home}/.ssh/id_ed25519; cat {hidden}/jaynes.sqlite3",
              Box((copy,), hidden=(hidden,), home=home))
    assert done.stdout == "plainpage"
    assert done.stderr.count("Operation not permitted") == 2


@boxed
def test_it_reaches_only_the_local_ports_it_is_given(tmp_path: Path) -> None:
    servers = [http.server.ThreadingHTTPServer(("127.0.0.1", 0), http.server.SimpleHTTPRequestHandler) for _ in "ab"]
    for s in servers:
        threading.Thread(target=s.serve_forever, daemon=True).start()
    allowed, other = (int(s.server_address[1]) for s in servers)
    try:
        probe = ("import sys, urllib.request\nfor p in sys.argv[1:]:\n    try:\n"
                 "        urllib.request.urlopen(f'http://127.0.0.1:{p}/', timeout=3); print(p, 'open')\n"
                 "    except OSError: print(p, 'closed')\n")
        (tmp_path / "probe.py").write_text(probe)
        done = sh(f"{sys.executable} -I {tmp_path}/probe.py {allowed} {other}", Box((tmp_path,), ports=(allowed,)))
        assert done.stdout.split("\n")[:2] == [f"{allowed} open", f"{other} closed"], done.stderr
    finally:
        for s in servers:
            s.shutdown()


@boxed
def test_it_cannot_signal_processes_outside_it(tmp_path: Path) -> None:
    outside = subprocess.Popen(["sleep", "30"])
    try:
        sh(f"kill {outside.pid}", Box((tmp_path,)))
        assert outside.poll() is None
    finally:
        outside.send_signal(signal.SIGTERM)
        outside.wait()


@boxed
def test_its_temporary_and_cache_folders_are_inside_it(tmp_path: Path) -> None:
    env = sandbox.environment(tmp_path / "tmp")
    assert all(env[k].startswith(str(tmp_path / "tmp")) for k in ("TMPDIR", "npm_config_cache", "XDG_CACHE_HOME"))


def test_without_a_box_project_commands_refuse_to_run(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    monkeypatch.setattr(sandbox, "available", lambda: False)
    monkeypatch.delenv("JAYNES_UNBOXED", raising=False)
    with pytest.raises(Unavailable, match="JAYNES_UNBOXED"):
        sandbox.command("true", Box((tmp_path,)))
    monkeypatch.setenv("JAYNES_UNBOXED", "1")
    assert sandbox.command("true", Box((tmp_path,))) == ["/bin/sh", "-c", "true"]


def test_installed_dependencies_are_linked_entry_by_entry_so_caches_stay_in_the_copy(tmp_path: Path) -> None:
    from jaynes.preview import link_dependencies
    src, dest = tmp_path / "src", tmp_path / "dest"
    (src / "node_modules" / "vite").mkdir(parents=True)
    (src / "node_modules" / ".vite").mkdir()
    (src / "node_modules" / ".vite-temp").mkdir()
    dest.mkdir()
    link_dependencies(src, dest)
    nm = dest / "node_modules"
    assert nm.is_dir() and not nm.is_symlink()
    assert (nm / "vite").is_symlink() and os.readlink(nm / "vite") == str(src / "node_modules" / "vite")
    assert not (nm / ".vite").exists() and not (nm / ".vite-temp").exists()  # the person's caches are not shared
