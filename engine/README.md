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
| `python/jaynes_menu.py` | the menu of steps and checks a TOML role picks by name, run in the box of `jaynes_box.py`; what its checks of quotes against sources prove and do not: `docs/source-checks.md` |
| `python/jaynes_role.py` | the small library for writing a role in Python |
| `crates/shell/tests/roles` | the roles the tests run: copies of the official ones, which live with the index in [jaynes-roles](https://github.com/alk0x1/jaynes-roles) |
| `crates/shell/src/index.rs` | the index: `jaynes add NAME`, and `jaynes index check` for its CI |

The record lives in `$JAYNES_ENGINE_HOME` (default `~/.jaynes/engine`), apart from the Python engine's.

## The run's copy of a repository

A run works in a copy of the person's repository, in its own folder (`runs/RUN/project`). The engine's git for it is
a repository beside the copy (`runs/RUN/project.git`), named explicitly in every command, with hooks and the file
monitor off and none of the person's own or the system's git configuration: nothing an agent writes in the copy, its
`.git` included, is run by the engine. The copy's `.git` is the agent's, for git status and git diff. So:

- a step creates or changes no submodule: a change that leaves a repository nested in the copy is refused;
- files kept in git-lfs are pointers in the copy, and submodules come without their content: the start says so;
- `.gitattributes`, `.gitmodules` and `.lfsconfig`, in any folder, are on the floor of the risk, since the person's own
  git obeys them when the delivery lands.
- the agent's git in the copy reads its history (the engine's objects are readable to it, never writable) and none of
  the person's git configuration; what it commits there is the agent's, as "Jaynes agent", and never the delivery;
- a run started before 0.2.2 goes on in the engine it started with (kept in `engines/`), with that engine's copy.

