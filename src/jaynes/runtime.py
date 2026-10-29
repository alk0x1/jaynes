"""The seam between the conductor and an agent: one bounded turn in a workspace.

A runtime runs a single agent turn with ``workspace`` as its working directory
and ``scratch`` as its temporary directory, and returns what its own meter read.
It never decides whether the turn was good; the conductor does that from the
files the turn left behind.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Protocol


@dataclass(frozen=True)
class Ran:
    """A shell command the agent ran in its box, as the runtime saw it: whether it exited 0, and its last output."""
    command: str
    ok: bool
    output: str


@dataclass(frozen=True)
class TurnResult:
    tokens_in: int  # from the runtime's meter, never from the agent's report
    tokens_out: int
    model: str  # the model that answered, as the runtime reports it
    web: tuple[str, ...] = ()  # addresses and searches the turn read on the web, as the runtime saw them
    commands: tuple[Ran, ...] = ()  # shell commands the turn ran, in order


class QuotaExhausted(Exception):
    """The plan's usage window is used up; ``until`` is the reset time (epoch seconds) if known."""

    def __init__(self, until: float | None) -> None:
        super().__init__(f"quota exhausted until {until}")
        self.until = until


class TurnTimeout(Exception):
    pass


class RuntimeFailure(Exception):
    """The turn did not complete for a reason other than quota or deadline."""


class Runtime(Protocol):
    name: str

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float) -> TurnResult:
        """Run one turn; ``deadline`` is in seconds. Raises the exceptions above.

        A runtime that can let a turn read the web also takes ``web: bool = False``; the conductor passes
        ``web=True`` only for steps marked ``web``, and refuses those steps on runtimes without it."""
        ...
