# quotes-found: the cases that come with the check

`accept.json` holds quotes the check must accept: true quotes that a check too strict would refuse, each naming
what it keeps (`keeps`). `reject.json` holds quotes it must refuse: each a near miss naming the hole it closes
(`hole`). The rejects are written by someone other than the check's author, and only they change them.

A case: `id`, `url` (the page as downloaded), `type` (its content type), `page` (its body, as text, encoded in
`page_charset` when given, else UTF-8; FILL in it is a long paragraph, so the page counts as one with text of its
own), `quote`, optional `cite` (the address the quote names, when it is not
`url`), and `expect` (`accept` or `reject`). `CASE_READERS["quotes-found"]` in `jaynes_menu.py` runs a case through
fetch-pages and quotes-found as a run would, without the network, and `python/tests/test_quotes_found_cases.py`
runs every case with it.
