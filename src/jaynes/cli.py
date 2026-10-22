"""``jaynes`` command line.

  jaynes hook                       Claude Code hook (reads the event on stdin)
  jaynes autonomy [--config P]      status, labels and evidence per category, from the log
  jaynes history  [--config P]      classify past tool calls from transcripts; audit load estimate
  jaynes parity HOOK [--config P]   compare floors against an existing deny hook on past Bash calls
  jaynes simulate                   Monte Carlo check of the certification guarantee
  jaynes run --role M:R --task T    start a run of role R from module M
  jaynes run ID                     continue a run from its log (after a crash, a pause or a stop)
"""

from __future__ import annotations

import argparse
import collections
import importlib
import json
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Any

from jaynes import bounds, gate, hook
from jaynes.conductor import Conductor, new_run_id
from jaynes.history import past_calls
from jaynes.policy import Policy, classify, find_policy, jaynes_home, load_policy
from jaynes.role import Role
from jaynes.sim import simulate
from jaynes.store import Store


def _policy(path: str | None) -> Policy:
    found = Path(path) if path else find_policy(str(Path.cwd()))
    if found is None:
        sys.exit("no jaynes.toml found; pass --config")
    return load_policy(found)


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
    if not calls:
        sys.exit("no calls found")
    weeks = max((calls[-1].ts - calls[0].ts).total_seconds() / (7 * 86400), 1 / 7)
    per_cat: collections.Counter[str] = collections.Counter()
    who: dict[str, collections.Counter[str]] = collections.defaultdict(collections.Counter)
    unknown: collections.Counter[str] = collections.Counter()
    for c in calls:
        cat = classify(policy, c.tool, c.tool_input, c.cwd or "/")
        per_cat[cat.name] += 1
        who[cat.name][c.decided_by.split("/")[0]] += 1
        if cat.name == "unknown":
            key = c.tool if c.tool != "Bash" else "Bash: " + str(c.tool_input.get("command", ""))[:70].replace("\n", " ")
            unknown[key] += 1
    human = sum(1 for c in calls if c.label in ("correct", "error", "context"))
    print(f"{len(calls)} calls over {weeks:.1f} weeks; {human} answered by a person "
          f"({human / weeks:.1f} per week)\n")
    print(f"{'category':14} {'status':10} {'calls':>6} {'/week':>7} {'audits/wk':>9} {'weeks to certify':>16}  approved by today")
    need0 = bounds.labels_to_certify(0, policy.budget, policy.delta) or 0
    audits = 0.0
    for name, count in per_cat.most_common():
        c = policy.category(name)
        kind = "floor" if c.floor else ("pure" if c.effect == 0 else ("unknown" if name == "unknown" else "learns"))
        rate = count / weeks
        aud = rate * policy.audit_rate if kind == "learns" else 0.0
        audits += aud
        wk = f"{need0 / aud:.1f}" if aud else ""
        approvers = ", ".join(f"{k} {v}" for k, v in who[name].most_common(3))
        print(f"{name:14} {kind:10} {count:>6} {rate:>7.0f} {aud:>9.1f} {wk:>16}  {approvers}")
    print(f"\naudit prompts per week at {policy.audit_rate:.0%}: {audits:.0f} "
          f"(weeks to certify assume zero errors: {need0} labels)")
    share = per_cat['unknown'] / len(calls)
    print(f"unknown: {per_cat['unknown']} calls ({share:.1%})")
    for k, v in unknown.most_common(args.top):
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


def load_ref(ref: str) -> Any:
    module, _, attr = ref.partition(":")
    if not module or not attr:
        sys.exit(f"expected module:attribute, got {ref!r}")
    return getattr(importlib.import_module(module), attr)


def cmd_run(args: argparse.Namespace) -> None:
    home = Path(args.home) if args.home else jaynes_home()
    c = Conductor(home)
    if args.id:
        first = next(iter(c.store.events("RunStarted", run_id=args.id)), None)
        if first is None:
            sys.exit(f"no run {args.id} in {home}")
        if "ref" not in first.payload:
            sys.exit(f"run {args.id} was started from Python; continue it with Conductor.run")
        ref, task, extra = str(first.payload["ref"]), str(first.payload["task"]), dict(first.payload["runtimes"])
        run_id = args.id
    else:
        if not args.role or args.task is None:
            sys.exit("a new run needs --role and --task")
        ref, task, run_id = args.role, args.task, new_run_id()
        if any("=" not in pair for pair in args.runtime):
            sys.exit("--runtime takes name=module:attribute")
        extra = dict(pair.split("=", 1) for pair in args.runtime)
    r = load_ref(ref)
    if not isinstance(r, Role):
        sys.exit(f"{ref} is not a role")
    # imported on first use, so a run that never calls an agent needs neither CLI installed
    c.runtimes = {name: (lambda t=target: load_ref(t)()) for name, target in {**DEFAULT_RUNTIMES, **extra}.items()}
    result = c.run(r, task, run_id, started={"ref": ref, "runtimes": extra})
    print(json.dumps({"run": run_id, "status": result.status, "step": result.step, "until": result.until,
                      "reasons": list(result.reasons)}))


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="jaynes")
    sub = ap.add_subparsers(dest="cmd", required=True)
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
    r.add_argument("--role", help="module:attribute of the role")
    r.add_argument("--task")
    r.add_argument("--runtime", action="append", default=[], help="name=module:attribute, overrides a runtime")
    r.add_argument("--home")
    args = ap.parse_args(argv)
    if args.cmd == "hook":
        return hook.main()
    {"autonomy": cmd_autonomy, "history": cmd_history, "parity": cmd_parity, "simulate": cmd_simulate,
     "run": cmd_run}[args.cmd](args)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

