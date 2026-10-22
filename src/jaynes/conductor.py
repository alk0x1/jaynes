"""The conductor: runs a task through a role, one step at a time (SPEC section 5).

All state lives in the event log. Each pass of the loop folds the run's events
into a ``RunState``, takes exactly one action, records it, and folds again, so a
crash at any point leaves a log the next ``run`` can continue from. A turn that
was in flight when the process died is marked interrupted and rerun in a fresh
workspace; the old workspace stays on disk for inspection.

Layout under ``home``::

    jaynes.sqlite3                     the event log (shared with the action gate)
    objects/<sha256>.json              checked outputs, content addressed
    runs/<run>/<step>/turn-<k>/        workspace and scratch directory of each turn
"""

from __future__ import annotations

import fcntl
import hashlib
import json
import os
import secrets
import shutil
import time
from collections.abc import Callable, Generator, Iterable, Mapping
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Literal, TypeVar, cast

from pydantic import BaseModel, ValidationError

from jaynes import bounds
from jaynes.gate import Window
from jaynes.role import OUTPUT, Checked, Fail, Pass, Role, Step, StepContext, Witnesses
from jaynes.runtime import QuotaExhausted, Runtime, RuntimeFailure, TurnTimeout
from jaynes.store import Event, Store

T = TypeVar("T", bound=BaseModel)

Answer = Literal["accept", "change", "return", "context"]
LABELS: dict[str, str] = {"accept": "correct", "change": "error", "return": "error", "context": "context"}
OUTPUT_BYTES = 1_000_000
UNKNOWN_RESET = 30 * 60  # pause this long when a runtime reports exhaustion without a reset time


@dataclass(frozen=True)
class RunResult:
    status: Literal["finished", "waiting", "blocked", "paused"]
    step: str | None = None
    until: float | None = None
    reasons: tuple[str, ...] = ()


class RunBusy(Exception):
    """Another process holds the run."""


class AnswerRejected(Exception):
    def __init__(self, reasons: Iterable[str]) -> None:
        self.reasons = tuple(reasons)
        super().__init__("; ".join(self.reasons))


# --- projection ----------------------------------------------------------------

@dataclass
class StepState:
    round: int = 1
    attempts: int = 0  # turns rejected in this round
    turns: int = 0  # turns started in all rounds, for workspace numbering
    in_flight: dict[str, Any] | None = None
    checked: dict[str, Any] | None = None
    previous: dict[str, Any] | None = None  # checked output a person returned
    open_stop: tuple[float, dict[str, Any]] | None = None
    resolved: bool = False  # the stop was skipped, accepted or changed
    note: str | None = None
    reasons: tuple[str, ...] = ()


@dataclass
class RunState:
    started: dict[str, Any] | None = None
    steps: dict[str, StepState] = field(default_factory=dict[str, StepState])
    finished: bool = False
    blocked: dict[str, Any] | None = None
    paused_until: float | None = None

    def step(self, name: str) -> StepState:
        return self.steps.setdefault(name, StepState())


def project(events: Iterable[Event]) -> RunState:
    st = RunState()
    for e in events:
        p = e.payload
        s = st.step(str(p["step"])) if "step" in p else None
        match e.type:
            case "RunStarted":
                st.started = p
            case "StepStarted":
                assert s is not None
                s.turns += 1
                s.in_flight = p
                st.paused_until = None
            case "TurnRejected":
                assert s is not None
                s.in_flight = None
                s.attempts += 1
                s.reasons = tuple(p.get("reasons", ()))
            case "TurnInterrupted":
                assert s is not None
                s.in_flight = None
            case "Checked":
                assert s is not None
                s.in_flight = None
                s.checked = p
            case "StopOpened":
                assert s is not None
                s.open_stop = (e.ts, p)
            case "StopSkipped":
                assert s is not None
                s.resolved = True
            case "StopAnswered":
                assert s is not None
                s.open_stop = None
                if p["answer"] in ("accept", "change"):
                    s.resolved = True
                else:
                    s.round += 1
                    s.attempts = 0
                    s.previous, s.checked = s.checked, None
                    s.note = p.get("note")
                    s.reasons = ()
            case "RunPaused":
                st.paused_until = float(p["until"])
            case "RunBlocked":
                st.blocked = p
            case "RunFinished":
                st.finished = True
            case _:
                pass
    return st


# --- workspace -----------------------------------------------------------------

def manifest(root: Path) -> dict[str, str]:
    """Every entry under ``root`` with its content hash; symlinks and special files are marked, not followed."""
    out: dict[str, str] = {}
    for d, dirs, files in os.walk(root, followlinks=False):
        for name in dirs + files:
            p = Path(d) / name
            rel = p.relative_to(root).as_posix()
            if p.is_symlink():
                out[rel] = "symlink:" + os.readlink(p)
            elif p.is_dir():
                out[rel] = "dir"
            elif p.is_file():
                out[rel] = hashlib.sha256(p.read_bytes()).hexdigest()
            else:
                out[rel] = "special"
    return out


def manifest_hash(m: Mapping[str, str]) -> str:
    return hashlib.sha256(json.dumps(sorted(m.items())).encode()).hexdigest()[:16]


def canonical(value: BaseModel) -> bytes:
    return json.dumps(value.model_dump(mode="json"), sort_keys=True, separators=(",", ":"),
                      ensure_ascii=False).encode()


def coin(run_id: str, step: str) -> float:
    """Deterministic uniform draw per run and step, so a replay reproduces every decision."""
    h = hashlib.sha256(f"{run_id}:{step}".encode()).digest()
    return int.from_bytes(h[:8], "big") / 2**64


def new_run_id() -> str:
    return "r" + time.strftime("%Y%m%d%H%M%S") + secrets.token_hex(3)


# --- conductor -----------------------------------------------------------------

class Conductor:
    def __init__(self, home: Path, runtimes: Mapping[str, Callable[[], Runtime]] | None = None,
                 clock: Callable[[], float] = time.time) -> None:
        self.home = Path(home)
        self.home.mkdir(parents=True, exist_ok=True)
        self.store = Store(self.home / "jaynes.sqlite3")
        self.runtimes = dict(runtimes or {})
        self.clock = clock

    # public ---------------------------------------------------------------------

    def run(self, role: Role, task: str = "", run_id: str | None = None, *,
            started: Mapping[str, Any] | None = None) -> RunResult:
        """Start a run, or continue it from its log, until it finishes, waits, blocks or pauses."""
        run_id = run_id or new_run_id()
        with self._lock(run_id):
            first = next(iter(self.store.events("RunStarted", run_id=run_id)), None)
            if first is None:
                self._emit(run_id, "RunStarted", {**(started or {}), "role": role.name, "task": task,
                                                  "fingerprint": role.fingerprint()})
            elif first.payload["role"] != role.name:
                raise ValueError(f"run {run_id} belongs to role {first.payload['role']!r}")
            while True:
                result = self._advance(role, run_id, project(self.store.events(run_id=run_id)))
                if result is not None:
                    return result

    def answer(self, role: Role, run_id: str, step: str, answer: Answer, *, note: str | None = None,
               output: BaseModel | Mapping[str, Any] | None = None, seconds_spent: float | None = None) -> None:
        """Record the person's answer at an open stop. ``change`` needs the edited output, which must pass the checks."""
        if answer not in LABELS:
            raise ValueError(f"unknown answer {answer!r}")
        with self._lock(run_id):
            st = project(self.store.events(run_id=run_id))
            s = st.step(step)
            if s.open_stop is None:
                raise AnswerRejected([f"no stop is waiting at {step!r} in run {run_id}"])
            opened_at, opened = s.open_stop
            target = next(x for x in role.steps if x.name == step)
            value: BaseModel | None = None
            if answer == "change":
                if output is None or target.output is None:
                    raise AnswerRejected(["a change needs the edited output"])
                try:
                    value = target.output.model_validate(output.model_dump() if isinstance(output, BaseModel) else output)
                except ValidationError as err:
                    raise AnswerRejected(_schema_reasons(err)) from None
                failures = self._check(target, value, self._witnesses(target, st))
                if failures:
                    raise AnswerRejected(failures)
            rows: list[tuple[str, dict[str, Any]]] = [("StopAnswered", {
                "step": step, "answer": answer, "label": LABELS[answer], "note": note,
                "fingerprint": opened["fingerprint"], "seconds_waited": round(self.clock() - opened_at, 3),
                "seconds_spent": seconds_spent})]
            if value is not None:  # the edit and its witness are one write, so a crash cannot drop the edit
                assert s.checked is not None
                rows.append(self._witness(target, s, value, source="person", model=str(s.checked["model"])))
            self.store.append_all([(kind, p, run_id, self.clock()) for kind, p in rows])

    def output(self, run_id: str, step: Step[T]) -> T | None:
        """The latest checked output of ``step`` in ``run_id``."""
        checked = project(self.store.events(run_id=run_id)).step(step.name).checked
        if checked is None or step.output is None:
            return None
        return step.output.model_validate_json(self._object(str(checked["output_hash"])))

    def window(self, fingerprint: str) -> Window:
        """Labels recorded under a stop fingerprint, across all runs."""
        n = k = 0
        for e in self.store.events("StopAnswered"):
            if e.payload.get("fingerprint") != fingerprint or e.payload["label"] == "context":
                continue
            n += 1
            k += e.payload["label"] == "error"
        return Window(n, k)

    # one action per call ------------------------------------------------------

    def _advance(self, role: Role, run_id: str, st: RunState) -> RunResult | None:
        if st.finished:
            return RunResult("finished")
        if st.blocked is not None:
            return RunResult("blocked", st.blocked["step"], reasons=tuple(st.blocked["reasons"]))
        for step in role.steps:
            s = st.step(step.name)
            if s.in_flight is not None:
                self._emit(run_id, "TurnInterrupted", {"step": step.name, "attempt": s.in_flight["attempt"],
                                                       "reason": "crash"})
                return None
        for step in role.steps:
            s = st.step(step.name)
            if s.checked is not None and (step.stop is None or s.resolved):
                continue
            if s.open_stop is not None:
                return RunResult("waiting", step.name)
            if s.checked is not None:
                self._decide_stop(run_id, step, s)
                return None
            if s.attempts >= step.max_turns:
                self._emit(run_id, "RunBlocked", {"step": step.name, "reasons": list(s.reasons)})
                return None
            return self._execute(role, run_id, step, s, st)
        self._emit(run_id, "RunFinished", {"outcome": "delivered"})
        return None

    def _execute(self, role: Role, run_id: str, step: Step[Any], s: StepState, st: RunState) -> RunResult | None:
        needs = self._witnesses(step, st)
        if step.output is None:
            self._emit(run_id, "Checked", {"step": step.name, "round": s.round, "attempt": 0, "output_hash": None,
                                           "check_hash": step.check_hash(), "source": "none", "model": "none"})
            return None
        if step.run_by is None:
            until = self._paused_until(step.agent)
            if until is not None:
                if st.paused_until != until:
                    self._emit(run_id, "RunPaused", {"reason": "quota", "agent": step.agent, "until": until})
                return RunResult("paused", step.name, until)
        attempt = s.attempts + 1
        ws, scratch = self._workspace(run_id, step, s, needs)
        before = manifest(ws)
        self._emit(run_id, "StepStarted", {"step": step.name, "round": s.round, "attempt": attempt,
                                           "executor": step.executor, "manifest": manifest_hash(before)})
        task = str(st.started["task"]) if st.started else ""
        t0 = time.monotonic()
        if step.run_by is not None:
            try:
                value = step.run_by(StepContext(run_id, task, needs, ws))
            except Exception as err:  # a trusted executor failed; it counts as a rejected turn
                self._reject(run_id, step, attempt, "executor", [f"{type(err).__name__}: {err}"[:500]], t0)
                return None
            if not isinstance(value, step.output):
                self._reject(run_id, step, attempt, "schema", [f"executor returned {type(value).__name__}"], t0)
                return None
            model = "jaynes"
        else:
            runtime = self._runtime(step.agent)
            try:
                turn = runtime.turn(ws, scratch, self.prompt(role, step, task, s), step.deadline)
            except QuotaExhausted as q:
                until = q.until if q.until and q.until > self.clock() else self.clock() + UNKNOWN_RESET
                self._emit(run_id, "TurnInterrupted", {"step": step.name, "attempt": attempt, "reason": "quota"})
                self._emit(run_id, "RunPaused", {"reason": "quota", "agent": step.agent, "until": until})
                return RunResult("paused", step.name, until)
            except TurnTimeout:
                self._reject(run_id, step, attempt, "deadline", [f"the turn passed its {step.deadline:.0f} s deadline"], t0)
                return None
            except RuntimeFailure as err:
                self._reject(run_id, step, attempt, "runtime", [str(err)[:500]], t0)
                return None
            after = manifest(ws)
            changed = sorted(p for p in before.keys() | after.keys() if p != OUTPUT and before.get(p) != after.get(p))
            self._emit(run_id, "TurnFinished", {"step": step.name, "attempt": attempt, "files_changed": changed,
                                                "tokens_in": turn.tokens_in, "tokens_out": turn.tokens_out,
                                                "model": turn.model, "seconds": round(time.monotonic() - t0, 3)})
            if changed:
                self._reject(run_id, step, attempt, "write_boundary",
                             [f"only {OUTPUT} may be written; the turn changed: " + ", ".join(changed[:10])], t0)
                return None
            loaded = self._load(step, ws / OUTPUT)
            if isinstance(loaded, list):
                self._reject(run_id, step, attempt, "schema", loaded, t0)
                return None
            value, model = loaded, turn.model
        failures = self._check(step, value, needs)
        if failures:
            self._reject(run_id, step, attempt, "check", failures, t0)
            return None
        self._accept(run_id, step, s, value, source=step.executor, model=model, attempt=attempt)
        return None

    def _decide_stop(self, run_id: str, step: Step[Any], s: StepState) -> None:
        assert step.stop is not None and s.checked is not None
        fp = step.stop_fingerprint(str(s.checked["model"]))
        w = self.window(fp)
        n, k = w.n, w.k
        sp = step.stop
        if not sp.sampled:
            status = "floor"
        elif bounds.certified(n, k, sp.budget, sp.delta):
            status = "certified"
        elif bounds.refuted(n, k, sp.budget, sp.delta):
            status = "refuted"
        else:
            status = "undetermined"
        u = coin(run_id, step.name)
        if status == "certified" and u >= sp.audit_rate:
            self._emit(run_id, "StopSkipped", {"step": step.name, "status": status, "coin": u, "fingerprint": fp})
            return
        self._emit(run_id, "StopOpened", {
            "step": step.name, "round": s.round, "status": status, "coin": u, "n": n, "k": k,
            "log_e": bounds.log_certify_evalue(n, k, sp.budget) if n else 0.0, "fingerprint": fp})

    # helpers ------------------------------------------------------------------

    def prompt(self, role: Role, step: Step[Any], task: str, s: StepState) -> str:
        lines: list[str] = []
        if method := role.method_text():
            lines += [method.rstrip(), ""]
        lines += [f"# Step: {step.name}", ""]
        if step.instructions:
            lines += [step.instructions.rstrip(), ""]
        lines += [f"Task: {task}", ""]
        if step.needs or s.previous:
            lines.append("Inputs, read-only:")
            lines += [f"- inputs/{n.name}.json: the checked output of step {n.name}" for n in step.needs]
            if s.previous:
                lines.append("- inputs/previous.json: your previous output for this step, which the person returned")
            lines.append("")
        lines += [f"Write exactly one file, {OUTPUT}, in the current directory: one JSON document valid against "
                  "this JSON Schema:", "", json.dumps(step.schema(), indent=1), "",
                  "Do not create, change or delete any other file. Nobody can answer questions during this turn."]
        if s.note:
            lines += ["", "The person returned the previous output with this note, which outweighs your own "
                      "reading of the inputs:", s.note]
        if s.reasons:
            lines += ["", "Your previous attempt was rejected. Fix exactly this:"]
            lines += [f"- {r}" for r in s.reasons[:30]]
        return "\n".join(lines) + "\n"

    def _workspace(self, run_id: str, step: Step[Any], s: StepState, needs: Witnesses) -> tuple[Path, Path]:
        root = self.home / "runs" / run_id / step.name / f"turn-{s.turns + 1}"
        if root.exists():  # left by a crash before StepStarted was recorded; no event refers to it
            _make_writable(root)
            shutil.rmtree(root)
        ws, scratch = root / "workspace", root / "scratch"
        (ws / "inputs").mkdir(parents=True, mode=0o700)
        scratch.mkdir(mode=0o700)
        (ws / "step.json").write_text(json.dumps({"step": step.name, "round": s.round,
                                                  "attempt": s.attempts + 1}) + "\n")
        for c in needs.all():
            if c.value is not None:
                (ws / "inputs" / f"{c.step}.json").write_bytes(self._object(c.output_hash))
        if s.previous and s.previous.get("output_hash"):
            (ws / "inputs" / "previous.json").write_bytes(self._object(str(s.previous["output_hash"])))
        for d, _, files in os.walk(ws):
            for f in files:
                os.chmod(Path(d) / f, 0o444)  # a hint only; the manifest comparison is the guard
        return ws, scratch

    def _load(self, step: Step[Any], path: Path) -> BaseModel | list[str]:
        assert step.output is not None
        if path.is_symlink() or not path.is_file():
            return [f"{OUTPUT} was not written as a regular file"]
        if path.stat().st_size > OUTPUT_BYTES:
            return [f"{OUTPUT} is larger than {OUTPUT_BYTES} bytes"]
        try:
            return step.output.model_validate_json(path.read_bytes())
        except ValidationError as err:
            return _schema_reasons(err)

    def _check(self, step: Step[Any], value: Any, needs: Witnesses) -> list[str]:
        failures: list[str] = []
        for check in step.checks:
            try:
                result = cast(object, check(value, needs))  # user code; its annotation is not a guarantee
            except Exception as err:
                failures.append(f"check {getattr(check, '__name__', check)} raised {type(err).__name__}: {err}"[:500])
                continue
            if isinstance(result, Fail):
                failures += result.reasons
            elif not isinstance(result, Pass):
                failures.append(f"check {getattr(check, '__name__', check)} returned {result!r}, not Pass or Fail")
        return failures

    def _accept(self, run_id: str, step: Step[Any], s: StepState, value: BaseModel, *, source: str, model: str,
                attempt: int = 0) -> None:
        self._emit(run_id, *self._witness(step, s, value, source=source, model=model, attempt=attempt))

    def _witness(self, step: Step[Any], s: StepState, value: BaseModel, *, source: str, model: str,
                 attempt: int = 0) -> tuple[str, dict[str, Any]]:
        """Store the output and return its ``Checked`` event, not yet recorded."""
        data = canonical(value)
        h = hashlib.sha256(data).hexdigest()
        path = self.home / "objects" / f"{h}.json"
        if not path.exists():
            path.parent.mkdir(parents=True, exist_ok=True)
            tmp = path.with_suffix(".tmp")
            tmp.write_bytes(data)
            os.replace(tmp, path)
        return "Checked", {"step": step.name, "round": s.round, "attempt": attempt, "output_hash": h,
                           "check_hash": step.check_hash(), "source": source, "model": model}

    def _reject(self, run_id: str, step: Step[Any], attempt: int, reason: str, reasons: list[str], t0: float) -> None:
        self._emit(run_id, "TurnRejected", {"step": step.name, "attempt": attempt, "reason": reason,
                                            "reasons": reasons[:50], "seconds": round(time.monotonic() - t0, 3)})

    def _witnesses(self, step: Step[Any], st: RunState) -> Witnesses:
        by_name: dict[str, Checked[Any]] = {}
        for need in step.needs:
            c = st.step(need.name).checked
            assert c is not None, f"{step.name} started before {need.name} was checked"
            h = c["output_hash"]
            value = None if need.output is None or h is None else need.output.model_validate_json(self._object(h))
            by_name[need.name] = Checked(need.name, value, str(h), str(c["check_hash"]))
        return Witnesses(by_name)

    def _object(self, h: str) -> bytes:
        return (self.home / "objects" / f"{h}.json").read_bytes()

    def _paused_until(self, agent: str) -> float | None:
        until = None
        for e in self.store.events("RunPaused"):
            if e.payload.get("agent") == agent:
                until = float(e.payload["until"])
        return until if until is not None and until > self.clock() else None

    def _runtime(self, agent: str) -> Runtime:
        if agent not in self.runtimes:
            raise RuntimeError(f"no runtime registered for agent {agent!r}")
        return self.runtimes[agent]()

    def _emit(self, run_id: str, kind: str, payload: dict[str, Any]) -> None:
        self.store.append(kind, payload, run_id=run_id, ts=self.clock())

    @contextmanager
    def _lock(self, run_id: str) -> Generator[None]:
        path = self.home / "runs" / f"{run_id}.lock"
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("a") as fh:
            try:
                fcntl.flock(fh, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                raise RunBusy(run_id) from None
            yield


def _schema_reasons(err: ValidationError) -> list[str]:
    return [f"{'.'.join(str(x) for x in e['loc']) or '(root)'}: {e['msg']}" for e in err.errors()[:30]]


def _make_writable(root: Path) -> None:
    for d, dirs, files in os.walk(root):
        for name in dirs + files:
            p = Path(d) / name
            if not p.is_symlink():
                os.chmod(p, 0o700 if p.is_dir() else 0o600)
