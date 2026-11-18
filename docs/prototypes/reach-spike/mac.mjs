// THROWAWAY (reach spike). The probe and a browser on macOS, with no container: open, in the box Jaynes gives a
// project's commands today (sandbox-exec, any local port), and in the same box opened only to the proxy's port.
//   node mac.mjs

import { execFileSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const HERE = import.meta.dirname;
const ROOT = path.resolve(HERE, '../../..');
const RUNS = path.join(HERE, 'runs', 'mac');
fs.rmSync(RUNS, { recursive: true, force: true });
fs.mkdirSync(RUNS, { recursive: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
// A test build of Chromium (the one Playwright installs), not the person's Google Chrome: that one starts Google's
// updater even headless, which leaves the box's process group, and asks macOS for the login keychain.
const CHROME = process.env.SPIKE_CHROME || `${os.homedir()}/Library/Caches/ms-playwright/chromium_headless_shell-1255/chrome-headless-shell-mac-arm64/chrome-headless-shell`;

const P = { proxy: 9100, allowed: 9101, evil: 9102, debugger: 9103, host: 9104 };
const children = [];
// Each child leads a process group of its own, so killing it kills what it started (Chrome's helpers, and the Chrome
// that sandbox-exec starts).
const start = (args, opts = {}) => { const c = spawn(args[0], args.slice(1).map(String), { stdio: 'ignore', detached: true, ...opts }); children.push(c); return c; };
const killGroup = (c) => { try { process.kill(-c.pid, 'SIGKILL'); } catch {} };

// Every Chrome this spike starts, as the engine starts one in its box (engine/python/jaynes_box.py, photograph):
// headless, a profile of its own, a pretend keychain, a home of its own (CFFIXED_USER_HOME), no background traffic.
// Without the pretend keychain and the home a Chrome in the box finds no login keychain, and macOS asks the person,
// on their screen, where to store one.
const CHROME_FLAGS = ['--use-mock-keychain', '--password-store=basic', '--no-first-run', '--no-default-browser-check',
  '--disable-crash-reporter', '--disable-breakpad', '--disable-gpu', '--disable-extensions', '--disable-background-networking', '--disable-component-update'];
const chromeEnv = (dir) => ({ ...process.env, TMPDIR: dir, CFFIXED_USER_HOME: dir });

/// Start a Chrome at the page, wait until the page has posted what it saw to its own origin, then kill its whole group.
const runChrome = async (argv, ms, log, dir) => {
  const c = start(argv, { stdio: ['ignore', 'ignore', 'pipe'], env: chromeEnv(dir) });
  c.stderr.on('data', (d) => fs.appendFileSync(log, d));
  let exited = null;
  c.on('exit', (code, signal) => (exited = signal || `exit ${code}`));
  const end = Date.now() + ms;
  let seen = '';
  while (!seen && Date.now() < end && !exited) {
    await sleep(300);
    seen = await (await fetch(`http://127.0.0.1:${P.allowed}/__result`)).text();
  }
  killGroup(c);
  return { seen, ended: seen ? 'page posted' : exited ? `exited (${exited})` : 'timed out, killed' };
};

/// The sandbox-exec profile Jaynes writes today, from its own code (engine/python/jaynes_box.py), read and not changed.
const profile = (box) => execFileSync('python3', ['-I', '-c', `
import sys; sys.path.insert(0, ${JSON.stringify(path.join(ROOT, 'engine/python'))})
from pathlib import Path; import jaynes_box as b
print(b.profile(b.Box(writable=(Path(${JSON.stringify(RUNS)}),), ${box})), end='')`]).toString();

const BOXES = {
  'open': null,
  'Jaynes box today (any local port)': profile('local=True'),
  'Jaynes box, only the proxy port': profile(`ports=(${P.proxy},)`),
};
const boxed = (prof, argv) => (prof ? ['/usr/bin/sandbox-exec', '-p', prof, ...argv] : argv);

const hits = async (port) => (await (await fetch(`http://127.0.0.1:${port}/__hits`)).json()).length;
const reset = (port) => fetch(`http://127.0.0.1:${port}/__reset`);
const refusedSoFar = () => fs.readFileSync(path.join(RUNS, 'proxy.jsonl'), 'utf8').split('\n').filter((l) => l.includes('"refused"')).length;

try {
  const node = process.execPath;
  start([node, 'web.mjs', 'allowed', P.allowed, `http://evil.test:${P.evil}`], { cwd: HERE });
  start([node, 'web.mjs', 'evil', P.evil], { cwd: HERE });
  start([node, 'web.mjs', 'host', P.host], { cwd: HERE });
  start([node, 'proxy.mjs', '--port', P.proxy, '--allow', `allowed.test:${P.allowed},rebind.test:80`,
    '--map', `allowed.test=127.0.0.1:${P.allowed},evil.test=127.0.0.1:${P.evil},rebind.test=127.0.0.1:${P.host}`,
    '--open', `127.0.0.1:${P.allowed},127.0.0.1:${P.evil}`, '--log', path.join(RUNS, 'proxy.jsonl')], { cwd: HERE });
  // A browser outside every box with its debugger open, as a person's or another run's might be.
  const debugProfile = fs.mkdtempSync(path.join(os.tmpdir(), 'reach-debugger-'));
  start([CHROME, ...CHROME_FLAGS, `--remote-debugging-port=${P.debugger}`, `--user-data-dir=${debugProfile}`, 'about:blank'], { env: chromeEnv(debugProfile) });
  await sleep(2500);
  fs.writeFileSync(path.join(RUNS, 'proxy.jsonl'), '', { flag: 'a' });

  const env = { ...process.env, PROXY: `127.0.0.1:${P.proxy}`, ALLOWED: `allowed.test:${P.allowed}`, EVIL: `evil.test:${P.evil}`,
    EVIL_IP: `127.0.0.1:${P.evil}`, HOST_SERVICE: `127.0.0.1:${P.host}`, DEBUGGER: `127.0.0.1:${P.debugger}` };
  const table = {};
  for (const [name, prof] of Object.entries(BOXES)) {
    await Promise.all([reset(P.evil), reset(P.host)]);
    const before = refusedSoFar();
    const argv = boxed(prof, [node, path.join(HERE, 'probe.mjs')]);
    const out = JSON.parse(execFileSync(argv[0], argv.slice(1), { env, cwd: HERE }).toString());
    table[name] = { tries: out, far: { elsewhere: await hits(P.evil), 'person service': await hits(P.host) }, proxyRefused: refusedSoFar() - before };
    if (prof) fs.writeFileSync(path.join(RUNS, `${name.replace(/\W+/g, '-')}.sb`), prof);
  }

  // A browser in each box: a page of the allowed origin tries to reach elsewhere every way a page can.
  const browsers = {};
  for (const [name, prof, evil, flags] of [
    ['proxy flag only, no box', null, `http://127.0.0.1:${P.evil}`, []],
    // Chrome's own sandbox cannot start inside sandbox-exec, so in the box it runs without it: the box is the wall.
    ['proxy flag + Jaynes box, only the proxy port', BOXES['Jaynes box, only the proxy port'], `http://127.0.0.1:${P.evil}`, ['--no-sandbox']],
    ['same, elsewhere by name', BOXES['Jaynes box, only the proxy port'], `http://evil.test:${P.evil}`, ['--no-sandbox']],
  ]) {
    await Promise.all([reset(P.evil), reset(P.allowed)]);
    const before = refusedSoFar();
    const dir = fs.mkdtempSync(path.join(RUNS, 'chrome-'));
    const argv = boxed(prof, [CHROME, ...CHROME_FLAGS, `--user-data-dir=${dir}`, `--proxy-server=http://127.0.0.1:${P.proxy}`, ...flags,
      `http://allowed.test:${P.allowed}/page?evil=${encodeURIComponent(evil)}`]);
    const { seen, ended } = await runChrome(argv, 30000, path.join(dir, 'stderr.txt'), dir);
    browsers[name] = { page: seen ? JSON.parse(seen) : null, ended, far: { elsewhere: await hits(P.evil) }, proxyRefused: refusedSoFar() - before };
  }
  fs.writeFileSync(path.join(RUNS, 'result.json'), JSON.stringify({ table, browsers }, null, 2));
  console.log(JSON.stringify({ table, browsers }, null, 1));
  fs.rmSync(debugProfile, { recursive: true, force: true });
} finally {
  children.forEach(killGroup);
  await sleep(500);
  // Nothing of this spike may outlive it: any process still naming its folders is a leak, killed and reported.
  const left = execFileSync('ps', ['-axo', 'pid=,command=']).toString().split('\n').filter((l) => l.includes(RUNS) || l.includes('reach-debugger-'));
  for (const l of left) { try { process.kill(Number(l.trim().split(' ')[0]), 'SIGKILL'); } catch {} }
  console.log(`processes of this spike left alive: ${left.length}`);
}
