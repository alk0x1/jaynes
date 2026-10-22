"""Safety properties of the gate and consistency of the log projection."""

import itertools

from hypothesis import given, settings
from hypothesis import strategies as st

from jaynes import bounds, gate
from jaynes.policy import UNKNOWN, Category, Effect, Policy
from jaynes.store import Event, Store, window_from

POLICY = Policy(budget=0.05, delta=0.05, audit_rate=0.1)
EFFECTS = st.sampled_from(list(Effect))
FLOORS = st.sampled_from([None, "deny", "ask"])
windows = st.integers(min_value=0, max_value=3000).flatmap(
    lambda n: st.builds(gate.Window, st.just(n), st.integers(min_value=0, max_value=n))
)


@given(effect=EFFECTS, floor=FLOORS, w=windows, u=st.floats(min_value=0, max_value=1, exclude_max=True),
       undetermined=st.sampled_from(["defer", "ask"]))
@settings(max_examples=2000)
def test_gate_safety(effect: Effect, floor: str | None, w: gate.Window, u: float, undetermined: str) -> None:
    pol = Policy(budget=0.05, delta=0.05, audit_rate=0.1, undetermined=undetermined)  # type: ignore[arg-type]
    cat = Category(name="c", effect=effect, floor=floor)  # type: ignore[arg-type]
    d = gate.decide(cat, w, pol, u)
    if floor:
        # a floor never runs without a person, whatever the labels say
        assert d.verdict == ("deny" if floor == "deny" else "ask")
        return
    if d.verdict == "allow":
        # Jaynes skips the host only for pure effects or a certified window
        assert effect == Effect.PURE or bounds.certified(w.n, w.k, 0.05, 0.05)
    if effect != Effect.PURE and bounds.refuted(w.n, w.k, 0.05, 0.05):
        assert d.verdict == "ask"
    if d.audit:
        assert d.verdict == "ask" and u < pol.audit_rate


def test_unknown_defers_and_is_never_certified() -> None:
    d = gate.decide(UNKNOWN, gate.Window(5000, 0), POLICY, 0.99)
    assert (d.verdict, d.status) == ("defer", "unknown")


def test_audit_rate_is_respected_by_the_coin() -> None:
    hits = sum(gate.coin(POLICY, f"toolu_{i}") < POLICY.audit_rate for i in range(20000))
    assert 0.09 < hits / 20000 < 0.11


# --- projection --------------------------------------------------------------

event_specs = st.lists(
    st.tuples(st.sampled_from(["GateDecided", "HumanLabeled"]), st.integers(0, 15),
              st.sampled_from(["fpA", "fpB"]), st.sampled_from(["correct", "error", "context", "none"])),
    max_size=80,
)


@given(specs=event_specs)
@settings(max_examples=300)
def test_sql_window_equals_pure_fold(specs: list[tuple[str, int, str, str]]) -> None:
    store = Store(":memory:")
    events: list[Event] = []
    for seq, (type_, aid, fp, label) in enumerate(specs, start=1):
        payload = {"fingerprint": fp} if type_ == "GateDecided" else {"label": label}
        store.append(type_, payload, action_id=f"a{aid}", session="s", ts=float(seq))
        events.append(Event(seq, float(seq), type_, f"a{aid}", "s", payload))
    for fp in ("fpA", "fpB"):
        assert store.window(fp) == window_from(events, fp)


def test_changing_the_fingerprint_opens_a_new_window() -> None:
    store = Store(":memory:")
    for i in range(100):
        store.append("GateDecided", {"fingerprint": "old"}, action_id=f"a{i}", session="s")
        store.append("HumanLabeled", {"label": "correct"}, action_id=f"a{i}", session="s")
    assert store.window("old") == gate.Window(100, 0)
    assert store.window("new") == gate.Window(0, 0)


def test_a_certified_category_runs_and_one_error_in_a_small_window_takes_it_back() -> None:
    cat = Category(name="edit_docs", effect=Effect.REVERSIBLE)
    need = bounds.labels_to_certify(0, 0.05, 0.05)
    assert need is not None
    not_audited = next(u for u in itertools.count(0.5, 0.01) if u >= POLICY.audit_rate)
    assert gate.decide(cat, gate.Window(need - 1, 0), POLICY, not_audited).verdict == "defer"
    assert gate.decide(cat, gate.Window(need, 0), POLICY, not_audited).verdict == "allow"
    assert gate.decide(cat, gate.Window(need + 1, 1), POLICY, not_audited).verdict == "defer"
