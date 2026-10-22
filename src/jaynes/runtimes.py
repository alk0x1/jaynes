"""Runtimes for real agents: Claude Code (``claude -p``) and Codex (``codex exec``).

Each turn is one child process in its own session, run in the workspace with
the scratch directory as TMPDIR, killed with its whole process group at the
deadline. Both use the person's subscription login; neither copies credentials.

The runtime sandbox is the first wall; the conductor's manifest comparison is
the second. Claude Code's file tools are bounded by its permission layer and
its Bash by the OS sandbox; Codex runs its commands in its workspace-write
sandbox with network off. Settings ported from the first user's qualified
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
from typing import Any

from jaynes.runtime import QuotaExhausted, RuntimeFailure, TurnResult, TurnTimeout

LIMIT = re.compile(r"usage limit|hit your limit|limit reached|rate.?limit|quota", re.I)


def _binary(name: str) -> str:
    found = shutil.which(name) or str(Path.home() / ".local/bin" / name)
    if not Path(found).is_file():
        raise RuntimeFailure(f"{name} is not installed")
    return found


def _run(args: list[str], cwd: Path, scratch: Path, prompt: str, deadline: float,
         env: dict[str, str] | None = None) -> tuple[int, str, str]:
    environ = dict(os.environ, TMPDIR=str(scratch), **(env or {}))
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

def claude_settings(workspace: Path) -> dict[str, Any]:
    return {
        "permissions": {"defaultMode": "dontAsk",
                        "allow": ["Read(./**)", "Edit(./**)", "Write(./**)", "Bash"],
                        "deny": ["WebFetch", "WebSearch", "Agent", "Task", "NotebookEdit"],
                        "blockReadsOutsideWorkingDirectories": True,
                        "disableBypassPermissionsMode": "disable"},
        "sandbox": {"enabled": True, "allowUnsandboxedCommands": False, "failIfUnavailable": True,
                    "autoAllowBashIfSandboxed": True,
                    "network": {"allowedDomains": [], "strictAllowlist": True},
                    "filesystem": {"denyRead": ["~/"], "allowRead": [str(workspace)]}},
        "disableAllHooks": True, "autoMemoryEnabled": False, "disableClaudeAiConnectors": True,
        "syncClaudeAiPlugins": False, "syncClaudeAiSkills": False, "disableBundledSkills": True,
    }


class ClaudeRuntime:
    name = "claude"

    def __init__(self, model: str | None = None, max_turns: int = 60) -> None:
        self.model = model or os.environ.get("JAYNES_CLAUDE_MODEL")
        self.max_turns = max_turns

    def command(self, workspace: Path) -> list[str]:
        args = [_binary("claude"), "-p", "--output-format", "json", "--setting-sources", "",
                "--settings", json.dumps(claude_settings(workspace)), "--strict-mcp-config",
                "--mcp-config", '{"mcpServers":{}}', "--no-session-persistence",
                "--permission-mode", "dontAsk", "--max-turns", str(self.max_turns)]
        return args + (["--model", self.model] if self.model else [])

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float) -> TurnResult:
        code, out, err = _run(self.command(workspace), workspace, scratch, prompt, deadline,
                              {"CLAUDE_CODE_DISABLE_CLAUDE_MDS": "1"})
        try:
            result: dict[str, Any] = json.loads(out)
        except ValueError:
            result = {"is_error": True, "result": out[-400:]}
        text = f"{result.get('result') or ''} {err[-400:]}"
        if result.get("is_error") or code != 0:
            if LIMIT.search(text):
                raise QuotaExhausted(_reset_epoch(text))
            raise RuntimeFailure(f"claude turn did not complete ({result.get('subtype') or code}): {text.strip()[:300]}")
        usage: dict[str, Any] = result.get("usage") or {}
        tokens_in = sum(int(usage.get(k) or 0) for k in
                        ("input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"))
        return TurnResult(tokens_in, int(usage.get("output_tokens") or 0), self._model(result))

    def _model(self, result: dict[str, Any]) -> str:
        per_model: dict[str, dict[str, Any]] = result.get("modelUsage") or {}
        if per_model:  # the model that wrote the most is the one that answered
            return max(per_model, key=lambda m: int(per_model[m].get("outputTokens") or 0))
        return self.model or "claude"


# --- Codex ------------------------------------------------------------------------

class CodexRuntime:
    name = "codex"

    def __init__(self, model: str | None = None) -> None:
        self.model = model or os.environ.get("JAYNES_CODEX_MODEL")

    def command(self, workspace: Path) -> list[str]:
        args = [_binary("codex"), "exec", "--json", "--sandbox", "workspace-write", "--cd", str(workspace),
                "--skip-git-repo-check", "--ephemeral", "--ignore-user-config", "--ignore-rules",
                "-c", "sandbox_workspace_write.network_access=false", "-c", "project_doc_max_bytes=0",
                "-c", 'web_search="disabled"', "-c", 'approval_policy="never"']
        return args + (["--model", self.model] if self.model else []) + ["-"]

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float) -> TurnResult:
        code, out, err = _run(self.command(workspace), workspace, scratch, prompt, deadline)
        usage: dict[str, Any] = {}
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
            elif kind in ("turn.failed", "error"):
                failure += " " + json.dumps(event.get("error") or event.get("message") or event)[:400]
        if failure or code != 0 or not usage:
            text = f"{failure} {err[-400:]}"
            if LIMIT.search(text):
                raise QuotaExhausted(_reset_epoch(text))
            raise RuntimeFailure(f"codex turn did not complete ({code}): {text.strip()[:300]}")
        tokens_in = int(usage.get("input_tokens") or 0)  # cached input is a subset of input for Codex
        return TurnResult(tokens_in, int(usage.get("output_tokens") or 0), model)

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
