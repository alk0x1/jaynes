// THROWAWAY (target spike). Case B run for real against the test page, both ways of driving the browser.
//   node case-b.mjs cdp            the debugging protocol
//   node case-b.mjs screen pipe    pixels and pointer only, carried by a headless browser (no display needed)
//   node case-b.mjs screen x11     pixels and pointer only, on a real X screen with no debugger (Linux)

import { Record } from './lib.mjs';
import { serve, arrange, truth, ORIGIN, VARIANTS } from './page.mjs';
import * as cdp from './browser-cdp.mjs';

const URL_ = `${ORIGIN}/`;
const LABEL = 'Weekly digest';

function line(way, name, r, t) {
  const what = r.outcome === 'delivered' ? `delivered (acted: ${r.acted}, effect: ${r.effect.kind}) ${r.before} -> ${r.after}` : `stopped: ${r.reason} (acted: ${r.acted}) ${r.why}`;
  console.log(`B/${way} ${name.padEnd(14)} ${what}\n  ${''.padEnd(way.length + 15)} server holds digest=${t.digest}, saves=${t.saves}, requests elsewhere=${t.elsewhere}, ${r.ms ?? '-'} ms`);
}

export async function run(way, driver) {
  const server = await serve();
  const results = {};
  try {
    let go;
    if (way === 'cdp') {
      await arrange({ variant: 'normal', digest: 0 });
      const shape = await cdp.shape(URL_);
      go = (name, want, before) => cdp.run({ url: URL_, origin: ORIGIN, label: LABEL, want, expect: { shape, before }, record: new Record(`b-cdp-${name}`) });
    } else {
      const screen = await import('./browser-screen.mjs');
      const refs = await screen.record(driver, URL_, arrange);
      go = (name, want, before) => screen.run({ driver, url: URL_, want, refs, expect: { before }, record: new Record(`b-${driver}-${name}`) });
    }
    const tag = way === 'cdp' ? 'cdp' : driver;
    const scenario = async (name, set, want, before) => {
      await arrange(set);
      const started = Date.now();
      const r = await go(name, want, before);
      r.ms = Date.now() - started; // the whole run, the browser's start included
      const t = await truth();
      line(tag, name, r, t);
      results[name] = { ...r, truth: t };
      return r;
    };
    for (const variant of VARIANTS) await scenario(variant, { variant, digest: 0 }, true);
    await scenario('already so', { variant: 'normal', digest: 1 }, true);
    // Undo is the same script the other way, and only from the state the run left.
    await scenario('undo', { variant: 'normal', digest: 1 }, false, true);
    await scenario('undo refused', { variant: 'normal', digest: 0 }, false, true);
  } finally {
    server.close();
  }
  return results;
}

if (import.meta.filename === process.argv[1]) await run(process.argv[2] || 'cdp', process.argv[3] || 'pipe');
