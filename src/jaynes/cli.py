"""``jaynes`` command line.

  jaynes hook                       Claude Code hook (reads the event on stdin)
  jaynes autonomy [--config P]      status, labels and evidence per category, from the log
  jaynes history  [--config P]      classify past tool calls from transcripts; audit load estimate
  jaynes parity HOOK [--config P]   compare floors against an existing deny hook on past Bash calls
  jaynes simulate                   Monte Carlo check of the certification guarantee
  jaynes                            the review page, opened in the browser
  jaynes run [ID] --role R --task T [--project DIR] [--preview CMD]
                                    start a run of a built-in role (visual-identity, redesign) or module:attribute,
                                    on the git repository at DIR
  jaynes run ID                     continue a run from its log (after a crash, a pause or a stop)
  jaynes queue                      stops waiting for an answer and blocked runs, oldest first
  jaynes answer RUN STEP ANSWER     accept, change (--output JSON), return or context (--note); the run continues
                                    recorded as an agent's: it releases the stop but is not a label
  jaynes review [--port 8740]       the review page on 127.0.0.1: queue, stops, runs, roles, log
  jaynes policy                     print the built-in policy, a starting point for ~/.jaynes/jaynes.toml
  jaynes demo [--port 8740]         a scripted run that stops for you, then the review page
"""

from __future__ import annotations

import argparse
import collections
import importlib
import json
import os
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import TYPE_CHECKING, Any

from jaynes import bounds, gate, hook
from jaynes.history import past_calls, report
from jaynes.install import BUILT_IN_POLICY
from jaynes.policy import Policy, classify, find_policy, jaynes_home, load_policy
from jaynes.sim import simulate
from jaynes.store import Store

if TYPE_CHECKING:  # the conductor pulls in pydantic; `jaynes hook` runs on every tool call and needs none of it
    from jaynes.conductor import Conductor
    from jaynes.role import Role


def _policy(path: str | None) -> Policy:
    """--config, else jaynes.toml found from cwd, else the built-in policy (these commands only read)."""
    found = Path(path) if path else find_policy(str(Path.cwd()))
    return load_policy(found or BUILT_IN_POLICY)


def cmd_autonomy(args: argparse.Namespace) -> None:
    store = Store(Path(args.db) if args.db else hook.db_path())
    rows: dict[str, dict[str, object]] = {}
    totals: collections.Counter[str] = collections.Counter()
    verdicts: dict[str, collections.Counter[str]] = collections.defaultdict(collections.Counter)
    latest_fp: dict[str, tuple[int, str, str]] = {}
    for e in store.events("GateDecided"):
        cat = str(e.payload.get("category"))
        totals[cat] += 1
        verdicts[cat][str(e.payload.get("verdict"))] += 1
        latest_fp[cat] = (e.seq, str(e.payload.get("fingerprint")), str(e.payload.get("status")))
    policy = _policy(args.config)
    for cat, (_, fp, _) in latest_fp.items():
        w = store.window(fp)
        c = policy.category(cat)
        status = gate.status_of(c, w, policy)
        need = gate.labels_needed(w, policy) if status in ("undetermined", "refuted") else None
        rows[cat] = {"status": status, "calls": totals[cat], "n": w.n, "k": w.k,
                     "auto": verdicts[cat]["allow"], "asked": verdicts[cat]["ask"],
                     "observed": (w.k / w.n) if w.n else None,
                     "cp_upper": bounds.clopper_pearson_upper(w.n, w.k, 1 - policy.delta) if w.n else None,
                     "evidence": gate.evidence(w, policy), "need": need}
    total_calls = sum(totals.values())
    unattended = sum(v["allow"] + v["defer"] for v in verdicts.values())
    print(f"policy {policy.source}  budget {policy.budget:.0%}  delta {policy.delta:.0%}  audit {policy.audit_rate:.0%}")
    print(f"calls {total_calls}  ran without Jaynes asking {unattended} ({unattended / max(total_calls, 1):.1%})\n")
    print(f"{'category':14} {'status':12} {'calls':>6} {'asked':>6} {'labels':>7} {'errors':>6} {'obs':>6} {'evidence':>8} {'to go':>6}")
    for cat, r in sorted(rows.items(), key=lambda kv: -int(kv[1]["calls"])):  # type: ignore[arg-type]
        obs = "" if r["observed"] is None else f"{float(r['observed']):.1%}"  # type: ignore[arg-type]
        need = "" if r["need"] is None else str(r["need"])
        print(f"{cat:14} {str(r['status']):12} {r['calls']:>6} {r['asked']:>6} {r['n']:>7} {r['k']:>6} {obs:>6} "
              f"{float(r['evidence']):>8.0%} {need:>6}")  # type: ignore[arg-type]
    store.close()


def cmd_history(args: argparse.Namespace) -> None:
    policy = _policy(args.config)
    calls = past_calls(days=args.days)
    if args.cwd:
        calls = [c for c in calls if c.cwd.startswith(args.cwd)]
    r = report(policy, calls, args.top)
    if r is None:
        sys.exit("no calls found")
    print(f"{r.calls} calls over {r.weeks:.1f} weeks; {r.answered} answered by a person "
          f"({r.answered / r.weeks:.1f} per week)\n")
    print(f"{'category':14} {'status':10} {'calls':>6} {'/week':>7} {'audits/wk':>9} {'weeks to certify':>16}  approved by today")
    for row in r.rows:
        wk = "" if row.weeks_to_certify is None else f"{row.weeks_to_certify:.1f}"
        approvers = ", ".join(f"{k} {v}" for k, v in row.approvers)
        print(f"{row.category:14} {row.kind:10} {row.calls:>6} {row.per_week:>7.0f} {row.audits_per_week:>9.1f} "
              f"{wk:>16}  {approvers}")
    print(f"\naudit prompts per week at {policy.audit_rate:.0%}: {r.audits_per_week:.0f} "
          f"(weeks to certify assume zero errors: {r.labels_to_certify} labels)")
    unknown = sum(row.calls for row in r.rows if row.category == "unknown")
    print(f"unknown: {unknown} calls ({unknown / r.calls:.1%})")
    for k, v in r.unknown:
        print(f"  {v:5}  {k}")


def cmd_parity(args: argparse.Namespace) -> None:
    policy = _policy(args.config)
    calls = [c for c in past_calls(days=args.days) if c.tool == "Bash"]

    def run_hook(command: str) -> bool:
        payload = json.dumps({"tool_name": "Bash", "tool_input": {"command": command}})
        r = subprocess.run([args.hook], input=payload, capture_output=True, text=True, timeout=30)
        return r.returncode == 2

    commands = [str(c.tool_input.get("command", "")) for c in calls]
    with ThreadPoolExecutor(max_workers=8) as pool:
        theirs = list(pool.map(run_hook, commands))
    ours = [bool(classify(policy, "Bash", {"command": cmd}, "/").floor) for cmd in commands]
    both = sum(1 for a, b in zip(theirs, ours) if a and b)
    only_hook = [cmd for cmd, a, b in zip(commands, theirs, ours) if a and not b]
    only_jaynes = [cmd for cmd, a, b in zip(commands, theirs, ours) if b and not a]
    print(f"{len(commands)} past Bash calls; hook blocks {sum(theirs)}, Jaynes floors {sum(ours)}, both {both}")
    print(f"blocked by the hook only: {len(only_hook)}")
    for cmd in only_hook[: args.top]:
        print("  ---", cmd[:400].replace("\n", "\n      "))
    print(f"floored by Jaynes only: {len(only_jaynes)}")
    for cmd in only_jaynes[: args.top]:
        print("  ---", cmd[:400].replace("\n", "\n      "))


def cmd_simulate(args: argparse.Namespace) -> None:
    print(f"budget {args.budget:.0%}  delta {args.delta:.0%}  runs {args.runs}  horizon {args.horizon} labels\n")
    print(f"{'true error':>10} {'rule':>8} {'certified':>10} {'median labels':>14}")
    for p in args.p:
        for rule in ("fixed", "anytime"):
            o = simulate(rule, p, budget=args.budget, delta=args.delta, runs=args.runs, horizon=args.horizon, seed=args.seed)
            med = "" if o.median_labels_to_certify is None else f"{o.median_labels_to_certify:.0f}"
            flag = "  <-- breaks the guarantee" if p >= args.budget and o.certified_fraction > args.delta else ""
            print(f"{p:>10.1%} {rule:>8} {o.certified_fraction:>10.1%} {med:>14}{flag}")


DEFAULT_RUNTIMES = {"claude": "jaynes.runtimes:ClaudeRuntime", "codex": "jaynes.runtimes:CodexRuntime"}


def role_ref(name: str) -> str:
    """A built-in role's name, as its module:attribute; anything with a colon is already one."""
    from jaynes.roles import BUILT_IN
    if ":" in name:
        return name
    if name not in BUILT_IN:
        sys.exit(f"no built-in role {name!r}: use one of {', '.join(BUILT_IN)}, or module:attribute")
    return BUILT_IN[name]


def load_ref(ref: str) -> Any:
    module, _, attr = ref.partition(":")
    if not module or not attr:
        sys.exit(f"expected module:attribute, got {ref!r}")
    if os.getcwd() not in sys.path:  # a role in the current folder, as continue_in_background already allows
        sys.path.insert(0, os.getcwd())
    return getattr(importlib.import_module(module), attr)


def _home(args: argparse.Namespace) -> Path:
    return Path(args.home) if args.home else jaynes_home()


def _started(c: Conductor, run_id: str) -> tuple[Role, str, str, dict[str, str]]:
    """The role, its reference, the task and the runtime overrides a CLI run was started with."""
    first = next(iter(c.store.events("RunStarted", run_id=run_id)), None)
    if first is None:
        sys.exit(f"no run {run_id} in {c.home}")
    if "ref" not in first.payload:
        sys.exit(f"run {run_id} was started from Python; continue it with Conductor.run")
    ref = str(first.payload["ref"])
    return _role(ref), ref, str(first.payload["task"]), dict(first.payload["runtimes"])


def _role(ref: str) -> Role:
    from jaynes.role import Role
    r = load_ref(ref)
    if not isinstance(r, Role):
        sys.exit(f"{ref} is not a role")
    return r


def cmd_run(args: argparse.Namespace) -> None:
    from jaynes.conductor import Conductor, new_run_id
    c = Conductor(_home(args))
    if args.id and not args.role:
        r, ref, task, extra = _started(c, args.id)
        run_id = args.id
    else:
        if not args.role or args.task is None:
            sys.exit("a new run needs --role and --task")
        if any("=" not in pair for pair in args.runtime):
            sys.exit("--runtime takes name=module:attribute")
        ref, task, run_id = role_ref(args.role), args.task, args.id or new_run_id()
        extra = dict(pair.split("=", 1) for pair in args.runtime)
        r = _role(ref)
    # imported on first use, so a run that never calls an agent needs neither CLI installed
    c.runtimes = {name: (lambda t=target: load_ref(t)()) for name, target in {**DEFAULT_RUNTIMES, **extra}.items()}
    started: dict[str, Any] = {"ref": ref, "runtimes": extra}
    if not args.id or args.role:
        if args.preview:
            started["preview"] = {"command": args.preview, "path": args.preview_path or "/",
                                  "ports": _ports(args.preview_ports or "")}
    from jaynes.project import ProjectError
    try:
        result = c.run(r, task, run_id, started=started,
                       project=Path(args.project) if args.project and args.role else None)
    except ProjectError as err:
        sys.exit(str(err))
    print(json.dumps({"run": run_id, "status": result.status, "step": result.step, "until": result.until,
                      "reasons": list(result.reasons)}))


def cmd_queue(args: argparse.Namespace) -> None:
    from jaynes.conductor import Conductor
    from jaynes.review import age
    c = Conductor(_home(args))
    items = c.queue()
    if not items:
        print("nothing is waiting")
    now = c.clock()
    for w in items:
        print(f"{w.run_id}  {w.kind}  {w.step}  {age(now - w.since)} ago  {w.role}: {w.task}")
        for line in ([w.question] if w.question else []) + list(w.reasons) + (
                [f"evidence {w.evidence}"] if w.evidence else []):
            print(f"    {line}")


def _ports(text: str) -> list[int]:
    try:
        ports = [int(p) for p in text.replace(",", " ").split()]
    except ValueError:
        sys.exit(f"--preview-ports takes port numbers, not {text!r}")
    if any(not 0 < p < 65536 for p in ports):
        sys.exit(f"--preview-ports takes port numbers, not {text!r}")
    return ports


def cmd_answer(args: argparse.Namespace) -> None:
    from jaynes.conductor import Answer, AnswerRejected, Conductor, RunBusy, continue_in_background
    c = Conductor(_home(args))
    r, _, _, _ = _started(c, args.run)
    output = None
    if args.output is not None:
        text = Path(args.output[1:]).read_text() if args.output.startswith("@") else args.output
        try:
            output = json.loads(text)
        except ValueError as err:
            sys.exit(f"--output is not JSON: {err}")
    answer: Answer = args.answer
    try:
        c.answer(r, args.run, args.step, answer, note=args.note, output=output, to=args.to, by="agent",
                 evidence=args.evidence)
    except AnswerRejected as err:
        sys.exit("answer not recorded: " + "; ".join(err.reasons))
    except RunBusy:
        sys.exit(f"run {args.run} is busy in another process; try again in a moment")
    continue_in_background(c.home, args.run)
    print(json.dumps({"run": args.run, "step": args.step, "answer": answer, "continuing": True}))


def cmd_demo(args: argparse.Namespace) -> None:
    from jaynes.conductor import Conductor, new_run_id
    from jaynes.demo import ROLE
    home = _home(args)
    c = Conductor(home)
    c.runtimes = {"scripted": lambda: load_ref("jaynes.demo:Agent")()}
    run_id = new_run_id()
    result = c.run(ROLE, "Redesign the People screen", run_id,
                   started={"ref": "jaynes.demo:ROLE", "runtimes": {"scripted": "jaynes.demo:Agent"}})
    print(json.dumps({"run": run_id, "status": result.status, "step": result.step}), flush=True)
    if not args.no_serve:
        _open_page(home, args.port)


def _open_page(home: Path, port: int) -> None:
    from jaynes.review import serve
    serve(home, port, browser=True)


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="jaynes")
    sub = ap.add_subparsers(dest="cmd")
    sub.add_parser("hook")
    a = sub.add_parser("autonomy")
    a.add_argument("--config")
    a.add_argument("--db")
    h = sub.add_parser("history")
    h.add_argument("--config")
    h.add_argument("--days", type=int, default=30)
    h.add_argument("--cwd")
    h.add_argument("--top", type=int, default=25)
    p = sub.add_parser("parity")
    p.add_argument("hook")
    p.add_argument("--config")
    p.add_argument("--days", type=int, default=50)
    p.add_argument("--top", type=int, default=10)
    s = sub.add_parser("simulate")
    s.add_argument("--budget", type=float, default=0.05)
    s.add_argument("--delta", type=float, default=0.05)
    s.add_argument("--runs", type=int, default=4000)
    s.add_argument("--horizon", type=int, default=2000)
    s.add_argument("--seed", type=int, default=1)
    s.add_argument("--p", type=float, nargs="+", default=[0.0, 0.01, 0.02, 0.03, 0.05, 0.06, 0.08])
    r = sub.add_parser("run")
    r.add_argument("id", nargs="?")
    r.add_argument("--role", help="a built-in role (visual-identity, redesign) or module:attribute")
    r.add_argument("--task")
    r.add_argument("--runtime", action="append", default=[], help="name=module:attribute, overrides a runtime")
    r.add_argument("--project", help="a folder in a git repository, for steps that work on a project")
    r.add_argument("--preview", help="a command that serves the project on {port}, shown at stops")
    r.add_argument("--preview-path", help="the path to open in the preview, / by default")
    r.add_argument("--preview-ports", help="local ports the preview may connect to, such as a backend: 8789,5432")
    r.add_argument("--home")
    q = sub.add_parser("queue")
    q.add_argument("--home")
    w = sub.add_parser("answer")
    w.add_argument("run")
    w.add_argument("step")
    w.add_argument("answer", choices=["accept", "change", "return", "context"])
    w.add_argument("--note")
    w.add_argument("--output", help="the edited output for change: JSON, or @file")
    w.add_argument("--to", help="with return or context: an earlier step to run again, with every step after it")
    w.add_argument("--evidence", help="the evidence hash jaynes queue printed; refuses the answer if the stop changed")
    w.add_argument("--home")
    v = sub.add_parser("review")
    v.add_argument("--port", type=int, default=8740)
    v.add_argument("--home")
    sub.add_parser("policy")
    d = sub.add_parser("demo")
    d.add_argument("--port", type=int, default=8740)
    d.add_argument("--home")
    d.add_argument("--no-serve", action="store_true", help="start the run and exit, without the review page")
    args = ap.parse_args(argv)
    if args.cmd is None:
        _open_page(jaynes_home(), 8740)
        return 0
    if args.cmd == "hook":
        return hook.main()
    if args.cmd == "policy":
        print(BUILT_IN_POLICY.read_text(), end="")
        return 0
    if args.cmd == "review":
        from jaynes.review import serve
        serve(_home(args), args.port)
        return 0
    {"autonomy": cmd_autonomy, "history": cmd_history, "parity": cmd_parity, "simulate": cmd_simulate,
     "run": cmd_run, "queue": cmd_queue, "answer": cmd_answer, "demo": cmd_demo}[args.cmd](args)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

