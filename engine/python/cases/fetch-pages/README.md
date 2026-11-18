# fetch-pages: the cases that come with the step

fetch-pages downloads addresses an agent chose, so what it must refuse matters more than what it reads.
`reject.json` holds addresses and answers it must not read: each a near miss naming the hole it closes (`hole`),
such as this machine's address written another way, a name that resolves to a private network, a redirect to
another scheme, a page too large or that never ends. `accept.json` holds pages it must read, each naming what a
step too strict would refuse (`keeps`). The rejects are written by someone other than the step's author, and only
they change them.

A case: `id`, `expect` (`reject` or `accept`), `url` (the address an agent listed), `resolve` (what each name
resolves to; left out, and always for an address written as a number or for localhost, the machine's own resolver,
which needs no network for them), and `answers` (what the server answers at each address asked: `status`,
`headers`, `body`, where FILL is a long paragraph, `body_bytes` to make the body that many bytes long, and
`endless` for an answer that never ends).

`CASE_READERS["fetch-pages"]` in `jaynes_menu.py` runs a case as a run reads a page, with the resolver, the
server and the clock stood in for, so nothing reaches the network. A reject holds when the page is not read and
nothing was sent to an address off the public internet; an accept holds when the page is read.
`python/tests/test_fetch_pages_cases.py` runs every case with it.

Not here yet: an address outside the origins a role declares, since fetch-pages has no list of origins until the
proxy that holds a step to one exists.
