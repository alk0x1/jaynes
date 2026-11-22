//! Who started a run, when an agent did: the nearest process above this one that is an agent program, named by its
//! pid and its start time, so a pid the system gives again to another process is not taken for it. The core lets
//! that agent continue a block of Jaynes's that only judges the same change again, and set its run aside; a run
//! with no agent above its start is the person's alone.
//!
//! This is no secret: any program of the person's can read the process table, as it can write the record. It tells
//! apart the agents that keep to Jaynes's commands, which is what it is for. The agent Jaynes starts inside a run
//! is itself the nearest agent above whatever it runs, so it never passes for the one that started the run.

/// The agent programs, by the name the process table gives them: Claude Code's and Codex's command-line programs.
/// A program not named here is not taken for an agent, and the run it starts is the person's alone: the safe side,
/// since an agent's run then only lacks what its agent may do. A new agent program goes here when Jaynes runs it.
pub const AGENT_PROGRAMS: [&str; 2] = ["claude", "codex"];

/// How many parents up the walk looks before it gives up.
const DEPTH: usize = 64;

/// One process as `ps -o pid=,ppid=,lstart=,comm=` shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    /// When it started, as `ps` writes it ("Sat Nov 21 20:03:39 2026").
    pub started: String,
    /// The program's name, without its folder.
    pub program: String,
}

/// The agent above this process, as the core's `starter` takes it; None when there is none, or `ps` cannot say.
pub fn of_this_process() -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,lstart=,comm="])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    nearest_agent(&table(&String::from_utf8_lossy(&out.stdout)), std::process::id())
}

/// The process table `ps` wrote; a line it cannot read is left out.
pub fn table(text: &str) -> Vec<Proc> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let pid = words.next()?.parse().ok()?;
            let ppid = words.next()?.parse().ok()?;
            let started: Vec<&str> = words.by_ref().take(5).collect();
            if started.len() < 5 {
                return None;
            }
            let comm = words.collect::<Vec<_>>().join(" ");
            let program = comm.rsplit('/').next().unwrap_or("").to_string();
            (!program.is_empty()).then(|| Proc { pid, ppid, started: started.join(" "), program })
        })
        .collect()
}

/// The nearest process above `from`, not `from` itself, that is an agent program: "<program>:<pid>@<start>".
pub fn nearest_agent(table: &[Proc], from: u32) -> Option<String> {
    let by_pid: std::collections::BTreeMap<u32, &Proc> = table.iter().map(|p| (p.pid, p)).collect();
    let mut at = by_pid.get(&from)?.ppid;
    for _ in 0..DEPTH {
        if at <= 1 {
            return None;
        }
        let p = by_pid.get(&at)?;
        if AGENT_PROGRAMS.contains(&p.program.as_str()) {
            return Some(format!("{}:{}@{}", p.program, p.pid, p.started));
        }
        at = p.ppid;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS: &str = "    1     0 Sat Nov 21 01:51:00 2026     /sbin/launchd
  500     1 Sat Nov 21 09:00:00 2026     /Applications/herdr.app/Contents/MacOS/herdr-core
  600   500 Sat Nov 21 09:00:01 2026     /bin/zsh
  700   600 Sat Nov 21 09:00:02 2026     claude
  800   700 Sat Nov 21 10:00:00 2026     /Users/x/.jaynes/bin/jaynes
  900   800 Sat Nov 21 10:00:05 2026     /usr/bin/sandbox-exec
  950   900 Sat Nov 21 10:00:06 2026     claude
  960   950 Sat Nov 21 10:01:00 2026     /bin/sh
  970   960 Sat Nov 21 10:01:01 2026     /Users/x/.jaynes/bin/jaynes
  980   700 Sat Nov 21 11:00:00 2026     /Applications/My Tools/bin/codex
  990   980 Sat Nov 21 11:00:01 2026     jaynes
 1000   600 Sat Nov 21 12:00:00 2026     jaynes
 bad line
";

    #[test]
    fn the_run_an_agent_starts_names_that_agent_by_pid_and_start() {
        let t = table(PS);
        assert_eq!(nearest_agent(&t, 800).as_deref(), Some("claude:700@Sat Nov 21 09:00:02 2026"));
    }

    #[test]
    fn the_agent_jaynes_starts_inside_a_run_is_the_nearest_agent_of_what_it_runs_never_the_outer_one() {
        // the outer claude (700) started the run (800); its agent (950) calls jaynes (970) from a shell
        let t = table(PS);
        assert_eq!(nearest_agent(&t, 970).as_deref(), Some("claude:950@Sat Nov 21 10:00:06 2026"));
        assert_ne!(nearest_agent(&t, 970), nearest_agent(&t, 800));
    }

    #[test]
    fn the_nearest_agent_wins_and_a_folder_with_spaces_is_read() {
        let t = table(PS);
        assert_eq!(nearest_agent(&t, 990).as_deref(), Some("codex:980@Sat Nov 21 11:00:00 2026"));
    }

    #[test]
    fn with_no_agent_above_nobody_but_the_person_started_it() {
        let t = table(PS);
        assert_eq!(nearest_agent(&t, 1000), None, "a terminal of the person's");
        assert_eq!(nearest_agent(&t, 700), None, "an agent itself is not above itself");
        assert_eq!(nearest_agent(&t, 4242), None, "a process the table does not hold");
    }

    #[test]
    fn a_pid_given_again_is_another_starter() {
        let again = PS.replace("  700   600 Sat Nov 21 09:00:02 2026", "  700   600 Sun Nov 22 08:00:00 2026");
        assert_ne!(nearest_agent(&table(&again), 800), nearest_agent(&table(PS), 800));
    }

    #[test]
    fn a_loop_in_the_table_ends() {
        let t = table("  10    20 Sat Nov 21 09:00:00 2026     sh\n  20    10 Sat Nov 21 09:00:00 2026     sh\n");
        assert_eq!(nearest_agent(&t, 10), None);
    }

    #[test]
    fn this_process_reads_the_real_table() {
        // whatever is above the test, the walk ends; above it an agent program or none
        if let Some(s) = of_this_process() {
            assert!(AGENT_PROGRAMS.iter().any(|p| s.starts_with(&format!("{p}:"))), "{s}");
        }
    }
}
