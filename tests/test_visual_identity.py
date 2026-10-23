"""The built-in visual identity role: its checks, on a small project."""

from __future__ import annotations

from pathlib import Path

from jaynes.role import Checked, Fail, Pass, Witnesses
from jaynes.roles import visual_identity as vi


def project(tmp_path: Path) -> Path:
    p = tmp_path / "project"
    (p / "src").mkdir(parents=True)
    (p / "src" / "theme.css").write_text(":root { --bg: #FFFFFF; --ink: #111111; font-family: Inter, sans-serif; }\n")
    return p


def palette(**overrides: str) -> list[vi.Swatch]:
    base = {"background": "#0f1115", "surface": "#171a21", "text": "#eef1f5", "muted": "#a3abb8",
            "accent": "#6fb4ff", "border": "#2a303b"}
    return [vi.Swatch(role=k, value=v) for k, v in {**base, **overrides}.items()]


def direction(**overrides: str) -> vi.Direction:
    return vi.Direction(name="Night", idea="Calm and dark.", palette=palette(**overrides),
                        display_font="Instrument Sans", body_font="Inter", keep=[])


def test_current_colors_and_fonts_must_be_found_where_they_are_cited(tmp_path: Path) -> None:
    needs = Witnesses({}, project(tmp_path))
    good = vi.Current(summary="Light.", colors=[vi.Color(name="bg", value="#ffffff", file="src/theme.css")],
                      fonts=[vi.Font(family="Inter", file="src/theme.css")], style_files=["src/theme.css"])
    assert isinstance(vi.cited_in_project(good, needs), Pass)
    bad = vi.Current(summary="Light.", colors=[vi.Color(name="bg", value="#123456", file="src/theme.css"),
                                               vi.Color(name="x", value="#ffffff", file="src/missing.css")],
                     fonts=[vi.Font(family="Roboto", file="src/theme.css")], style_files=["src/theme.css"])
    result = vi.cited_in_project(bad, needs)
    assert isinstance(result, Fail)
    assert any("#123456" in r for r in result.reasons) and any("src/missing.css" in r for r in result.reasons)
    assert any("Roboto" in r for r in result.reasons)


def test_a_direction_needs_every_role_and_readable_contrast() -> None:
    assert isinstance(vi.readable(direction(), Witnesses({})), Pass)
    low = vi.readable(direction(muted="#2b2f36"), Witnesses({}))
    assert isinstance(low, Fail) and any("muted" in r and "4.5" in r for r in low.reasons)
    missing = vi.readable(vi.Direction(name="x", idea="y", palette=palette()[:3], display_font="a", body_font="b"),
                          Witnesses({}))
    assert isinstance(missing, Fail) and any("accent" in r for r in missing.reasons)
    assert isinstance(vi.readable(direction(text="blue"), Witnesses({})), Fail)


def test_the_contrast_ratio_matches_wcag() -> None:
    assert round(vi.contrast("#000000", "#ffffff"), 1) == 21.0
    assert round(vi.contrast("#777777", "#ffffff"), 2) == 4.48


def test_the_applied_palette_must_appear_in_the_project(tmp_path: Path) -> None:
    p = project(tmp_path)
    chosen = Checked("direction", direction(), "h", "c")
    needs = Witnesses({"direction": chosen}, p)
    assert isinstance(vi.palette_applied(vi.Applied(summary="s", files=["src/theme.css"]), needs), Fail)
    (p / "src" / "theme.css").write_text(":root {" + " ".join(f"--{s.role}: {s.value.upper()};"
                                                                for s in direction().palette) + "}\n")
    assert isinstance(vi.palette_applied(vi.Applied(summary="s", files=["src/theme.css"]), needs), Pass)


def test_the_role_reads_then_changes_only_style_files_and_ends_at_a_final_review() -> None:
    steps = {s.name: s for s in vi.ROLE.steps}
    assert list(steps) == ["current", "direction", "apply", "accept"]
    assert steps["current"].project == "read" and steps["apply"].project == "write"
    assert "**/*.css" in steps["apply"].may_change and not any(g.endswith((".js", ".ts", ".html"))
                                                               for g in steps["apply"].may_change)
    assert steps["direction"].stop is not None and steps["apply"].stop is not None
    final = steps["accept"].stop
    assert final is not None and not final.sampled
