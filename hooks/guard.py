"""The guard against the irreversible: a Claude Code PreToolUse hook shipped with the Jaynes plugin.

It runs in every Claude Code session where the plugin is on, before jaynes may be installed, so it uses only
python3's standard library. It holds git and GitHub actions that cannot be undone, and edits of its own files, by
answering "ask"; everything else gets no answer at all, so Claude Code's own permission flow applies. It never
blocks (never exits 2): in claude -p, where nobody can answer, Claude Code turns the ask into a deny by itself. One
thing it refuses outright ("deny"), whatever its settings: changing what a project's checks reach (jaynes project
open-ports, close-ports, open-network, close-network), which is the person's alone, at a terminal of their own.

Settings: $JAYNES_HOME/guard.json (default ~/.jaynes/guard.json) with "on", "watch" and "owners". On a free owner's
repositories on GitHub, what can be taken back passes: a plain push, a pull request opened or merged. With no valid
owners list, the free owner is the GitHub user gh is signed in as, read from gh's own hosts file. The guard never
goes to the network, and writes no file but its notes: where it was not needed it does nothing, and a command it
does not hold runs no program at all.

Watching ("watch": true beside "on": true, which only the Setup page writes), what it would ask that can be taken
back it does not ask: it notes it instead, one JSON line in guard-watch.jsonl beside the settings, the person's alone
(0600), keeping the newest 200. What cannot be undone still asks, and so does a change to its own files. Every
action it holds is noted or asked, never neither: a note it cannot write is asked, saying why. A guard from before
"watch" does not know the key, and asks.
"""

from __future__ import annotations

import json
import os
import re
import sys

import switch  # beside this file: a project's own switch, .jaynes.toml

HERE = os.path.dirname(os.path.realpath(__file__))
GIT_TIMEOUT = 5.0
NOTES_KEEP = 200
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


def is_watching(data: dict[str, object]) -> bool:
    """Whether the guard, being on, only watches: notes what it would ask that can be taken back."""
    return is_on(data) and data.get("watch") is True


def notes_path() -> str:
    """Where the watching guard notes what it would have asked: beside its settings."""
    return os.path.join(os.path.dirname(settings_path()), "guard-watch.jsonl")


def configured_owners(data: dict[str, object]) -> list[str] | None:
    owners = data.get("owners")
    if isinstance(owners, list) and all(isinstance(o, str) for o in owners):
        return [o for o in owners if o.strip()]
    return None


def run_quiet(argv: list[str], cwd: str | None, timeout: float, data: bytes | None = None) -> str | None:
    """stdout of argv, given data on its stdin, or None if it fails, is missing or takes longer than timeout; it never
    waits past it."""
    # imported here: subprocess is most of the guard's own start, and most commands need no program run
    import signal
    import subprocess
    env = dict(os.environ, GIT_TERMINAL_PROMPT="0", GH_PROMPT_DISABLED="1", GIT_OPTIONAL_LOCKS="0")
    try:
        proc = subprocess.Popen(argv, cwd=cwd, stdin=subprocess.DEVNULL if data is None else subprocess.PIPE, stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL, env=env, start_new_session=True)
    except OSError:
        return None
    try:
        out, _ = proc.communicate(input=data, timeout=timeout)
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
    if name == "update-ref" and ("-d" in args or "--stdin" in args):
        return "branch" if deleted_branches(sub) else "ref"
    return None


def deleted_branches(sub: list[str]) -> list[str] | None:
    """The local branches a branch deletion or an update-ref -d names, or None when the guard cannot tell which (a
    remote-tracking branch, refs other than refs/heads, a name it cannot read, refs read from stdin)."""
    name, args = sub[0], sub[1:]
    if name == "update-ref":
        if "--stdin" in args:
            return None
        rest = [a for k, a in enumerate(args) if not a.startswith("-") and (k == 0 or args[k - 1] != "-m")]
        ref = rest[0] if rest else ""
        names = [ref[len("refs/heads/"):]] if ref.startswith("refs/heads/") else []
    else:
        if "r" in short_flags(args) or "a" in short_flags(args) or "--remotes" in args or "--all" in args:
            return None
        names = [a for a in args if not a.startswith("-")]
    if not names or any(not re.fullmatch(r"[A-Za-z0-9._/+-]+", n) or n.startswith("-") for n in names):
        return None
    return names


GIT_SAYS = {
    "push": "git push publishes commits to {where}",
    "push-force": "git push, forced or deleting, rewrites or removes what others may have on {where}",
    "reset": "git reset --hard throws away uncommitted work in {where}",
    "clean": "git clean deletes untracked files in {where}",
    "branch": "git branch -D deletes a branch that may hold unmerged commits in {where}",
    "tag": "git tag -d deletes a tag in {where}",
    "ref": "git update-ref -d deletes a ref in {where}",
}


class Kept(str):
    """A branch deletion that loses nothing, as the line the guard says: another ref holds every commit."""


SAME_CHANGES_MOST = 50  # lost commits the guard looks for elsewhere by their change; more, and it asks


def patch_ids(git: list[str], where: str, commits: list[str]) -> dict[str, str] | None:
    """Each commit's patch id (its change, whatever its hash), or None if git cannot say."""
    if not commits:
        return {}
    diffs = run_quiet([*git, "show", "--no-walk", "--format=commit %H", "-p", "--no-color", "--no-ext-diff", *commits],
                      where, GIT_TIMEOUT)
    if diffs is None:
        return None
    ids = run_quiet([*git, "patch-id", "--stable"], where, GIT_TIMEOUT, diffs.encode())
    if ids is None:
        return None
    return {c: p for p, c in (line.split() for line in ids.splitlines() if len(line.split()) == 2)}


def same_changes(git: list[str], where: str, tip: str, lost: list[str], others: list[str]) -> tuple[str, list[str]] | None:
    """A ref that holds the same change as every lost commit (as a cherry-pick leaves it), with those commits' hashes
    there, newest first; None when some lost commit has no same change in any one ref, or the guard cannot tell."""
    if not lost or len(lost) > SAME_CHANGES_MOST:
        return None
    mine = patch_ids(git, where, lost)
    if not mine or len(mine) != len(lost):  # a merge has no change of its own to find
        return None
    order = sorted(others, key=lambda r: (r not in ("refs/heads/main", "refs/heads/master"),
                                          r not in ("refs/remotes/origin/main", "refs/remotes/origin/master"),
                                          not r.startswith("refs/heads/"), not r.startswith("refs/remotes/"), r))
    for ref in order[:30]:
        theirs_out = run_quiet([*git, "rev-list", "--no-merges", f"{tip}..{ref}"], where, GIT_TIMEOUT)
        theirs = (theirs_out or "").split()
        if not theirs or len(theirs) > 2000:
            continue
        ids = patch_ids(git, where, theirs)
        if ids is None:
            continue
        by_id: dict[str, str] = {}
        for commit in reversed(theirs):  # the newest with that change wins
            by_id[ids.get(commit, "")] = commit
        if all(mine[c] in by_id for c in lost):
            return ref, [by_id[mine[c]] for c in lost]
    return None


def short_ref(ref: str) -> str:
    for prefix in ("refs/heads/", "refs/remotes/", "refs/tags/"):
        if ref.startswith(prefix):
            return ref[len(prefix):]
    return ref


def branch_holds(where: str, gitdir: list[str], names: list[str], gone: set[str], target: "Target") -> list[tuple[str, "Target"]]:
    """For each branch deleted: Kept when another ref (another branch, a tag or a remote-tracking ref, never one deleted
    in the same command) reaches its tip, else what is lost with the tip's hash; a branch the guard cannot read asks
    as before."""
    git = ["git", *([f"--git-dir={gitdir[0]}"] if gitdir else [])]
    found: list[tuple[str, Target]] = []
    refs_out = run_quiet([*git, "for-each-ref", "--format=%(refname)", "refs/heads", "refs/tags", "refs/remotes"], where,
                         GIT_TIMEOUT) if os.path.isdir(where) else None
    others = [r for r in (refs_out or "").split() if r not in gone and not r.endswith("/HEAD")]
    for name in names:
        tip = run_quiet([*git, "rev-parse", "--verify", "--quiet", f"refs/heads/{name}^{{commit}}"], where, GIT_TIMEOUT) \
            if refs_out is not None else None
        tip = (tip or "").strip()
        if not tip:
            found.append((GIT_SAYS["branch"], target))
            continue
        holding = run_quiet([*git, "for-each-ref", "--contains", tip, "--format=%(refname)", *others], where, GIT_TIMEOUT) \
            if others else ""
        holders = [r for r in (holding or "").split() if r in others]
        if holding is not None and holders:
            holders.sort(key=lambda r: (not r.startswith("refs/heads/"), not r.startswith("refs/remotes/"), r))
            found.append((Kept(f"git branch -D {name}: nothing is lost: its commits stay in {short_ref(holders[0])}."),
                          target))
            continue
        lost_out = run_quiet([*git, "rev-list", tip, "--not", *others], where, GIT_TIMEOUT) if holding is not None else None
        lost = (lost_out or "").split()
        picked = same_changes(git, where, tip, lost, others) if lost_out is not None else None
        if picked:
            ref, hashes = picked
            found.append((Kept(f"git branch -D {name}: nothing is lost: its changes are in {short_ref(ref)} as "
                               f"{', '.join(h[:7] for h in hashes)}; the old commits stay in the reflog."), target))
            continue
        count = str(len(lost)) if lost_out is not None else None
        short = tip[:7]
        lost = (f"{count.strip()} commits" if count and count.strip() != "1" else "1 commit") if count else "commits"
        found.append((f"git branch -D deletes the branch {name} in {{where}}, and {lost} of it no other branch, tag or "
                      f"remote holds (its tip is {short}: while the reflog keeps it, `git branch {name} {short}` brings "
                      "it back)", target))
    return found
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


def without_userinfo(url: str) -> str:
    """url as the guard says it, with no user, password, query or fragment, where a token goes: the question is read
    by the person and by the model. An address with an @ past its host, as a password holding a bare / leaves it, is
    not read whole: only its scheme and the host after its last @ are said."""
    m = re.match(r"([A-Za-z][A-Za-z0-9+.-]*://)(.*)", url.strip(), re.S)
    if not m:
        return url.strip()
    scheme, rest = m.group(1), re.split(r"[?#]", m.group(2), maxsplit=1)[0]
    authority, slash, path = rest.partition("/")
    if "@" in path:
        return f"{scheme}{path.rsplit('@', 1)[1].split('/', 1)[0]}/..."
    return scheme + authority.rsplit("@", 1)[-1] + slash + path


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
        return Target(None, f"{without_userinfo(url)} (not on GitHub)")
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
PASSABLE = {GIT_SAYS["push"]: "git push", GH_SAYS[("pr", "create")]: "gh pr create", GH_SAYS[("pr", "merge")]: "gh pr merge"}
SETUP = "http://localhost:8750/"


def holds(cmd: Simple, gone: set[str] | None = None) -> list[tuple[str, Target]]:
    """The irreversible actions in one simple command, each with where it lands. gone: the refs every branch deletion
    of the whole command names, which never hold one another's commits."""
    gone = gone or set()
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
        target = remote_target(where, gitdir, push_remote(sub[1:]) if what == "push" else "origin")
        if what == "branch":
            names = deleted_branches(sub)
            if names:
                return branch_holds(where, gitdir, names, gone, target)
        return [(GIT_SAYS[what], target)]
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
            return [(says, Target(None, without_userinfo(named)))]
        return [(says, Target(repo, f"github.com/{repo[0]}/{repo[1]}"))]
    if key == ("repo", "create"):
        return [(says, Target(None, "it does not name"))]
    return [(says, remote_target(cmd.cwd, []))]


# --- its own files --------------------------------------------------------------

def own_paths() -> tuple[list[str], list[str]]:
    """(folders, files) the guard keeps: the plugin's hooks folder, the settings file and the notes."""
    folders = {HERE}
    root = os.environ.get("CLAUDE_PLUGIN_ROOT")
    if root:
        folders.add(os.path.realpath(os.path.join(root, "hooks")))
    return sorted(folders), [os.path.realpath(settings_path()), os.path.realpath(notes_path())]


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


# --- the person's alone -----------------------------------------------------------

# What a project's checks reach is the person's to change, at a terminal of their own: the command line refuses a
# program without one, but an agent can give the command a terminal (script -q /dev/null jaynes ...), so the guard
# refuses these in an agent's session, whatever its settings. It reads words, as everywhere: a command put together
# when it runs (a verb in a variable, a script written first) is not seen, and the command line's refusal stays.
PERSON_ONLY = {
    "open-ports": "which local ports a project's checks reach",
    "close-ports": "which local ports a project's checks reach",
    "open-network": "whether the machines on your network reach the servers a project's checks start",
    "close-network": "whether the machines on your network reach the servers a project's checks start",
}
PERSON_ONLY_TEXT = re.compile(r"\bproject\W{1,8}((?:open|close)-(?:ports|network))\b")
TALKS_ONLY = {"git", "gh", "grep", "egrep", "fgrep", "rg", "ag", "ack"}  # their words are searched or written, not run


def person_only(cmd: Simple) -> str | None:
    """The person's-alone verb one simple command runs (jaynes project open-ports and its family), or None.

    Any program counts, since jaynes is reached many ways (its whole path, a variable, cargo run, script for a
    terminal); a word that holds a command line counts too (sh -lc, script -c, python -c, echo into a shell). A
    search, a commit message and a pull request's text only talk about the commands."""
    if not cmd.words or os.path.basename(cmd.words[0]) in TALKS_ONLY:
        return None
    for a, b in zip(cmd.words, cmd.words[1:]):
        if a == "project" and b in PERSON_ONLY:
            return b
    for w in cmd.words:
        found = PERSON_ONLY_TEXT.search(w)
        if found:
            return found.group(1)
    return None


def refused(event: dict[str, object]) -> str | None:
    """The reason to refuse an agent's Bash command, or None: only the person's-alone verbs are refused."""
    tool_input = event.get("tool_input")
    if event.get("tool_name") != "Bash" or not isinstance(tool_input, dict):
        return None
    command = tool_input.get("command")
    if not isinstance(command, str) or not command.strip():
        return None
    cwd = event.get("cwd")
    for cmd in simple_commands(command, cwd if isinstance(cwd, str) and cwd else os.getcwd()):
        verb = person_only(cmd)
        if verb:
            return (f"`jaynes project {verb}` changes {PERSON_ONLY[verb]}, which is the person's alone: the guard "
                    "refuses it in an agent's session, whatever its settings. To do it, run "
                    f"`jaynes project {verb} DIR` in your own terminal; `jaynes project ports DIR` says what is open "
                    "now.")
    return None


# --- the answer -----------------------------------------------------------------

def answer(decision: str, reason: str) -> None:
    print(json.dumps({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": decision,
                                             "permissionDecisionReason": reason}}))


def ask(reason: str) -> None:
    answer("ask", reason)


OWN_REASON = ("This changes the guard's own files ({path}), which would let it be turned off without you. "
              "The guard asks first; to change its settings, use the Setup page of Jaynes.")
SWITCH_REASON = ("This changes {path}, the project's switch for Jaynes, which decides whether Jaynes works in this "
                 "project and how strict its guard is here. The guard asks first.")


def own_reason(path: str) -> str:
    return (SWITCH_REASON if switch.is_switch(path) else OWN_REASON).format(path=path)


# the reason to ask ("" when nothing asks), what a watching guard notes instead, and the branch deletions that lose nothing
Verdict = tuple[str, "list[dict[str, object]] | None", "list[str]"]


def decide(event: dict[str, object]) -> Verdict | None:
    """None to stay silent; else the reason to ask with, and what a watching guard notes instead of asking: each held
    action that can be taken back, with where it lands and its owner; None when something held must be asked."""
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
            return own_reason(path), None, []
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
            return own_reason(own), None, []
    gone = {f"refs/heads/{n}" for cmd in commands if (p := git_parts(cmd.words)) and git_hold(p[2]) == "branch"
            for n in (deleted_branches(p[2]) or [])}
    held = [h for cmd in commands for h in holds(cmd, gone)]
    if not held or switch.off_in(cwd, settings):
        return None  # off in this project is the person's word, in their settings; a project's file cannot say it
    kept = [str(says) for says, _ in held if isinstance(says, Kept)]
    held = [h for h in held if not isinstance(h[0], Kept)]
    if not held:
        return "", None, kept
    project = switch.read(cwd)
    strict = project["guard"] == "strict"
    owners: list[str] | None = [] if strict else None
    said: list[str] = []
    offers: list[str] = []
    notes: list[dict[str, object]] = []
    must_ask = False
    for says, target in held:
        if says not in PASSABLE:
            said.append(f"{says.format(where=target.words)}, which cannot be undone: the guard asks every time, on "
                        "your own repositories too.")
            must_ask = True
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
            notes.append({"action": PASSABLE[says], "where": target.words, "owner": target.repo[0]})
            if not strict and target.repo[0] not in offers:
                offers.append(target.repo[0])
        else:
            said.append(f"{says.format(where=target.words)}, which the guard cannot tell belongs to one of your "
                        "free owners.")
            notes.append({"action": PASSABLE[says], "where": target.words, "owner": None})
    if not said:
        return ("", None, kept) if kept else None
    offer = "".join(f" To let pushes and pull requests to {o} pass from now on, add {o} to Free owners in Setup: "
                    f"{SETUP} (an organization frees all its repositories)." for o in offers)
    return " ".join(said + kept) + " The guard asks you first." + offer + " The switch is on the Setup page of Jaynes.", \
        (None if must_ask else notes), kept


def note(event: dict[str, object], held: list[dict[str, object]]) -> str | None:
    """Add one line to the notes, keeping the newest NOTES_KEEP; None when it is written, else why it could not be.

    The file is the person's alone (0600), never followed through a link, and changed in place under a lock, so two
    sessions noting at once both land. It holds no command's words: only when, the session, the folder and what was
    held."""
    import datetime
    import fcntl
    session, cwd = event.get("session_id"), event.get("cwd")
    line = json.dumps({"at": datetime.datetime.now().astimezone().isoformat(timespec="seconds"),
                       "session": session if isinstance(session, str) else None,
                       "cwd": cwd if isinstance(cwd, str) else None, "held": held}, ensure_ascii=False)
    try:
        fd = os.open(notes_path(), os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    except OSError as err:
        return err.strerror or repr(err)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
        os.fchmod(fd, 0o600)
        chunks = []
        while True:
            chunk = os.read(fd, 1 << 16)
            if not chunk:
                break
            chunks.append(chunk)
        kept = [k for k in b"".join(chunks).decode("utf-8", "replace").splitlines() if k.strip()]
        data = ("\n".join(kept[-(NOTES_KEEP - 1):] + [line]) + "\n").encode("utf-8")
        os.lseek(fd, 0, os.SEEK_SET)
        os.ftruncate(fd, 0)
        while data:
            data = data[os.write(fd, data):]
    except OSError as err:
        return err.strerror or repr(err)
    finally:
        os.close(fd)
    return None


def main() -> int:
    try:
        event = json.loads(sys.stdin.read() or "null")
        if isinstance(event, dict):
            refusal = refused(event)
            if refusal:
                answer("deny", refusal)
                return 0
            verdict = decide(event)
            if verdict and not verdict[0]:
                # only deletions that lose nothing: a line for the person, never a decision, so Claude Code's own
                # permissions still apply
                print(json.dumps({"systemMessage": "jaynes guard: " + " ".join(verdict[2])}))
                return 0
            if verdict:
                reason, held, _ = verdict
                if held is not None and is_watching(read_settings()):
                    failed = note(event, held)
                    if failed is None:
                        return 0
                    reason = (f"The guard only watches, but could not note this in {notes_path()} ({failed}), so it "
                              f"asks. {reason}")
                ask(reason)
    except Exception as err:  # never in the way: a guard that breaks says why and lets Claude Code decide
        print(f"jaynes guard: {err!r}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
