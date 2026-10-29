"""Append-only event log in SQLite, and the projection that derives state from it.

Events (one row each, payload as JSON):

* ``ActionProposed``  action_id, session, tool, category, effect, summary, cwd, transcript
* ``GateDecided``     action_id, category, fingerprint, verdict, status, audit, n, k, log_e, reason
* ``Executed``        action_id, ok
* ``HumanLabeled``    action_id, label (correct | error | context | none), source, note

Conductor events carry a ``run_id`` instead; their payloads are listed in SPEC
section 8 and written by ``jaynes.conductor``.

State is never stored: the window of a category is the set of labels whose
decision carried the category's current fingerprint.
"""

from __future__ import annotations

import json
import sqlite3
import time
from collections.abc import Iterable, Iterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from jaynes.gate import Window

SCHEMA = """
create table if not exists events (
  seq integer primary key autoincrement,
  ts real not null,
  type text not null,
  action_id text,
  session text,
  payload text not null,
  run_id text
);
create index if not exists events_action on events(action_id);
create index if not exists events_type on events(type);
"""
# Logs created before the conductor existed have no run_id column.
MIGRATE = "alter table events add column run_id text"
INDEXES = "create index if not exists events_run on events(run_id)"


@dataclass(frozen=True)
class Event:
    seq: int
    ts: float
    type: str
    action_id: str | None
    session: str | None
    payload: dict[str, Any]
    run_id: str | None = None


class Store:
    def __init__(self, path: str | Path) -> None:
        self.path = str(path)
        if self.path != ":memory:":
            Path(self.path).parent.mkdir(parents=True, exist_ok=True)
        self.db = sqlite3.connect(self.path, timeout=5.0, isolation_level=None)
        self.db.execute("pragma journal_mode=wal")
        self.db.executescript(SCHEMA)
        if "run_id" not in {row[1] for row in self.db.execute("pragma table_info(events)")}:
            try:
                self.db.execute(MIGRATE)
            except sqlite3.OperationalError as err:  # another process migrated first
                if "duplicate column" not in str(err):
                    raise
        self.db.execute(INDEXES)

    def close(self) -> None:
        self.db.close()

    def append(self, type_: str, payload: dict[str, Any], *, action_id: str | None = None,
               session: str | None = None, run_id: str | None = None, ts: float | None = None) -> None:
        self.db.execute(
            "insert into events(ts, type, action_id, session, payload, run_id) values (?, ?, ?, ?, ?, ?)",
            (time.time() if ts is None else ts, type_, action_id, session, json.dumps(payload, sort_keys=True),
             run_id),
        )

    def append_all(self, rows: Iterable[tuple[str, dict[str, Any], str | None, float | None]]) -> None:
        """Append (type, payload, run_id, ts) rows in one transaction: all of them or none."""
        encoded = [(time.time() if ts is None else ts, t, json.dumps(p, sort_keys=True), rid)
                   for t, p, rid, ts in rows]
        with self.db:
            self.db.execute("begin")
            self.db.executemany("insert into events(ts, type, payload, run_id) values (?, ?, ?, ?)", encoded)

    def events(self, type_: str | None = None, *, run_id: str | None = None) -> Iterator[Event]:
        q = "select seq, ts, type, action_id, session, payload, run_id from events"
        where: list[str] = []
        args: list[Any] = []
        if type_:
            where.append("type = ?")
            args.append(type_)
        if run_id is not None:
            where.append("run_id = ?")
            args.append(run_id)
        if where:
            q += " where " + " and ".join(where)
        for seq, ts, t, aid, sess, payload, rid in self.db.execute(q + " order by seq", args):
            yield Event(seq, ts, t, aid, sess, json.loads(payload), rid)

    def max_seq(self) -> int:
        return int(self.db.execute("select coalesce(max(seq), 0) from events").fetchone()[0])

    def max_run_seq(self) -> int:
        """The last conductor event: what the review page watches, so the action gate's events leave it be."""
        return int(self.db.execute("select coalesce(max(seq), 0) from events where run_id is not null").fetchone()[0])

    def kinds(self) -> list[str]:
        return sorted(str(r[0]) for r in self.db.execute("select distinct type from events"))

    def runs(self) -> list[str]:
        """Conductor runs in the order they started."""
        rows = self.db.execute("select run_id from events where type = 'RunStarted' order by seq").fetchall()
        return [str(r[0]) for r in rows]

    def unfinished_runs(self) -> list[str]:
        """Conductor runs with no ``RunFinished`` yet, in the order they started: what a queue reads."""
        rows = self.db.execute(
            """
            select s.run_id from events s where s.type = 'RunStarted' and not exists (
              select 1 from events f where f.run_id = s.run_id and f.type = 'RunFinished')
            order by s.seq
            """).fetchall()
        return [str(r[0]) for r in rows]

    # --- projections ---------------------------------------------------------

    def window(self, fingerprint: str) -> Window:
        """Same result as ``window_from(self.events(), fingerprint)``, computed in SQL."""
        row = self.db.execute(
            """
            with first_label as (
              select action_id, json_extract(payload, '$.label') as label
              from events where seq in (
                select min(seq) from events where type = 'HumanLabeled' group by action_id)
            ), last_decision as (
              select action_id, json_extract(payload, '$.fingerprint') as fp
              from events where seq in (
                select max(seq) from events where type = 'GateDecided' group by action_id)
            )
            select
              coalesce(sum(case when l.label in ('correct', 'error') then 1 else 0 end), 0),
              coalesce(sum(case when l.label = 'error' then 1 else 0 end), 0)
            from first_label l join last_decision d on d.action_id = l.action_id
            where d.fp = ?
            """,
            (fingerprint,),
        ).fetchone()
        return Window(int(row[0]), int(row[1]))

    def last_model(self, session: str) -> str | None:
        row = self.db.execute(
            """
            select json_extract(payload, '$.model') from events
            where session = ? and type in ('ActionProposed', 'ModelSeen')
              and json_extract(payload, '$.model') not in ('?', '')
            order by seq desc limit 1
            """,
            (session,),
        ).fetchone()
        return None if row is None else str(row[0])

    def pending_audits(self, session: str) -> list[tuple[str, str]]:
        """Asked actions in a session that have no label yet: (action_id, transcript path)."""
        rows = self.db.execute(
            """
            select d.action_id, json_extract(p.payload, '$.transcript')
            from events d join events p on p.action_id = d.action_id and p.type = 'ActionProposed'
            where d.type = 'GateDecided' and d.session = ? and json_extract(d.payload, '$.verdict') = 'ask'
              and not exists (select 1 from events l where l.type = 'HumanLabeled' and l.action_id = d.action_id)
            """,
            (session,),
        ).fetchall()
        return [(str(a), str(t or "")) for a, t in rows]


def window_from(events: Iterable[Event], fingerprint: str) -> Window:
    """Fold: labels on decisions taken under ``fingerprint``. Pure, used for replay."""
    decided: dict[str, str] = {}
    labels: dict[str, str] = {}
    for e in events:
        if e.type == "GateDecided" and e.action_id:
            decided[e.action_id] = str(e.payload.get("fingerprint"))
        elif e.type == "HumanLabeled" and e.action_id and e.action_id not in labels:
            labels[e.action_id] = str(e.payload.get("label"))
    n = k = 0
    for aid, label in labels.items():
        if decided.get(aid) != fingerprint:
            continue
        if label == "correct":
            n += 1
        elif label == "error":
            n += 1
            k += 1
    return Window(n, k)
