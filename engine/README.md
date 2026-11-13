# The redesigned engine

A small core in Rust that is a pure function, a shell around it, and roles outside it behind a fixed protocol.
The design and what this slice leaves out: `docs/specs/2026-11-12-engine-redesign-design.md`. What the core
promises: `docs/guarantees.md`.

```
cargo test                      # the core's guarantees on random runs, and the engine end to end
cargo build
JAYNES_PYTHON=../.venv/bin/python PYTHONPATH=../src \
  ./target/debug/jaynes start --role roles/small-change --project ~/some/repo --request "say hi"
./target/debug/jaynes show RUN
./target/debug/jaynes answer RUN QUESTION item=keep --send
./target/debug/jaynes verify
./target/debug/jaynes conformance roles/small-change
```

| Folder | What |
|---|---|
| `crates/core` | `decide(state, input) -> (events, commands)`; no disk, network, process or clock |
| `crates/core/tests/guarantees.rs` | one test per guarantee, on thousands of random runs |
| `crates/shell` | `jaynes`: the record in SQLite, the role's process, the agent program, the project copy |
| `protocol/role-protocol-1.schema.json` | the protocol's schema, generated (`jaynes schema`) and checked by a test |
| `python/jaynes_role.py` | the small library for writing a role in Python |
| `roles/small-change` | the first role on the other side of the protocol |

The record lives in `$JAYNES_ENGINE_HOME` (default `~/.jaynes/engine`), apart from the Python engine's.
