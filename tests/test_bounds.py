# pyright: reportMissingTypeStubs=false, reportUnknownMemberType=false, reportUnknownVariableType=false
import math

import pytest
from hypothesis import given, settings
from hypothesis import strategies as st
from scipy import integrate, special, stats

from jaynes import bounds


@given(
    x=st.floats(min_value=1e-6, max_value=1 - 1e-6),
    a=st.floats(min_value=0.5, max_value=400),
    b=st.floats(min_value=0.5, max_value=400),
)
@settings(max_examples=400)
def test_betainc_matches_scipy(x: float, a: float, b: float) -> None:
    assert bounds.betainc(x, a, b) == pytest.approx(float(special.betainc(a, b, x)), rel=1e-7, abs=1e-12)


@pytest.mark.parametrize("n,k", [(10, 0), (59, 0), (93, 1), (124, 2), (311, 9), (1000, 30)])
def test_clopper_pearson_matches_scipy(n: int, k: int) -> None:
    expected = float(stats.beta.ppf(0.95, k + 1, n - k))
    assert bounds.clopper_pearson_upper(n, k, 0.95) == pytest.approx(expected, rel=1e-8)


def test_cp_reference_numbers_from_spec() -> None:
    assert [bounds.labels_to_certify_cp(k, 0.05, 0.95) for k in (0, 1, 2, 9)] == [59, 93, 124, 311]


@pytest.mark.parametrize("n,k,budget", [(0, 0, 0.05), (40, 0, 0.05), (88, 0, 0.05), (200, 3, 0.05), (50, 10, 0.1)])
def test_evalues_match_numeric_integration(n: int, k: int, budget: float) -> None:
    def lr(q: float) -> float:
        return (q / budget) ** k * ((1 - q) / (1 - budget)) ** (n - k)

    certify, _ = integrate.quad(lr, 0, budget, epsabs=0, epsrel=1e-11, limit=200)
    refute, _ = integrate.quad(lr, budget, 1, epsabs=0, epsrel=1e-11, limit=200)
    assert math.exp(bounds.log_certify_evalue(n, k, budget)) == pytest.approx(certify / budget, rel=1e-6)
    assert math.exp(bounds.log_refute_evalue(n, k, budget)) == pytest.approx(refute / (1 - budget), rel=1e-6)


@given(n=st.integers(min_value=0, max_value=3000), k=st.integers(min_value=0, max_value=60))
def test_certify_evalue_grows_with_correct_labels_and_falls_with_errors(n: int, k: int) -> None:
    k = min(k, n)
    here = bounds.log_certify_evalue(n, k, 0.05)
    assert bounds.log_certify_evalue(n + 1, k, 0.05) >= here - 1e-9
    assert bounds.log_certify_evalue(n + 1, k + 1, 0.05) <= here + 1e-9


def test_certification_and_refutation_are_exclusive_at_reasonable_delta() -> None:
    for n in range(0, 400, 7):
        for k in range(0, n + 1, max(1, n // 20)):
            assert not (bounds.certified(n, k, 0.05, 0.05) and bounds.refuted(n, k, 0.05, 0.05))


def test_anytime_threshold_costs_more_labels_than_fixed_sample() -> None:
    anytime = bounds.labels_to_certify(0, 0.05, 0.05)
    assert anytime is not None and 59 < anytime < 120
