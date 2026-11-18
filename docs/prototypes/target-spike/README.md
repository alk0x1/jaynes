# Target spike (THROWAWAY)

A throwaway prototype, not part of Jaynes: it shows what a run needs from the engine when what it works on is not a
git repository. Nothing here is imported by `engine/` or `app/`. Delete it once the target design is settled.

Two cases, each a fixed script with no model:

- **A, a plain folder** (`case-a.mjs`, `folder.mjs`): the run adds a note to a copy of the folder; the delivery puts
  the changed files in the person's folder and reads them back.
- **B, a browser session** (`case-b.mjs`, `page.mjs`): a test page of its own on 127.0.0.1:8793 (8794 stands for
  "somewhere else"); the script turns one setting on, saves, reloads and checks. Two ways to drive it:
  `browser-cdp.mjs` (the browser's debugging protocol) and `browser-screen.mjs` (pictures in, clicks and keys out).

## Run

```sh
node case-a.mjs
node case-b.mjs cdp
node case-b.mjs screen pipe          # screen only, carried by a headless browser
docker build -t jaynes-target-spike linux
docker run --rm -v "$PWD":/spike:ro jaynes-target-spike sh linux/run.sh   # all of it in Linux, plus screen x11
```

Each run leaves its record in `runs/<scenario>/record.jsonl` (ignored by git).

## Scenarios

| | A, folder | B, browser |
|---|---|---|
| delivers | note added, index linked | setting on, kept after a reload |
| page or file moved under it | person edited `index.md` meanwhile: `changed` | `renamed`, `extra-field`, `restyled` |
| control not found | | `renamed` |
| check fails | note without a title: `check_failed` before anything is written | save answers 200 and keeps nothing: `check_failed` after acting |
| outside reach | change touches `budget.md`: `outside_reach` | a request to 8794: failed by CDP, not stoppable by screen |
| undo | previous versions put back; refused if the file changed since | same script the other way, only from the state the run left |
