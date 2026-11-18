# Quotes checked against their sources

Three items of the menu let a role hold what an agent says a source says to the source itself, by code. An
agent lists pages and the passages it took from them. **fetch-pages** downloads each page itself, keeps its text
in the run's record and says which passages are in it. **quotes-found**, a check, holds every quote a later step
writes to that text. **quote-windows** shows each quote in its page, with the text around it, at a stop.

```toml
[[step]]
name = "fetch"
by = "jaynes"
use = "fetch-pages"
needs = ["read"]
with = { pages = "read.sources" }   # a list of https addresses, or of items with a url and their quotes
reach = { network = "web" }         # required: the step downloads addresses an agent chose

[[step]]
name = "note"
needs = ["read", "fetch"]
checks = [{ use = "quotes-found", field = "claims", pages = "fetch" }]   # claims: items with a url and a quote

[[step]]
name = "windows"
by = "jaynes"
use = "quote-windows"
needs = ["note", "fetch"]
with = { claims = "note.claims", pages = "fetch" }
stop = { ask = "Do the sources say what the note says?", verdicts = ["keep", "change"], redo = "note" }
```

## What quotes-found proves

These words stand here, in this page, as Jaynes downloaded it. Precisely:

- The page is one fetch-pages downloaded in this run, found by the address the quote names, with or without a
  section after `#`, or by the address the page came from after a redirect.
- The quote's words are inside one block of the page's text: a paragraph, a heading, a cell, never across two.
- They start and end at the edge of a word or a number. A hyphenated word is one word (`compliant` is not in
  `non-compliant`), as is a name joined by an underscore (`no_cache`) and a word with an apostrophe (`can` is not in
  `can't`). `12,500`, `1.5` and `12 500` grouped by a no-break or thin space are one number, and a minus or a
  percent sign belongs to its number.
- The page's own address, with its escapes undone until nothing changes, does not carry the quote, nor four of its
  words in a row: a search page that repeats what it was asked proves nothing.
- The quote has at least 20 characters, as compared. Every item of the field brings a quote; an item without one,
  or a field with no quote at all, fails.
- The text kept is the text downloaded: its fingerprint is checked, and a text that differs stops the check.

Two texts are compared after one normalization of both: Unicode compatibility forms folded, characters that do not
show dropped, every quotation mark and apostrophe one mark, every dash one dash, every run of spaces one space,
and a word wrapped at its hyphen at the end of a line read both as one word and as a hyphen left hanging.
Lowered text joins its base (CO2), raised text after a letter joins it (km2, 1st), raised digits or a fraction
after a digit stay apart (10 raised to 6 is not 106), and a footnote mark may be left out of a quote. Letters,
their case and every other mark count.

## What it does not prove

- **That the source asserts the words.** A source may quote a claim to deny it, or say "not safe" a block before.
  That is for a person to judge, and quote-windows exists for it: each quote is shown with up to 160 characters of
  the page's text on each side, reaching into the blocks before and after, and a quote that stands in more than
  one place says so.
- **Text hidden by a style sheet.** Text is left out when it is in a script, a style, a `noscript`, a template, an
  SVG, struck or deleted, or when its own element is hidden (the `hidden` attribute, `aria-hidden`, or `display: none` or
  `visibility: hidden` written on the element). A class whose rule hides it, opacity 0 or a font size of 0 is read
  as text: there is no CSS engine.
- **An address that carries the quote in no words**, such as base64 or a paste site: only words in an address are
  seen.
- **What only a browser shows.** A page a script puts together is not read; `<textarea>`, `<option>`, a closed
  `<dialog>` and what an `<iframe>` loads are read as text of the page, or not at all.

## How a page is downloaded

The address comes from an agent, so it is held to rules before anything is sent:

- Only `https`, on the usual port, with no name or password in the address, and at most 2048 characters.
- Every address the name resolves to must be on the public internet: never this machine, a private, link-local,
  shared or reserved network, multicast, an IPv6 form that carries an IPv4 address inside, or an address with a
  zone. Only global unicast IPv6 counts. This is checked again at every redirect, at most 5.
- The connection goes to an address that was checked, IPv4 first, each in turn until one answers, with the
  certificate held to the name. A proxy set in the environment is not used.
- What the headers say decides first: only HTML, XHTML, plain text and Markdown are read, and a page that says it
  is larger than 2 MB is not read. The body is read as it comes, up to 2 MB, within 20 seconds each time it is
  asked.
- One clock of 60 seconds runs around downloading a page and another of 20 seconds around taking its text apart,
  each in a program of its own that is killed when its clock runs out and that a halt of the run stops too.
- A server that answers 5xx or 429 is asked three times in all, with waits of 1 and 3 seconds, or its Retry-After
  up to 10 seconds.
- At most 12 pages per step, a page listed twice with different sections is downloaded once, and every address
  asked is kept in the record, with the host a redirect led to.
- What a server writes reaches the record only bounded: its format as type/subtype, a character set from a closed
  list (UTF-8, ASCII, ISO-8859-1 and -15, windows-1252; ISO-8859-1 and ASCII read as windows-1252, as browsers do).

A page that is not read is one of two things, and they go apart:

- **The source said no, or does not exist**: a 4xx other than 429, too large, a format or character set Jaynes
  does not read, no text, a page a script puts together, markup the parser cannot read, a certificate that does
  not hold, a broken rule above, or a name that does not exist while a name that always exists is found. Nothing
  quoted from it is proved; the check fails, and the agent may choose another source or drop the claim.
- **Jaynes could not ask**: nothing answered, neither another page of the list nor a page that always answers
  (`https://example.com/`). The step stops as Jaynes's fault, names each page and why, and spends no attempt;
  continuing the run downloads every page again. A single page that does not answer while others do is the
  source's: it is a page not read, so one silent source never stops a run.

The cases quotes-found must accept and refuse are in `engine/python/cases/quotes-found/`, each naming what it
keeps or the hole it closes; the reviewer writes the ones it must refuse.
