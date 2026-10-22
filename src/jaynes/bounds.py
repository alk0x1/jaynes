"""Statistical tests behind certification.

Everything here is pure and dependency-free. Two families:

* Clopper-Pearson upper bound: exact for a sample size fixed in advance.
  Recomputing it after every label and certifying the first time it fits the
  budget is *not* valid (optional stopping), so it is used only for display.
* Mixture likelihood-ratio e-values: valid at any stopping time by Ville's
  inequality. A category is certified while ``certify_evalue >= 1/delta`` and
  refuted while ``refute_evalue >= 1/delta``.

Certify: H0 is p >= b. The statistic mixes the likelihood ratio q/b over
q ~ Uniform[0, b]. For p = b it is a nonnegative martingale with mean 1; for
p > b each factor has expectation below 1, so it is a supermartingale. Hence
P(ever >= 1/delta) <= delta whenever the true error rate p >= b.

Refute: H0 is p <= b, mixing over q ~ Uniform[b, 1]. Symmetric argument.
"""

from __future__ import annotations

import math

_MAXIT = 400
_EPS = 3e-15
_FPMIN = 1e-300


def _betacf(x: float, a: float, b: float) -> float:
    """Continued fraction for the incomplete beta (Lentz's method)."""
    qab, qap, qam = a + b, a + 1.0, a - 1.0
    c, d = 1.0, 1.0 - qab * x / qap
    d = 1.0 / (d if abs(d) > _FPMIN else _FPMIN)
    h = d
    for m in range(1, _MAXIT + 1):
        m2 = 2 * m
        aa = m * (b - m) * x / ((qam + m2) * (a + m2))
        d = 1.0 + aa * d
        d = d if abs(d) > _FPMIN else _FPMIN
        c = 1.0 + aa / c
        c = c if abs(c) > _FPMIN else _FPMIN
        d = 1.0 / d
        h *= d * c
        aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2))
        d = 1.0 + aa * d
        d = d if abs(d) > _FPMIN else _FPMIN
        c = 1.0 + aa / c
        c = c if abs(c) > _FPMIN else _FPMIN
        d = 1.0 / d
        delta = d * c
        h *= delta
        if abs(delta - 1.0) < _EPS:
            return h
    return h


def log_beta(a: float, b: float) -> float:
    return math.lgamma(a) + math.lgamma(b) - math.lgamma(a + b)


def log_betainc_lower(x: float, a: float, b: float) -> float:
    """log of the regularized incomplete beta I_x(a, b), accurate in the lower tail."""
    if x <= 0.0:
        return -math.inf
    if x >= 1.0:
        return 0.0
    log_front = a * math.log(x) + b * math.log1p(-x) - log_beta(a, b)
    if x < (a + 1.0) / (a + b + 2.0):
        return log_front + math.log(_betacf(x, a, b)) - math.log(a)
    upper = math.exp(log_front + math.log(_betacf(1.0 - x, b, a)) - math.log(b))
    return math.log1p(-upper) if upper < 1.0 else -math.inf


def betainc(x: float, a: float, b: float) -> float:
    """Regularized incomplete beta I_x(a, b)."""
    return math.exp(log_betainc_lower(x, a, b))


def clopper_pearson_upper(n: int, k: int, confidence: float) -> float:
    """One-sided exact upper bound on a binomial rate for a fixed sample size."""
    _check(n, k)
    if k >= n:
        return 1.0
    target = confidence  # P(X <= k | p) = 1 - confidence  <=>  I_p(k+1, n-k) = confidence
    lo, hi = 0.0, 1.0
    for _ in range(80):
        mid = (lo + hi) / 2.0
        if betainc(mid, k + 1.0, n - k) < target:
            lo = mid
        else:
            hi = mid
    return hi


def log_certify_evalue(n: int, k: int, budget: float) -> float:
    """log e-value against "error rate >= budget" after k errors in n labels."""
    _check(n, k)
    _check_budget(budget)
    a, b = k + 1.0, n - k + 1.0
    # (1/budget) * integral_0^budget q^k (1-q)^(n-k) dq  /  (budget^k (1-budget)^(n-k))
    return (
        log_betainc_lower(budget, a, b)
        + log_beta(a, b)
        - (k + 1) * math.log(budget)
        - (n - k) * math.log1p(-budget)
    )


def log_refute_evalue(n: int, k: int, budget: float) -> float:
    """log e-value against "error rate <= budget" after k errors in n labels."""
    _check(n, k)
    _check_budget(budget)
    a, b = k + 1.0, n - k + 1.0
    # integral_budget^1 = B(a,b) * I_{1-budget}(b, a), computed from the accurate tail.
    return (
        log_betainc_lower(1.0 - budget, b, a)
        + log_beta(a, b)
        - k * math.log(budget)
        - (n - k + 1) * math.log1p(-budget)
    )


def certified(n: int, k: int, budget: float, delta: float) -> bool:
    return log_certify_evalue(n, k, budget) >= math.log(1.0 / delta)


def refuted(n: int, k: int, budget: float, delta: float) -> bool:
    return log_refute_evalue(n, k, budget) >= math.log(1.0 / delta)


def labels_to_certify(k: int, budget: float, delta: float, limit: int = 100_000) -> int | None:
    """Smallest n at which k errors still certify; the e-value grows with each correct label."""
    lo = max(k + 1, 1)
    if not certified(limit, k, budget, delta):
        return None
    hi = limit
    while lo < hi:
        mid = (lo + hi) // 2
        if certified(mid, k, budget, delta):
            hi = mid
        else:
            lo = mid + 1
    return lo


def labels_to_certify_cp(k: int, budget: float, confidence: float, limit: int = 100_000) -> int | None:
    """Same threshold for the fixed-sample Clopper-Pearson rule (display and comparison only)."""
    lo = max(k + 1, 1)
    if clopper_pearson_upper(limit, k, confidence) > budget:
        return None
    hi = limit
    while lo < hi:
        mid = (lo + hi) // 2
        if clopper_pearson_upper(mid, k, confidence) <= budget:
            hi = mid
        else:
            lo = mid + 1
    return lo


def _check(n: int, k: int) -> None:
    if n < 0 or k < 0 or k > n:
        raise ValueError(f"need 0 <= k <= n, got n={n}, k={k}")


def _check_budget(budget: float) -> None:
    if not 0.0 < budget < 1.0:
        raise ValueError(f"budget must be in (0, 1), got {budget}")
