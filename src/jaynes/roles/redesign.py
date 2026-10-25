"""Redesign a project's interface: colours, type, motion and the markup of its screens.

Wider than the visual identity role, which changes style files only. It starts from references: the task
names one or more links or pictures of interfaces like the one wanted, and Jaynes captures them. The agent
reads the identity and the screens as they are, proposes one direction (palette, fonts, motion, layout) that
says what it takes from each reference, reading the web if it helps, then applies it to style and markup files, keeping what the screens
do. Logic, configuration and dependencies stay out of reach.

Some frameworks render markup on the server, so the preview runs the changed components on the person's
machine before anyone reads them; it does so in a box (``jaynes.sandbox``).
"""

from __future__ import annotations

from pydantic import BaseModel, Field

from jaynes.role import Fail, Pass, Witnesses, role, stop
from jaynes.roles import references as rf
from jaynes.roles import visual_identity as vi

MARKUP = ("**/*.html", "**/*.jsx", "**/*.tsx", "**/*.vue", "**/*.svelte")
MOTION = ("@keyframes", "animation:", "transition:")


class Screen(BaseModel):
    name: str
    file: str = Field(description="the file that lays it out, relative to the project folder")
    purpose: str = Field(description="what the person does there, in one sentence")


class Current(vi.Current):
    screens: list[Screen] = Field(max_length=20, description="the main screens and the files that lay them out")


class Direction(vi.Direction):
    motion: str = Field(description="what moves, when and how fast, and what stays still")
    layout: str = Field(description="what changes in the composition of the screens and their components, and why")
    references: list[rf.Taken] = Field(description="what the direction takes from each reference")


class Applied(vi.Applied):
    pass


def cited(out: Current, needs: Witnesses) -> Pass | Fail:
    """Colours and fonts are found where they are cited, and every screen's file exists."""
    assert needs.project is not None
    found = vi.cited_in_project(out, needs)
    reasons = list(found.reasons) if isinstance(found, Fail) else []
    reasons += [f"{s.file} does not exist in the project" for s in out.screens if not (needs.project / s.file).is_file()]
    return Fail(*list(dict.fromkeys(reasons))[:20]) if reasons else Pass()


def drawn_from(out: Direction, needs: Witnesses) -> Pass | Fail:
    """The direction says what it takes from every reference."""
    return rf.drawn_from(out.references, needs[REFERENCES].value)


def applied(out: Applied, needs: Witnesses) -> Pass | Fail:
    """The palette is in the style files, and any motion stops for people who ask for reduced motion."""
    assert needs.project is not None
    found = vi.palette_applied(out, needs)
    reasons = list(found.reasons) if isinstance(found, Fail) else []
    text = "\n".join(p.read_text(errors="replace").lower() for g in vi.STYLE_FILES + MARKUP
                     for p in needs.project.glob(g) if p.is_file() and "node_modules" not in p.parts)
    if any(m in text for m in MOTION) and "prefers-reduced-motion" not in text:
        reasons.append("the project animates but never checks prefers-reduced-motion; motion must stop for "
                       "people who ask for less")
    return Fail(*reasons) if reasons else Pass()


ROLE = role("redesign")
REFERENCES = ROLE.step("references", output=rf.References, run_by=rf.gather, max_turns=1, effect="web")
CURRENT = ROLE.step(
    "current", output=Current, project="read", check=cited,
    instructions="Read the project and describe its interface as it is today: the colours and fonts it uses and "
                 "where they are defined, and its main screens with the files that lay them out. Cite only what you "
                 "found; every value must appear in the file you cite, and files are cited relative to the project "
                 "folder (src/App.tsx, not project/src/App.tsx).")
DIRECTION = ROLE.step(
    "direction", output=Direction, needs=[REFERENCES, CURRENT], check=[vi.readable, drawn_from], stop=stop(0.2),
    web=True, question=lambda out, _: f"Apply the {out.name} direction?",
    instructions="Look at every reference picture in inputs/references/ first: they are interfaces the person wants "
                 "theirs to be like. Propose one direction for the interface that answers the task and draws on them, "
                 "and for each reference say concretely what you take from it. You may read the references' sites, look up "
                 "fonts and read documentation on the web. Give a palette with a colour for each "
                 f"of {', '.join(vi.ROLES)}, a display font and a body font (prefer Google Fonts), how things move, "
                 "and what changes in the layout of the screens and their components. Say what stays. Text must stay "
                 "readable: Jaynes checks WCAG AA contrast of text and muted text on the background and of text on "
                 "surfaces. If the person returned an earlier direction, depart from it as their note asks.")
APPLY = ROLE.step(
    "apply", output=Applied, needs=[REFERENCES, CURRENT, DIRECTION], project="write",
    may_change=vi.STYLE_FILES + MARKUP, check=applied, stop=stop(0.2), deadline=3600.0,
    question=lambda out, _: "Does the project look right with the redesign?",
    instructions="Apply the direction to the project: colours, fonts, motion, and the markup and classes of the "
                 "screens and their components. Where the direction leaves a detail open, judge it from the reference "
                 "pictures in inputs/references/. Keep what each screen does: the data it shows, its routes, handlers "
                 "and state stay as they are. Load fonts from CSS (an @import of Google Fonts). Animate with CSS, and "
                 "stop the motion under @media (prefers-reduced-motion: reduce).")
ROLE.step("accept", needs=[APPLY], stop=stop(0.05, sampled=False))
