"""The guard against the irreversible: a Claude Code PreToolUse hook shipped with the Jaynes plugin.

It runs in every Claude Code session where the plugin is on, before jaynes may be installed, so it uses only
python3's standard library. It holds git and GitHub actions that cannot be undone, and edits of its own files, by
answering "ask"; everything else gets no answer at all, so Claude Code's own permission flow applies. It never
blocks (never exits 2): in claude -p, where nobody can answer, Claude Code turns the ask into a deny by itself.

Settings: $JAYNES_HOME/guard.json (default ~/.jaynes/guard.json) with "on" and "owners". Repositories on GitHub
whose owner is a free owner pass freely. With no valid owners list, the free owner is the GitHub user gh is signed
in as, read from gh's own hosts file. The guard never goes to the network and never writes a file: where it was not
needed it does nothing, and a command it does not hold runs no program at all.
"""

from __future__ import annotations

import json
import os
import re
import sys

import switch  # beside this file: a project's own switch, .jaynes.toml

HERE = os.path.dirname(os.path.realpath(__file__))
GIT_TIMEOUT = 5.0
OWNER = re.compile(r"[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})")


# --- settings -----------------------------------------------------------------

def settings_path() -> str:
    home = os.environ.get("JAYNES_HOME") or os.path.join(os.path.expanduser("~"), ".jaynes")
    return os.path.join(os.path.expanduser(home), "guard.json")


def read_settings() -> dict[str, object]:
    """The settings file as a dict; {} when it is missing or broken."""
    try:
        with open(settings_path(), encoding="utf-8") as f:
            data = json.load(f)
    except (OSError, ValueError):
        return {}
    return data if isinstance(data, dict) else {}


def is_on(data: dict[str, object]) -> bool:
    on = data.get("on", True)
    return on if isinstance(on, bool) else True


def configured_owners(data: dict[str, object]) -> list[str] | None:
    owners = data.get("owners")
    if isinstance(owners, list) and all(isinstance(o, str) for o in owners):
        return [o for o in owners if o.strip()]
    return None


def run_quiet(argv: list[str], cwd: str | None, timeout: float) -> str | None:
    """stdout of argv, or None if it fails, is missing or takes longer than timeout; it never waits past it."""
    # imported here: subprocess is most of the guard's own start, and most commands need no program run
    import signal
    import subprocess
    env = dict(os.environ, GIT_TERMINAL_PROMPT="0", GH_PROMPT_DISABLED="1", GIT_OPTIONAL_LOCKS="0")
    try:
        proc = subprocess.Popen(argv, cwd=cwd, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL, env=env, start_new_session=True)
    except OSError:
        return None
    try:
        out, _ = proc.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except OSError:
            proc.kill()
        proc.wait()
        return None
    return out.decode(errors="replace") if proc.returncode == 0 else None


def gh_hosts_path() -> str:
    """gh's hosts file, where gh keeps who it is signed in as: $GH_CONFIG_DIR, else $XDG_CONFIG_HOME/gh, else
    ~/.config/gh, as gh itself looks."""
    folder = (os.environ.get("GH_CONFIG_DIR")
              or (os.path.join(os.environ["XDG_CONFIG_HOME"], "gh") if os.environ.get("XDG_CONFIG_HOME") else "")
              or os.path.join(os.path.expanduser("~"), ".config", "gh"))
    return os.path.join(folder, "hosts.yml")


def github_user() -> str | None:
    """The GitHub user gh is signed in as on github.com, read from its hosts file: no network, no program run.
    The file is YAML; only the ``user:`` line of the ``github.com:`` block is read, at that block's first indent."""
    try:
        with open(gh_hosts_path(), encoding="utf-8") as f:
            lines = f.read().splitlines()
    except (OSError, ValueError):
        return None
    inside, indent = False, None
    for line in lines:
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        depth = len(line) - len(line.lstrip(" "))
        if depth == 0:
            inside, indent = line.strip() in ("github.com:", '"github.com":', "'github.com':"), None
            continue
        if not inside:
            continue
        indent = depth if indent is None else indent
        m = re.fullmatch(r"user:\s*[\"']?([^\"'\s#]+)[\"']?\s*(?:#.*)?", line.strip())
        if depth == indent and m and OWNER.fullmatch(m.group(1)):
            return m.group(1)
    return None


def free_owners() -> list[str]:
    """The owners the person set; else the GitHub user gh is signed in as; else none. Nothing is saved."""
    owners = configured_owners(read_settings())
    if owners is not None:
        return owners
    login = github_user()
    return [login] if login else []


# --- shell words --------------------------------------------------------------

OPS = ("&&", "||", ";;", "<<<", "<<-", "<<", ">>", ">|", "&>>", "&>", ">&", "<&", "|&", ";", "|", "&", "(", ")",
       "<", ">", "\n")
SEPARATORS = {"&&", "||", ";;", ";", "|", "&", "|&", "(", ")", "\n", "`"}


def tokens(text: str) -> list[tuple[str, str]]:
    """Shell tokens as ("w", word) or ("op", operator); quotes are removed, heredoc bodies and comments skipped.

    Unquoted $( and backticks count as a command boundary. It is a reading, not a shell: good enough to see
    which commands run, never to run them."""
    out: list[tuple[str, str]] = []
    word: list[str] = []
    have = False  # a word is being built, even an empty quoted one
    heredocs: list[tuple[str, bool]] = []
    want_delim: bool | None = None  # after << or <<-: whether tabs are stripped
    i, n = 0, len(text)

    def end_word() -> None:
        nonlocal have, want_delim
        if have:
            w = "".join(word)
            if want_delim is not None:
                heredocs.append((w, want_delim))
                want_delim = None
            out.append(("w", w))
        word.clear()
        have = False

    while i < n:
        c = text[i]
        if c == "\\" and i + 1 < n:
            if text[i + 1] != "\n":
                word.append(text[i + 1])
                have = True
            i += 2
            continue
        if c == "'":
            j = text.find("'", i + 1)
            j = n if j < 0 else j
            word.append(text[i + 1:j])
            have, i = True, j + 1
            continue
        if c == '"':
            i += 1
            while i < n and text[i] != '"':
                if text[i] == "\\" and i + 1 < n and text[i + 1] in '"\\$`\n':
                    if text[i + 1] != "\n":
                        word.append(text[i + 1])
                    i += 2
                    continue
                word.append(text[i])
                i += 1
            have, i = True, i + 1
            continue
        if c == "#" and not have:
            j = text.find("\n", i)
            i = n if j < 0 else j
            continue
        if c in " \t":
            end_word()
            i += 1
            continue
        if c == "$" and text.startswith("$(", i) and not text.startswith("$((", i):
            end_word()
            out.append(("op", "("))
            i += 2
            continue
        if c == "`":
            end_word()
            out.append(("op", "`"))
            i += 1
            continue
        op = next((o for o in OPS if text.startswith(o, i)), None)
        if op is None:
            word.append(c)
            have = True
            i += 1
            continue
        if op in ("<", ">", ">>", ">|", "<<", "<<-", "<<<", ">&", "<&") and have and "".join(word).isdigit():
            word.clear()  # a file descriptor number, as in 2>
            have = False
        end_word()
        out.append(("op", op))
        i += len(op)
        if op in ("<<", "<<-"):
            want_delim = op == "<<-"
        if op == "\n" and heredocs:
            for delim, tabs in heredocs:  # skip each body up to its delimiter line
                while i < n:
                    j = text.find("\n", i)
                    line = text[i:] if j < 0 else text[i:j]
                    i = n if j < 0 else j + 1
                    if (line.lstrip("\t") if tabs else line) == delim:
                        break
            heredocs.clear()
    end_word()
    return out


class Simple:
    """One simple command: its words, the files it redirects output into, and the directory it runs in."""

    def __init__(self, words: list[str], writes: list[str], cwd: str) -> None:
        self.words, self.writes, self.cwd = words, writes, cwd


PREFIXES = {"time", "exec", "command", "builtin", "nohup", "sudo", "env", "xargs"}  # run the command after them
KEYWORDS = {"if", "then", "else", "elif", "do", "while", "until", "!", "{", "}"} | PREFIXES
ASSIGN = re.compile(r"[A-Za-z_][A-Za-z0-9_]*=.*", re.S)


def simple_commands(text: str, cwd: str, depth: int = 0) -> list[Simple]:
    """The simple commands in text, each with the directory a leading cd X left it in."""
    found: list[Simple] = []
    words: list[str] = []
    writes: list[str] = []
    here = cwd
    toks = tokens(text)

    def flush() -> None:
        nonlocal here
        ws = list(words)
        prefix = False
        while ws and (ws[0] in KEYWORDS or ASSIGN.fullmatch(ws[0]) or (prefix and ws[0].startswith("-"))):
            prefix = prefix or ws[0] in PREFIXES
            ws.pop(0)
        if ws and ws[0] == "cd":
            target = next((w for w in ws[1:] if not w.startswith("-")), "~")
            here = os.path.normpath(os.path.join(here, os.path.expanduser(os.path.expandvars(target))))
        elif ws or writes:
            if depth < 3 and len(ws) >= 3 and os.path.basename(ws[0]) in ("sh", "bash", "zsh", "dash") and "-c" in ws:
                k = ws.index("-c")
                if k + 1 < len(ws):
                    found.extend(simple_commands(ws[k + 1], here, depth + 1))
            elif depth < 3 and ws and ws[0] == "eval":
                found.extend(simple_commands(" ".join(ws[1:]), here, depth + 1))
            found.append(Simple(ws, list(writes), here))
        words.clear()
        writes.clear()

    k = 0
    while k < len(toks):
        kind, value = toks[k]
        if kind == "op" and value in SEPARATORS:
            flush()
        elif kind == "op":
            target = toks[k + 1][1] if k + 1 < len(toks) and toks[k + 1][0] == "w" else None
            if target is not None:
                k += 1
                if value in (">", ">>", ">|", "&>", "&>>") or (value == ">&" and not re.fullmatch(r"\d+|-", target)):
                    writes.append(target)
        else:
            words.append(value)
        k += 1
    flush()
    return found


# --- what is held ---------------------------------------------------------------

def git_parts(words: list[str]) -> tuple[list[str], list[str], list[str]] | None:
    """For a git command: the -C directories, the --git-dir, and the subcommand with its arguments."""
    if not words or os.path.basename(words[0]) != "git":
        return None
    dirs: list[str] = []
    gitdir: list[str] = []
    k = 1
    while k < len(words):
        w = words[k]
        if w in ("-C", "-c", "--git-dir", "--work-tree", "--namespace", "--config-env") and k + 1 < len(words):
            if w == "-C":
                dirs.append(words[k + 1])
            elif w == "--git-dir":
                gitdir = [words[k + 1]]
            k += 2
        elif w.startswith("--git-dir="):
            gitdir = [w.split("=", 1)[1]]
            k += 1
        elif w.startswith("-"):
            k += 1
        else:
            return dirs, gitdir, words[k:]
    return None


def short_flags(args: list[str]) -> set[str]:
    return {ch for a in args if re.fullmatch(r"-[A-Za-z]+", a) for ch in a[1:]}


def git_hold(sub: list[str]) -> str | None:
    """What the git subcommand does that cannot be undone, or None when it is not held."""
    name, args = sub[0], sub[1:]
    flags = short_flags(args)
    if name == "push" and "--dry-run" not in args and "n" not in flags:
        refspecs = [a for a in args if not a.startswith("-")][1:]
        forced = (any(a in ("--force", "--force-with-lease", "--force-if-includes", "--mirror", "--delete", "--prune") or
                      a.startswith(("--force-with-lease=", "--force-if-includes=")) for a in args)
                  or bool(flags & {"f", "d"}) or any(r.startswith(("+", ":")) for r in refspecs))
        return "push-force" if forced else "push"
    if name == "reset" and "--hard" in args:
        return "reset"
    if name == "clean" and "--dry-run" not in args and "n" not in flags:
        return "clean"
    if name == "branch":
        delete = "--delete" in args or "d" in flags
        if "D" in flags or (delete and ("--force" in args or "f" in flags)):
            return "branch"
    if name == "tag" and ("--delete" in args or "d" in flags):
        return "tag"
    return None


GIT_SAYS = {
    "push": "git push publishes commits to {where}",
    "push-force": "git push, forced or deleting, rewrites or removes what others may have on {where}",
    "reset": "git reset --hard throws away uncommitted work in {where}",
    "clean": "git clean deletes untracked files in {where}",
    "branch": "git branch -D deletes a branch that may hold unmerged commits in {where}",
    "tag": "git tag -d deletes a tag in {where}",
}
GH_SAYS = {
    ("pr", "create"): "gh pr create opens a pull request on {where}, which others see at once",
    ("pr", "merge"): "gh pr merge merges a pull request on {where}",
    ("pr", "close"): "gh pr close closes a pull request on {where}, telling everyone who follows it",
    ("repo", "create"): "gh repo create creates the repository {where}",
    ("repo", "delete"): "gh repo delete deletes the repository {where} for good",
    ("repo", "archive"): "gh repo archive archives the repository {where}",
}
REPO_VALUE_FLAGS = {"-d", "--description", "-h", "--homepage", "-t", "--team", "-p", "--template", "-g",
                    "--gitignore", "-l", "--license", "-s", "--source", "-r", "--remote"}  # gh repo create's


def github_of(url: str) -> tuple[str, str] | None:
    """(owner, name) of a GitHub remote URL or owner/name, or None when it is not on GitHub."""
    url = url.strip()
    m = (re.fullmatch(r"(?:https?|git|ssh|git\+ssh)://(?:[^@/]+@)?(?:www\.)?github\.com(?::\d+)?/([^/]+)/([^/]+?)(?:\.git)?/?", url, re.I)
         or re.fullmatch(r"(?:[^@/:]+@)?github\.com:/?([^/]+)/([^/]+?)(?:\.git)?/?", url, re.I))
    return (m.group(1), m.group(2)) if m else None


def gh_repo(value: str) -> tuple[str, str] | None:
    """owner/name, HOST/owner/name or a URL, as gh's -R takes it."""
    found = github_of(value)
    if found:
        return found
    parts = value.strip().strip("/").split("/")
    if len(parts) == 2 and all(parts):
        return parts[0], parts[1]
    if len(parts) == 3 and parts[0].lower() in ("github.com", "www.github.com"):
        return parts[1], parts[2]
    return None


class Target:
    """Where an action lands: a GitHub repository, or something else said in words."""

    def __init__(self, repo: tuple[str, str] | None, words: str) -> None:
        self.repo, self.words = repo, words


def remote_target(cwd: str, gitdir: list[str], remote: str = "origin") -> Target:
    url: str | None = None
    if re.match(r"[A-Za-z][A-Za-z0-9+.-]*://|[^/\s]+@[^/\s]+:", remote):  # a URL, not a remote's name
        url = remote
    elif os.path.isdir(cwd):
        argv = ["git", *([f"--git-dir={gitdir[0]}"] if gitdir else []), "remote", "get-url", "--push", remote]
        url = (run_quiet(argv, cwd, GIT_TIMEOUT) or "").strip() or None
    if url is None:
        return Target(None, f"the repository in {cwd} (it has no {remote} remote)")
    repo = github_of(url)
    if repo is None:
        return Target(None, f"{url} (not on GitHub)")
    return Target(repo, f"github.com/{repo[0]}/{repo[1]}")


def push_remote(args: list[str]) -> str:
    positional = [a for a in args if not a.startswith("-")]
    for a in args:
        if a.startswith("--repo="):
            return a.split("=", 1)[1]
    return positional[0] if positional else "origin"


def resolve(cwd: str, path: str) -> str:
    return os.path.normpath(os.path.join(cwd, os.path.expanduser(os.path.expandvars(path))))


# What a free owner lets pass: what can be taken back (a push adds commits; a pull request opened is closed, one
# merged is reverted). Everything else held cannot be undone, and asks every time, on the person's own repositories
# too: a forced or deleting push, git reset --hard, git clean, deleting a branch or a tag, closing a pull request that
# tells everyone who follows it, and creating, deleting or archiving a repository.
PASSABLE = {GIT_SAYS["push"], GH_SAYS[("pr", "create")], GH_SAYS[("pr", "merge")]}
SETUP = "http://localhost:8750/"


def holds(cmd: Simple) -> list[tuple[str, Target]]:
    """The irreversible actions in one simple command, each with where it lands."""
    parts = git_parts(cmd.words)
    if parts is not None:
        dirs, gitdir, sub = parts
        what = git_hold(sub)
        if what is None:
            return []
        where = cmd.cwd
        for d in dirs:
            where = resolve(where, d)
        if gitdir:
            gitdir = [resolve(where, gitdir[0])]
        return [(GIT_SAYS[what], remote_target(where, gitdir, push_remote(sub[1:]) if what == "push" else "origin"))]
    if not cmd.words or os.path.basename(cmd.words[0]) != "gh":
        return []
    repo_flag: str | None = None
    positional: list[str] = []
    args = cmd.words[1:]
    k = 0
    while k < len(args):
        a = args[k]
        if a in ("-R", "--repo") and k + 1 < len(args):
            repo_flag = args[k + 1]
            k += 2
            continue
        if a.startswith("--repo="):
            repo_flag = a.split("=", 1)[1]
        elif a in REPO_VALUE_FLAGS and k + 1 < len(args) and positional[:1] == ["repo"] and len(positional) >= 2:
            k += 2
            continue
        elif not a.startswith("-"):
            positional.append(a)
        k += 1
    key = tuple(positional[:2])
    if key not in GH_SAYS:
        return []
    says = GH_SAYS[key]  # type: ignore[index]
    named = positional[2] if key[0] == "repo" and len(positional) > 2 else repo_flag
    if named:
        repo = gh_repo(named)
        saved = read_settings().get("github_user")  # an earlier guard saved it there
        login = github_user() or saved
        if repo is None and "/" not in named and isinstance(login, str) and OWNER.fullmatch(login):
            repo = (login, named)  # gh puts a bare name under the person's own GitHub user
        elif repo is None and "/" not in named:
            return [(says, Target(None, f"{named} under your GitHub user"))]
        if repo is None:
            return [(says, Target(None, named))]
        return [(says, Target(repo, f"github.com/{repo[0]}/{repo[1]}"))]
    if key == ("repo", "create"):
        return [(says, Target(None, "it does not name"))]
    return [(says, remote_target(cmd.cwd, []))]


# --- its own files --------------------------------------------------------------

def own_paths() -> tuple[list[str], list[str]]:
    """(folders, files) the guard keeps: the plugin's hooks folder and the settings file."""
    folders = {HERE}
    root = os.environ.get("CLAUDE_PLUGIN_ROOT")
    if root:
        folders.add(os.path.realpath(os.path.join(root, "hooks")))
    return sorted(folders), [os.path.realpath(settings_path())]


def is_own(path: str, removing: bool = False) -> bool:
    """Whether path is one of the guard's files, or a project's switch; when removing, also any folder holding the
    guard's files."""
    real = os.path.realpath(path)
    if switch.is_switch(real):
        return True
    folders, files = own_paths()
    for p in folders + files:
        if real == p or (p in folders and real.startswith(p + os.sep)):
            return True
        if removing and p.startswith(real.rstrip(os.sep) + os.sep):
            return True
    return False


def writes_own(cmd: Simple) -> str | None:
    """The guard's file a shell command visibly writes, or None."""
    for target in cmd.writes:
        if is_own(resolve(cmd.cwd, target)):
            return target
    if not cmd.words:
        return None
    name, args = os.path.basename(cmd.words[0]), cmd.words[1:]
    paths = [a for a in args if not a.startswith("-")]
    candidates: list[tuple[str, bool]] = []
    if name in ("rm", "rmdir", "unlink", "shred", "trash"):
        candidates = [(p, True) for p in paths]
    elif name == "mv":
        candidates = [(p, True) for p in paths[:-1]] + [(p, False) for p in paths[-1:]]
    elif name in ("cp", "ln", "install", "rsync"):
        candidates = [(p, False) for p in paths[-1:]]
    elif name in ("tee", "truncate", "touch", "chmod", "chown"):
        candidates = [(p, False) for p in paths]
    elif name in ("sed", "gsed", "perl") and any(a == "-i" or a.startswith("-i") or a.startswith("--in-place")
                                                  or (re.fullmatch(r"-[A-Za-z]+", a) and "i" in a) for a in args):
        candidates = [(p, False) for p in paths]
    elif name == "dd":
        candidates = [(a[3:], False) for a in args if a.startswith("of=")]
    for p, removing in candidates:
        if is_own(resolve(cmd.cwd, p), removing):
            return p
    return None


# --- the answer -----------------------------------------------------------------

def ask(reason: str) -> None:
    print(json.dumps({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "ask",
                                             "permissionDecisionReason": reason}}))


OWN_REASON = ("This changes the guard's own files ({path}), which would let it be turned off without you. "
              "The guard asks first; to change its settings, use the Setup page of Jaynes.")
SWITCH_REASON = ("This changes {path}, the project's switch for Jaynes, which decides whether Jaynes works in this "
                 "project and how strict its guard is here. The guard asks first.")


def own_reason(path: str) -> str:
    return (SWITCH_REASON if switch.is_switch(path) else OWN_REASON).format(path=path)


def decide(event: dict[str, object]) -> str | None:
    """The reason to ask with, or None to stay silent."""
    tool = event.get("tool_name")
    tool_input = event.get("tool_input")
    cwd = event.get("cwd")
    if not isinstance(tool_input, dict) or not isinstance(tool, str):
        return None
    cwd = cwd if isinstance(cwd, str) and cwd else os.getcwd()
    settings = read_settings()
    if not is_on(settings):
        return None
    if tool in ("Edit", "Write", "MultiEdit", "NotebookEdit"):
        path = tool_input.get("file_path") or tool_input.get("notebook_path")
        if isinstance(path, str) and path and is_own(resolve(cwd, path)):
            return own_reason(path)
        return None
    if tool != "Bash":
        return None
    command = tool_input.get("command")
    if not isinstance(command, str) or not command.strip():
        return None
    commands = simple_commands(command, cwd)
    for cmd in commands:
        own = writes_own(cmd)
        if own:
            return own_reason(own)
    held = [h for cmd in commands for h in holds(cmd)]
    if not held or switch.off_in(cwd, settings):
        return None  # off in this project is the person's word, in their settings; a project's file cannot say it
    project = switch.read(cwd)
    strict = project["guard"] == "strict"
    owners: list[str] | None = [] if strict else None
    said: list[str] = []
    offers: list[str] = []
    for says, target in held:
        if says not in PASSABLE:
            said.append(f"{says.format(where=target.words)}, which cannot be undone: the guard asks every time, on "
                        "your own repositories too.")
            continue
        if target.repo is not None:
            if owners is None:
                owners = [o.lower() for o in free_owners()]
            if target.repo[0].lower() in owners:
                continue
            why = (f"and {project['path']} asks before every one in this project" if strict else
                   "whose owner is not one of your free owners" if owners else
                   "and you have no free owners yet (gh is signed in as no GitHub user)")
            said.append(f"{says.format(where=target.words)}, {why}.")
            if not strict and target.repo[0] not in offers:
                offers.append(target.repo[0])
        else:
            said.append(f"{says.format(where=target.words)}, which the guard cannot tell belongs to one of your "
                        "free owners.")
    if not said:
        return None
    offer = "".join(f" To let pushes and pull requests to {o} pass from now on, add {o} to Free owners in Setup: "
                    f"{SETUP} (an organization frees all its repositories)." for o in offers)
    return " ".join(said) + " The guard asks you first." + offer + " The switch is on the Setup page of Jaynes."


def main() -> int:
    try:
        event = json.loads(sys.stdin.read() or "null")
        if isinstance(event, dict):
            reason = decide(event)
            if reason:
                ask(reason)
    except Exception as err:  # never in the way: a guard that breaks says why and lets Claude Code decide
        print(f"jaynes guard: {err!r}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
