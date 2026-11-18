// THROWAWAY (target spike). Case B, second way: the fixed script knows only what the screen shows. A picture comes
// in, a click or a key goes out. It supposes a browser where nothing can be installed and no debugger attached, so
// it reads no structure: it finds a control by how it looks, and reads the state as which picture it sees.

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import { short } from './lib.mjs';
import { debugged, launch } from './cdp.mjs';
import { decode, crop, find } from './png.mjs';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let shots = 0;

/// A screen: `open` a browser at a page, take a `picture`, `click` a point, `reload`. Two of them:
const DRIVERS = {
  // `pipe`: a headless browser whose debugger is used as a wire for pixels and pointer and for nothing else (no
  // structure read, no request held). It lets the same script run where there is no display; it is not the
  // supposed browser, and it says nothing about the permissions a real screen asks.
  pipe: async (url) => {
    const b = await debugged();
    const load = async (how) => {
      const loaded = b.once('Page.loadEventFired');
      await b.send(how, how === 'Page.navigate' ? { url } : {});
      await loaded;
    };
    await load('Page.navigate');
    return { picture: b.screenshot, click: b.click, reload: () => load('Page.reload'), close: b.close };
  },
  // `x11`: the supposed browser. An ordinary window on an X screen, no debugger port; the picture is the screen's,
  // the click is the X pointer's, the reload is the F5 key.
  x11: async (url) => {
    const browser = launch(['--kiosk', url]);
    return {
      picture: async () => {
        const png = execFileSync('import', ['-window', 'root', '-depth', '8', 'png32:-'], { maxBuffer: 1 << 26 });
        if (process.env.SPIKE_SHOTS) fs.writeFileSync(`${process.env.SPIKE_SHOTS}/${String(++shots).padStart(3, '0')}.png`, png);
        return png;
      },
      click: async (x, y) => void execFileSync('xdotool', ['mousemove', String(Math.round(x)), String(Math.round(y)), 'click', '1']),
      reload: async () => void execFileSync('xdotool', ['key', 'F5']),
      close: browser.close,
    };
  },
};

// What a person marked on a picture of the page when the script was written: where the title, the setting and the
// button are. Only used to cut the reference pictures; at run time the script looks for them anywhere on the screen.
const MARKS = {
  title: { x: 40, y: 30, w: 300, h: 40 },
  setting: { x: 36, y: 118, w: 160, h: 40 },
  save: { x: 40, y: 190, w: 120, h: 40 },
  saved: { x: 176, y: 190, w: 80, h: 40 },
};
const BOX = { x: 18, y: 20 }; // the setting's box, inside its mark
const SAVE = { x: 60, y: 20 };

/// Take pictures until `test` answers something, or the time is up.
async function until(screen, test, ms = 5000) {
  const end = Date.now() + ms;
  for (;;) {
    const png = await screen.picture();
    const got = test(decode(png));
    if (got || Date.now() > end) return { got, png };
    await sleep(150);
  }
}

/// The screen once it stands still and shows something. By the screen alone nothing says "the page has loaded":
/// the script can only wait for two pictures in a row to be the same, and a blank one is not an answer.
async function settled(screen, ms = 30000) {
  const end = Date.now() + ms;
  let last = null;
  for (;;) {
    await sleep(250);
    const img = decode(await screen.picture());
    const blank = img.rgb.every((v, i) => v === img.rgb[i % 3]);
    if (!blank && last && last.rgb.equals(img.rgb)) return img;
    if (Date.now() > end) throw new Error('the screen never stood still showing something');
    last = img;
  }
}

/// Cut the reference pictures, once, from the page as the script's writer saw it.
export async function record(driver, url, arrange) {
  await arrange({ variant: 'normal', digest: 0 });
  const screen = await DRIVERS[driver](url);
  try {
    const img = await settled(screen);
    const refs = { title: crop(img, MARKS.title), off: crop(img, MARKS.setting), save: crop(img, MARKS.save) };
    await screen.click(MARKS.save.x + SAVE.x, MARKS.save.y + SAVE.y);
    await sleep(500);
    refs.saved = crop(await settled(screen), MARKS.saved);
    await arrange({ variant: 'normal', digest: 1 });
    await screen.reload();
    await sleep(500);
    refs.on = crop(await settled(screen), MARKS.setting);
    for (const [name, ref] of Object.entries(refs)) if (ref.rgb.every((v, i) => v === ref.rgb[i % 3])) throw new Error(`the reference picture of ${name} is blank`);
    return refs;
  } finally {
    await screen.close();
  }
}

/// Make the setting show `want`, and prove it, by the screen alone.
export async function run({ driver, url, want, refs, expect, record: rec }) {
  const started = Date.now();
  const screen = await DRIVERS[driver](url);
  try {
    // Reach cannot be imposed from here: the script sees pixels, not requests. It has to come from outside the browser.
    rec.note('opened', { target: 'browser session', by: `screen (${driver})`, url, reach: 'not imposed by this script' });

    // The state as the screen shows it: which of the two pictures of the setting is there, and where.
    const read = (img) => {
      const [on, off] = [find(img, refs.on), find(img, refs.off)];
      return on.length + off.length === 1 ? { state: on.length === 1, at: on[0] || off[0] } : on.length + off.length === 0 ? null : { many: on.length + off.length };
    };

    const first = await until(screen, (img) => find(img, refs.title)[0], 15000);
    if (!first.got) return rec.stopped('changed', false, 'the screen does not show the title of the page the script was written for');
    const img = decode(first.png);
    const seen = read(img);
    if (!seen) return rec.stopped('not_found', false, 'nothing on the screen looks like the setting, on or off');
    if (seen.many) return rec.stopped('not_found', false, `${seen.many} things on the screen look like the setting`);
    const before = seen.state;
    rec.note('observed', { setting: before, at: seen.at, picture: short(rec.keep('before.png', first.png)) });
    if (expect.before !== undefined && before !== expect.before) return rec.stopped('changed', false, `the setting shows ${before}, not the ${expect.before} this run would start from`);
    if (before === want) return rec.delivered({ effect: { kind: 'none' }, acted: false, before, after: before, ms: Date.now() - started });

    await screen.click(seen.at.x + BOX.x, seen.at.y + BOX.y);
    rec.note('acted', { click: [seen.at.x + BOX.x, seen.at.y + BOX.y] });
    const flipped = await until(screen, (i) => read(i)?.state === want, 3000);
    if (!flipped.got) return rec.stopped('check_failed', true, 'the setting did not change its look after the click', { ms: Date.now() - started });

    const save = find(decode(flipped.png), refs.save);
    if (save.length !== 1) return rec.stopped('not_found', true, `${save.length} things on the screen look like the Save button`, { ms: Date.now() - started });
    await screen.click(save[0].x + SAVE.x, save[0].y + SAVE.y);
    rec.note('acted', { click: [save[0].x + SAVE.x, save[0].y + SAVE.y] });
    const said = await until(screen, (i) => find(i, refs.saved)[0], 5000);

    // The check: a new load, then which picture of the setting the screen shows.
    await screen.reload();
    await sleep(500);
    await settled(screen);
    const last = await until(screen, (i) => (find(i, refs.title)[0] ? read(i) : null), 8000);
    const after = last.got?.state ?? null;
    const facts = { before, after, page_said: said.got ? 'Saved' : '', picture: short(rec.keep('after.png', last.png)), off_by: last.got?.at?.off, ms: Date.now() - started };
    if (after !== want) return rec.stopped('check_failed', true, `after a new load the setting shows ${after}, and ${want} was promised`, facts);
    return rec.delivered({ effect: { kind: 'compensable', undo: `click the setting back to ${before}` }, acted: true, ...facts });
  } finally {
    await screen.close();
  }
}
