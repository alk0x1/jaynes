"""References for design roles: the links and pictures a task names, as pictures the agent can look at.

Jaynes runs this step itself. It finds web links and image paths in the task, captures each link with
headless Chrome, copies each picture, and leaves them as files for the steps that need it. Agents have no
network, so these pictures are all they see of a reference. A direction must then say what it takes from each.
"""

from __future__ import annotations

import os
import re
import shutil
import signal
import subprocess
import tempfile
from pathlib import Path

from pydantic import BaseModel, Field

from jaynes.role import Fail, Pass, StepContext

IMAGES = (".png", ".jpg", ".jpeg", ".webp", ".gif")
URL = re.compile(r"https?://[^\s<>\"'`]+")
PATH = re.compile(r"""(?:^|(?<=[\s"'`(]))((?:~|/)(?:\\ |[^\n"'`])*?\.(?:png|jpe?g|webp|gif))(?![\w.])""", re.IGNORECASE)
CHROME = ("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
          "/Applications/Chromium.app/Contents/MacOS/Chromium")
PICTURE_BYTES = 10_000_000
WORDS = 8  # what a direction takes from a reference, at least


class Reference(BaseModel):
    name: str = Field(description="how steps refer to it, e.g. ref-1")
    source: str = Field(description="the link or image path as the task gave it")
    file: str = Field(description="its picture, e.g. ref-1.png")


class References(BaseModel):
    references: list[Reference]


class Taken(BaseModel):
    reference: str = Field(description="the reference's name, e.g. ref-1")
    took: str = Field(description="what the direction takes from it, concretely: colours, type, density, layout, "
                                  "components, motion")


class Unreachable(Exception):
    pass


def named(task: str) -> list[str]:
    """The links and image paths in ``task``, in order, each once."""
    found = [(m.start(), m.group().rstrip(".,;:!?)]")) for m in URL.finditer(task)]
    found += [(m.start(1), m.group(1).replace("\\ ", " ")) for m in PATH.finditer(task)]
    return list(dict.fromkeys(s for _, s in sorted(found)))


def chrome() -> str | None:
    for c in (os.environ.get("JAYNES_CHROME"), *CHROME, shutil.which("google-chrome"), shutil.which("chromium")):
        if c and Path(c).is_file():
            return c
    return None


def capture(url: str, out: Path, timeout: float = 60.0) -> None:
    """Save a picture of the page at ``url``, 1440 pixels wide, with a fresh browser profile."""
    binary = chrome()
    if binary is None:
        raise Unreachable("Jaynes captures links with Google Chrome and found none; install it, set JAYNES_CHROME, "
                          "or give a picture instead of the link")
    with tempfile.TemporaryDirectory(prefix="jaynes-chrome-") as profile:
        proc = subprocess.Popen([binary, "--headless=new", f"--user-data-dir={profile}", "--no-first-run",
                                 "--no-default-browser-check", "--disable-extensions", "--hide-scrollbars",
                                 "--window-size=1440,1600", "--virtual-time-budget=8000", f"--screenshot={out}", url],
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        try:
            proc.wait(timeout)
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGKILL)
            proc.wait()
            raise Unreachable(f"{url} did not load within {timeout:.0f} s") from None
    if not out.is_file() or out.stat().st_size == 0:
        raise Unreachable(f"Chrome made no picture of {url}")


def gather(ctx: StepContext) -> References:
    """Capture or copy every reference the task names; a task with none is sent back to the person."""
    sources = named(ctx.task)
    if not sources:
        raise ValueError("the task names no reference: start a new run whose task has one or more links "
                         "(https://...) or image paths (~/Desktop/shot.png) of interfaces like the one you want")
    folder = ctx.workspace / "files"
    folder.mkdir(exist_ok=True)
    refs: list[Reference] = []
    for i, source in enumerate(sources, 1):
        name = f"ref-{i}"
        if source.startswith(("http://", "https://")):
            file = f"{name}.png"
            capture(source, folder / file)
        else:
            path = Path(source).expanduser()
            if not path.is_file():
                raise Unreachable(f"{source} is not a file")
            if path.stat().st_size > PICTURE_BYTES:
                raise Unreachable(f"{source} is larger than {PICTURE_BYTES // 1_000_000} MB")
            file = name + path.suffix.lower()
            shutil.copyfile(path, folder / file)
        refs.append(Reference(name=name, source=source, file=file))
    return References(references=refs)


def drawn_from(references: list[Taken], given: References) -> Pass | Fail:
    """Every reference is named, with what is taken from it in at least a few words, and no other is."""
    names = [r.name for r in given.references]
    said = {t.reference: t.took for t in references}
    reasons = [f"say what the direction takes from {n} ({next(r.source for r in given.references if r.name == n)})"
               for n in names if n not in said]
    reasons += [f"{t.reference} is not a reference; they are {', '.join(names)}" for t in references
                if t.reference not in names]
    reasons += [f"what is taken from {n} is too vague; name concrete traits in at least {WORDS} words"
                for n, took in said.items() if n in names and len(took.split()) < WORDS]
    return Fail(*reasons) if reasons else Pass()

