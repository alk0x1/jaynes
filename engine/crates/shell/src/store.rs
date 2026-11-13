//! The record in SQLite: one writer, every change of a run in one transaction, synced to disk before it returns,
//! and each entry chained to the one before it.

use jaynes_core::{chain, Digest, Event, Run};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::Value;
use std::path::Path;

const SCHEMA: &str = "
create table if not exists events (
  seq integer primary key,
  run text not null,
  type text not null,
  ts integer not null,
  body text not null,
  hash text not null
);
create index if not exists events_run on events(run);
create table if not exists objects (digest text primary key, body text not null);
create table if not exists runs (run text primary key, setup text not null);
create table if not exists inbox (id integer primary key, run text not null, input text not null);
create table if not exists displays (item text primary key, body text not null);
";

/// Seconds since 1970, as the record keeps time.
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub struct Store {
    db: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
        }
        let db = Connection::open(path).map_err(|e| format!("cannot open the record {}: {e}", path.display()))?;
        db.busy_timeout(std::time::Duration::from_secs(10)).map_err(|e| e.to_string())?;
        // An answer and the intent before a step that cannot be undone must survive a power cut.
        db.execute_batch("pragma journal_mode=wal; pragma synchronous=full;").map_err(|e| e.to_string())?;
        db.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        Ok(Store { db })
    }

    /// Append a run's events, all or none, each chained to the last entry of the record.
    #[cfg(test)]
    pub fn append(&mut self, run: &str, events: &[Event]) -> Result<(), String> {
        if events.is_empty() {
            return Ok(());
        }
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        Self::append_in(&tx, run, events)?;
        tx.commit().map_err(|e| e.to_string())
    }

    /// Read the run's state, decide, and record what was decided, all in one transaction: no other process can
    /// record between the reading and the writing.
    pub fn decide(&mut self, run: &str, input: &jaynes_core::Input) -> Result<(Run, Vec<Event>, Vec<jaynes_core::Command>), String> {
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        let events = Self::events_in(&tx, run)?;
        let state = Run::fold(&events);
        let out = jaynes_core::decide(state.as_ref(), input).map_err(|r| r.0)?;
        Self::append_in(&tx, run, &out.events)?;
        let mut after = state;
        for e in &out.events {
            after = match after {
                None => Run::started(e),
                Some(mut r) => {
                    r.apply(e);
                    Some(r)
                }
            };
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok((after.ok_or("the run has no state")?, out.events, out.commands))
    }

    fn events_in(db: &Connection, run: &str) -> Result<Vec<Event>, String> {
        let mut q = db.prepare("select body from events where run = ?1 order by seq").map_err(|e| e.to_string())?;
        let rows = q.query_map([run], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for body in rows {
            let body = body.map_err(|e| e.to_string())?;
            out.push(serde_json::from_str(&body).map_err(|e| format!("an entry of {run} does not read: {e}"))?);
        }
        Ok(out)
    }

    fn append_in(tx: &Connection, run: &str, events: &[Event]) -> Result<(), String> {
        if events.is_empty() {
            return Ok(());
        }
        let last: Option<String> = tx
            .query_row("select hash from events order by seq desc limit 1", [], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        let mut previous = last.map(Digest).unwrap_or_else(chain::genesis);
        let ts = now();
        for e in events {
            let body = serde_json::to_string(e).map_err(|e| e.to_string())?;
            // The time is part of what the chain fingerprints: a stop's count reads it.
            let hash = chain::link(&previous, &format!("{ts}\n{body}"));
            tx.execute("insert into events(run, type, ts, body, hash) values (?1, ?2, ?3, ?4, ?5)", params![run, e.name(), ts as i64, body, hash.0])
                .map_err(|e| e.to_string())?;
            previous = hash;
        }
        Ok(())
    }

    pub fn events(&self, run: &str) -> Result<Vec<Event>, String> {
        let mut q = self.db.prepare("select body from events where run = ?1 order by seq").map_err(|e| e.to_string())?;
        let rows = q.query_map([run], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for body in rows {
            let body = body.map_err(|e| e.to_string())?;
            out.push(serde_json::from_str(&body).map_err(|e| format!("an entry of {run} does not read: {e}"))?);
        }
        Ok(out)
    }

    /// Every event of every run, with its run and when it was recorded: what a stop's count reads.
    pub fn history(&self) -> Result<Vec<(u64, String, Event)>, String> {
        let mut q = self.db.prepare("select ts, run, body from events order by seq").map_err(|e| e.to_string())?;
        let rows = q.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            let (ts, run, body) = row.map_err(|e| e.to_string())?;
            out.push((ts as u64, run, serde_json::from_str(&body).map_err(|e| e.to_string())?));
        }
        Ok(out)
    }

    /// When each run's first and last entries were recorded.
    pub fn times(&self) -> Result<std::collections::BTreeMap<String, (u64, u64)>, String> {
        let mut q = self.db.prepare("select run, min(ts), max(ts) from events group by run").map_err(|e| e.to_string())?;
        let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64, r.get::<_, i64>(2)? as u64))).map_err(|e| e.to_string())?;
        rows.map(|r| r.map(|(run, a, b)| (run, (a, b))).map_err(|e| e.to_string())).collect()
    }

    pub fn state(&self, run: &str) -> Result<Option<Run>, String> {
        Ok(Run::fold(&self.events(run)?))
    }

    /// The last entry of the record: what the page watches to know something happened.
    pub fn last_seq(&self) -> Result<i64, String> {
        self.db.query_row("select coalesce(max(seq), 0) from events", [], |r| r.get(0)).map_err(|e| e.to_string())
    }

    pub fn runs(&self) -> Result<Vec<String>, String> {
        let mut q = self.db.prepare("select run from runs order by rowid").map_err(|e| e.to_string())?;
        let rows = q.query_map([], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
        rows.map(|r| r.map_err(|e| e.to_string())).collect()
    }

    /// The first entry of the whole record, counting from 0, that does not follow from those before it.
    pub fn verify(&self) -> Result<Option<(u64, String)>, String> {
        let mut q = self.db.prepare("select seq, ts, body, hash from events order by seq").map_err(|e| e.to_string())?;
        let rows: Vec<(u64, String, Digest)> = q
            .query_map([], |r| Ok((r.get::<_, i64>(0)? as u64, format!("{}\n{}", r.get::<_, i64>(1)?, r.get::<_, String>(2)?), Digest(r.get(3)?))))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        let broken = chain::first_break(rows.iter().map(|(_, b, h)| (b.as_str(), h)));
        Ok(broken.map(|n| (rows[n].0, rows[n].1.chars().take(120).collect())))
    }

    pub fn put(&self, value: &Value) -> Result<Digest, String> {
        let digest = Digest::of_json(value);
        let body = serde_json::to_string(value).map_err(|e| e.to_string())?;
        self.db
            .execute("insert or ignore into objects(digest, body) values (?1, ?2)", params![digest.0, body])
            .map_err(|e| e.to_string())?;
        Ok(digest)
    }

    pub fn get(&self, digest: &Digest) -> Result<Value, String> {
        let body: Option<String> = self
            .db
            .query_row("select body from objects where digest = ?1", [&digest.0], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        let body = body.ok_or_else(|| format!("the record has no object {}", digest.short()))?;
        serde_json::from_str(&body).map_err(|e| e.to_string())
    }

    pub fn put_setup(&self, run: &str, setup: &Value) -> Result<(), String> {
        let body = serde_json::to_string(setup).map_err(|e| e.to_string())?;
        self.db.execute("insert into runs(run, setup) values (?1, ?2)", params![run, body]).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn setup(&self, run: &str) -> Result<Value, String> {
        let body: Option<String> = self
            .db
            .query_row("select setup from runs where run = ?1", [run], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        let body = body.ok_or_else(|| format!("there is no run {run}"))?;
        serde_json::from_str(&body).map_err(|e| e.to_string())
    }

    /// How an item is shown beyond its label: its detail and its pictures, by the item's fingerprint.
    pub fn put_display(&self, item: &Digest, display: &Value) -> Result<(), String> {
        let body = serde_json::to_string(display).map_err(|e| e.to_string())?;
        self.db
            .execute("insert or replace into displays(item, body) values (?1, ?2)", params![item.0, body])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn display(&self, item: &Digest) -> Value {
        self.db
            .query_row("select body from displays where item = ?1", [&item.0], |r| r.get::<_, String>(0))
            .ok()
            .and_then(|b| serde_json::from_str(&b).ok())
            .unwrap_or(Value::Null)
    }

    /// Leave an input for the process that drives the run.
    pub fn inbox_put(&self, run: &str, input: &jaynes_core::Input) -> Result<(), String> {
        let body = serde_json::to_string(input).map_err(|e| e.to_string())?;
        self.db.execute("insert into inbox(run, input) values (?1, ?2)", params![run, body]).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Take the oldest input waiting for the run.
    pub fn inbox_take(&mut self, run: &str) -> Result<Option<jaynes_core::Input>, String> {
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        let row: Option<(i64, String)> = tx
            .query_row("select id, input from inbox where run = ?1 order by id limit 1", [run], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((id, body)) = row else { return Ok(None) };
        tx.execute("delete from inbox where id = ?1", [id]).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        serde_json::from_str(&body).map(Some).map_err(|e| e.to_string())
    }

    #[cfg(test)]
    pub fn tamper(&self, seq: u64, body: &str) {
        let _changed = self.db.execute("update events set body = ?1 where seq = ?2", params![body, seq as i64]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jaynes_core::question::By;

    #[test]
    fn a_changed_entry_shows() {
        let dir = std::env::temp_dir().join(format!("jaynes-store-{}", std::process::id()));
        let _gone = std::fs::remove_dir_all(&dir);
        let mut s = Store::open(&dir.join("record.sqlite3")).expect("open");
        let e = vec![Event::Halted { by: By::Person }, Event::Resumed { by: By::Person }];
        s.append("r", &e).expect("append");
        s.append("r", &e).expect("append");
        assert_eq!(s.verify().expect("verify"), None);
        s.tamper(2, r#"{"type":"Resumed","by":{"by":"agent","who":"x"}}"#);
        assert_eq!(s.verify().expect("verify").map(|(n, _)| n), Some(2));
        s.tamper(2, r#"{"type":"Resumed","by":{"by":"person"}}"#);
        assert_eq!(s.verify().expect("verify"), None);
        let _changed = s.db.execute("update events set ts = 0 where seq = 3", []);
        assert_eq!(s.verify().expect("verify").map(|(n, _)| n), Some(3));
    }
}
