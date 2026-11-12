"""A role's tools: the agent of a step calls them during its turn through an MCP server, Jaynes runs them in its own
process and records every call, and the pictures they leave become the step's files."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path
from typing import Any

import pytest
from pydantic import BaseModel

from jaynes.conductor import Conductor
from jaynes.role import ToolContext, role, tool
from jaynes.runtime import TurnResult
from jaynes.runtimes import ClaudeRuntime
from jaynes.tools import Offered
from tests.fakes import Scripted, write

PNG = b"\x89PNG\r\n\x1a\n picture"


class Note(BaseModel):
    text: str


def echo(ctx: ToolContext, text: str, times: int = 1) -> str:
    """Says the text back, as many times as asked."""
    return " ".join([text] * times)


def photograph(ctx: ToolContext, name: str) -> list[Any]:
    """Leaves a picture called name and shows it."""
    (ctx.folder / f"{name}.png").write_bytes(PNG)
    return [f"the picture of {name}, for run {ctx.run_id} at {ctx.step}", ctx.folder / f"{name}.png"]


def broken(ctx: ToolContext) -> str:
    """Always fails."""
    raise RuntimeError("the site did not answer")


def mcp(command: tuple[str, ...], *messages: dict[str, Any]) -> list[dict[str, Any]]:
    """The answers the role's MCP server gives to ``messages``, as a runtime's agent would talk to it."""
    lines = "".join(json.dumps({"jsonrpc": "2.0", **m}) + "\n" for m in messages)
    out = subprocess.run(list(command), input=lines, capture_output=True, text=True, timeout=60, check=True).stdout
    return [json.loads(x) for x in out.splitlines()]


class WithTools(Scripted):
    """A scripted agent that also talks to the role's tools, over MCP, before it writes its output."""

    def __init__(self, calls: list[dict[str, Any]], **kw: Any) -> None:
        super().__init__(**kw)
        self.calls = calls
        self.answers: list[dict[str, Any]] = []

    def turn(self, workspace: Path, scratch: Path, prompt: str, deadline: float, web: bool = False,  # type: ignore[override]
             max_messages: int | None = None, tools: Offered | None = None) -> TurnResult:
        assert tools is not None
        self.answers = mcp(tools.command, {"id": 1, "method": "initialize", "params": {}},
                           {"method": "notifications/initialized"}, {"id": 2, "method": "tools/list"},
                           *({"id": 3 + n, "method": "tools/call", "params": c} for n, c in enumerate(self.calls)))
        return super().turn(workspace, scratch, prompt, deadline, web, max_messages)


def test_the_agent_calls_the_roles_tools_and_jaynes_records_each_call_and_keeps_its_pictures(tmp_path: Path) -> None:
    r = role("look", agent="fake")
    r.step("direction", output=Note, tools=[echo, photograph, broken])
    agent = WithTools([{"name": "echo", "arguments": {"text": "hi", "times": 2}},
                       {"name": "photograph", "arguments": {"name": "ref-1"}},
                       {"name": "broken", "arguments": {}},
                       {"name": "echo", "arguments": {"words": "x"}}],
                      plan={("direction", 1): write({"text": "dark"})})
    c = Conductor(tmp_path / "home", runtimes={"fake": lambda: agent})
    assert c.run(r, "t", "r1").status == "finished"

    listed = agent.answers[1]["result"]["tools"]
    assert [t["name"] for t in listed] == ["echo", "photograph", "broken"]
    assert listed[0]["inputSchema"] == {"type": "object", "properties": {"text": {"type": "string"},
                                                                         "times": {"type": "integer"}},
                                        "required": ["text"]}
    said, shown, failed, wrong = (a["result"] for a in agent.answers[2:])
    assert said == {"content": [{"type": "text", "text": "hi hi"}], "isError": False}
    assert shown["content"][0]["text"] == "the picture of ref-1, for run r1 at direction"
    assert shown["content"][1]["type"] == "image" and shown["content"][1]["mimeType"] == "image/png"
    assert failed["isError"] and "the site did not answer" in failed["content"][0]["text"]
    assert wrong["isError"] and "takes no words" in wrong["content"][0]["text"]

    turn = next(e.payload for e in c.store.events("TurnFinished", run_id="r1"))
    assert [(x["tool"], x["ok"], x["files"]) for x in turn["tools"]] == [
        ("echo", True, []), ("photograph", True, ["ref-1.png"]), ("broken", False, []), ("echo", False, [])]
    checked = next(e.payload for e in c.store.events("Checked", run_id="r1"))
    assert list(checked["files"]) == ["ref-1.png"]
    assert (tmp_path / "home" / "objects" / checked["files"]["ref-1.png"]).read_bytes() == PNG
    assert "- photograph: Leaves a picture called name and shows it." in agent.prompts[0]


def test_a_runtime_that_cannot_offer_tools_rejects_the_turn(tmp_path: Path) -> None:
    r = role("look", agent="fake")
    r.step("direction", output=Note, tools=[echo], max_turns=1)
    c = Conductor(tmp_path / "home", runtimes={"fake": lambda: Scripted(default=lambda _: {"text": "x"})})
    c.run(r, "t", "r1")
    rejected = next(e.payload for e in c.store.events("TurnRejected", run_id="r1"))
    assert rejected["reason"] == "runtime" and "cannot offer them" in rejected["reasons"][0]


def test_a_tool_says_what_it_does_takes_simple_arguments_and_belongs_to_an_agents_step() -> None:
    def quiet(ctx: ToolContext, url: str) -> str:
        return url

    def vague(ctx: ToolContext, where: list[str]) -> str:
        """Takes a list."""
        return ""

    r = role("look")
    with pytest.raises(ValueError, match="no docstring"):
        r.step("a", output=Note, tools=[quiet])
    with pytest.raises(ValueError, match="must be a str, int, float or bool"):
        r.step("b", output=Note, tools=[vague])
    with pytest.raises(ValueError, match="no agent runs it"):
        r.step("c", output=Note, run_by=lambda ctx: Note(text=""), tools=[echo])
    with pytest.raises(ValueError, match="two tools called echo"):
        r.step("d", output=Note, tools=[echo, tool(echo)])


def test_offering_tools_changes_the_steps_fingerprint_and_nothing_else_does() -> None:
    plain, offered = role("look"), role("look")
    plain.step("a", output=Note)
    offered.step("a", output=Note, tools=[echo])
    again = role("look")
    again.step("a", output=Note)
    assert plain.fingerprint() == again.fingerprint() != offered.fingerprint()


def test_claude_offers_exactly_the_steps_tools_through_jaynes_mcp_server(tmp_path: Path,
                                                                          monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("jaynes.runtimes._binary", lambda name: name)  # pyright: ignore[reportUnknownLambdaType, reportUnknownArgumentType]
    args = ClaudeRuntime().command(tmp_path, tools=Offered(("python", "-m", "jaynes.tools", "/tmp/s"), ("capture",)))
    servers = json.loads(args[args.index("--mcp-config") + 1])["mcpServers"]
    assert servers == {"role": {"type": "stdio", "command": "python", "args": ["-m", "jaynes.tools", "/tmp/s"]}}
    allow = json.loads(args[args.index("--settings") + 1])["permissions"]["allow"]
    assert "mcp__role__capture" in allow and not any(a.startswith("mcp__") and a != "mcp__role__capture" for a in allow)
    plain = ClaudeRuntime().command(tmp_path)
    assert json.loads(plain[plain.index("--mcp-config") + 1]) == {"mcpServers": {}}



def test_the_tools_socket_finds_a_folder_where_tmp_cannot_be_written(tmp_path: Path,
                                                                      monkeypatch: pytest.MonkeyPatch) -> None:
    """Jaynes's own suite runs in the box, where /tmp is refused: the socket goes to the temporary folder instead."""
    import errno
    import tempfile

    from jaynes import tools

    made = tempfile.mkdtemp

    def refuse_tmp(*args: Any, **kw: Any) -> str:
        if kw.get("dir") == "/tmp":
            raise PermissionError(errno.EPERM, "refused", "/tmp/jt-x")  # not the box's words: Jaynes reads those
        return made(*args, **kw)

    monkeypatch.setattr(tools.tempfile, "mkdtemp", refuse_tmp)
    test_the_agent_calls_the_roles_tools_and_jaynes_records_each_call_and_keeps_its_pictures(tmp_path)
