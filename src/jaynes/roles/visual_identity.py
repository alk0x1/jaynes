"""Redesign a project's visual identity: colours and type, applied to its style files.

The agent first reads what the identity is today, citing where each colour and font is defined; then
proposes one direction, whose contrast Jaynes computes; then applies it to the project's style files only.
The person sees the direction, then the project before and after, then gives the final review. Accepted
changes come back as a branch.
"""

from __future__ import annotations

import re
from pathlib import Path

from pydantic import BaseModel, Field

from jaynes.role import Fail, Pass, Witnesses, role, stop

ROLES = ("background", "surface", "text", "muted", "accent", "border")
STYLE_FILES = ("**/*.css", "**/*.scss", "**/*.sass", "**/*.less", "**/*.svg")
HEX = re.compile(r"#(?:[0-9a-fA-F]{3}){1,2}")


class Color(BaseModel):
    name: str = Field(description="what the colour is for, as the project names it")
    value: str = Field(description="the colour as written in the file, e.g. #1a73e8")
    file: str = Field(description="where it is defined, relative to the project folder, e.g. src/theme.css")


class Font(BaseModel):
    family: str
    file: str = Field(description="where it is set, relative to the project folder")


class Current(BaseModel):
    summary: str = Field(description="what the identity looks and feels like today, in two or three sentences")
    colors: list[Color] = Field(max_length=40)
    fonts: list[Font] = Field(max_length=10)
    style_files: list[str] = Field(description="project paths where the identity is defined")


class Swatch(BaseModel):
    role: str = Field(description=f"one of {', '.join(ROLES)}, or another role the project needs")
    value: str = Field(description="a hex colour, e.g. #0f1115")


class Direction(BaseModel):
    name: str
    idea: str = Field(description="the feeling it aims for and why it answers the task, in one paragraph")
    palette: list[Swatch]
    display_font: str
    body_font: str
    keep: list[str] = Field(default_factory=list, description="what stays from the current identity")


class Applied(BaseModel):
    summary: str = Field(description="what changed, for the person reviewing the preview")
    files: list[str]


def cited_in_project(out: Current, needs: Witnesses) -> Pass | Fail:
    """Every colour and font is found in the file it cites."""
    assert needs.project is not None
    reasons: list[str] = []
    for item, text in [(c, c.value) for c in out.colors] + [(f, f.family) for f in out.fonts]:
        path = needs.project / item.file
        if not path.is_file():
            reasons.append(f"{item.file} does not exist in the project")
        elif text.lower() not in path.read_text(errors="replace").lower():
            reasons.append(f"{text} is not in {item.file}")
    reasons = list(dict.fromkeys(reasons))
    return Fail(*reasons[:20]) if reasons else Pass()


def luminance(hex_color: str) -> float:
    h = hex_color.lstrip("#")
    h = "".join(c * 2 for c in h) if len(h) == 3 else h
    channels = [int(h[i:i + 2], 16) / 255 for i in (0, 2, 4)]
    r, g, b = [c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4 for c in channels]
    return 0.2126 * r + 0.7152 * g + 0.0722 * b


def contrast(a: str, b: str) -> float:
    """The WCAG 2 contrast ratio between two hex colours."""
    hi, lo = sorted((luminance(a), luminance(b)), reverse=True)
    return (hi + 0.05) / (lo + 0.05)


def readable(out: Direction, needs: Witnesses) -> Pass | Fail:
    """Every role is present as a hex colour, and text meets WCAG AA on the backgrounds it sits on."""
    by_role = {s.role: s.value for s in out.palette}
    reasons = [f"the palette has no {r}" for r in ROLES if r not in by_role]
    reasons += [f"{s.role} is {s.value!r}, not a hex colour" for s in out.palette if not HEX.fullmatch(s.value)]
    if reasons:
        return Fail(*reasons)
    pairs = [("text", "background", 4.5), ("text", "surface", 4.5), ("muted", "background", 4.5),
             ("accent", "background", 3.0)]
    reasons = [f"{fg} on {bg} has contrast {contrast(by_role[fg], by_role[bg]):.2f}; it needs {need}"
               for fg, bg, need in pairs if contrast(by_role[fg], by_role[bg]) < need]
    return Fail(*reasons) if reasons else Pass()


def in_style_files(project: Path, palette: list[Swatch]) -> Pass | Fail:
    """Every colour of the palette appears in the project's style files."""
    text = "\n".join(p.read_text(errors="replace").lower() for g in STYLE_FILES
                     for p in project.glob(g) if p.is_file() and "node_modules" not in p.parts)
    missing = [f"{s.role} {s.value}" for s in palette if s.value.lower() not in text]
    return Fail(f"these colours appear in no style file: {', '.join(missing)}") if missing else Pass()


def palette_applied(out: Applied, needs: Witnesses) -> Pass | Fail:
    """Every colour of the direction appears in the project's style files."""
    assert needs.project is not None
    return in_style_files(needs.project, needs[DIRECTION].value.palette)


ROLE = role("visual-identity")
CURRENT = ROLE.step(
    "current", output=Current, project="read", check=cited_in_project,
    instructions="Read the project and describe its visual identity as it is today: the colours and fonts it "
                 "uses and the files where they are defined. Cite only what you found; every value must appear "
                 "in the file you cite, and files are cited relative to the project folder (styles.css, not "
                 "project/styles.css).")
DIRECTION = ROLE.step(
    "direction", output=Direction, needs=[CURRENT], check=readable, stop=stop(0.2),
    question=lambda out, _: f"Apply the {out.name} direction?",
    instructions="Propose one direction for the identity that answers the task: a palette with a colour for "
                 f"each of {', '.join(ROLES)}, a display font and a body font. Prefer fonts available from Google "
                 "Fonts. Say what stays from the current identity. Text must stay readable: Jaynes checks WCAG AA "
                 "contrast of text and muted text on the background and of text on surfaces.")
APPLY = ROLE.step(
    "apply", output=Applied, needs=[CURRENT, DIRECTION], project="write", may_change=STYLE_FILES,
    check=palette_applied, stop=stop(0.2), deadline=1800.0,
    question=lambda out, _: "Does the project look right with the new identity?",
    instructions="Apply the direction to the project. Change colours and fonts where the identity is defined, "
                 "preferring the project's existing variables or tokens over new ones. Load fonts from CSS (an "
                 "@import of Google Fonts), since only style files may change. Do not restructure layouts.")
ROLE.step("accept", needs=[APPLY], stop=stop(0.05, sampled=False))
