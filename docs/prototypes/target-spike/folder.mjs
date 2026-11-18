// THROWAWAY (target spike). Case A: a plain folder, no git. The run works in a copy; the delivery is files put in
// the person's folder and read back.

import fs from 'node:fs';
import path from 'node:path';
import { sha, short, allowed } from './lib.mjs';

/// Every file of a folder with its fingerprint. A link is recorded as a link and never followed.
export function listing(dir) {
  const out = {};
  const walk = (rel) => {
    for (const e of fs.readdirSync(path.join(dir, rel), { withFileTypes: true }).sort((a, b) => (a.name < b.name ? -1 : 1))) {
      const p = rel ? `${rel}/${e.name}` : e.name;
      if (e.isSymbolicLink()) out[p] = 'link:' + fs.readlinkSync(path.join(dir, p));
      else if (e.isDirectory()) walk(p);
      else out[p] = sha(fs.readFileSync(path.join(dir, p)));
    }
  };
  walk('');
  return out;
}

/// One fingerprint for the whole folder: what `git write-tree` is for a repository.
export const fingerprint = (list) => sha(Object.entries(list).map(([p, h]) => `${h}  ${p}`).join('\n'));

/// What the role program gets: a copy of the folder as it was, and the listing it was copied at.
export function open(source, record) {
  const copy = path.join(record.dir, 'copy');
  fs.cpSync(source, copy, { recursive: true, verbatimSymlinks: true });
  const base = listing(source);
  record.note('opened', { target: 'folder', source, base: short(fingerprint(base)), files: Object.keys(base).length });
  return { source, copy, base, record };
}

/// What the copy has that the base did not: measured by Jaynes, never told by the role.
export function changed(h) {
  const now = listing(h.copy);
  const out = [];
  for (const p of new Set([...Object.keys(h.base), ...Object.keys(now)])) {
    if (h.base[p] === now[p]) continue;
    const lines = now[p] && !now[p].startsWith('link:') ? fs.readFileSync(path.join(h.copy, p), 'utf8').split('\n').length : 0;
    out.push({ path: p, kind: !h.base[p] ? 'added' : !now[p] ? 'deleted' : 'modified', before: h.base[p] || null, after: now[p] || null, lines });
  }
  return out.sort((a, b) => (a.path < b.path ? -1 : 1));
}

/// Deliver the copy's change into the person's folder. Nothing is written unless every path it touches is inside
/// the reach and is still, in the person's folder, what it was when the run copied it.
export function deliver(h, write, checks) {
  const { record } = h;
  const change = changed(h);
  record.note('measured', { changed: change.map((c) => `${c.kind} ${c.path}`) });

  const outside = change.filter((c) => !allowed(c.path, write) || String(c.after).startsWith('link:')).map((c) => c.path);
  if (outside.length) return record.stopped('outside_reach', false, `the change touches ${outside.join(', ')}, outside ${JSON.stringify(write)}`);

  for (const check of checks) {
    const failed = check.run(h.copy);
    record.note('check', { name: check.name, passed: !failed, ...(failed ? { failed } : {}) });
    if (failed) return record.stopped('check_failed', false, `${check.name}: ${failed}`);
  }

  const real = listing(h.source);
  const moved = change.filter((c) => (real[c.path] || null) !== c.before).map((c) => c.path);
  if (moved.length) return record.stopped('changed', false, `${moved.join(', ')} changed in the folder since the run copied it`);

  // The previous version of every path is kept before any is written: it is what Undo puts back.
  const kept = path.join(record.dir, 'previous');
  for (const c of change) {
    if (!c.before) continue;
    fs.mkdirSync(path.dirname(path.join(kept, c.path)), { recursive: true });
    fs.copyFileSync(path.join(h.source, c.path), path.join(kept, c.path));
  }
  record.note('delivering', { paths: change.map((c) => c.path), previous: kept });
  for (const c of change) {
    const to = path.join(h.source, c.path);
    if (!c.after) {
      fs.rmSync(to);
      continue;
    }
    fs.mkdirSync(path.dirname(to), { recursive: true });
    const tmp = `${to}.jaynes-${process.pid}`;
    fs.copyFileSync(path.join(h.copy, c.path), tmp);
    fs.renameSync(tmp, to); // each file lands whole or not at all; the set of files does not
  }

  // The proof: the person's folder, read again from disk, is the checked copy, path by path.
  const after = listing(h.source);
  const wrong = Object.keys({ ...after, ...listing(h.copy) }).filter((p) => after[p] !== listing(h.copy)[p]);
  if (wrong.length) return record.stopped('check_failed', true, `after delivery ${wrong.join(', ')} is not what was checked`);
  return record.delivered({
    effect: { kind: 'compensable', undo: 'put the previous versions back' },
    before: short(fingerprint(h.base)),
    after: short(fingerprint(after)),
    files: change.map((c) => ({ path: c.path, kind: c.kind, before: short(c.before), after: short(c.after), lines: c.lines })),
    source: h.source,
    previous: kept,
  });
}

/// Put back what a delivery replaced. Refused, touching nothing, when a delivered path is no longer what was delivered.
export function undo(delivery, record) {
  const real = listing(delivery.source);
  const moved = delivery.files.filter((f) => short(real[f.path]) !== f.after).map((f) => f.path);
  if (moved.length) return record.stopped('changed', false, `${moved.join(', ')} changed since it was delivered, so it is not put back`);
  for (const f of delivery.files) {
    const to = path.join(delivery.source, f.path);
    if (f.kind === 'added') fs.rmSync(to);
    else fs.copyFileSync(path.join(delivery.previous, f.path), to);
  }
  const after = short(fingerprint(listing(delivery.source)));
  if (after !== delivery.before) return record.stopped('check_failed', true, `after undo the folder is ${after}, not ${delivery.before}`);
  return record.delivered({ undone: true, effect: { kind: 'compensable', undo: 'deliver again' }, after });
}
