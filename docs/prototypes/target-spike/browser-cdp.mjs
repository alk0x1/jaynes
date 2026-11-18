// THROWAWAY (target spike). Case B, first way: the fixed script drives the browser through its debugging protocol.
// It reads the page's structure, so it finds a control by what it is called and reads the state as a value.

import { sha, short } from './lib.mjs';
import { debugged } from './cdp.mjs';

// The form as code sees it: every control with its kind, name and label. Its fingerprint is what "the page the
// script was written for" means here.
const SHAPE = `JSON.stringify([...document.querySelectorAll('form input, form button, form select, form textarea')].map((e) => [e.tagName, e.type, e.name, (e.closest('label') || e).textContent.trim()]))`;

// A control by what it is called, with where to click it and what it holds. Null when the page has none, or two.
const control = (label) => `(() => {
  const all = [...document.querySelectorAll('form label, form button')].filter((e) => e.textContent.trim() === ${JSON.stringify(label)});
  if (all.length !== 1) return null;
  const e = all[0].querySelector('input') || all[0];
  const r = e.getBoundingClientRect();
  return { x: r.x + r.width / 2, y: r.y + r.height / 2, checked: e.checked ?? null };
})()`;

/// The fingerprint of the form at `url`, taken once when the script is written.
export async function shape(url) {
  const b = await debugged();
  try {
    await b.send('Page.navigate', { url });
    await b.once('Page.loadEventFired');
    return sha(await b.evaluate(SHAPE));
  } finally {
    await b.close();
  }
}

/// Make the setting called `label` hold `want`, and prove it. `expect.shape` is the form the script knows;
/// `expect.before`, when given, is the only state it may start from (how an undo refuses a setting changed since).
export async function run({ url, origin, label, want, expect, record }) {
  const started = Date.now();
  const b = await debugged();
  const blocked = [];
  try {
    // Reach, imposed: every request of the session is held, and one to another origin is failed before it leaves.
    await b.send('Fetch.enable', { patterns: [{ urlPattern: '*' }] });
    b.on('Fetch.requestPaused', ({ requestId, request }) => {
      if (new URL(request.url).origin === origin) return b.send('Fetch.continueRequest', { requestId });
      blocked.push(request.url);
      return b.send('Fetch.failRequest', { requestId, errorReason: 'BlockedByClient' });
    });
    record.note('opened', { target: 'browser session', by: 'debugging protocol', url, reach: [origin] });

    const load = async (how) => {
      const loaded = b.once('Page.loadEventFired');
      await b.send(how, how === 'Page.navigate' ? { url } : {});
      await loaded;
    };
    await load('Page.navigate');

    const form = sha(await b.evaluate(SHAPE));
    if (form !== expect.shape) return record.stopped('changed', false, `the form is ${short(form)}, and the script was written for ${short(expect.shape)}`, { blocked });

    let box = await b.evaluate(control(label));
    if (!box) return record.stopped('not_found', false, `no one control is called "${label}"`, { blocked });
    const before = box.checked;
    record.note('observed', { [label]: before, form: short(form) });
    if (expect.before !== undefined && before !== expect.before) return record.stopped('changed', false, `"${label}" is ${before}, not the ${expect.before} this run would start from`, { blocked });
    if (before === want) return record.delivered({ effect: { kind: 'none' }, acted: false, before, after: before, blocked, ms: Date.now() - started });

    // From here the script has acted on the real thing: every stop below says so, and nothing is tried twice.
    await b.click(box.x, box.y);
    record.note('acted', { click: label });
    const save = await b.evaluate(control('Save'));
    if (!save) return record.stopped('not_found', true, 'no one control is called "Save"', { blocked });
    await b.click(save.x, save.y);
    record.note('acted', { click: 'Save' });
    let said = '';
    for (let i = 0; i < 50 && !said; i++) {
      said = await b.evaluate(`document.getElementById('status').textContent`);
      if (!said) await new Promise((r) => setTimeout(r, 100));
    }

    // The check: a new load of the page, so what is read is what the service kept, not what the page drew.
    await load('Page.reload');
    box = await b.evaluate(control(label));
    const after = box ? box.checked : null;
    const picture = record.keep('after.png', await b.screenshot());
    const facts = { before, after, page_said: said, picture: short(picture), blocked, ms: Date.now() - started };
    if (after !== want) return record.stopped('check_failed', true, `after a new load "${label}" is ${after}, and ${want} was promised`, facts);
    return record.delivered({ effect: { kind: 'compensable', undo: `set "${label}" back to ${before}` }, acted: true, ...facts });
  } finally {
    await b.close();
  }
}
