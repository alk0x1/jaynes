"""The menu's pages: what Jaynes downloads itself, what it refuses to download, and which quotes it finds.

No test reaches the network: the name lookup and the connection are passed in. Run from engine/:
    PYTHONPATH=python python3 -m unittest discover -s python/tests
"""

from __future__ import annotations

import socket
import sys
import unittest
from pathlib import Path
from typing import Any, Callable

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import jaynes_menu as menu  # noqa: E402
from jaynes_role import Blocked, Fail, Pass, Refused  # noqa: E402

PUBLIC = "93.184.216.34"
LONG = "Caches keep copies of responses so that later requests are answered faster. " * 4
HTML = (f"<html><head><title>Caching</title><style>p {{ color: red }}</style></head><body><h1>Caching</h1>"
        f"<p>A cache MUST NOT store a response when the <code>no-store</code> directive is present.</p>"
        f"<script>var secret = 'only in a script';</script><p>{LONG}</p></body></html>").encode()


class Net:
    """A network that is only a table: names to addresses, and addresses of pages to answers."""

    def __init__(self, names: dict[str, list[str]] | None = None, pages: dict[str, Any] | None = None) -> None:
        self.names = names if names is not None else {"example.org": [PUBLIC]}
        self.pages = pages or {}
        self.sent: list[tuple[str, str, str]] = []
        self.now = 0.0

    def resolve(self, host: str) -> list[str]:
        if host not in self.names:
            raise socket.gaierror("unknown name")
        return self.names[host]

    def send(self, host: str, ip: str, target: str, timeout: float) -> tuple[int, dict[str, str], Callable[[int], bytes]]:
        self.sent.append((host, ip, target))
        answer = self.pages.get(f"{host}{target}")
        if answer is None:
            return 404, {}, lambda n: b""
        if isinstance(answer, Exception):
            raise answer
        status, headers, body = answer
        left = [body]

        def read(n: int) -> bytes:
            self.now += getattr(self, "slow", 0.0)
            piece, left[0] = left[0][:n], left[0][n:]
            return piece

        return status, headers, read

    def download(self, url: str) -> dict[str, Any]:
        return menu.download(url, resolve=self.resolve, send=self.send, clock=lambda: self.now)


def page(body: bytes = HTML, kind: str = "text/html; charset=utf-8", status: int = 200, **headers: str) -> tuple[int, dict[str, str], bytes]:
    return status, {"content-type": kind, **headers}, body


class SameWords(unittest.TestCase):
    def test_spaces_and_line_breaks_do_not_matter(self):
        self.assertEqual(menu.same_words("a  cache\n MUST NOT\tstore "), "a cache MUST NOT store")

    def test_every_quotation_mark_is_one_mark(self):
        self.assertEqual(menu.same_words("“don’t cache”"), menu.same_words("\"don't cache\""))

    def test_every_dash_is_one_dash(self):
        self.assertEqual(menu.same_words("best‑effort — always"), "best-effort - always")

    def test_characters_that_do_not_show_are_dropped(self):
        self.assertEqual(menu.same_words("no­store​"), "nostore")

    def test_case_and_letters_matter(self):
        self.assertNotEqual(menu.same_words("MUST NOT"), menu.same_words("must not"))
        self.assertFalse(menu.held("a cache MUST NOT store a response", "a cache SHOULD NOT store a response"))

    # a word wrapped at its hyphen, as RFC 9111's text has "MUST make a best-" at the end of a line and "effort
    # attempt" at the start of the next
    RFC = ("   store the information in non-volatile storage and MUST make a best-\n"
           "   effort attempt to remove the information from volatile storage as\n")

    def test_a_word_wrapped_at_its_hyphen_is_one_word(self):
        self.assertTrue(menu.held(self.RFC, "MUST make a best-effort attempt to remove the information from volatile storage"))
        self.assertTrue(menu.held("best-\r\n  effort", "best-effort"))

    def test_the_quote_is_read_by_the_same_rule(self):
        self.assertTrue(menu.held("MUST make a best-effort attempt", "MUST make a best-\neffort attempt"))

    def test_a_wrapped_word_keeps_its_hyphen(self):
        self.assertFalse(menu.held(self.RFC, "MUST make a best effort attempt"))
        self.assertFalse(menu.held(self.RFC, "MUST make a besteffort attempt"))
        self.assertFalse(menu.held("the remo-\nval of the entry", "the removal of the entry"))

    def test_a_hyphen_and_a_space_on_one_line_stay_apart(self):
        self.assertFalse(menu.held("both pre- and post-processing", "both pre-and post-processing"))
        self.assertFalse(menu.held("MUST make a best- effort attempt", "MUST make a best-effort attempt"))

    def test_a_hyphen_before_a_blank_line_stays_apart(self):
        self.assertFalse(menu.held("a list ending in non-\n\nvolatile things", "non-volatile things"))
        self.assertFalse(menu.held("non-\n\n- next", "non-- next"))

    def test_a_hyphen_at_the_end_of_an_html_block_does_not_join_the_next(self):
        text = menu.page_text(f"<ul><li>keep it non-</li><li>volatile storage</li></ul><p>{LONG}</p>".encode(), "text/html")
        self.assertFalse(menu.held(text, "non-volatile storage"))
        self.assertTrue(menu.held(text, "keep it non-"))

    def test_a_quote_is_held_whatever_its_spacing(self):
        self.assertTrue(menu.held("a cache MUST NOT\n   store a response", "MUST NOT store  a response"))


class PageText(unittest.TestCase):
    def test_html_gives_the_text_a_reader_sees(self):
        text = menu.page_text(HTML, "text/html; charset=utf-8")
        self.assertTrue(menu.held(text, "A cache MUST NOT store a response when the no-store directive is present."))
        self.assertNotIn("only in a script", text)
        self.assertNotIn("color: red", text)

    def test_blocks_are_apart_and_inline_marks_are_not(self):
        text = menu.page_text(f"<p>one</p><p>two <b>bold</b>er</p><p>{LONG}</p>".encode(), "text/html")
        self.assertIn("one two bolder", menu.same_words(text))

    def test_entities_are_the_characters_they_name(self):
        text = menu.page_text(f"<p>a &lt; b &amp; c&nbsp;d</p><p>{LONG}</p>".encode(), "text/html")
        self.assertIn("a < b & c d", menu.same_words(text))

    def test_plain_text_is_itself(self):
        self.assertEqual(menu.page_text(b"3.  A cache MUST NOT store", "text/plain"), "3.  A cache MUST NOT store")

    def test_the_charset_the_server_names_is_used(self):
        self.assertEqual(menu.page_text("café".encode("latin-1"), "text/plain; charset=ISO-8859-1"), "café")

    def test_the_charset_the_page_names_is_used(self):
        body = ('<html><head><meta charset="iso-8859-1"></head><body><p>café ' + LONG + "</p></body></html>").encode("latin-1")
        self.assertIn("café", menu.page_text(body, "text/html"))

    def test_a_page_a_script_puts_together_is_not_read(self):
        with self.assertRaises(menu.Unread) as e:
            menu.page_text(b'<html><body><div id="root"></div><script src="/app.js"></script></body></html>', "text/html")
        self.assertIn("script", str(e.exception))

    def test_a_format_jaynes_does_not_read_is_not_read(self):
        with self.assertRaises(menu.Unread) as e:
            menu.page_text(b"%PDF-1.7 ...", "application/pdf")
        self.assertIn("application/pdf", str(e.exception))

    def test_bytes_that_are_not_text_are_not_read(self):
        with self.assertRaises(menu.Unread):
            menu.page_text(b"\x89PNG\x00\x00\x00data", "text/plain")


class Download(unittest.TestCase):
    def refused(self, url: str, saying: str, net: Net | None = None) -> Net:
        net = net or Net()
        with self.assertRaises(menu.Unread) as e:
            net.download(url)
        self.assertIn(saying, str(e.exception))
        return net

    def test_a_public_https_page_is_downloaded_from_the_address_that_was_checked(self):
        net = Net(pages={"example.org/a?b=1": page()})
        got = net.download("https://example.org/a?b=1")
        self.assertEqual(got["body"], HTML)
        self.assertEqual(got["final_url"], "https://example.org/a?b=1")
        self.assertEqual(net.sent, [("example.org", PUBLIC, "/a?b=1")])

    def test_only_https(self):
        for url in ["http://example.org/", "ftp://example.org/x", "file:///etc/passwd", "example.org/x", "https://"]:
            net = self.refused(url, "https")
            self.assertEqual(net.sent, [], url)

    def test_no_name_and_password_in_the_address(self):
        self.assertEqual(self.refused("https://user:pw@example.org/", "name or password").sent, [])

    def test_no_port_but_the_usual(self):
        self.assertEqual(self.refused("https://example.org:8443/", "port").sent, [])
        net = Net(pages={"example.org/": page()})
        self.assertEqual(net.download("https://example.org:443/")["body"], HTML)

    def test_nothing_on_this_machine_or_a_private_network(self):
        for ip in ["127.0.0.1", "10.0.0.5", "172.16.3.1", "192.168.1.10", "169.254.169.254", "100.64.0.1", "0.0.0.0",
                   "::1", "fc00::1", "fe80::1", "::ffff:10.0.0.5", "224.0.0.1"]:
            net = self.refused("https://inside.example/", "public internet", Net(names={"inside.example": [ip]}))
            self.assertEqual(net.sent, [], ip)

    def test_an_address_written_as_numbers_is_checked_too(self):
        for url in ["https://127.0.0.1/", "https://[::1]/", "https://192.168.0.1/admin", "https://localhost/"]:
            net = Net(names={"localhost": ["127.0.0.1"]})
            net.resolve = lambda host, names=net.names: names.get(host) or [host]  # a number resolves to itself
            self.assertEqual(self.refused(url, "public internet", net).sent, [], url)

    def test_a_name_with_one_private_address_among_public_ones_is_refused(self):
        net = Net(names={"mixed.example": [PUBLIC, "10.0.0.5"]})
        self.assertEqual(self.refused("https://mixed.example/", "public internet", net).sent, [])

    def test_a_name_that_does_not_exist_is_not_read(self):
        self.refused("https://nowhere.example/", "not found")

    def test_a_redirect_is_followed_to_a_public_page(self):
        net = Net(names={"example.org": [PUBLIC], "www.example.org": ["93.184.216.35"]},
                  pages={"example.org/old": page(b"", status=301, location="https://www.example.org/new"), "www.example.org/new": page()})
        got = net.download("https://example.org/old")
        self.assertEqual(got["final_url"], "https://www.example.org/new")
        self.assertEqual(net.sent[-1], ("www.example.org", "93.184.216.35", "/new"))

    def test_a_relative_redirect_stays_on_the_host(self):
        net = Net(pages={"example.org/old": page(b"", status=302, location="/new"), "example.org/new": page()})
        self.assertEqual(net.download("https://example.org/old")["final_url"], "https://example.org/new")

    def test_a_redirect_into_a_private_network_is_refused(self):
        net = Net(names={"example.org": [PUBLIC], "inside.example": ["10.0.0.5"]},
                  pages={"example.org/": page(b"", status=302, location="https://inside.example/secret")})
        self.refused("https://example.org/", "public internet", net)
        self.assertEqual(len(net.sent), 1)

    def test_a_redirect_to_this_machine_by_number_is_refused(self):
        net = Net(pages={"example.org/": page(b"", status=307, location="https://169.254.169.254/latest/meta-data")})
        net.resolve = lambda host, names=net.names: names.get(host) or [host]
        self.refused("https://example.org/", "public internet", net)
        self.assertEqual(len(net.sent), 1)

    def test_a_redirect_out_of_https_is_refused(self):
        net = Net(pages={"example.org/": page(b"", status=301, location="http://example.org/plain")})
        self.refused("https://example.org/", "https", net)

    def test_too_many_redirects_end(self):
        net = Net(pages={f"example.org/{n}": page(b"", status=302, location=f"/{n + 1}") for n in range(20)})
        self.refused("https://example.org/0", "redirected", net)
        self.assertEqual(len(net.sent), menu.PAGE_REDIRECTS + 1)

    def test_an_answer_that_is_not_the_page_is_not_read(self):
        self.refused("https://example.org/missing", "404")
        self.refused("https://example.org/", "403", Net(pages={"example.org/": page(b"blocked", status=403)}))

    def test_a_page_past_the_size_limit_is_not_read(self):
        net = Net(pages={"example.org/": page(b"x" * (menu.PAGE_BYTES + 1), "text/plain")})
        self.refused("https://example.org/", "larger than", net)

    def test_a_page_at_the_size_limit_is_read(self):
        net = Net(pages={"example.org/": page(b"x" * menu.PAGE_BYTES, "text/plain")})
        self.assertEqual(len(net.download("https://example.org/")["body"]), menu.PAGE_BYTES)

    def test_a_page_past_the_time_limit_is_not_read(self):
        net = Net(pages={"example.org/": page(b"x" * 500_000, "text/plain")})
        net.slow = menu.PAGE_SECONDS / 2
        self.refused("https://example.org/", "longer than", net)

    def test_a_connection_that_fails_is_not_read(self):
        self.refused("https://example.org/", "connection", Net(pages={"example.org/": ConnectionResetError("reset")}))
        self.refused("https://example.org/", "connection", Net(pages={"example.org/": socket.timeout("timed out")}))


def fake(pages: dict[str, Any]) -> Callable[[str], dict[str, Any]]:
    def download(url: str) -> dict[str, Any]:
        got = pages.get(url)
        if got is None:
            raise menu.Unread("the server answered 404")
        if isinstance(got, Exception):
            raise got
        return {"final_url": url, "type": "text/html", "body": got}
    return download


READ = {"sources": [
    {"url": "https://example.org/caching", "quotes": ["A cache MUST NOT store a response when the no-store directive is present.",
                                                      "A cache MAY store anything it likes, forever."]},
    {"url": "https://example.org/gone", "quotes": ["This page used to say something."]},
]}
PAGES = {"https://example.org/caching": HTML}


def step(given: dict[str, Any], field: str = "read.sources", pages: dict[str, Any] | None = None) -> Any:
    call = {"given": [{"step": k, "output": v} for k, v in given.items()]}
    return menu.fetch_pages(call, {"pages": field}, download=fake(PAGES if pages is None else pages))


class FetchPages(unittest.TestCase):
    def test_each_page_is_read_or_said_not_to_be_and_why(self):
        out = step({"read": READ})
        first, second = out["pages"]
        self.assertEqual((first["url"], first["read"], first["why"]), ("https://example.org/caching", True, ""))
        self.assertEqual((second["read"], second["why"]), (False, "the server answered 404"))
        self.assertTrue(out["any_read"])

    def test_the_text_that_was_downloaded_is_kept_with_its_fingerprint(self):
        import hashlib
        out = step({"read": READ})
        self.assertIn("A cache MUST NOT store", out["texts"]["https://example.org/caching"])
        self.assertNotIn("https://example.org/gone", out["texts"])
        self.assertEqual(out["pages"][0]["sha256"], hashlib.sha256(HTML).hexdigest())
        self.assertEqual(out["pages"][0]["bytes"], len(HTML))

    def test_each_quote_listed_with_a_page_is_found_or_not(self):
        out = step({"read": READ})
        self.assertEqual([q["found"] for q in out["pages"][0]["quotes"]], [True, False])
        self.assertEqual([q["found"] for q in out["pages"][1]["quotes"]], [False])

    def test_a_list_of_addresses_alone_is_enough(self):
        out = step({"read": {"links": ["https://example.org/caching", "https://example.org/caching"]}}, "read.links")
        self.assertEqual(len(out["pages"]), 1)
        self.assertNotIn("quotes", out["pages"][0])

    def test_a_page_in_a_format_jaynes_does_not_read_is_not_read(self):
        def download(url: str) -> dict[str, Any]:
            return {"final_url": url, "type": "application/pdf", "body": b"%PDF-1.7"}
        call = {"given": [{"step": "read", "output": READ}]}
        out = menu.fetch_pages(call, {"pages": "read.sources"}, download=download)
        self.assertFalse(out["any_read"])
        self.assertIn("application/pdf", out["pages"][0]["why"])

    def test_nothing_read_is_said(self):
        self.assertFalse(step({"read": READ}, pages={})["any_read"])

    def test_no_list_of_pages_is_refused(self):
        self.assertIsInstance(step({"read": {"sources": "https://example.org/"}}), Refused)
        self.assertIsInstance(step({"read": {"sources": []}}), Refused)
        self.assertIsInstance(step({"read": {"sources": [{"title": "no address"}]}}), Refused)

    def test_more_pages_than_the_limit_is_refused(self):
        many = {"links": [f"https://example.org/{n}" for n in range(menu.PAGES_MAX + 1)]}
        self.assertIsInstance(step({"read": many}, "read.links"), Refused)


def check(output: dict[str, Any], fetched: Any = "fetch", field: str = "claims") -> Any:
    given = [] if fetched is None else [{"step": "fetch", "output": step({"read": READ}) if fetched == "fetch" else fetched}]
    return menu.quotes_found({"output": output, "given": given}, {"field": field, "pages": "fetch"})


class QuotesFound(unittest.TestCase):
    def fails(self, saying: str, *a: Any, **k: Any) -> None:
        verdict = check(*a, **k)
        self.assertIsInstance(verdict, Fail)
        self.assertIn(saying, " ".join(verdict.reasons))

    def test_quotes_that_are_in_the_downloaded_text_pass(self):
        out = {"claims": [{"url": "https://example.org/caching", "quote": "MUST NOT store a response when the no-store"},
                          {"url": "https://example.org/caching", "quotes": ["a response when the “no-store” directive".replace("“", "").replace("”", "")]}]}
        self.assertIsInstance(check(out), Pass)

    def test_an_invented_quote_fails(self):
        self.fails("are not in the text Jaynes downloaded", {"claims": [{"url": "https://example.org/caching", "quote": "A cache MAY store anything it likes, forever."}]})

    def test_a_quote_from_a_page_that_was_not_read_fails_and_says_why(self):
        self.fails("the server answered 404", {"claims": [{"url": "https://example.org/gone", "quote": "This page used to say something."}]})

    def test_a_quote_from_a_page_jaynes_did_not_download_fails(self):
        self.fails("was not downloaded", {"claims": [{"url": "https://example.org/other", "quote": "A cache MUST NOT store a response"}]})

    def test_a_quote_too_short_to_prove_anything_fails(self):
        self.fails("too short", {"claims": [{"url": "https://example.org/caching", "quote": "MUST NOT"}]})

    def test_nothing_quoted_fails(self):
        self.fails("no quote", {"claims": []})
        self.fails("no quote", {"claims": [{"url": "https://example.org/caching"}]})

    def test_a_field_that_is_not_a_list_of_quotes_fails(self):
        self.fails("a list", {"claims": "A cache MUST NOT store a response"})

    def test_without_what_was_downloaded_the_check_cannot_judge(self):
        out = {"claims": [{"url": "https://example.org/caching", "quote": "A cache MUST NOT store a response"}]}
        self.assertIsInstance(check(out, fetched=None), Blocked)
        self.assertIsInstance(check(out, fetched={"pages": "not what fetch-pages makes"}), Blocked)

    def test_a_text_changed_after_the_download_is_not_trusted(self):
        fetched = step({"read": READ})
        fetched["texts"]["https://example.org/caching"] += " A cache MAY store anything it likes, forever."
        out = {"claims": [{"url": "https://example.org/caching", "quote": "A cache MAY store anything it likes, forever."}]}
        self.assertIsInstance(check(out, fetched=fetched), Blocked)


class Menu(unittest.TestCase):
    def test_the_step_and_the_check_are_on_the_menu(self):
        self.assertIn("fetch-pages", menu.STEPS)
        self.assertIn("quotes-found", menu.CHECKS)


if __name__ == "__main__":
    unittest.main()
