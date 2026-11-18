"""Where a quote stands in the page Jaynes downloaded, and the window of text around it, for the person to judge
whether the source says it or only mentions it. Run from engine/:
    PYTHONPATH=python python3 -m unittest discover -s python/tests
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
sys.path.insert(0, str(Path(__file__).resolve().parent))

import jaynes_menu as menu  # noqa: E402
from jaynes_role import Refused  # noqa: E402
from test_menu_pages import HTML, LONG, READ, step  # noqa: E402

NEGATED = ("<p>Some guides claim that a cache MAY keep a no-store response for an hour. That is wrong: the "
           "specification forbids it outright.</p><p>" + LONG + "</p>").encode()


class Where(unittest.TestCase):
    def test_a_quote_found_says_where_it_stands_and_what_is_around_it(self):
        got = step({"read": READ})
        first = got["pages"][0]["quotes"][0]
        self.assertTrue(first["found"])
        self.assertIsInstance(first["block"], int)
        self.assertIsInstance(first["at"], int)
        self.assertIn("»A cache MUST NOT store a response when the no-store directive is present.«", first["window"])
        # the words before and after the quote, from the page itself
        self.assertIn("Caching", first["window"])

    def test_a_quote_not_found_has_no_place(self):
        got = step({"read": READ})
        second = got["pages"][0]["quotes"][1]
        self.assertFalse(second["found"])
        self.assertEqual((second["block"], second["at"], second["window"]), (None, None, ""))

    def test_the_window_shows_a_source_that_mentions_a_claim_to_deny_it(self):
        url = "https://example.org/guide"
        got = step({"read": {"sources": [{"url": url, "quotes": ["a cache MAY keep a no-store response for an hour"]}]}}, pages={url: NEGATED})
        q = got["pages"][0]["quotes"][0]
        # the words are on the page, so the check of code finds them: what they mean is the person's to judge
        self.assertTrue(q["found"])
        self.assertIn("Some guides claim that", q["window"])
        self.assertIn("That is wrong", q["window"])

    def test_a_quote_that_stands_in_more_than_one_place_says_how_many(self):
        url = "https://example.org/rfc"
        body = (f"<h2>The request directive</h2><p>It applies to both private and shared caches.</p>"
                f"<h2>The response directive</h2><p>It applies to both private and shared caches.</p><p>{LONG}</p>").encode()
        out = windows([{"url": url, "quote": "It applies to both private and shared caches."}],
                      fetched=step({"read": {"sources": [{"url": url, "quotes": []}]}}, pages={url: body}))
        q = out["quotes"][0]
        self.assertEqual(q["places"], 2)
        self.assertIn("in 2 places on the page; the first:", out["windows"])

    def test_blocks_are_parted_by_a_mark_pages_do_not_use_themselves(self):
        # a page's own pilcrow, as RFC pages have after each paragraph, is not a break between blocks
        got = step({"read": READ})
        window = got["pages"][0]["quotes"][0]["window"]
        self.assertIn("Caching \u2016 \u00bbA cache", window)

    def test_the_window_is_bounded(self):
        got = step({"read": READ})
        window = got["pages"][0]["quotes"][0]["window"]
        quote = got["pages"][0]["quotes"][0]["quote"]
        self.assertLessEqual(len(window), len(quote) + 2 * menu.WINDOW_CHARS + 4)


def windows(claims: list[dict[str, Any]], fetched: Any = None, beside: str | None = None) -> Any:
    given = [{"step": "note", "output": {"claims": claims, "reading": "Reading, not measured: it matters."}},
             {"step": "fetch", "output": step({"read": READ}) if fetched is None else fetched}]
    params = {"claims": "note.claims", "pages": "fetch"}
    if beside:
        params["beside"] = beside
    return menu.quote_windows({"given": given}, params)


class QuoteWindows(unittest.TestCase):
    def test_each_quote_of_the_note_is_shown_in_its_page(self):
        out = windows([{"url": "https://example.org/caching", "quote": "A cache MUST NOT store a response when the no-store"}])
        self.assertEqual(out["judged"], 1)
        q = out["quotes"][0]
        self.assertEqual((q["n"], q["url"], q["found"]), (1, "https://example.org/caching", True))
        self.assertEqual(q["page_sha256"], step({"read": READ})["pages"][0]["sha256"])
        self.assertIn("»A cache MUST NOT store a response when the no-store«", q["window"])
        self.assertIn('1. "A cache MUST NOT store a response when the no-store"', out["windows"])
        self.assertIn("https://example.org/caching", out["windows"])

    def test_a_quote_not_on_its_page_says_why_in_place_of_a_window(self):
        out = windows([{"url": "https://example.org/caching", "quote": "A cache MAY store anything it likes, forever."},
                       {"url": "https://example.org/gone", "quote": "This page used to say something."}])
        self.assertEqual([q["found"] for q in out["quotes"]], [False, False])
        self.assertIn("not on the page: these words are not in the text Jaynes downloaded", out["windows"])
        self.assertIn("not on the page: the page was not read: the server answered 404", out["windows"])

    def test_nothing_to_show_is_said(self):
        out = windows([])
        self.assertEqual(out["judged"], 0)
        self.assertEqual(out["windows"], "No quotation to show: the note quotes nothing.")

    def test_a_text_of_an_earlier_step_goes_beside_the_quotes_as_it_was(self):
        out = windows([], beside="note.reading")
        self.assertEqual(out["beside"], "Reading, not measured: it matters.")

    def test_without_what_was_downloaded_the_step_refuses(self):
        given = [{"step": "note", "output": {"claims": []}}, {"step": "fetch", "output": {"pages": "x"}}]
        self.assertIsInstance(menu.quote_windows({"given": given}, {"claims": "note.claims", "pages": "fetch"}), Refused)

    def test_the_step_is_on_the_menu(self):
        self.assertIn("quote-windows", menu.STEPS)


if __name__ == "__main__":
    unittest.main()
