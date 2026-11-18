// THROWAWAY (target spike). A browser of the run's own (its own profile, so no account of the person's), and the
// least client of its debugging protocol. No dependency: Node's own WebSocket.

import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

export const CHROME = process.env.CHROME
  || ['/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', '/usr/bin/chromium', '/usr/bin/chromium-browser', '/usr/bin/google-chrome'].find((p) => fs.existsSync(p));

export const WIDTH = 800;
export const HEIGHT = 600;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/// Start a browser with a profile of its own. `flags` decides whether it is headless, debugged, on a screen.
export function launch(flags) {
  const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'spike-profile-'));
  const root = process.getuid?.() === 0 ? ['--no-sandbox'] : [];
  const child = spawn(CHROME, [
    `--user-data-dir=${profile}`, '--no-first-run', '--no-default-browser-check', '--password-store=basic', '--test-type',
    '--disable-gpu', '--disable-dev-shm-usage', '--hide-scrollbars', '--force-device-scale-factor=1',
    `--window-size=${WIDTH},${HEIGHT}`, '--window-position=0,0', ...root, ...flags,
  ], { stdio: 'ignore' });
  const close = async () => {
    child.kill('SIGKILL');
    await sleep(200);
    fs.rmSync(profile, { recursive: true, force: true, maxRetries: 5 });
  };
  return { profile, child, close };
}

/// A headless browser with its debugger open on a port of its own choosing, and a session on its one page.
export async function debugged() {
  const browser = launch(['--headless=new', '--remote-debugging-port=0', 'about:blank']);
  const portFile = path.join(browser.profile, 'DevToolsActivePort');
  for (let i = 0; i < 100 && !fs.existsSync(portFile); i++) await sleep(100);
  const port = fs.readFileSync(portFile, 'utf8').split('\n')[0];
  let target;
  for (let i = 0; i < 50 && !target; i++) {
    const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
    target = list.find((t) => t.type === 'page');
    if (!target) await sleep(100);
  }
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((ok, bad) => { ws.onopen = ok; ws.onerror = bad; });
  let id = 0;
  const waiting = new Map();
  const listeners = [];
  ws.onmessage = (m) => {
    const msg = JSON.parse(m.data);
    if (msg.id) {
      const w = waiting.get(msg.id);
      waiting.delete(msg.id);
      msg.error ? w.bad(new Error(`${w.method}: ${msg.error.message}`)) : w.ok(msg.result);
    } else listeners.forEach((l) => l(msg));
  };
  const send = (method, params = {}) => new Promise((ok, bad) => { waiting.set(++id, { ok, bad, method }); ws.send(JSON.stringify({ id, method, params })); });
  const on = (method, f) => listeners.push((m) => m.method === method && f(m.params));
  const once = (method, ms = 10000) => new Promise((ok, bad) => {
    const t = setTimeout(() => bad(new Error(`${method} did not come in ${ms} ms`)), ms);
    const l = (m) => { if (m.method === method) { clearTimeout(t); listeners.splice(listeners.indexOf(l), 1); ok(m.params); } };
    listeners.push(l);
  });
  /// Run an expression in the page and take its value.
  const evaluate = async (expression) => {
    const r = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.text);
    return r.result.value;
  };
  const click = async (x, y) => {
    for (const type of ['mousePressed', 'mouseReleased']) await send('Input.dispatchMouseEvent', { type, x, y, button: 'left', clickCount: 1 });
  };
  const screenshot = async () => Buffer.from((await send('Page.captureScreenshot', { format: 'png' })).data, 'base64');
  await send('Page.enable');
  await send('Emulation.setDeviceMetricsOverride', { width: WIDTH, height: HEIGHT, deviceScaleFactor: 1, mobile: false });
  return { send, on, once, evaluate, click, screenshot, close: async () => { ws.close(); await browser.close(); } };
}
