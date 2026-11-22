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
- **The numbers you export.** `jaynes export --numbers` prints each finished run's numbers to your terminal, and
  sends nothing: the task's id and the arm you gave at the start (`--task`, `--arm`, letters, digits and hyphens),
  minutes on the stops' screens, stops, what became of the run, and the agents' minutes and tokens. No request,
  note, name of a step or a file, path or run id is in it. A run started with no task's id is named by a fingerprint
  of its id and a random salt kept in `~/.jaynes/engine/export.salt`, readable by you alone.
- **A defect planted in an evaluation.** `jaynes plant` keeps who planted it, the note and the patch in
  `runs/<run>/evaluation.json`, beside the run and out of its record, and `jaynes export --evaluation RUN` prints
  them to your terminal, with what became of the defect and the answer at the stop that showed it. Nothing is sent;
  the log, the run's screen and its summary never show them.
- **The roles and their proofs.** Installed roles live in `~/.jaynes/engine/roles`; the promises proved of each role
  are kept in `~/.jaynes/engine/proofs`.
- **The page.** The page where you answer is a server on `127.0.0.1`, on this machine only. It counts an answer as
  yours only from a browser holding its key: a random key made on this machine, kept in `~/.jaynes/engine/page.key`.
- **The app.** The desktop app talks to the page on `127.0.0.1` and to herdr-core, which keeps your terminals on this
  machine. Its settings are a file in `~/.jaynes`.
- **The guard.** The guard that asks before what cannot be undone in git reaches no network and writes no file. Its
  switch and the owners whose repositories pass freely are in `~/.jaynes/guard.json`, which only the page's Setup
  writes; when you set no owners, the owner that passes is the GitHub user `gh` is signed in as, read from gh's own
  `hosts.yml`. A command it does not hold runs no program; for one it holds, it asks `git` for the remote's address,
  and reads the project's `.jaynes.toml`, which may make it stricter there and never turns it off.

## What lands in your repository

The commits of a run, the merge included, have Jaynes as author and committer and are not signed; a remote that
requires signed commits or a verified email refuses the push.

## What the engine reaches on the network by itself

The engine's program has no network library. It reaches the network by running `git`, `curl`, `gh` and `cargo`, and
in one step of the menu from its own library, only for these:

- **Checking for a newer version.** While the page runs, it reads the latest release's manifest with `curl`, once
  an hour, so it can offer an update when that release is newer than the program installed. `JAYNES_NO_UPDATE_CHECK`
  turns this off.
- **Installing a role by name.** `jaynes add NAME`, and updating roles installed from git, clone the role index (a
  public git repository; `JAYNES_INDEX` names another) and the role's own repository with `git clone`.
- **Installing a role from a URL.** `jaynes add URL#path` clones that repository.
- **Updating.** `jaynes update` reads the newest release's manifest and runs its installer with `curl`. Where no
  release is built for the machine, it builds the newest source with `cargo install --git`. It updates the Claude
  Code plugin through Claude Code's own `claude plugin` commands.
- **Downloading the pages a step lists.** Only when a role uses the menu's `fetch-pages` step, which must declare
  `network = "web"`. Jaynes itself downloads the pages an earlier step listed, at most 12 a step, and holds each
  address to rules before anything is sent: `https` on port 443 only, no name or password in it, at most 2,048
  characters, and every address its name resolves to on the public internet (not this machine, not a private
  network), checked again at each redirect, up to 5. A page larger than 2 MB is not read, and each page has a
  clock of 60 seconds, in a process of its own. Every address asked and the text downloaded are kept in the
  run's record. When no page of a list answers, it asks `https://example.com/` once, to tell a page that is down
  from a machine that cannot reach the internet.

## No telemetry

The engine sends no usage data, crash report or analytics anywhere. Nothing in its code sends anything beyond the
list above. Nor does the app: by itself it reaches only the page on this machine and herdr-core; it goes out only
when you act (a link you open, a push you ask for in its Changes panel).

## What goes to the model's provider

Jaynes talks to no model by itself. Each step runs the agent program you already use, Claude Code, as a command on
your machine, and that program sends to its provider what it sends for any session of yours: the step's prompt, the
files it reads in the run's copy, and its tool results. When you answer a stop in your own words, on the page or in
the app, Jaynes asks the run's model to read them, through the same program, with no tools and no project: your
words and the stop go to the provider. A run driven by a command of yours sends what that command sends.

Inside a run, the agent program does not read your instruction files (`CLAUDE.md` and its settings): Jaynes starts it
with them turned off and writes the settings of each step itself. It also names the only tools the agent gets:
reading, searching and editing files, the shell when the step runs commands, and the web tools when the step reaches
the web. None of Claude Code's other tools come with them, such as those that send a message to your other Claude
sessions or start work elsewhere, and a turn whose agent program says it runs with a tool beyond these does not count.

## What a step with network reaches

A role declares, per step, what the step may reach on the network:

- **None** (the default): the agent's tools that read the web are denied, and its commands run in a sandbox with no
  network.
- **Web**: the agent program's own web tools (WebFetch, WebSearch) are allowed; its commands still have no network.
- **Packages**: its commands may reach only the package registries (npm, Yarn, Go's proxy and checksum database,
  PyPI and its files, crates.io), and nothing else.

Jaynes sets these as the agent program's own guards, for each step: they also keep a step's commands from reading
your home folder beyond the run's copy, the folders the role names and the toolchains' folders, and from the
temporary folder your Claude Code sessions share (their scratchpads and the output of their background tasks): each
turn of the agent gets one of its own, taken away when it ends. Where you let the project's tests start their own
servers or opened its ports, or where the step names local ports, on macOS the agent's commands may serve on and reach
every local port of this Mac, the Jaynes page's among them. Those guards are partial (in Claude Code the sandbox
covers shell commands only):
[what the engine guarantees](guarantees.md#what-this-page-does-not-promise) says how far they go.

The checks a role runs with your project's commands (its tests, its app) run in a box of their own on macOS. It
denies the network except local TCP ports and sends no UDP, and it hides what the engine keeps (the page's key and
log, the record, other runs). The local ports are a list, closed by default: the commands bind and reach only the
free ports the run reserved for them (handed over in `JAYNES_PORTS`) and the ones the step or the check names in the
role (`local_ports`), never the Jaynes page's; a suite that serves on another port is stopped, and told which port
and how to take one of its own. **The known limits:** where you open one project's checks to every local port
(`jaynes project open-ports`, at a terminal), its commands reach everything that listens on the machine, the Jaynes
page among them (without its key, so what comes through it is not you). A command may also serve on `0.0.0.0` while
it runs, where other machines on your network can reach it.

## What this page does not promise

- It says what the engine and the page do. What the agent program, its provider, `git`, `gh`, `cargo` or your
  browser do with what they get is theirs.
- It covers macOS. Elsewhere there is no box yet, and a check runs only if you accept running it unboxed.
- A role is a program you install. This page says what the engine lets each step and each check reach; it cannot
  say what a role you install does beyond that.
