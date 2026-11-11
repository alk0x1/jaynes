"""The local API (SPEC section 7): every action of the page, and what the page shows, as data. The page's buttons,
another frontend and the MCP server for agents (``jaynes.mcp``) go through these functions, so an action means
the same wherever it comes from.

Who acts is the person or an agent. The page's server counts a request as the person's when it carries the key
the person's browser holds, and as an agent's otherwise; the MCP server always acts as an agent. An agent may do
what an agent can resolve: answer a stop that holds its run, send a blocked run back with a note, try it again,
start a run, record a correction, halt everything, and add or remove a package of roles when the person asks for
it. Only the person answers a sample, since an agent's answer
never counts as proof, and a floor or a braked stop; only the person drops a run, files a correction as another
kind, undoes a delivery, resumes after a halt and takes a run out of a plan.
"""

from __future__ import annotations

import json
from collections.abc import Callable
from pathlib import Path
from typing import TYPE_CHECKING, Any, cast

from jaynes import inputs as run_inputs
from jaynes import project as repo
from jaynes import catalog, packages, proof, role_file, verify
from jaynes.ask import page
from jaynes.conductor import (LABELS, Answer, AnswerRejected, By, RunBusy, StepState, Waiting,
                              continue_in_background, new_run_id, project, start_in_background)

if TYPE_CHECKING:
    from jaynes.review import Delivery, Review


class Refused(Exception):
    """An action not taken, with the HTTP status that says why: 400 wrong, 403 not for whoever asked, 404 nothing
    there, 409 busy."""

    def __init__(self, status: int, reason: str) -> None:
        super().__init__(reason)
        self.status = status
        self.reason = reason


def _text(f: dict[str, Any], name: str) -> str:
    value = f.get(name)
    return "" if value is None else str(value)


def _person_only(by: By, what: str) -> None:
    if by != "person":
        raise Refused(403, f"only the person {what}, from the page")


# what the page shows ----------------------------------------------------------


def _who(w: Waiting, status: str | None) -> By | None:
    """Who may answer what waits: anyone at a stop that holds its run, the person alone at a sample, a floor or a
    braked stop; nobody while it runs."""
    if w.kind == "stop":
        return "agent" if w.holds and status not in ("floor", "braked") else "person"
    return "agent" if w.kind in ("blocked", "idle") else None


def dispute_choices(step: str, dispute: dict[str, Any]) -> list[tuple[str, str, str]]:
    """When two agents still disagree about what an earlier step made: the two ways to send the run back, as the
    button's label, the step it goes back to and the note for that step's agent."""
    about, to, said = str(dispute["about"]), str(dispute["to"]), str(dispute["said"])
    return [(f"Fix the {about}", to, f"The agent at {step} says this {about} is wrong; fix it: {said}"),
            (f"Keep the {about}", step, f"The {about} is right as it is; make your work pass it.")]


def item(r: Review, w: Waiting) -> dict[str, Any]:
    """Something in the queue, as data."""
    st = project(r.c.store.events(run_id=w.run_id))
    opened = st.step(w.step).open_stop if w.kind == "stop" else None
    status = str(opened[1].get("status")) if opened else None
    base = page(r.c.home)
    out: dict[str, Any] = {
        "run": w.run_id, "kind": w.kind, "step": w.step, "role": w.role, "task": w.task, "since": w.since,
        "holding": w.holding, "sample": w.kind == "stop" and not w.holds, "status": status,
        "question": w.question, "reasons": list(w.reasons), "evidence": w.evidence, "until": w.until,
        "who_answers": {"agent": "anyone", "person": "the person", None: None}[_who(w, status)],
        "page": f"{base}/stop/{w.run_id}/{w.step}" if w.kind == "stop" else f"{base}/run/{w.run_id}"}
    dispute = (st.blocked or {}).get("dispute") if w.kind == "blocked" else None
    if dispute:  # each choice is a send back
        out["dispute"] = {"about": dispute["about"], "said": dispute["said"],
                          "choices": [{"label": label, "to": to, "note": note}
                                      for label, to, note in dispute_choices(w.step, dispute)]}
    return out


def delivery(r: Review, d: Delivery) -> dict[str, Any]:
    """What a run delivered: where it landed, its hidden checks, the samples it left and its corrections."""
    st, role = d.st, r.role_of(d.run_id)
    started = st.started or {}
    out: dict[str, Any] = {"run": d.run_id, "at": d.at, "role": str(started.get("role", "")),
                           "task": str(started.get("task", ""))}
    if st.undone is not None:
        out |= {"landed": "undone", "note": st.undone["note"]}
    elif st.merged:
        out |= {"landed": "merged", "into": st.merged[-1]["into"], "over": bool(st.merged[-1].get("over"))}
    elif st.branch is not None:
        out |= {"landed": "branch", "branch": st.branch["branch"],
                "not_merged": st.not_merged["reason"] if st.not_merged is not None else None}
    else:
        out["landed"] = "delivered"
    proofs = [r.c.output(d.run_id, x) for x in (role.steps if role else [])
              if x.output is not None and issubclass(x.output, proof.Proved)]
    results = [res for p in proofs if p is not None for res in p.results]
    out["hidden_checks"] = {"passed": sum(res.passed for res in results), "of": len(results)} if results else None
    out["samples"] = sum(s.open_stop is not None for s in st.steps.values())
    out["corrections"] = len(r.c.corrections(d.run_id))
    out["page"] = f"{page(r.c.home)}/run/{d.run_id}"
    return out


def home(r: Review, delivered: int = 20) -> dict[str, Any]:
    """The home page's four parts: what holds a run, the samples, the latest deliveries and what runs now."""
    items = r.queue()
    return {"halted": r.c.halted(),
            "waiting": [item(r, w) for w in items if w.holding],
            "samples": [item(r, w) for w in items if w.kind == "stop" and not w.holds],
            "delivered": [delivery(r, d) for d in r.deliveries()[:delivered]],
            "running": [item(r, w) for w in items if w.kind in ("running", "paused")]}


def stop(r: Review, run_id: str, step: str) -> dict[str, Any]:
    """An open stop: its question, the step's checked output it asks about, and who may answer it."""
    w = next((x for x in r.queue() if (x.run_id, x.step) == (run_id, step) and x.kind == "stop"), None)
    if w is None:
        raise Refused(404, f"no stop is waiting at {step!r} in run {run_id}")
    role = r.role_of(run_id)
    target = next((s for s in role.steps if s.name == step), None) if role else None
    value = r.c.output(run_id, target) if target is not None else None
    names = [s.name for s in role.steps] if role else []
    return {**item(r, w), "output": value.model_dump(mode="json") if value is not None else None,
            "answers": sorted(LABELS), "earlier": names[:names.index(step)] if step in names else []}


def run(r: Review, run_id: str) -> dict[str, Any]:
    """A run: what it was asked, where each step is, and what it delivered."""
    st = project(r.c.store.events(run_id=run_id))
    if st.started is None:
        raise Refused(404, f"no run {run_id}")
    role = r.role_of(run_id)
    names = [s.name for s in role.steps] if role else list(st.steps)
    waiting = [item(r, w) for w in r.queue() if w.run_id == run_id]
    d = next((d for d in r.deliveries() if d.run_id == run_id), None)
    landed = {k: v for k, v in delivery(r, d).items() if k not in ("run", "role", "task")} if d else None
    return {"run": run_id, "role": str(st.started.get("role", "")), "task": str(st.started.get("task", "")),
            "finished": st.finished, "outcome": st.outcome,
            "blocked": list(st.blocked.get("reasons", [])) if st.blocked else None,
            "steps": [_step(st.step(n), n) for n in names],
            "waiting": waiting, "delivered": landed, "page": f"{page(r.c.home)}/run/{run_id}"}


def _step(s: StepState, name: str) -> dict[str, Any]:
    opened = s.open_stop
    return {"name": name, "checked": s.checked is not None, "stop": str(opened[1].get("status")) if opened else None}


# actions ------------------------------------------------------------------------


def answer(r: Review, f: dict[str, Any], by: By, spent: float | None = None) -> dict[str, Any]:
    """Answer an open stop. ``spent`` is how long the person looked at it, which only the page knows."""
    run_id, step, raw = _text(f, "run"), _text(f, "step"), _text(f, "answer")
    note = _text(f, "note").strip() or None
    if raw not in LABELS:
        raise Refused(400, f"unknown answer {raw!r}")
    a = cast(Answer, raw)
    if a in ("return", "context") and not note:
        raise Refused(400, f"{a} needs a note for the agent")
    role = r.role_of(run_id)
    if role is None:
        raise Refused(400, "the role of this run cannot be imported here")
    if by == "agent":
        w = next((x for x in r.queue() if (x.run_id, x.step) == (run_id, step) and x.kind == "stop"), None)
        if w is not None and not w.holds:
            raise Refused(403, f"the stop at {step!r} is a sample: the run went past it, and only the person answers "
                               "it, from the page, since an agent's answer never counts as proof")
    output = f.get("output")
    if a != "change":
        output = None
    elif isinstance(output, str):
        try:
            output = json.loads(output)
        except ValueError as err:
            raise Refused(400, f"the edited output is not JSON: {err}") from None
    chosen = f.get("chosen") or ""
    picked = ([x.strip() for x in chosen.replace(",", " ").split()] if isinstance(chosen, str)
              else [str(x).strip() for x in cast(list[Any], chosen) if str(x).strip()])
    given = f.get("rejected")  # by id, each with its line; the page sends them as reject:<id>
    rejected = ({str(k): str(v) for k, v in cast(dict[Any, Any], given).items()} if isinstance(given, dict) else
                {k[len("reject:"):]: str(v) for k, v in f.items() if k.startswith("reject:")})
    answers: dict[str, str] | None = None  # the interview's, by question: written words win over the option picked
    if isinstance(f.get("answers"), dict):
        answers = {str(k): str(v) for k, v in cast(dict[Any, Any], f["answers"]).items()}
    elif ids := {k.split(":", 1)[1] for k in f if k.startswith(("pick:", "own:"))}:
        answers = {i: str(f.get(f"own:{i}") or "").strip() or str(f.get(f"pick:{i}") or "") for i in ids}
    try:
        r.c.answer(role, run_id, step, a, note=note, output=output, seconds_spent=spent, chosen=picked,
                   rejected=rejected or None, answers=answers,
                   # the page's select goes with every answer; only return and context go back
                   to=(_text(f, "to") or None) if a in ("return", "context") else None,
                   by=by, evidence=_text(f, "evidence") or None)
    except AnswerRejected as err:
        raise Refused(400, "not recorded: " + "; ".join(err.reasons)) from None
    except RunBusy:
        raise Refused(409, "busy: another answer to this run is being recorded; try again in a moment") from None
    continue_in_background(r.c.home, run_id)  # when a process already runs it, that one goes on with this
    return {"run": run_id, "step": step, "answer": "return" if rejected else a, "by": by, "continuing": True}


def continue_run(r: Review, f: dict[str, Any], by: By) -> dict[str, Any]:
    """Go on with a run that is not running: blocked, left idle, or past its quota pause."""
    run_id = _text(f, "run")
    if not list(r.c.store.events("RunStarted", run_id=run_id)):
        raise Refused(404, f"No run {run_id}")
    if r.c._held(run_id):  # pyright: ignore[reportPrivateUsage]
        raise Refused(409, f"run {run_id} is running in another process")
    continue_in_background(r.c.home, run_id)
    return {"run": run_id, "continuing": True}


def send_back(r: Review, f: dict[str, Any], by: By) -> dict[str, Any]:
    """Send a blocked run back to an earlier step with a note saying what was wrong."""
    run_id = _text(f, "run")
    role = r.role_of(run_id)
    if role is None:
        raise Refused(404, f"No role to send run {run_id} back with")
    try:
        r.c.send_back(role, run_id, _text(f, "to"), _text(f, "note"), by=by)
    except AnswerRejected as err:
        raise Refused(400, "; ".join(err.reasons)) from None
    except RunBusy:
        raise Refused(409, f"run {run_id} is running in another process") from None
    continue_in_background(r.c.home, run_id)
    return {"run": run_id, "to": _text(f, "to"), "continuing": True}


def dismiss(r: Review, f: dict[str, Any], by: By) -> dict[str, Any]:
    """Drop an unfinished run: it finishes with nothing delivered, at once or, while a process works on it, when its
    current step ends."""
    _person_only(by, "drops a run")
    run_id = _text(f, "run")
    try:
        done = r.c.dismiss(run_id)
    except AnswerRejected as err:
        raise Refused(400, "; ".join(err.reasons)) from None
    if done == "asked":  # a process is working on it: it ends when the step under way does
        return {"run": run_id, "dismissed": False, "when": "its current step ends"}
    return {"run": run_id, "dismissed": True}


def correct(r: Review, f: dict[str, Any], by: By) -> dict[str, Any]:
    """Record that a delivery was corrected afterwards: which step was wrong, what kind of wrong, and why."""
    run_id = _text(f, "run")
    role = r.role_of(run_id)
    if role is None:
        raise Refused(404, f"No role for run {run_id}")
    try:
        p = r.c.correct(role, run_id, _text(f, "step"), _text(f, "kind"), _text(f, "note"), by=by)
    except AnswerRejected as err:
        raise Refused(400, "; ".join(err.reasons)) from None
    except RunBusy:
        raise Refused(409, f"run {run_id} is running in another process") from None
    return {"run": run_id, "correction": p["id"], "kind": p["kind"],
            "counted_at": p["stop"] if p["kind"] == "error" else None}


def file_as(r: Review, f: dict[str, Any], by: By) -> dict[str, Any]:
    """File a correction as another kind: only an error counts against the stop that let it through."""
    _person_only(by, "files a correction as another kind")
    run_id = _text(f, "run")
    try:
        p = r.c.reclassify(run_id, _text(f, "id"), _text(f, "kind"), by=by)
    except AnswerRejected as err:
        raise Refused(400, "; ".join(err.reasons)) from None
    except RunBusy:
        raise Refused(409, f"run {run_id} is running in another process") from None
    return {"run": run_id, "correction": p["id"], "kind": p["kind"]}


def undo(r: Review, f: dict[str, Any], by: By) -> dict[str, Any]:
    """Revert what a run merged."""
    _person_only(by, "undoes a delivery")
    run_id = _text(f, "run")
    try:
        r.c.undo(run_id, _text(f, "note"), by=by)
    except AnswerRejected as err:
        raise Refused(400, "; ".join(err.reasons)) from None
    except RunBusy:
        raise Refused(409, f"run {run_id} is running in another process") from None
    return {"run": run_id, "undone": True}


def halt(r: Review, f: dict[str, Any], by: By) -> dict[str, Any]:
    """Halt every run before its next step, and every merge, until the person resumes."""
    r.c.halt(by=by, note=_text(f, "note"))
    return {"halted": True}


def resume(r: Review, f: dict[str, Any], by: By) -> dict[str, Any]:
    """End the halt, and go on with every run it held."""
    _person_only(by, "resumes")
    try:
        runs = r.c.resume(by=by)
    except AnswerRejected as err:
        raise Refused(400, "; ".join(err.reasons)) from None
    for run_id in runs:
        continue_in_background(r.c.home, run_id)
    return {"resumed": runs}


def start(r: Review, f: dict[str, Any], by: By) -> dict[str, Any]:
    """Start a run of a role on a task, in the background."""
    ref = _text(f, "ref").strip() or _text(f, "role")
    if ref and not ref.endswith(".toml") and ":" not in ref:  # a role by its name, as jaynes run takes it
        try:
            ref = catalog.ref(ref, r.c.home, Path(_text(f, "project").strip()) if _text(f, "project").strip() else None)
        except catalog.Unknown as err:
            raise Refused(400, str(err)) from None
    task = _text(f, "task").strip()
    if not task:
        raise Refused(400, "a run needs a task")
    if ref.endswith(".toml"):  # stored whole, so the run continues from any folder; its mistakes are shown
        ref = str(Path(ref).expanduser().resolve())
        try:
            role_file.load(Path(ref))
        except role_file.RoleFileError as err:
            raise Refused(400, str(err)) from None
    found = r.import_role(ref)
    if found is None:
        raise Refused(400, f"{ref} cannot be imported from {Path.cwd()}")
    if problems := verify.verify(found):
        raise Refused(400, verify.report(found, problems))
    folder = _text(f, "project").strip()
    preview = _text(f, "preview").strip()
    if folder:
        try:
            repo.resolve(Path(folder).expanduser())
        except repo.ProjectError as err:
            raise Refused(400, str(err)) from None
    if preview and "{port}" not in preview:
        raise Refused(400, "the preview command needs {port} where the port goes")
    ports = _text(f, "preview_ports").replace(",", " ").split()
    if not all(p.isdigit() and 0 < int(p) < 65536 for p in ports):
        raise Refused(400, "local ports are numbers, such as 8789")
    given_in = f.get("inputs")
    if isinstance(given_in, dict):
        given = {str(k).strip(): str(v).strip() for k, v in cast(dict[Any, Any], given_in).items()}
    else:
        lines = [x.strip() for x in _text(f, "inputs").splitlines() if x.strip()]
        if any("=" not in x for x in lines):
            raise Refused(400, "inputs are name=value, one per line")
        given = {k.strip(): v.strip() for k, v in (x.split("=", 1) for x in lines)}
    if problems := run_inputs.missing(found, given):
        raise Refused(400, "; ".join(problems))
    run_id = new_run_id()
    start_in_background(r.c.home, run_id, ref, task, r.known_roles().get(ref, {}),
                        str(Path(folder).expanduser()) if folder else "", preview,
                        _text(f, "preview_path").strip(), " ".join(ports), _text(f, "suite").strip(), given)
    return {"run": run_id, "started": True, "page": f"{page(r.c.home)}/run/{run_id}"}


def drop_run(r: Review, f: dict[str, Any], by: By) -> dict[str, Any]:
    """Take a run out of a plan before it starts."""
    from jaynes import plan
    _person_only(by, "takes a run out of a plan")
    rid, name = _text(f, "request"), _text(f, "name")
    try:
        plan.drop(r.c, rid, name)
    except plan.PlanError as err:
        raise Refused(400, str(err)) from None
    return {"request": rid, "dropped": name}


def installed(r: Review) -> dict[str, Any]:
    """The packages of roles installed, each with its roles, and which projects use which: the default list, for
    every project with no list of its own, and each project's own list, first package first."""
    home = r.c.home
    shown: list[dict[str, Any]] = []
    for p in packages.installed(home).values():
        folder = Path(p.path)
        try:
            about = packages.manifest(folder).about
        except packages.PackageError:
            about = ""
        shown.append({"name": p.name, "version": p.version, "source": p.source, "linked": p.linked,
                      "commit": p.commit, "ref": p.ref, "about": about, "missing": not folder.is_dir(),
                      "roles": [x.name for _, x in catalog.found(folder)] if folder.is_dir() else []})
    return {"packages": shown, "default": packages.uses(home), "projects": packages.projects(home)}


def _project_of(f: dict[str, Any]) -> Path | None:
    given = _text(f, "project").strip()
    if not given:
        return None
    folder = Path(given).expanduser()
    if not folder.is_dir():
        raise Refused(400, f"{given} is not a folder on this computer")
    return folder


def add_package(r: Review, f: dict[str, Any], by: By) -> dict[str, Any]:
    """Use a package of roles in a project, or in every project with no list of its own: a folder, linked, a git
    repository, or one that comes with Jaynes by its name."""
    source = _text(f, "source").strip()
    if not source:
        raise Refused(400, "name the package: a folder with jaynes-package.toml, a git address, or software or "
                           "design")
    project = _project_of(f)
    try:
        p = packages.add(r.c.home, source, project, ref=_text(f, "ref").strip(), folder=_text(f, "folder").strip())
    except packages.PackageError as err:
        raise Refused(400, str(err)) from None
    return {"name": p.name, "version": p.version, "linked": p.linked,
            "project": packages.root(project) if project else None, "uses": packages.uses(r.c.home, project)}


def remove_package(r: Review, f: dict[str, Any], by: By) -> dict[str, Any]:
    """Stop using a package in a project, or in the default list; its installed versions stay for the runs that
    use them."""
    project = _project_of(f)
    try:
        packages.remove(r.c.home, _text(f, "name").strip(), project)
    except packages.PackageError as err:
        raise Refused(400, str(err)) from None
    return {"project": packages.root(project) if project else None, "uses": packages.uses(r.c.home, project)}


Action = Callable[["Review", dict[str, Any], By], dict[str, Any]]
ACTIONS: dict[str, Action] = {"answer": answer, "continue": continue_run, "send-back": send_back, "dismiss": dismiss,
                              "correct": correct, "correction": file_as, "undo": undo, "halt": halt,
                              "resume": resume, "start": start, "add-package": add_package, "drop-run": drop_run,
                              "remove-package": remove_package}
