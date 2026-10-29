"""Runtimes for real agents: Claude Code (``claude -p``) and Codex (``codex exec``).

Each turn is one child process in its own session, run in the workspace with
the scratch directory as TMPDIR, killed with its whole process group at the
deadline. Both use the person's subscription login; neither copies credentials.

The runtime sandbox is the first wall; the conductor's manifest comparison is
the second. Claude Code's file tools are bounded by its permission layer and
its Bash by the OS sandbox; Codex runs its commands in its workspace-write
sandbox with network off. A step marked ``web`` may read the web through the
agent's own web tools (fetch and search), which the runtime records; shell
commands stay offline either way. Settings ported from the first user's qualified
factory runtime.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import signal
import subprocess
from pathlib import Path
from typing import Any, cast

from jaynes import suite
from jaynes.runtime import QuotaExhausted, Ran, RuntimeFailure, TurnResult, TurnTimeout

LIMIT = re.compile(r"usage limit|hit your limit|limit reached|rate.?limit|quota", re.I)
NOT_QUOTA = ("error_max_turns", "error_max_budget_usd")  # Claude result subtypes that name another failure
OUTPUT_TAIL = 2000  # characters of a command's output kept, from its end, where test summaries are


def _binary(name: str) -> str:
    found = shutil.which(name) or str(Path.home() / ".local/bin" / name)
    if not Path(found).is_file():
        raise RuntimeFailure(f"{name} is not installed")
    return found


def _run(args: list[str], cwd: Path, scratch: Path, prompt: str, deadline: float,
         env: dict[str, str] | None = None) -> tuple[int, str, str]:
    environ = {**os.environ, "TMPDIR": str(scratch), **(env or {})}
    proc = subprocess.Popen(args, cwd=cwd, env=environ, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, text=True, start_new_session=True)
    try:
        out, err = proc.communicate(prompt, timeout=deadline)
    except subprocess.TimeoutExpired:
        _kill(proc)
        raise TurnTimeout() from None
    finally:
        if proc.poll() is None:
            _kill(proc)
    return proc.returncode, out, err


def _project_env(workspace: Path, scratch: Path) -> dict[str, str]:
    """On a project step, what the project's tests run with in Jaynes's own checks (``jaynes.suite``)."""
    project = workspace / "project"
    return suite.environment(project, scratch) if project.is_dir() else {}


def _kill(proc: subprocess.Popen[str]) -> None:
    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(proc.pid, sig)
            proc.wait(timeout=5)
            return
        except ProcessLookupError:
            return
        except subprocess.TimeoutExpired:
            continue


def _reset_epoch(text: str) -> float | None:
    """Claude reports exhaustion as text, sometimes with an epoch after '|'."""
    m = re.search(r"\|(\d{10})\b", text)
    return float(m.group(1)) if m else None


# --- Claude Code ----------------------------------------------------------------

WEB_TOOLS = ["WebFetch", "WebSearch"]


def claude_settings(workspace: Path, web: bool = False, scratch: Path | None = None) -> dict[str, Any]:
    """On a project step the box also opens the installed dependencies and local ports, for the project's tests."""
    project = workspace / "project"
    reads = [str(workspace), *(str(p) for p in suite.reads(project))] if project.is_dir() else [str(workspace)]
    return {
        "permissions": {"defaultMode": "dontAsk",
                        "allow": ["Read(./**)", "Edit(./**)", "Write(./**)", "Bash"] + (WEB_TOOLS if web else []),
                        "deny": ([] if web else WEB_TOOLS) + ["Agent", "Task", "NotebookEdit"],
                        "blockReadsOutsideWorkingDirectories": True,
                        "disableBypassPermissionsMode": "disable"},
        "sandbox": {"enabled": True, "allowUnsandboxedCommands": False, "failIfUnavailable": True,
                    "autoAllowBashIfSandboxed": True,
                    "network": {"allowedDomains": [], "strictAllowlist": True,
                                **({"allowLocalBinding": True} if project.is_dir() else {})},
                    "filesystem": {"denyRead": ["~/"], "allowRead": reads,
                                   **({"allowWrite": [str(scratch)]} if scratch else {})}},
        "disableAllHooks": True, "autoMemoryEnabled": False, "disableClaudeAiConnectors": True,
        "syncClaudeAiPlugins": False, "syncClaudeAiSkills": False, "disableBundledSkills": True,
    }


def claude_result(out: str) -> tuple[dict[str, Any], list[str]]:
    """The final result of a ``stream-json`` turn, and the addresses and searches its web tools read."""
    result: dict[str, Any] = {"is_error": True, "result": out[-400:]}
    read: list[str] = []
    for line in out.splitlines():
        try:
            event: dict[str, Any] = json.loads(line)
        except ValueError:
            continue
        if event.get("type") == "result":
            result = event
        elif event.get("type") == "assistant":
            message: dict[str, Any] = event.get("message") or {}
            blocks = cast(list[dict[str, Any]], message.get("content") or [])
            for block in blocks:
                if block.get("type") != "tool_use":
                    continue
                args: dict[str, Any] = block.get("input") or {}
                if block.get("name") == "WebFetch" and args.get("url"):
                    read.append(str(args["url"]))
                elif block.get("name") == "WebSearch" and args.get("query"):
                    read.append(f"search: {args['query']}")
    return result, read


def claude_commands(out: str) -> list[Ran]:
    """The shell commands a ``stream-json`` turn ran. A command the turn ended before is not ok, with no output."""
    commands: dict[str, str] = {}
    results: dict[str, Ran] = {}
    for line in out.splitlines():
        try:
            event: dict[str, Any] = json.loads(line)
        except ValueError:
            continue
        message: dict[str, Any] = event.get("message") or {}
        content = message.get("content")
        if not isinstance(content, list):
            continue
        for block in cast(list[dict[str, Any]], content):
            if block.get("type") == "tool_use" and block.get("name") == "Bash":
                args: dict[str, Any] = block.get("input") or {}
                commands[str(block.get("id"))] = str(args.get("command", ""))
            elif block.get("type") == "tool_result" and str(block.get("tool_use_id")) in commands:
                i = str(block["tool_use_id"])
                body = block.get("content")
                text = body if isinstance(body, str) else "".join(
                    str(x.get("text", "")) for x in cast(list[dict[str, Any]], body or []))
                results[i] = Ran(commands[i], not block.get("is_error"), text[-OUTPUT_TAIL:])
    return [results.get(i, Ran(c, False, "")) for i, c in commands.items()]


def claude_failure(result: dict[str, Any], code: int, err: str) -> QuotaExhausted | RuntimeFailure:
    """Why a turn failed. The result text is Claude's own error only when it reports one of no other kind;
    otherwise it is the agent's last message, which may mention limits without having hit one."""
    said = str(result.get("result") or "")
    own = bool(result.get("is_error")) and result.get("subtype") not in NOT_QUOTA
    text = f"{said if own else ''} {err[-400:]}"
    if LIMIT.search(text):
        return QuotaExhausted(_reset_epoch(text))
    if result.get("subtype") == "error_max_turns":
        return RuntimeFailure("the turn ran out of messages before it finished; the step may need a higher "
                              "max_messages")
    return RuntimeFailure(f"claude turn did not complete ({result.get('subtype') or code}): "
                          f"{f'{said} {err[-400:]}'.strip()[:300]}")


class ClaudeRuntime:
    name = "claude"
    scratch_dirs = (".claude",)  # Claude Code 2.1 creates .claude/.cc-writes in its working directory

    def __init__(self, model: str | None = None, max_messages: int = 200) -> None:
        self.model = model or os.environ.get("JAYNES_CLAUDE_MODEL")
        self.max_messages = max_messages  # the deadline is what usually ends a long turn

    def command(self, workspace: Path, web: bool = False, scratch: Path | None = None,
                max_messages: int | None = None) -> list[str]:
        args = [_binary("claude"), "-p", "--output-format", "stream-json", "--verbose", "--setting-sources", "",
                "--settings", json.dumps(claude_settings(workspace, web, scratch)), "--strict-mcp-config",
                "--mcp-config", '{"mcpServers":{}}', "--no-session-persistence",
                "--permission-mode", "dontAsk", "--max-turns", str(max_messages or self.max_messages)]
        return args + (["--model", self.model] if self.model else [])

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float, web: bool = False,
             max_messages: int | None = None) -> TurnResult:
        code, out, err = _run(self.command(workspace, web, scratch, max_messages), workspace, scratch, prompt, deadline,
                              {"CLAUDE_CODE_DISABLE_CLAUDE_MDS": "1", **_project_env(workspace, scratch)})
        result, read = claude_result(out)
        if result.get("is_error") or code != 0:
            raise claude_failure(result, code, err)
        usage: dict[str, Any] = result.get("usage") or {}
        tokens_in = sum(int(usage.get(k) or 0) for k in
                        ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"))
        return TurnResult(tokens_in, int(usage.get("output_tokens") or 0), self._model(result), tuple(read),
                          tuple(claude_commands(out)))

    def _model(self, result: dict[str, Any]) -> str:
        per_model: dict[str, dict[str, Any]] = result.get("modelUsage") or {}
        if per_model:  # the model that wrote the most is the one that answered
            return max(per_model, key=lambda m: int(per_model[m].get("outputTokens") or 0))
        return self.model or "claude"


# --- Codex ------------------------------------------------------------------------

def codex_command(item: dict[str, Any]) -> Ran | None:
    """A shell command from a completed ``codex exec --json`` item, or None for any other item."""
    if item.get("type") != "command_execution":
        return None
    return Ran(str(item.get("command", "")), item.get("exit_code") == 0,
               str(item.get("aggregated_output") or "")[-OUTPUT_TAIL:])


class CodexRuntime:
    name = "codex"

    def __init__(self, model: str | None = None) -> None:
        self.model = model or os.environ.get("JAYNES_CODEX_MODEL")

    def command(self, workspace: Path, web: bool = False) -> list[str]:
        args = [_binary("codex"), "exec", "--json", "--sandbox", "workspace-write", "--cd", str(workspace),
                "--skip-git-repo-check", "--ephemeral", "--ignore-user-config", "--ignore-rules",
                "-c", "sandbox_workspace_write.network_access=false", "-c", "project_doc_max_bytes=0",
                "-c", f'web_search="{"live" if web else "disabled"}"', "-c", 'approval_policy="never"']
        return args + (["--model", self.model] if self.model else []) + ["-"]

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float, web: bool = False) -> TurnResult:
        code, out, err = _run(self.command(workspace, web), workspace, scratch, prompt, deadline,
                              _project_env(workspace, scratch))
        usage: dict[str, Any] = {}
        read: list[str] = []
        ran: list[Ran] = []
        model = self.model or self._default_model()
        failure = ""
        for line in out.splitlines():
            try:
                event: dict[str, Any] = json.loads(line)
            except ValueError:
                continue
            kind = event.get("type")
            if kind == "turn.completed":
                usage = event.get("usage") or {}
            elif kind == "item.completed":
                item: dict[str, Any] = event.get("item") or {}
                if item.get("type") == "web_search":
                    read.append(f"search: {item.get('query', '')}")
                elif command := codex_command(item):
                    ran.append(command)
            elif kind in ("turn.failed", "error"):
                failure += " " + json.dumps(event.get("error") or event.get("message") or event)[:400]
        if failure or code != 0 or not usage:
            text = f"{failure} {err[-400:]}"
            if LIMIT.search(text):
                raise QuotaExhausted(_reset_epoch(text))
            raise RuntimeFailure(f"codex turn did not complete ({code}): {text.strip()[:300]}")
        tokens_in = int(usage.get("input_tokens") or 0)  # cached input is a subset of input for Codex
        return TurnResult(tokens_in, int(usage.get("output_tokens") or 0), model, tuple(read), tuple(ran))

    @staticmethod
    def _default_model() -> str:
        """``codex exec --json`` does not name the model. Without a pinned model, the CLI version stands
        in for it, so an update that may change the default opens a new window."""
        try:
            version = subprocess.run([_binary("codex"), "--version"], capture_output=True, text=True,
                                     timeout=20).stdout.strip()
        except (OSError, subprocess.TimeoutExpired):
            version = "unknown"
        return f"default ({version or 'unknown'})"
