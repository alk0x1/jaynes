"""The verifier: what a role gets wrong, found before any agent runs (SPEC section 4).

It reads checks, questions and trusted executors without running them, from their bytecode. A step a function
names is a step it reads, unless it only takes the step's name: directly, through a closure, a default value, a
module or class attribute, a dict or list, a partial or a callable object, or any plain function it names in
turn. A role file's checks read steps by name, ``needs["draft"]``: a constant string looked up in ``needs``
(or the function's second parameter) is a read of the step with that name. A step can only read what it needs: the conductor hands it nothing else, looked up by step name, so an
undeclared read fails mid-run, or quietly reads another step that happens to share the name.

Steps reached through a method of some other object, or a name computed at run time, are not seen.
"""

from __future__ import annotations

import dis
import functools
import inspect
import sysconfig
import types
import typing
from collections.abc import Callable
from pathlib import Path
from types import CodeType, FunctionType, MethodType, ModuleType, SimpleNamespace
from typing import Any, cast

from pydantic import BaseModel

from jaynes.role import Role, Step

# The standard library is never walked; site-packages can sit inside it, and is walked
_STDLIB = tuple(str(Path(sysconfig.get_paths()[k]).resolve()) for k in ("stdlib", "platstdlib"))


def verify(role: Role) -> list[str]:
    """The role's problems, each with its step and how to fix it; none when the role is sound."""
    problems: list[str] = []
    for i, step in enumerate(role.steps):
        named = [(f"check {getattr(c, '__name__', None) or type(c).__name__}", c) for c in step.checks]
        named += [("question", step.question)] if step.question else []
        for what, fn in named:
            problems += _reads(role, step, what, fn) + _takes(step, what, fn)
        if step.run_by is not None:
            problems += _reads(role, step, "executor", step.run_by)
        later = role.steps[i + 1:]  # the last step's output is the run's result
        used = (not later or step.stop is not None or step.project == "write"
                or step.effect in ("write-copy", "irreversible") or any(step in x.needs for x in later))
        if step.output is not None and not used:
            problems.append(f"{step.name}: its output reaches no later step and no stop; need it from a later step, "
                            "stop at it or remove it")
    return problems


def report(role: Role, problems: list[str]) -> str:
    if not problems:
        return f"{role.name} has no problems"
    return f"{role.name} has {len(problems)} problem{'s' * (len(problems) > 1)}:\n" + "\n".join(
        f"  {p}" for p in problems)


def _reads(role: Role, step: Step[Any], what: str, fn: Callable[..., Any]) -> list[str]:
    out: list[str] = []
    for other in _named_steps(fn):
        if isinstance(other, str):
            match = [s for s in role.steps if s.name == other]
            if not match:
                out.append(f"{step.name}: {what} reads {other}, which is not a step of {role.name}")
                continue
            other = match[0]
        if other.role is not role:
            out.append(f"{step.name}: {what} reads {other.name} of role {other.role.name}; use a step of "
                       f"{role.name}, in {step.name}'s needs")
        elif other not in step.needs:
            out.append(f"{step.name}: {what} reads {other.name}, which {step.name} does not need; add "
                       f"{other.name} to its needs")
    return out


def _takes(step: Step[Any], what: str, fn: Callable[..., Any]) -> list[str]:
    """A check or question annotated for outputs its step does not produce."""
    try:
        first = next(iter(inspect.signature(fn).parameters), None)
        wanted = typing.get_type_hints(fn).get(first) if first else None
    except Exception:  # no signature, or annotations that do not resolve here
        return []
    union = isinstance(wanted, types.UnionType) or typing.get_origin(wanted) is typing.Union
    models = [t for t in (typing.get_args(wanted) if union else (wanted,))
              if isinstance(t, type) and issubclass(t, BaseModel)]
    if step.output is None or not models or any(issubclass(step.output, t) for t in models):
        return []
    names = " or ".join(t.__name__ for t in models)
    return [f"{step.name}: {what} takes a {names}, but {step.name}'s output is a {step.output.__name__}"]


def _named_steps(fn: Callable[..., Any]) -> list[Step[Any] | str]:
    """Steps a function names for more than their name, in order; a step looked up by name is its name."""
    found: list[Step[Any] | str] = []
    seen: set[int] = set()

    def visit(f: FunctionType) -> None:
        if id(f) in seen or _stdlib(f.__code__):
            return
        seen.add(id(f))
        try:
            cells: dict[str, object] = {}
            for name, cell in zip(f.__code__.co_freevars, f.__closure__ or ()):
                try:
                    cells[name] = cell.cell_contents
                except ValueError:  # a cell never assigned yet
                    pass
            for value in (*(f.__defaults__ or ()), *(f.__kwdefaults__ or {}).values()):
                consider(value)
            params = [p for p in f.__code__.co_varnames[:f.__code__.co_argcount] if p != "self"]
            lookups = {"needs", *params[1:2]}  # where a check, question or executor finds the steps it needs
            for code in _codes(f.__code__):
                ins = list(dis.get_instructions(code))
                for i, x in enumerate(ins):
                    if x.opname == "LOAD_CONST" and isinstance(x.argval, str) and 0 < i < len(ins) - 1 \
                            and ins[i - 1].argval in lookups and _subscript(ins[i + 1]):
                        if x.argval not in found:
                            found.append(x.argval)
                        continue
                    if x.opname in ("LOAD_GLOBAL", "LOAD_NAME"):
                        value = f.__globals__.get(x.argval)
                    elif x.opname == "LOAD_DEREF":
                        value = cells.get(x.argval)  # a nested function's cells are locals, unknown here
                    else:
                        continue
                    j = i + 1  # module.attribute or Class.attribute, as many levels as there are
                    while (j < len(ins) and ins[j].opname in ("LOAD_ATTR", "LOAD_METHOD")
                           and isinstance(value, (ModuleType, type, SimpleNamespace))):
                        value = getattr(value, ins[j].argval, None)
                        j += 1
                    if isinstance(value, Step) and j < len(ins) and ins[j].opname == "LOAD_ATTR" \
                            and ins[j].argval == "name":
                        continue  # only the step's name, such as in a question's text
                    consider(cast(object, value), inside=True)
        except Exception:  # code the verifier cannot read is skipped, never a crash
            return

    def consider(value: object, inside: bool = False) -> None:
        if isinstance(value, Step):
            if not any(value is s for s in found):
                found.append(cast(Step[Any], value))
        elif isinstance(value, FunctionType):
            visit(value)
        elif isinstance(value, MethodType):
            consider(value.__func__)
        elif isinstance(value, functools.partial):
            p = cast(functools.partial[Any], value)
            for part in (p.func, *p.args, *p.keywords.values()):
                consider(part)
        elif inside and isinstance(value, (dict, list, tuple, set, frozenset)):
            items = cast(dict[Any, Any], value).values() if isinstance(value, dict) else cast(Any, value)
            for item in items:  # one level: a table of steps or checks
                consider(item)
        elif not inside and callable(value) and not isinstance(value, type):
            consider(getattr(type(value), "__call__", None))  # a callable object given as the check

    consider(fn)
    return found


def _subscript(x: dis.Instruction) -> bool:
    return x.opname == "BINARY_SUBSCR" or (x.opname == "BINARY_OP" and x.argrepr == "[]")


def _stdlib(code: CodeType) -> bool:
    path = str(Path(code.co_filename).resolve())
    return path.startswith(_STDLIB) and "-packages" not in path


def _codes(code: CodeType) -> list[CodeType]:
    """A code object and the code nested in it: lambdas, comprehensions, inner functions."""
    return [code, *(c for k in code.co_consts if isinstance(k, CodeType) for c in _codes(k))]
