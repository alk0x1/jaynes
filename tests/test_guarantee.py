"""The guarantee, checked by simulation.

When the true error rate is at or above the budget, a category must be
certified in at most ``delta`` of the runs, no matter when certification is
checked. The fixed-sample rule recomputed after every label is the known
failure mode and is kept here as the counterexample.
"""

import math

import pytest

from jaynes.sim import simulate

BUDGET, DELTA, RUNS, HORIZON = 0.05, 0.05, 4000, 2000


def _upper(frac: float, runs: int) -> float:
    # three standard errors of Monte Carlo slack
    return frac + 3 * math.sqrt(max(frac * (1 - frac), 1e-4) / runs)


@pytest.mark.parametrize("p", [0.05, 0.06, 0.08])
def test_anytime_rule_keeps_the_guarantee(p: float) -> None:
    out = simulate("anytime", p, budget=BUDGET, delta=DELTA, runs=RUNS, horizon=HORIZON, seed=1)
    assert out.certified_fraction <= DELTA, out


def test_fixed_sample_rule_checked_after_every_label_breaks_the_guarantee() -> None:
    out = simulate("fixed", 0.05, budget=BUDGET, delta=DELTA, runs=RUNS, horizon=HORIZON, seed=1)
    assert out.certified_fraction > _upper(DELTA, RUNS), out


@pytest.mark.parametrize("p,max_median", [(0.0, 100), (0.01, 200), (0.02, 400)])
def test_anytime_rule_still_certifies_good_categories(p: float, max_median: int) -> None:
    out = simulate("anytime", p, budget=BUDGET, delta=DELTA, runs=1000, horizon=HORIZON, seed=2)
    assert out.certified_fraction > 0.9, out
    assert out.median_labels_to_certify is not None and out.median_labels_to_certify <= max_median, out
