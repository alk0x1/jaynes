// THROWAWAY (target spike). Case A run for real: a fixed script, no model, adds a note to a plain folder.
//   node case-a.mjs

import fs from 'node:fs';
import path from 'node:path';
import { Record, RUNS, short } from './lib.mjs';
import * as folder from './folder.mjs';

const REACH = ['notes/*', 'index.md', '!notes/private/*'];

/// The role program's work: it is handed the copy and nothing else.
const work = {
  good(copy) {
    fs.writeFileSync(path.join(copy, 'notes/2026-11-17.md'), '# Garden, week 46\n\nThe beans are in. Water on Thursday.\n');
    fs.appendFileSync(path.join(copy, 'index.md'), '- [Garden, week 46](notes/2026-11-17.md)\n');
  },
  outside(copy) {
    work.good(copy);
    fs.writeFileSync(path.join(copy, 'budget.md'), '# Budget\n\nrewritten\n');
  },
  untitled(copy) {
    fs.writeFileSync(path.join(copy, 'notes/2026-11-17.md'), 'The beans are in.\n');
    fs.appendFileSync(path.join(copy, 'index.md'), '- [Garden, week 46](notes/2026-11-17.md)\n');
  },
};

/// Checks by code, on the copy, before anything is delivered. Each answers nothing, or the sentence of what failed.
const CHECKS = [
  {
    name: 'every note has a title',
    run: (dir) => fs.readdirSync(path.join(dir, 'notes')).filter((f) => f.endsWith('.md')).filter((f) => !fs.readFileSync(path.join(dir, 'notes', f), 'utf8').startsWith('# ')).map((f) => `notes/${f} has no title`)[0],
  },
  {
    name: 'every link of the index leads to a file',
    run: (dir) => [...fs.readFileSync(path.join(dir, 'index.md'), 'utf8').matchAll(/\]\(([^)]+)\)/g)].map((m) => m[1]).filter((p) => !fs.existsSync(path.join(dir, p))).map((p) => `index.md links to ${p}, which is not there`)[0],
  },
];

function fresh(name) {
  const dir = path.join(RUNS, 'folders', name);
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(path.join(dir, 'notes'), { recursive: true });
  fs.writeFileSync(path.join(dir, 'index.md'), '# Notes\n\n- [Garden, week 45](notes/2026-11-10.md)\n');
  fs.writeFileSync(path.join(dir, 'notes/2026-11-10.md'), '# Garden, week 45\n\nSoil turned.\n');
  fs.writeFileSync(path.join(dir, 'budget.md'), '# Budget\n\nnot the run\'s to touch\n');
  return dir;
}

const print = (name, r, dir, before) => {
  const now = short(folder.fingerprint(folder.listing(dir)));
  const what = r.outcome === 'delivered' ? (r.undone ? 'undone' : `delivered ${r.files.map((f) => `${f.kind} ${f.path}`).join(', ')}`) : `stopped: ${r.reason} (acted: ${r.acted}) ${r.why}`;
  console.log(`A ${name.padEnd(20)} ${what}\n  ${''.padEnd(20)} folder ${before} -> ${now}${before === now ? ' (untouched)' : ''}`);
};

export function run() {
  const results = {};
  const scenario = (name, script, between = () => {}) => {
    const dir = fresh(name);
    const before = short(folder.fingerprint(folder.listing(dir)));
    const record = new Record(`a-${name}`);
    const h = folder.open(dir, record);
    script(h.copy);
    between(dir);
    const r = folder.deliver(h, REACH, CHECKS);
    print(name, r, dir, between === noop ? before : short(folder.fingerprint(h.base)));
    results[name] = r;
    return { dir, r, before };
  };
  const noop = () => {};

  const ok = scenario('deliver', work.good);
  // Undo, checked the same way: the folder is again what it was.
  const back = folder.undo(ok.r, new Record('a-deliver-undo'));
  print('deliver, then undo', back, ok.dir, ok.r.after);
  results.undo = back;

  scenario('outside reach', work.outside);
  scenario('check fails', work.untitled);
  scenario('changed underneath', work.good, (dir) => fs.appendFileSync(path.join(dir, 'index.md'), '- the person wrote this meanwhile\n'));

  const again = scenario('undo refused', work.good);
  fs.appendFileSync(path.join(again.dir, 'notes/2026-11-17.md'), 'The person added a line.\n');
  const refused = folder.undo(again.r, new Record('a-undo-refused-undo'));
  print('  its undo', refused, again.dir, short(folder.fingerprint(folder.listing(again.dir))));
  results.undoRefused = refused;
  return results;
}

if (import.meta.filename === process.argv[1]) run();
