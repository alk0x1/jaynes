//! `jaynes`: the shell around the core, on the command line.

use jaynes_shell::{home, agent, conformance, declaration, drive, export, index, install, listing, mcp, page, permit, project, project_prefs, protocol, role, store, summary, toml_role};

use drive::{say, Driver, Setup};
use jaynes_core::question::{Answer, By, Verdict};
use jaynes_core::risk::Risk;
use jaynes_core::run::{Phase, Status};
use jaynes_core::{decide, Input, Manifest};
use protocol::{Body, Call};
use role::RoleProgram;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use store::Store;

const USAGE: &str = "jaynes: the redesigned Jaynes engine

  start --role NAME --request TEXT [--title WORDS] [--project DIR] [--agent claude|claude:MODEL|codex:MODEL|command:CMD, or JAYNES_AGENT] [--risk high] [--run ID]
        [--task ID] [--arm NAME]   in an evaluation: the task's id in your own list and the arm, letters, digits and hyphens
        [--never-merge]   in an evaluation: deliver the run's branch only, whatever is answered
  continue RUN            go on after a stop of the process, or after a block
  version, --version      the version, the commit it is built from, and the engine
  show RUN                where the run is, and what it asks you
  answer RUN QUESTION [ITEM=keep|skip|drop|change:TEXT|redo:TEXT ...] [--context TEXT] [--send]
  raise RUN high|--seconds N|--tokens N
  halt RUN | resume RUN | abandon RUN
  undo RUN [--note TEXT]   revert what a run merged
  correct RUN --note TEXT [--step STEP]  record that a delivery was wrong, so the rework it caused is counted
  runs | cost | verify | log RUN
  summary RUN             what a finished run did, in Markdown, to paste in a merge request or a note
  summary RUN --json [--without-words]   the same as fields, for an auditor's tools: what the record holds, what it does not;
                          with the person's words (the request, their notes) unless --without-words
  verify-summary FILE     make a --json summary again from the record and name every field the file says otherwise
  export --numbers [--since YYYY-MM-DD]   each finished run's numbers as JSON lines, and no word of the record: task, arm,
                          minutes of presence, stops, result, the agents' minutes and tokens (the day in UTC)
  export --evaluation RUN  the defects planted in a run, as JSON: who planted each, where, the note, the patch, who took
                          it out, and the answer at the stop that showed it. For the evaluator, never the person who reviews
  plant RUN --step STEP --patch FILE --note TEXT --by NAME   in an evaluation, at a terminal: plant a defect into a
                          done step's change while its stop waits; nothing is delivered while it is in
  unplant RUN ID --by NAME   take a planted defect out, at a terminal: the step's change is again the one it was
  page [--port N]         the page where you answer, at http://localhost:8750 (or the port given; 0: one the system gives, said in its first line)
  open [--port N]         start the page if it is not running and open it in your browser, with its key
  add NAME|SOURCE | remove NAME | roles   install a role (by name from the index, a folder, or URL#path), remove one, list them
  index check DIR [--write]   check an index: every role's content and promises, proved again
  guide                   how an agent carries out a change through Jaynes
  update                  install the newest Jaynes and update roles installed from git
  uninstall [--yes] [--keep-data|--delete-data]   remove what the installer put here, after showing it; says what stays in your repositories
  mcp                     the server agents in Claude Code use (the plugin starts it)
  conformance DIR         check a role against the protocol
  new NAME [DIR]          write a starting role.toml, with the menu in its comments
  check DIR               read a role in TOML strictly, every error with its line, and prove its promises
  project ports DIR       whether a project's checks reach any local port of this machine, and since when
  project open-ports DIR | project close-ports DIR   let them, or not: the person's alone, at a terminal
  project test-servers DIR [PORT...]  let a project's tests start their own test servers (macOS), and with PORT use
                            one Jaynes holds for databases and caches; the person's alone; close-ports closes it
  schema                  print the protocol's schema

The record lives in $JAYNES_ENGINE_HOME (default ~/.jaynes/engine).";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn library() -> PathBuf {
    install::library(&home())
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

/// A flag whose value is a label (`jaynes_core::io::is_label`), never a title.
fn label(args: &[String], name: &str) -> Result<Option<String>, String> {
    match flag(args, name) {
        Some(l) if !jaynes_core::io::is_label(&l) => Err(format!("{name} is a label: 1 to 64 letters, digits and hyphens, the first a letter or a digit, such as T-12; never a title")),
        given => Ok(given),
    }
}

/// `jaynes uninstall [--yes] [--keep-data | --delete-data]`: show what goes and what stays, then remove it on a yes.
fn uninstall(rest: &[String]) -> Result<(), String> {
    let at = jaynes_shell::uninstall::Places::from_env()?;
    let plan = jaynes_shell::uninstall::plan(&at);
    for line in jaynes_shell::uninstall::describe(&plan) {
        println!("{line}");
    }
    if !plan.blockers.is_empty() {
        for b in &plan.blockers {
            println!("{b}");
        }
        return Err("Nothing was removed.".into());
    }
    let has = |f: &str| rest.iter().any(|a| a == f);
    let tty = std::io::stdin().is_terminal();
    let ask = |question: &str| -> bool {
        print!("{question} [y/N] ");
        let _shown = std::io::Write::flush(&mut std::io::stdout());
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer).is_ok() && matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
    };
    if !has("--yes") && !(tty && ask("Remove all of this?")) {
        return Err(if tty { "Nothing was removed.".into() } else { "Nothing was removed: run jaynes uninstall --yes to remove it, or answer at a terminal.".into() });
    }
    let delete_data = if has("--delete-data") {
        true
    } else if has("--keep-data") || plan.data.is_empty() || !tty || has("--yes") {
        false
    } else {
        ask("Also delete the data folder, with the record of every run?")
    };
    for line in jaynes_shell::uninstall::apply(&plan, delete_data) {
        println!("{line}");
    }
    if !delete_data && !plan.data.is_empty() {
        println!("The data folder was kept; jaynes uninstall --yes --delete-data, or deleting it yourself, removes it.");
    }
    for line in jaynes_shell::uninstall::after(&plan) {
        println!("{line}");
    }
    Ok(())
}

fn store() -> Result<Store, String> {
    Store::open(&home().join("record.sqlite3"))
}

/// Who is answering: the person at a terminal, or a program, which the core refuses.
fn who() -> By {
    if std::io::stdin().is_terminal() { By::Person } else { By::Agent { who: "a program without a terminal".into() } }
}

/// This program again, with `args` and the variables of [`drive::RUN_ENV`] as `kept` says (those it holds set, the
/// others unset), in this process's place: what comes back is why it could not.
fn with_env(args: &[String], kept: &std::collections::BTreeMap<String, String>) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => return e,
    };
    let mut again = std::process::Command::new(exe);
    again.args(args).env_remove(drive::START_ENV);
    for k in drive::RUN_ENV {
        match kept.get(*k) {
            Some(v) => again.env(k, v),
            None => again.env_remove(k),
        };
    }
    again.exec()
}

/// The agent's process an input comes from, when an agent sends it: the core matches it with the run's starter.
fn starter_of(by: &By) -> Option<String> {
    (!by.person()).then(jaynes_shell::starter::of_this_process).flatten()
}

fn run(args: &[String]) -> Result<(), String> {
    let Some(cmd) = args.first().map(String::as_str) else {
        println!("{USAGE}");
        return Ok(());
    };
    let rest = &args[1..];
    // A command on a run of another engine is that engine's: this process becomes it.
    if matches!(cmd, "continue" | "answer" | "raise" | "halt" | "resume" | "abandon" | "undo" | "correct")
        && let Some(id) = rest.first()
        && let Ok(Some(state)) = store().and_then(|s| s.state(id))
        && state.engine != jaynes_core::ENGINE
    {
        use std::os::unix::process::CommandExt;
        let exe = drive::engine_exe(&home(), &state.engine)
            .ok_or_else(|| format!("run {id} started on engine {}, which is no longer on this machine: this one ({}) cannot carry it on", state.engine, jaynes_core::ENGINE))?;
        return Err(format!("the engine of run {id} did not start: {}", std::process::Command::new(exe).args(args).exec()));
    }
    // Every `jaynes` the person runs at a terminal keeps that terminal's environment, for the runs that start without
    // one; the install keeps it too, with `keep-env`.
    let terminal = std::io::stdin().is_terminal();
    if terminal || cmd == "keep-env" {
        drive::keep_terminal_env(&home());
    }
    if cmd == "keep-env" {
        return Ok(());
    }
    // A command that carries a run on runs with the environment the run started with (where the person's toolchains
    // are), whatever process gave it: the page and the app may have been opened with a short PATH. This process
    // becomes itself with it.
    if matches!(cmd, "continue" | "answer" | "raise" | "halt" | "resume" | "abandon" | "undo" | "correct" | "plant" | "unplant")
        && let Some(id) = rest.first()
        && let Some(kept) = store().and_then(|s| s.setup(id)).ok().as_ref().and_then(drive::run_env)
        && !drive::env_is(&kept)
    {
        return Err(format!("Jaynes could not carry run {id} on with the environment it started with: {}", with_env(args, &kept)));
    }
    // The app's start says it is the app's (START_ENV): it starts with the environment of the person's terminal, and
    // keeps it from there as any run. The mark only chooses the environment, so a forged one does no harm. Any other
    // start keeps its own: an agent's may be in a direnv or a PATH of the project that the terminal does not know.
    if cmd == "start"
        && std::env::var(drive::START_ENV).is_ok_and(|v| v == "terminal")
        && let Some(kept) = drive::terminal_env(&home())
        && !drive::env_is(&kept)
    {
        return Err(format!("Jaynes could not start the run with the environment of your terminal: {}", with_env(args, &kept)));
    }
    match cmd {
        "--version" | "version" => {
            println!("{}", install::version());
            Ok(())
        }
        "start" => start(rest),
        "role-serve" => {
            // the program of every role written in TOML: one request in, one reply out
            let dir = rest.first().ok_or("which role?")?;
            let library = flag(rest, "--library").map(PathBuf::from).unwrap_or_else(library);
            toml_role::serve(Path::new(dir), &library);
            Ok(())
        }
        "new" => {
            let name = rest.first().ok_or("jaynes new NAME [DIR]")?;
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                return Err(format!("{name} is not a role's name: letters, digits, - and _"));
            }
            let dir = PathBuf::from(rest.get(1).cloned().unwrap_or_else(|| name.clone()));
            if dir.join(toml_role::FILE).exists() {
                return Err(format!("{} already has a role", dir.display()));
            }
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            std::fs::write(dir.join(toml_role::FILE), toml_role::template(name)).map_err(|e| e.to_string())?;
            // a check of the role's own with the cases it comes with, to start from
            for (path, text, runs) in toml_role::scaffold() {
                let at = dir.join(path);
                if let Some(folder) = at.parent() {
                    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
                }
                std::fs::write(&at, text).map_err(|e| e.to_string())?;
                if runs {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&at, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
                }
            }
            println!("wrote {}: write the method in it, then `jaynes check {}` until every line is ✔", dir.join(toml_role::FILE).display(), dir.display());
            Ok(())
        }
        "check" => {
            let dir = rest.first().ok_or("which role's folder?")?;
            match toml_role::TomlRole::load(Path::new(dir)) {
                Ok(role) => {
                    println!("✔ {} {} reads: {}", role.manifest.role, role.manifest.version, role.manifest.steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(" → "));
                    // what each step does to the world as resolved, and each way the project's content can reach the network
                    for line in role.effects().into_iter().chain(role.flows_told()).chain(role.claims_listed()).chain(role.requires_listed()) {
                        println!("  {line}");
                    }
                    // what the author should know and is no error: a key that limits nothing, a step that makes every run risky
                    for note in role.notes() {
                        println!("! {note}");
                    }
                    // each check held to the cases it comes with, before anything is proved of the method
                    let (lines, held) = role.run_cases(&library());
                    for l in &lines {
                        println!("{l}");
                    }
                    let (lines, all) = role.prove();
                    for l in &lines {
                        println!("{l}");
                    }
                    match (held, all) {
                        (true, true) => Ok(()),
                        (false, _) => Err("a check of the role does not hold to its cases".into()),
                        (true, false) => Err("a promise of the role does not hold".into()),
                    }
                }
                Err(errors) => Err(format!("✘ the role does not read:\n{}", errors.iter().map(|e| format!("  {e}")).collect::<Vec<_>>().join("\n"))),
            }
        }
        "act" => {
            // an act handed over by another engine, on a run of this one: JSON in, JSON out
            let id = rest.first().ok_or("which run?")?;
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).map_err(|e| e.to_string())?;
            let act: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            println!("{}", drive::act_here(&home(), &exe, id, &act));
            Ok(())
        }
        "continue" => {
            let id = rest.first().ok_or("which run?")?;
            let mut s = store()?;
            let setup = Setup::load(&s, &home(), id)?;
            let state = s.state(id)?.ok_or("no such run")?;
            let input = if state.blocked() {
                let by = who();
                let starter = starter_of(&by);
                Input::Continue { by, starter }
            } else {
                Input::Recovered
            };
            let mut d = Driver::take(&mut s, setup)?;
            let first = if matches!(input, Input::Continue { .. }) {
                d.drive(Input::Recovered)?;
                input
            } else {
                input
            };
            let end = d.drive(first)?;
            print_state(&end);
            Ok(())
        }
        "show" => {
            let id = rest.first().ok_or("which run?")?;
            let s = store()?;
            let state = s.state(id)?.ok_or("no such run")?;
            print_state(&state);
            let chosen = state.ports.or_else(|| {
                let setup = s.setup(id).ok()?;
                let any = drive::ports_open(&setup);
                let servers = drive::servers_open(&setup);
                Some(jaynes_core::run::Ports { any: any.is_some(), servers: servers.is_some(), opened: any.or(servers).unwrap_or(0), freed_ports: drive::freed_open(&setup) })
            });
            if let Some(said) = chosen.as_ref().and_then(chosen_sentence) {
                println!("{said}");
            }
            Ok(())
        }
        "project" => project_cmd(rest),
        "answer" => answer(rest),
        "raise" | "halt" | "resume" | "abandon" => {
            let id = rest.first().ok_or("which run?")?;
            let s = store()?;
            let state = s.state(id)?.ok_or("no such run")?;
            let input = match cmd {
                "halt" => Input::Halt { by: who() },
                "resume" => Input::Resume { by: who() },
                "abandon" => {
                    let by = who();
                    let starter = starter_of(&by);
                    Input::Abandon { by, starter }
                }
                _ => {
                    if rest.get(1).map(String::as_str) == Some("high") {
                        Input::RaiseRisk { to: Risk::High, by: who() }
                    } else {
                        let mut c = state.ceiling;
                        if let Some(n) = flag(rest, "--seconds") {
                            c.seconds = n.parse().map_err(|_| "--seconds takes a number")?;
                        }
                        if let Some(n) = flag(rest, "--tokens") {
                            c.tokens = n.parse().map_err(|_| "--tokens takes a number")?;
                        }
                        Input::RaiseCeiling { ceiling: c, by: who() }
                    }
                }
            };
            send(s, id, input)
        }
        "undo" => {
            let id = rest.first().ok_or("which run?")?;
            let note = flag(rest, "--note").unwrap_or_default();
            let mut s = store()?;
            let reverted = drive::undo(&mut s, &home(), id, note, who())?;
            say(&format!("undone: {reverted}"));
            Ok(())
        }
        "ask" => {
            // the person asks a stop's agent about what it shows: a turn that only reads, which redoes nothing
            let id = rest.first().ok_or("which run?")?;
            let question = rest.get(1).ok_or("which question?")?.clone();
            let text = flag(rest, "--text").ok_or("--text WORDS says what you ask")?;
            let input = Input::AskAgent { question, item: flag(rest, "--item"), text, by: who() };
            let s = store()?;
            let before = s.state(id)?.ok_or("no such run")?;
            let asked = before.steps.iter().map(|x| x.thread.len()).sum::<usize>();
            send(s, id, input)?;
            let after = store()?.state(id)?.ok_or("no such run")?;
            let said: Vec<&jaynes_core::run::Entry> = after.steps.iter().flat_map(|x| &x.thread).collect();
            for e in said.iter().skip(asked).filter(|e| e.kind != jaynes_core::run::EntryKind::Ask) {
                match e.kind {
                    jaynes_core::run::EntryKind::AskFailed => say(&format!("no answer: {}", e.text)),
                    _ => say(&format!("the agent answered, nothing redone:\n{}", e.text)),
                }
            }
            Ok(())
        }
        "correct" => {
            let id = rest.first().ok_or("which run?")?;
            let note = flag(rest, "--note").ok_or("--note TEXT says what was wrong")?;
            let step = flag(rest, "--step");
            let mut s = store()?;
            let (_, events, _) = drive::decide_and_record(&mut s, id, &Input::Correct { note, by: who(), step })?;
            say(&format!("recorded: {} event", events.len()));
            Ok(())
        }
        "runs" => {
            let s = store()?;
            // each run by its name, as the page and the app show it: its project, its title, and what tells it apart
            let rows = listing::read(&home())?;
            // and how it stands, in the words the app and the page say it (listing::said), never the phase alone
            let named: BTreeMap<String, &listing::Row> = rows.needs.iter().chain(rows.projects.iter().flat_map(|(_, r)| r)).map(|r| (r.id.clone(), r)).collect();
            for id in s.runs()? {
                if let Some(r) = s.state(&id)? {
                    let (state, name) = named.get(&id).map(|row| (listing::said(row), listing::name(row).join(" · "))).unwrap_or_else(|| (phase(&r.phase), String::new()));
                    println!("{id}  {}  {state}  {name}", r.manifest.role);
                }
            }
            Ok(())
        }
        "summary" => {
            let id = rest.iter().find(|a| !a.starts_with("--")).ok_or("which run?")?;
            if rest.iter().any(|a| a == "--json") {
                let words = !rest.iter().any(|a| a == "--without-words");
                let d = declaration::declare(&home(), id, words)?;
                println!("{}", serde_json::to_string_pretty(&d).map_err(|e| e.to_string())?);
                if words {
                    say(declaration::HOLDS_WORDS);
                }
                return Ok(());
            }
            // the role's promises are proved again before it answers: once per role, and minutes for a large one
            print!("{}", summary::read(&home(), id, true)?.markdown());
            Ok(())
        }
        "verify-summary" => {
            let path = rest.first().ok_or("which file? one jaynes summary RUN --json wrote")?;
            let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            let file: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{path} is not JSON: {e}"))?;
            let differ = declaration::differences(&home(), &file)?;
            if differ.is_empty() {
                println!(
                    "the summary is what the record says: run {}, entries {} to {} of the record, its chain whole",
                    file["run"]["id"].as_str().unwrap_or(""), file["record"]["first"], file["record"]["last"]
                );
                return Ok(());
            }
            for d in &differ {
                println!("{d}");
            }
            Err(format!("{} field{} of the file {} not what the record says", differ.len(), if differ.len() == 1 { "" } else { "s" }, if differ.len() == 1 { "is" } else { "are" }))
        }
        "export" => {
            if let Some(id) = flag(rest, "--evaluation") {
                let events = store()?.events(&id)?;
                if events.is_empty() {
                    return Err(format!("there is no run {id}"));
                }
                println!("{}", jaynes_shell::planted::export(&home().join("runs").join(&id), &events));
                return Ok(());
            }
            if !rest.iter().any(|a| a == "--numbers") {
                return Err("export what? --numbers: each finished run's numbers, as JSON lines, with no word of the record; --evaluation RUN: the defects planted in a run".into());
            }
            let since = flag(rest, "--since").map(|d| export::day(&d)).transpose()?;
            let (lines, unfinished) = export::export(&home(), since)?;
            for l in &lines {
                println!("{}", serde_json::to_string(l).map_err(|e| e.to_string())?);
            }
            if unfinished > 0 {
                say(&format!("{unfinished} run{} not finished {} left out: a run's numbers come once it is finished", if unfinished == 1 { "" } else { "s" }, if unfinished == 1 { "is" } else { "are" }));
            }
            Ok(())
        }
        "cost" => cost(),
        "verify" => {
            let s = store()?;
            match s.verify()? {
                None => {
                    println!("the record holds: every entry follows from the ones before it");
                    Ok(())
                }
                Some((seq, body)) => Err(format!("the record was changed at entry {seq}: {body}")),
            }
        }
        "log" => {
            let id = rest.first().ok_or("which run?")?;
            // an evaluation's planted defects are never read by the person who reviews: the export has them
            for e in store()?.events(id)?.iter().filter(|e| !jaynes_shell::planted::hidden(e)) {
                println!("{}", serde_json::to_string(&e).map_err(|e| e.to_string())?);
            }
            Ok(())
        }
        "plant" => {
            let id = rest.first().ok_or("which run?")?;
            let step = flag(rest, "--step").ok_or("--step STEP: the done step whose change the defect goes into")?;
            let patch = flag(rest, "--patch").ok_or("--patch FILE: the defect, as a patch of the project")?;
            let note = flag(rest, "--note").ok_or("--note TEXT: what the defect is, for the evaluator")?;
            let planter = flag(rest, "--by").ok_or("--by NAME: who plants it")?;
            let mut s = store()?;
            let setup = Setup::load(&s, &home(), id)?;
            let mut d = Driver::take(&mut s, setup)?;
            let defect = d.plant(&step, Path::new(&patch), &note, &planter, who())?;
            say(&format!("planted {defect} in {step}: nothing is delivered until `jaynes unplant {id} {defect} --by NAME`"));
            Ok(())
        }
        "unplant" => {
            let id = rest.first().ok_or("which run?")?;
            let defect = rest.get(1).filter(|a| !a.starts_with("--")).ok_or("which defect? its id, from jaynes export --evaluation")?;
            let remover = flag(rest, "--by").ok_or("--by NAME: who takes it out")?;
            let mut s = store()?;
            let setup = Setup::load(&s, &home(), id)?;
            let mut d = Driver::take(&mut s, setup)?;
            let end = d.unplant(defect, &remover, who())?;
            say(&format!("{defect} taken out"));
            print_state(&end);
            Ok(())
        }
        "conformance" => {
            let dir = rest.first().ok_or("which role folder?")?;
            let findings = conformance::run(&RoleProgram::new(Path::new(dir), &library()));
            let failed = findings.iter().filter(|f| !f.ok).count();
            for f in &findings {
                println!("{} {}{}", if f.ok { "ok  " } else { "FAIL" }, f.case, if f.ok || f.detail.is_empty() { String::new() } else { format!(": {}", f.detail) });
            }
            if failed > 0 { Err(format!("{failed} of {} cases failed", findings.len())) } else { Ok(()) }
        }
        "open" => {
            let port = match rest {
                [] => install::PORT,
                [f, p] if f == "--port" => p.parse().map_err(|_| "--port takes a number")?,
                _ => return Err(format!("jaynes open takes only --port N, and {} was given\nusage: jaynes open [--port N]", rest.join(" "))),
            };
            // what keeps tests and agents from opening a browser keeps this one from it too, and says so
            for told in ["JAYNES_NO_PAGE", "JAYNES_NO_BROWSER"] {
                if std::env::var_os(told).is_some() {
                    return Err(format!("{told} is set here: the page's link was not opened"));
                }
            }
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let address = install::ensure_page_on(&home(), port, &exe, true)?;
            println!("the page is open in your browser: {address}");
            Ok(())
        }
        "page" => {
            let port = flag(rest, "--port").map(|p| p.parse().map_err(|_| "--port takes a number")).transpose()?.unwrap_or(8750);
            let key = match std::env::var("JAYNES_PAGE_KEY") {
                Ok(k) => k,
                Err(_) => install::page_key(&home())?,
            };
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            page::Page { home: home(), port, key, exe }.serve()
        }
        "permit" => {
            let need = |name: &str| flag(rest, name).ok_or(format!("{name} is needed"));
            permit::Permit {
                home: home(),
                run: need("--run")?,
                step: need("--step")?,
                attempt: need("--attempt")?.parse().map_err(|_| "--attempt takes a number")?,
                cwd: PathBuf::from(need("--cwd")?),
            }
            .serve()
        }
        "index" => {
            // what the index's CI runs: every entry's content and promises, checked again
            if rest.first().map(String::as_str) != Some("check") {
                return Err("jaynes index check DIR [--write]".into());
            }
            let dir = rest.get(1).ok_or("which index's folder?")?;
            let said = index::check(Path::new(dir), rest.iter().any(|a| a == "--write"));
            match said {
                Ok(lines) => {
                    println!("{}", lines.join("\n"));
                    Ok(())
                }
                Err(lines) => Err(lines),
            }
        }
        "add" => {
            let source = rest.first().ok_or("which role? a name from the index, a folder, or URL#path/in/the/repository")?;
            let name = install::add(&home(), source)?;
            say(&format!("role {name} installed"));
            // a role that is a program runs with the person's rights, outside any box: they are told so
            if let Ok(dir) = install::find_role(&home(), &name)
                && !dir.join(toml_role::FILE).is_file()
            {
                say(&format!("{name} is a program, not a role.toml: it is code of whoever wrote it and runs with your rights, outside any box; only a role in role.toml keeps the commands it runs on your project in Jaynes's box"));
            }
            Ok(())
        }
        "remove" => {
            let name = rest.first().ok_or("which role?")?;
            install::remove(&home(), name)?;
            say(&format!("role {name} removed; runs that started with it keep their own copy"));
            Ok(())
        }
        "roles" => {
            let roles = install::list(&home());
            if roles.is_empty() {
                println!("no role is installed: `jaynes guide` says how to install the ones that come with Jaynes");
            }
            for (name, version, about, origin) in roles {
                println!("{name} {version}  {about}{}", if origin.is_empty() { String::new() } else { format!("  (from {origin})") });
            }
            Ok(())
        }
        "guide" => {
            print!("{}", install::GUIDE);
            Ok(())
        }
        "uninstall" => uninstall(rest),
        "update" => {
            // a command that acts runs only as it is written: anything after it, --help too, is refused
            if !rest.is_empty() {
                return Err(format!("jaynes update takes nothing after it, and {} was given: nothing was updated\nusage: jaynes update", rest.join(" ")));
            }
            for line in install::update(&home())? {
                say(&line);
            }
            Ok(())
        }
        // The Python engine's plugin recorded answers from Claude Code's question dialog here; this engine asks on
        // its page, so there is nothing to record.
        "answered" => {
            let _drained = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
            Ok(())
        }
        "mcp" => mcp::serve(&home()),
        "schema" => {
            println!("{}", serde_json::to_string_pretty(&protocol::schema()).map_err(|e| e.to_string())?);
            Ok(())
        }
        _ => Err(format!("unknown command {cmd}\n\n{USAGE}")),
    }
}

fn start(args: &[String]) -> Result<(), String> {
    let role_given = flag(args, "--role").ok_or("--role NAME (or a role's folder) is needed")?;
    let role_dir = install::find_role(&home(), &role_given)?.display().to_string();
    let request = flag(args, "--request").ok_or("--request TEXT is needed")?;
    let project = flag(args, "--project").map(|p| std::fs::canonicalize(&p).map_err(|e| format!("{p}: {e}"))).transpose()?;
    // a role that does not fit the project says why before anything of a run exists
    if let Some(p) = &project {
        toml_role::fits_or_says(Path::new(&role_dir), &library(), p)?;
    }
    let agent = flag(args, "--agent").or_else(|| std::env::var("JAYNES_AGENT").ok()).unwrap_or_else(|| "claude".into());
    drive::known_agent(&agent)?;
    // the model is fixed for the run here, so every process that carries the run on uses the same one
    let agent = if agent == "claude" { format!("claude:{}", agent::Claude::default().model) } else { agent };
    let raise = match flag(args, "--risk").as_deref() {
        Some("high") => Some(Risk::High),
        Some("low") | None => None,
        Some(other) => return Err(format!("--risk is low or high, not {other}")),
    };
    // what a run carries of an evaluation leaves the machine in the export of numbers: labels only, said before
    // anything of a run exists
    let task = label(args, "--task")?;
    let arm = label(args, "--arm")?;
    let never_merge = args.iter().any(|a| a == "--never-merge");
    // the agent above this start, unless the person starts it at a terminal; one started for the page or an agent
    // through the plugin says it (none for the page's)
    let starter = match flag(args, "--starter").as_deref() {
        Some("none") => None,
        Some(given) => Some(given.to_string()),
        None if std::io::stdin().is_terminal() => None,
        None => jaynes_shell::starter::of_this_process(),
    };
    // the short name the run is shown by, kept as the start gave it, cut at a word past 40
    let title = flag(args, "--title").map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).map(|t| listing::run_title(Some(&t), ""));
    let id = flag(args, "--run").unwrap_or_else(|| {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        format!("r{t}-{}", std::process::id() % 1000)
    });
    let runs = home().join("runs");
    let dir = runs.join(&id);
    if dir.exists() {
        // a start that died before the run was recorded left its folder whole (it is moved in only once complete):
        // the run never started
        if !store()?.events(&id)?.is_empty() {
            return Err(format!("run {id} exists"));
        }
        std::fs::remove_dir_all(&dir).map_err(|e| format!("Jaynes could not clear what a start that died left in {}: {e}", dir.display()))?;
    }
    // The run's folder is made aside and moved into place whole: a start killed while it copies leaves children
    // (git, the role) still writing, and they write only into a folder nothing reads.
    clear_unfinished_starts(&runs);
    let building = runs.join(format!(".building-{id}-{}", std::process::id()));
    let _gone = std::fs::remove_dir_all(&building);
    let given = RoleProgram::new(Path::new(&role_dir), &library());
    // the fingerprint of the copy the run runs, never of the folder, which may change before the copy is made
    let (role, fingerprint) = given.pinned(&building)?;
    match role.call(Call::Hello { engine: jaynes_core::ENGINE.into(), features: protocol::FEATURES.iter().map(|s| s.to_string()).collect(), extensions: Default::default() })? {
        Body::Hello { required, .. } => {
            let unknown: Vec<String> = required.into_iter().filter(|r| !protocol::FEATURES.contains(&r.as_str())).collect();
            if !unknown.is_empty() {
                return Err(format!("the role needs features this engine lacks: {}", unknown.join(", ")));
            }
        }
        other => return Err(format!("the role answered hello with {other:?}")),
    }
    let manifest: Manifest = match role.call(Call::Describe {})? {
        Body::Manifest { manifest } => manifest,
        other => return Err(format!("the role answered describe with {other:?}")),
    };
    // the program that starts the run carries it on to the end, after any update
    drive::keep_engine(&home())?;
    let branch = format!("jaynes/{id}");
    if let Some(p) = &project {
        for w in project::warnings(p) {
            say(&format!("note: {w}"));
        }
        project::make(manifest.target, p, &building.join("project"), &branch)?;
    }
    drop(role);
    std::fs::rename(&building, &dir).map_err(|e| format!("Jaynes could not put the run's folder in place: {e}"))?;
    let mut s = store()?;
    // Each stop's count, from everything the record holds, as it stands now.
    let history = s.history()?;
    let mut attention = BTreeMap::new();
    for (i, step) in manifest.steps.iter().enumerate() {
        let Some(stop) = &step.stop else { continue };
        let key = jaynes_core::attention::stop_key(&manifest, &fingerprint, i);
        let lineage = jaynes_core::attention::lineage(&manifest, i);
        let c = jaynes_core::attention::count(&history, &key, &lineage, &agent, store::now(), stop.budget.map(|b| b.0), jaynes_core::DELTA);
        attention.insert(step.name.clone(), c);
    }
    // the project's open ports as the person left them now; a continue reads them again, and the record says so
    let ports = match &project {
        Some(p) => project_prefs::load(&home(), p)?.ports.filter(|p| p.any || p.servers),
        None => None,
    };
    s.put_setup(&id, &json!({
        "role_given": role_dir, "fingerprint": fingerprint, "agent": agent, "target": manifest.target,
        "project": project.as_ref().map(|p| p.display().to_string()), "branch": project.as_ref().map(|_| branch.clone()),
        "title": title,
        "ports": ports.as_ref().map(|p| json!({"any": p.any, "servers": p.servers, "opened": p.opened, "freed_ports": p.freed_ports})),
        "env": drive::started_env(),
    }))?;
    if let Some(said) = ports.as_ref().and_then(|p| chosen_sentence(&p.to_run())) {
        say(&said);
    }
    drive::sweep_engines(&home(), &s)?;
    drive::sweep_copies(&home(), &s)?;
    let setup = Setup::load(&s, &home(), &id)?;
    let mut d = Driver::take(&mut s, setup)?;
    say(&format!("run {id}: what needs the person is on the page, {}", install::ensure_page(&home())));
    let end = d.drive(Input::Start { run: id.clone(), request, manifest, role_fingerprint: fingerprint, agent, raise, attention, task, arm, never_merge, starter })?;
    print_state(&end);
    Ok(())
}

/// What a run whose project the person opened says about it, before it starts and when it is shown.
fn ports_sentence(opened: u64) -> String {
    format!("this project's checks reach any local port of this machine: opened by you on {}", summary::utc(opened))
}

/// What a run whose project's tests may start their own test servers says about it, before it starts and when it is
/// shown.
fn servers_sentence(opened: u64, freed: &[u16]) -> String {
    format!("this project's tests may start their own test servers: allowed by you on {}{}", summary::utc(opened), listing::freed_said(freed))
}

/// What the person's choice for a project's ports says, when they chose one.
fn chosen_sentence(p: &jaynes_core::run::Ports) -> Option<String> {
    if p.any {
        Some(ports_sentence(p.opened))
    } else if p.servers {
        Some(servers_sentence(p.opened, &p.freed_ports))
    } else {
        None
    }
}

/// `jaynes project ports|open-ports|close-ports DIR`. Opening and closing are the person's, at a terminal: an
/// agent's shell has none, and the agent's box cannot write the engine's home anyway.
fn project_cmd(args: &[String]) -> Result<(), String> {
    let (Some(what), Some(dir)) = (args.first(), args.get(1)) else {
        return Err("jaynes project ports|open-ports|test-servers|close-ports DIR".into());
    };
    let dir = std::fs::canonicalize(dir).map_err(|e| format!("{dir}: {e}"))?;
    let mut prefs = project_prefs::load(&home(), &dir)?;
    match what.as_str() {
        "ports" => {
            match prefs.ports.as_ref().and_then(|p| chosen_sentence(&p.to_run())) {
                Some(said) => println!("{said}"),
                None => println!("this project's checks reach only the ports a run reserves for them (JAYNES_PORTS) and the ones a step declares"),
            }
            Ok(())
        }
        "test-servers" => {
            if let By::Agent { who } = who() {
                return Err(format!("only the person lets a project's tests start their own test servers, at a terminal; {who} may not"));
            }
            if !cfg!(target_os = "macos") {
                return Err("test servers are a choice of macOS's box; here `jaynes project open-ports` opens a project's ports".into());
            }
            let name = dir.display();
            let ports = args[2..].iter().map(|p| p.parse::<u16>().map_err(|_| format!("{p} is not a port"))).collect::<Result<Vec<u16>, String>>()?;
            if !ports.is_empty() {
                // a port Jaynes holds for the project's own test servers, let to its tests: one of those it holds, and
                // only once they are let
                if let Some(p) = ports.iter().find(|p| !listing::HELD_PORTS.iter().any(|(h, _)| h == *p)) {
                    return Err(format!("Jaynes holds only the ports of the usual databases and caches while the checks run ({}); {p} is not one of them", listing::held_ports_said()));
                }
                let Some(chosen) = prefs.ports.as_mut().filter(|p| p.servers) else {
                    return Err(format!("Jaynes holds these ports only while a project's own test servers run: `jaynes project test-servers {name}` lets them first"));
                };
                println!("Tests of {name} may use {}, which Jaynes holds while the checks run so no test stands in for {}.", listing::ports_named(&ports), listing::held_kind(&ports));
                println!("{}", listing::freed_risk(&ports));
                print!("Allow this for {name}? [y/N] ");
                std::io::Write::flush(&mut std::io::stdout()).map_err(|e| e.to_string())?;
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer).map_err(|e| e.to_string())?;
                if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
                    println!("Nothing changed.");
                    return Ok(());
                }
                let mut freed: std::collections::BTreeSet<u16> = chosen.freed_ports.iter().copied().collect();
                freed.extend(ports);
                chosen.freed_ports = freed.into_iter().collect();
                project_prefs::save(&home(), &dir, &prefs)?;
                println!("Done. Go back to the run and press Continue.");
                return Ok(());
            }
            println!("Tests of {name} may start their own test servers, as when you run them yourself.");
            println!("They still cannot reach your services or the Jaynes page.");
            println!("Still open while the tests run:");
            for line in listing::test_servers_still_open() {
                println!("  · {line}");
            }
            print!("Allow this for {name}? [y/N] ");
            std::io::Write::flush(&mut std::io::stdout()).map_err(|e| e.to_string())?;
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer).map_err(|e| e.to_string())?;
            if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
                println!("Nothing changed.");
                return Ok(());
            }
            let any = prefs.ports.as_ref().is_some_and(|p| p.any);
            let opened = prefs.ports.as_ref().filter(|p| p.any).map(|p| p.opened).unwrap_or_else(store::now);
            let freed_ports = prefs.ports.as_ref().filter(|p| p.servers).map(|p| p.freed_ports.clone()).unwrap_or_default();
            prefs.ports = Some(project_prefs::Ports { any, servers: true, opened, freed_ports });
            project_prefs::save(&home(), &dir, &prefs)?;
            println!("Done. Go back to the run and press Continue.");
            Ok(())
        }
        "open-ports" | "close-ports" => {
            if let By::Agent { who } = who() {
                return Err(format!("only the person opens or closes a project's local ports, at a terminal; {who} may not"));
            }
            let open = what == "open-ports";
            // closing closes both choices, the open ports and the test servers: one way to close
            let servers = open && prefs.ports.as_ref().is_some_and(|p| p.servers);
            let freed_ports = if servers { prefs.ports.as_ref().map(|p| p.freed_ports.clone()).unwrap_or_default() } else { Vec::new() };
            prefs.ports = Some(project_prefs::Ports { any: open, servers, opened: if open { store::now() } else { 0 }, freed_ports });
            project_prefs::save(&home(), &dir, &prefs)?;
            println!("{}", if open {
                format!("{}; a run started from now on, and a blocked one when you continue it, lets its checks reach every service listening here, the Jaynes page's port among them. `jaynes project close-ports {}` closes them again", ports_sentence(store::now()), dir.display())
            } else {
                "closed: this project's checks reach only the ports a run reserves for them and the ones a step declares, and its tests start no test servers of their own".to_string()
            });
            Ok(())
        }
        other => Err(format!("jaynes project ports|open-ports|test-servers|close-ports DIR, not {other}")),
    }
}

/// Remove the folders of starts that died while they made them, once nothing can still be writing in them.
fn clear_unfinished_starts(runs: &Path) {
    let old = |p: &Path| p.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() > 600);
    for e in std::fs::read_dir(runs).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().starts_with(".building-") && old(&e.path()) {
            let _gone = std::fs::remove_dir_all(e.path());
        }
    }
}

fn parse_verdict(s: &str) -> Result<Verdict, String> {
    let (kind, text) = s.split_once(':').map(|(k, t)| (k, t.to_string())).unwrap_or((s, String::new()));
    Ok(match kind {
        "keep" => Verdict::Keep,
        "skip" => Verdict::Skip,
        "drop" => Verdict::Drop,
        "change" => Verdict::Change { text },
        "redo" => Verdict::Redo { note: text },
        other => return Err(format!("{other} is not a verdict: keep, skip, drop, change:TEXT or redo:TEXT")),
    })
}

fn answer(args: &[String]) -> Result<(), String> {
    let id = args.first().ok_or("which run?")?;
    let qid = args.get(1).ok_or("which question?")?;
    let s = store()?;
    let state = s.state(id)?.ok_or("no such run")?;
    let q = state.questions.get(qid).ok_or_else(|| format!("run {id} has no question {qid}"))?.question.clone();
    let mut verdicts = BTreeMap::new();
    for a in args[2..].iter().filter(|a| a.contains('=') && !a.starts_with("--")) {
        let (item, v) = a.split_once('=').unwrap_or((a, "keep"));
        verdicts.insert(item.to_string(), parse_verdict(v)?);
    }
    // a stop that chooses: --choose ITEM, and --adjust TEXT for how to adjust it; the other options leave
    if let Some(chosen) = flag(args, "--choose") {
        if !q.items.iter().any(|x| x.id == chosen) {
            return Err(format!("the question has no option {chosen}: {}", q.items.iter().map(|x| x.id.as_str()).collect::<Vec<_>>().join(", ")));
        }
        let adjust = flag(args, "--adjust").unwrap_or_default();
        verdicts = q
            .items
            .iter()
            .map(|x| {
                let v = match (x.id == chosen, adjust.trim().is_empty()) {
                    (false, _) => Verdict::Drop,
                    (true, true) => Verdict::Keep,
                    (true, false) => Verdict::Change { text: adjust.clone() },
                };
                (x.id.clone(), v)
            })
            .collect();
    }
    // finalists: --finalists A,B to have the agent develop them and ask again; none: --none WHY
    if let Some(names) = flag(args, "--finalists") {
        let names: Vec<&str> = names.split(',').map(str::trim).collect();
        verdicts = q.items.iter().map(|x| {
            let v = if names.contains(&x.id.as_str()) { Verdict::Change { text: jaynes_core::decide::SHORTLISTED.into() } } else { Verdict::Drop };
            (x.id.clone(), v)
        }).collect();
    }
    let why = flag(args, "--none");
    if why.is_some() {
        verdicts = q.items.iter().map(|x| (x.id.clone(), Verdict::Drop)).collect();
    }
    // the merge is one answer about every file it shows: a file not named is kept, as at any stop, and one dropped
    // takes the change as a branch only, as what this answer will do says before it is sent
    if q.kind == jaynes_core::question::QuestionKind::Merge {
        for x in &q.items {
            verdicts.entry(x.id.clone()).or_insert(Verdict::Keep);
        }
    }
    let context = [why.unwrap_or_default(), flag(args, "--context").unwrap_or_default()].iter().filter(|t| !t.trim().is_empty()).cloned().collect::<Vec<_>>().join(" ");
    let answer = Answer { back: Default::default(), question: q.id.clone(), shown: q.shown(), verdicts, by: who(), context };
    let effects = drive::effects(&home(), &state, &q, &answer)?;
    println!("What this answer will do:");
    for line in &effects.sentences {
        println!("  {line}");
    }
    if !args.iter().any(|a| a == "--send") {
        println!("Nothing is sent until you add --send.");
        return Ok(());
    }
    if let Err(r) = decide(Some(&state), &Input::Answer { answer: answer.clone() }) {
        return Err(format!("refused: {r}"));
    }
    send(s, id, Input::Answer { answer })
}

/// Give the run an input: drive it here if no process does, or leave it in the inbox for the one that does.
fn send(mut s: Store, id: &str, input: Input) -> Result<(), String> {
    let state = s.state(id)?.ok_or("no such run")?;
    if let Err(r) = decide(Some(&state), &input) {
        return Err(format!("refused: {r}"));
    }
    // A leave is answered while the agent waits inside a turn: it is recorded at once, for the leave server.
    if let Input::Answer { answer } = &input
        && state.questions.get(&answer.question).is_some_and(|q| q.question.kind == jaynes_core::question::QuestionKind::Permission) {
            s.decide(id, &input)?;
            say("recorded; the agent waiting for it gets it now");
            return Ok(());
        }
    let setup = Setup::load(&s, &home(), id)?;
    match Driver::take(&mut s, setup) {
        Ok(mut d) => {
            let end = d.drive(input)?;
            print_state(&end);
        }
        Err(_) => {
            s.inbox_put(id, &input)?;
            // Abandoning or halting stops the agent at once, after the input is left for the run's process.
            drive::stop_agent_for(&home().join("runs").join(id), &input);
            say("recorded for the process that drives the run");
        }
    }
    Ok(())
}

fn phase(p: &Phase) -> String {
    match p {
        Phase::Running => "running".into(),
        Phase::Waiting { why, .. } => format!("waiting: {why}"),
        Phase::Blocked { fault, reason, .. } => format!("blocked ({fault:?}): {reason}"),
        Phase::Finished { outcome } => format!("finished: {outcome:?}").to_lowercase(),
    }
}

fn print_state(r: &jaynes_core::Run) {
    println!("run {} — {} {} — {} risk", r.id, r.manifest.role, r.manifest.version, r.risk.word());
    let aside = " (abandoned by the agent that started it: its work is kept; resume brings it back, abandon ends it)";
    println!("{}{}", phase(&r.phase), if r.set_aside.is_some() { aside } else if r.halted { " (halted)" } else { "" });
    println!(
        "spent {} of {} s, {} of {} tokens ({} read from cache)",
        r.spent.seconds, r.ceiling.seconds, r.spent.tokens, r.ceiling.tokens, r.spent.cached
    );
    let path = r.path();
    for (i, s) in r.steps.iter().enumerate() {
        let d = &r.manifest.steps[i];
        let mut mark = if !path.contains(&i) {
            "left out at this risk".to_string()
        } else {
            match &s.status {
                Status::Done if r.fresh(i) => "done".into(),
                Status::Done => "to run again".into(),
                other => format!("{other:?}").to_lowercase(),
            }
        };
        // G14: what acts on something real and stopped in the middle is said, whatever the run did after
        if s.maybe_acted && s.status != Status::Done {
            mark.push_str(", may have acted: look at what it acts on before you trust either way");
        }
        println!("  {:<16} {:<8} round {}  {mark}", d.name, format!("{:?}", d.executor).to_lowercase(), s.round);
    }
    for q in r.open_questions() {
        let q = &q.question;
        println!("? {} at {}{}", q.id, q.step, if q.holds { ", the run waits for it" } else { ", the run goes on" });
        for item in &q.items {
            println!("    {} — {}", item.id, item.label);
        }
        let offered: Vec<String> = q.offered.iter().map(|v| format!("{v:?}").to_lowercase()).collect();
        println!("    verdicts: {}", offered.join(", "));
    }
}

fn cost() -> Result<(), String> {
    let s = store()?;
    let mut by: BTreeMap<String, (u32, u64, u64, u32, u32, u32)> = BTreeMap::new();
    for id in s.runs()? {
        let Some(r) = s.state(&id)? else { continue };
        let Phase::Finished { outcome } = r.phase else { continue };
        let e = by.entry(format!("{} {:?}", r.risk.word(), outcome).to_lowercase()).or_default();
        e.0 += 1;
        e.1 += r.spent.seconds;
        e.2 += r.spent.tokens;
        e.3 += r.counts.sent_back;
        e.4 += r.counts.corrections;
        e.5 += r.counts.spent;
    }
    println!("path, runs, minutes, tokens, sent back, corrected after delivery, attempts the agent spent");
    for (k, (n, sec, tok, back, corr, spent)) in by {
        println!("{k}: {n} runs, {} min, {tok} tokens, {back} sent back, {corr} corrected, {spent} spent", sec / 60);
    }
    Ok(())
}
