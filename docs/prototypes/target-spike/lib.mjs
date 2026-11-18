// THROWAWAY (target spike). What both cases share: the record a run leaves, the ways it ends, and reach by path.

import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

export const sha = (data) => createHash('sha256').update(data).digest('hex');
export const short = (h) => (h ? h.slice(0, 12) : '-');

export const RUNS = process.env.SPIKE_RUNS || path.join(import.meta.dirname, 'runs');

/// The four ways a script stops without acting again, the same for every target.
export const STOPS = ['not_found', 'changed', 'check_failed', 'outside_reach'];

/// A run's record: one line per fact, appended as it happens, so a run that dies leaves what it had done.
export class Record {
  constructor(name) {
    this.dir = path.join(RUNS, name);
    fs.rmSync(this.dir, { recursive: true, force: true });
    fs.mkdirSync(this.dir, { recursive: true });
    this.file = path.join(this.dir, 'record.jsonl');
    this.lines = [];
  }
  note(kind, fact) {
    const line = { at: new Date().toISOString(), kind, ...fact };
    this.lines.push(line);
    fs.appendFileSync(this.file, JSON.stringify(line) + '\n');
    return line;
  }
  /// Keep a file of evidence (a picture, a previous version) and say its fingerprint.
  keep(name, data) {
    fs.writeFileSync(path.join(this.dir, name), data);
    return sha(data);
  }
  delivered(fact) {
    return { outcome: 'delivered', ...this.note('delivered', fact) };
  }
  /// `acted`: whether anything real was changed before the stop. A stop never acts again.
  stopped(reason, acted, why, fact = {}) {
    if (!STOPS.includes(reason)) throw new Error(`unknown stop ${reason}`);
    return { outcome: 'stopped', ...this.note('stopped', { reason, acted, why, ...fact }) };
  }
}

// Path patterns as the engine reads them (core/src/glob.rs): `*` crosses slashes, `!` keeps paths out.
function fnmatch(s, p) {
  const re = new RegExp('^' + p.split('').map((c) => (c === '*' ? '.*' : c === '?' ? '.' : c.replace(/[.+^${}()|[\]\\]/g, '\\$&'))).join('') + '$', 's');
  return re.test(s);
}
const matches = (p, pattern) => fnmatch(p, pattern) || (pattern.startsWith('**/') && fnmatch(p, pattern.slice(3)));
export function allowed(p, patterns) {
  let inside = false;
  for (const pattern of patterns) {
    if (pattern.startsWith('!')) {
      if (matches(p, pattern.slice(1))) return false;
    } else if (matches(p, pattern)) inside = true;
  }
  return inside;
}
