"""Monte Carlo check of the certification guarantee.

For a category whose true error rate is ``p``, labels arrive one at a time
(each audit is an independent Bernoulli(p) draw). A rule *certifies* the first
time its condition holds. The guarantee to check: when ``p >= budget``, the
fraction of runs that are ever certified must stay at or below ``delta``.

Both rules are monotone in n for a fixed error count k (each correct label
makes them easier to satisfy), so each reduces to a table ``nstar[k]``: the
rule holds at (n, k) iff ``n >= nstar[k]``. That keeps the simulation cheap.
"""

from __future__ import annotations

import random
from dataclasses import dataclass
from statistics import median

from jaynes import bounds

_NEVER = 10**12


def threshold_table(rule: str, budget: float, delta: float, max_k: int, horizon: int) -> list[int]:
    table: list[int] = []
    for k in range(max_k + 1):
        if rule == "anytime":
            n = bounds.labels_to_certify(k, budget, delta, limit=horizon)
        elif rule == "fixed":
            n = bounds.labels_to_certify_cp(k, budget, 1.0 - delta, limit=horizon)
        else:
            raise ValueError(rule)
        table.append(_NEVER if n is None else n)
    return table


@dataclass(frozen=True)
class Outcome:
    rule: str
    p: float
    runs: int
    horizon: int
    certified_fraction: float
    median_labels_to_certify: float | None


def simulate(rule: str, p: float, *, budget: float = 0.05, delta: float = 0.05,
             runs: int = 2000, horizon: int = 2000, seed: int = 0) -> Outcome:
    max_k = horizon
    # Beyond some k the rule can never hold within the horizon; cap the table there.
    table = threshold_table(rule, budget, delta, max_k=min(max_k, int(horizon * budget) + 5), horizon=horizon)
    rng = random.Random(seed)
    hits: list[int] = []
    for _ in range(runs):
        k = 0
        for n in range(1, horizon + 1):
            if rng.random() < p:
                k += 1
                if k >= len(table):
                    break
            if n >= table[k]:
                hits.append(n)
                break
    frac = len(hits) / runs
    return Outcome(rule, p, runs, horizon, frac, median(hits) if hits else None)
