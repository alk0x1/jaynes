"""Split a shell command line into simple commands, conservatively.

Not a full shell parser. It splits on unquoted ``&&``, ``||``, ``;``, ``|``
and newlines, skips heredoc bodies, and adds the contents of ``$( ... )`` and
backticks as extra commands. Leading ``VAR=value`` assignments are dropped.
Anything it cannot read stays in the text, so an unmatched piece falls into
the ``unknown`` category instead of being silently allowed.
"""

from __future__ import annotations

import re

_HEREDOC = re.compile(r"<<-?\s*(['\"]?)([A-Za-z_][A-Za-z0-9_]*)\1")
_SHELLS = {"sh", "bash", "zsh", "dash", "ksh", "fish"}
_ASSIGN = re.compile(r"^(?:[A-Za-z_][A-Za-z0-9_]*=(?:'[^']*'|\"[^\"]*\"|\S*)\s+)+")
_ONLY_ASSIGN = re.compile(
    r"^(?:(?:export\s+|local\s+|readonly\s+)?[A-Za-z_][A-Za-z0-9_]*=(?:'[^']*'|\"[^\"]*\"|\S*)\s*)+$"
)


_KEYWORD_PREFIX = re.compile(r"^(?:(?:do|then|else|elif|if|while|until|!)\s+|[({]\s*)+")
_DROP = {"done", "fi", "esac", "break", "continue", "else", "then", "do", "}", ")", "{", "(", ";;", "true", ":"}
_FOR_HEADER = re.compile(r"^(?:for|select|case)\b")
_WRAPPER = re.compile(
    r"^(?:timeout(?:\s+-\S+)*\s+\S+|nohup|time|nice(?:\s+-n\s*-?\d+)?|command|exec"
    r"|env(?:\s+-\S+)*(?:\s+[A-Za-z_][A-Za-z0-9_]*=\S*)*"
    r"|xargs(?:\s+(?:-[IdLnPsE]\s*\S+|-\S+))*)\s+"
)


def split_commands(command: str) -> list[str]:
    out: list[str] = []
    _split_into(command, out)
    result: list[str] = []
    for piece in out:
        norm = normalize(piece)
        if norm:
            result.append(norm)
    return result


def normalize(piece: str) -> str:
    """Strip shell syntax around a simple command; empty means "no command here"."""
    cmd = piece.strip()
    for _ in range(8):
        before = cmd
        cmd = _KEYWORD_PREFIX.sub("", cmd).strip()
        if cmd.count("(") < cmd.count(")") or cmd.endswith("}"):
            cmd = cmd.rstrip(")}").strip()
        cmd = _strip_assignments(cmd)
        cmd = _WRAPPER.sub("", cmd, count=1).strip()
        if cmd == before:
            break
    first = cmd.split(None, 1)[0] if cmd else ""
    if not cmd or cmd.startswith("#") or first in _DROP or _FOR_HEADER.match(cmd) or _ONLY_ASSIGN.match(cmd):
        return ""
    return cmd


def _strip_assignments(cmd: str) -> str:
    return _ASSIGN.sub("", cmd, count=1).strip()


def _split_into(text: str, out: list[str]) -> None:
    buf: list[str] = []
    i, n = 0, len(text)
    quote: str | None = None
    pending_heredocs: list[tuple[str, bool]] = []

    def flush() -> None:
        piece = "".join(buf).strip()
        if piece:
            out.append(piece)
        buf.clear()

    while i < n:
        ch = text[i]
        if quote == '"' and text.startswith("$(", i):
            inner, end = _balanced(text, i + 2)
            _split_into(inner, out)
            buf.append("$(...)")
            i = end
            continue
        if quote:
            buf.append(ch)
            if ch == "\\" and quote == '"' and i + 1 < n:
                buf.append(text[i + 1])
                i += 2
                continue
            if ch == quote:
                quote = None
            i += 1
            continue
        if ch in ("'", '"'):
            quote = ch
            buf.append(ch)
            i += 1
            continue
        if ch == "\\" and i + 1 < n:
            buf.append(" " if text[i + 1] == "\n" else text[i : i + 2])  # backslash-newline continues the line
            i += 2
            continue
        if text.startswith("$(", i):
            inner, end = _balanced(text, i + 2)
            _split_into(inner, out)
            buf.append("$(...)")
            i = end
            continue
        if ch == "`":
            end = text.find("`", i + 1)
            end = n if end < 0 else end
            _split_into(text[i + 1 : end], out)
            buf.append("`...`")
            i = end + 1
            continue
        if ch == "<" and text.startswith("<<", i) and not text.startswith("<<<", i):
            m = _HEREDOC.match(text, i)
            if m:
                owner = _strip_assignments("".join(buf).strip()).split(" ", 1)[0]
                pending_heredocs.append((m.group(2), owner.rsplit("/", 1)[-1] in _SHELLS))
                buf.append(m.group(0))
                i = m.end()
                continue
        if ch == "\n":
            flush()
            i += 1
            for delim, is_shell in pending_heredocs:
                body, i = _skip_heredoc(text, i, delim)
                if is_shell:
                    _split_into(body, out)  # a heredoc fed to a shell is a script: read it
            pending_heredocs.clear()
            continue
        if text.startswith("&&", i) or text.startswith("||", i):
            flush()
            i += 2
            continue
        if ch in ";|&":
            # '&' alone backgrounds a job; treat it as a separator too.
            if ch == "&" and ((i + 1 < n and text[i + 1] == ">") or (buf and buf[-1] in ("<", ">"))):
                buf.append(ch)  # '&>' and '>&1' are redirections, not separators
                i += 1
                continue
            if ch == "|" and buf and buf[-1].endswith(">"):
                buf.append(ch)  # '>|' clobber
                i += 1
                continue
            flush()
            i += 1
            continue
        buf.append(ch)
        i += 1
    flush()


def _balanced(text: str, start: int) -> tuple[str, int]:
    depth, i, quote = 1, start, None
    while i < len(text):
        ch = text[i]
        if quote == '"' and text.startswith("$(", i):
            _, i = _balanced(text, i + 2)
            continue
        if quote:
            if ch == "\\" and quote == '"':
                i += 2
                continue
            if ch == quote:
                quote = None
        elif ch in ("'", '"'):
            quote = ch
        elif ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
            if depth == 0:
                return text[start:i], i + 1
        i += 1
    return text[start:], len(text)


def _skip_heredoc(text: str, i: int, delim: str) -> tuple[str, int]:
    start = i
    while i < len(text):
        end = text.find("\n", i)
        line = text[i:] if end < 0 else text[i:end]
        line_start = i
        i = len(text) if end < 0 else end + 1
        if line.strip() == delim:
            return text[start:line_start], i
    return text[start:], i
