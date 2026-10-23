"""The built-in redesign role: identity, motion and the markup of screens, with behaviour left alone."""

from __future__ import annotations

from pathlib import Path

import pytest

from jaynes.project import allowed
from jaynes.role import Checked, Fail, Pass, StepContext, Witnesses
from jaynes.roles import redesign as rd
from jaynes.roles import references as rf
from jaynes.roles import visual_identity as vi
from tests.test_visual_identity import direction, project


def test_screens_must_exist_where_they_are_cited(tmp_path: Path) -> None:
    p = project(tmp_path)
    (p / "src" / "App.tsx").write_text("export default () => <main/>\n")
    needs = Witnesses({}, p)

    def current(screen: str) -> rd.Current:
        return rd.Current(summary="Light.", colors=[vi.Color(name="bg", value="#ffffff", file="src/theme.css")],
                          fonts=[vi.Font(family="Inter", file="src/theme.css")], style_files=["src/theme.css"],
                          screens=[rd.Screen(name="home", file=screen, purpose="the list")])
    assert isinstance(rd.cited(current("src/App.tsx"), needs), Pass)
    bad = current("src/Home.tsx")
    result = rd.cited(bad, needs)
    assert isinstance(result, Fail) and any("src/Home.tsx" in r for r in result.reasons)


def test_motion_must_respect_reduced_motion(tmp_path: Path) -> None:
    p = project(tmp_path)
    chosen = Checked("direction", rd.Direction(**direction().model_dump(), motion="m", layout="l", references=[]), "h", "c")
    needs = Witnesses({"direction": chosen}, p)
    css = ":root {" + " ".join(f"--{s.role}: {s.value};" for s in direction().palette) + "}\n"
    (p / "src" / "theme.css").write_text(css + "@keyframes rise { from { opacity: 0 } }\n.card { animation: rise .3s; }\n")
    applied = rd.Applied(summary="s", files=["src/theme.css"])
    result = rd.applied(applied, needs)
    assert isinstance(result, Fail) and any("prefers-reduced-motion" in r for r in result.reasons)
    (p / "src" / "theme.css").write_text(css + "@media (prefers-reduced-motion: reduce) { * { animation: none; } }\n")
    assert isinstance(rd.applied(applied, needs), Pass)


def test_apply_may_change_markup_and_styles_but_not_logic_or_config() -> None:
    steps = {s.name: s for s in rd.ROLE.steps}
    assert list(steps) == ["references", "current", "direction", "apply", "accept"]
    globs = steps["apply"].may_change
    assert allowed(["src/App.tsx", "src/Card.vue", "index.html", "src/app.css", "public/logo.svg"], globs) == []
    assert allowed(["src/api.ts", "vite.config.ts", "package.json"], globs) == [
        "src/api.ts", "vite.config.ts", "package.json"]
    final = steps["accept"].stop
    assert final is not None and not final.sampled


def test_references_are_the_links_and_pictures_the_task_names() -> None:
    task = ("Make it calm like https://linear.app, and https://vercel.com/dashboard. Also "
            "~/Desktop/Screenshot\\ 2026-10-23\\ at\\ 10.00.00.png and \"/tmp/a b.JPG\" but not notes.png")
    assert rf.named(task) == ["https://linear.app", "https://vercel.com/dashboard",
                              "~/Desktop/Screenshot 2026-10-23 at 10.00.00.png", "/tmp/a b.JPG"]
    assert rf.named("no references here") == []


def test_pictures_are_copied_and_a_task_without_references_is_refused(tmp_path: Path) -> None:
    picture = tmp_path / "ref shot.png"
    picture.write_bytes(b"\x89PNG")
    ws = tmp_path / "ws"
    ws.mkdir()
    out = rf.gather(StepContext("r1", f"like {picture}", Witnesses({}), ws))
    assert out.references == [rf.Reference(name="ref-1", source=str(picture), file="ref-1.png")]
    assert (ws / "files" / "ref-1.png").read_bytes() == b"\x89PNG"
    with pytest.raises(ValueError, match="names no reference"):
        rf.gather(StepContext("r1", "make it nicer", Witnesses({}), ws))


def test_a_direction_says_what_it_takes_from_every_reference() -> None:
    given = rf.References(references=[rf.Reference(name="ref-1", source="https://linear.app", file="ref-1.png"),
                                      rf.Reference(name="ref-2", source="/tmp/a.png", file="ref-2.png")])
    full = "the near-black surfaces, thin grey borders and dense sidebar list"
    assert isinstance(rf.drawn_from([rf.Taken(reference="ref-1", took=full),
                                     rf.Taken(reference="ref-2", took=full)], given), Pass)
    result = rf.drawn_from([rf.Taken(reference="ref-1", took="the colours"),
                            rf.Taken(reference="ref-9", took=full)], given)
    assert isinstance(result, Fail)
    assert any("ref-2" in r and "/tmp/a.png" in r for r in result.reasons)
    assert any("ref-9 is not a reference" in r for r in result.reasons)
    assert any("ref-1" in r and "vague" in r for r in result.reasons)
