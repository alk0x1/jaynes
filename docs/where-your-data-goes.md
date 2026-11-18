# Where your code and your data go

Each sentence here is read from the engine's code as it is today, not from a plan. What this page does not promise
is said at the end, so nobody reads more into it.

## What stays on your machine

- **The record.** Every run, step, answer and cost is written to one SQLite file, `record.sqlite3`, in the engine's
  folder: `~/.jaynes/engine`, or the folder `JAYNES_ENGINE_HOME` names.
- **The copies of your project.** Each run works in its own copy of your project, under
  `~/.jaynes/engine/runs/<run>/project`, never in your folder. A finished run keeps its copy for a week (or
  `JAYNES_KEEP_COPIES_SECONDS`); then what each step changed is saved as a diff in `runs/<run>/changes` and the copy
  is deleted. The caches the toolchains filled during a run (Go's builds and modules, npm's packages) are cleared
  when it finishes.
- **The roles and their proofs.** Installed roles live in `~/.jaynes/engine/roles`; the promises proved of each role
  are kept in `~/.jaynes/engine/proofs`.
- **The page.** The page where you answer is a server on `127.0.0.1`, on this machine only. It counts an answer as
  yours only from a browser holding its key: a random key made on this machine, kept in `~/.jaynes/engine/page.key`.
- **The app.** The desktop app talks to the page on `127.0.0.1` and to herdr-core, which keeps your terminals on this
  machine. Its settings are a file in `~/.jaynes`.

## What the engine reaches on the network by itself

The engine has no network library of its own. It reaches the network only by running `git`, `curl` and `gh`, and
only for these:

- **Checking for a newer version.** While the page runs, it asks the published repository for its newest commit,
  with `git ls-remote`, once an hour, so it can offer an update. `JAYNES_NO_UPDATE_CHECK` turns this off.
- **Installing a role by name.** `jaynes add NAME`, and updating roles installed from git, clone the role index (a
  public git repository; `JAYNES_INDEX` names another) and the role's own repository with `git clone`.
- **Installing a role from a URL.** `jaynes add URL#path` clones that repository.
- **Updating.** `jaynes update` reads the newest release's manifest and runs its installer with `curl`. Where no
  release is built for the machine, it builds the newest source with `cargo install --git`. It updates the Claude
  Code plugin through Claude Code's own `claude plugin` commands.
- **The guard's owner.** The guard that asks before what cannot be undone in git learns whose repositories pass
  freely: the owners you set, or else your GitHub login, asked once of `gh api user` and saved in
  `~/.jaynes/guard.json`.
- **The page's fonts.** The page's own HTML loads its two fonts from `fonts.googleapis.com`: your browser fetches
  them when it opens the page.

## No telemetry

The engine sends no usage data, crash report or analytics anywhere. Nothing in its code sends anything beyond the
list above.

## What goes to the model's provider

Jaynes talks to no model by itself. Each step runs the agent program you already use, Claude Code, as a command on
your machine, and that program sends to its provider what it sends for any session of yours: the step's prompt,
the files it reads in the run's copy, and its tool results. When you answer a stop in your own words, the page asks
the run's model to read them, through the same program, with no tools and no project: your words and the stop go
to the provider. A run driven by a command of yours sends what that command sends.

Inside a run, the agent program does not read your instruction files (`CLAUDE.md` and its settings): Jaynes starts it
with them turned off and writes the settings of each step itself.

## What a step with network reaches

A role declares, per step, what the step may reach on the network:

- **None** (the default): the agent's tools that read the web are denied, and its commands run in a sandbox with no
  network.
- **Web**: the agent program's own web tools (WebFetch, WebSearch) are allowed; its commands still have no network.
- **Packages**: its commands may reach only the package registries (npm, Yarn, Go's proxy and checksum database,
  PyPI and its files, crates.io), and nothing else.

Jaynes sets these as the agent program's own guards, for each step: they also keep a step's commands from reading
your home folder beyond the run's copy, the folders the role names and the toolchains' folders. Those guards are
partial (in Claude Code the sandbox covers shell commands only); [what the engine guarantees](guarantees.md#what-this-page-does-not-promise)
says how far they go.

The checks a role runs with your project's commands (its tests, its app) run in a box of their own on macOS. It
denies the network except local TCP ports and sends no UDP, and it hides what the engine keeps (the page's key and
log, the record, other runs). **The known limit:** so that a test suite's own servers work, the commands reach any
local TCP port on the machine, so everything else that listens there too: a browser's debugger, a database without
a password, the Jaynes page (without its key, so what comes through it is not you). A command may also serve on
`0.0.0.0` while it runs, where other machines on your network can reach it.

## What this page does not promise

- It says what the engine and the page do. What the agent program, its provider, `git`, `gh`, `cargo` or your
  browser do with what they get is theirs.
- It covers macOS. Elsewhere there is no box yet, and a check runs only if you accept running it unboxed.
- A role is a program you install. This page says what the engine lets each step and each check reach; it cannot
  say what a role you install does beyond that.
