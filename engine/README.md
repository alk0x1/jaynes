# The redesigned engine

A small core in Rust that is a pure function, a shell around it, and roles outside it behind a fixed protocol.
The design and what this slice leaves out: `docs/specs/2026-11-12-engine-redesign-design.md`. What the core
promises: `docs/guarantees.md`.

```
cargo test                      # the core's guarantees on random runs, and the engine end to end
cargo build
JAYNES_PYTHON=../.venv/bin/python PYTHONPATH=../src \
  ./target/debug/jaynes start --role crates/shell/tests/roles/small-change --project ~/some/repo --request "say hi"
./target/debug/jaynes show RUN
./target/debug/jaynes answer RUN QUESTION item=keep --send
./target/debug/jaynes verify
./target/debug/jaynes conformance crates/shell/tests/roles/small-change
./target/debug/jaynes check crates/shell/tests/roles/fix-bug        # reads the role strictly and proves its promises
```

| Folder | What |
|---|---|
| `crates/core` | `decide(state, input) -> (events, commands)`; no disk, network, process or clock |
| `crates/core/tests/guarantees.rs` | one test per guarantee, on thousands of random runs |
| `crates/shell` | `jaynes`: the record in SQLite, the role's process, the agent program, the project copy |
| `protocol/role-protocol-1.schema.json` | the protocol's schema, generated (`jaynes schema`) and checked by a test |
| `crates/core/src/promise.rs` | a role's promises, proved by exploring every way its runs go through `decide` |
| `crates/shell/src/toml_role.rs` | a role written as `role.toml`: read strictly, served over the protocol (`jaynes role-serve`) |
| `python/jaynes_menu.py` | the menu of steps and checks a TOML role picks by name, run in the box of `jaynes_box.py` |
| `python/jaynes_role.py` | the small library for writing a role in Python |
| `crates/shell/tests/roles` | the roles the tests run: copies of the official ones, which live with the index in [jaynes-roles](https://github.com/alk0x1/jaynes-roles) |
| `crates/shell/src/index.rs` | the index: `jaynes add NAME`, and `jaynes index check` for its CI |

The record lives in `$JAYNES_ENGINE_HOME` (default `~/.jaynes/engine`), apart from the Python engine's.
