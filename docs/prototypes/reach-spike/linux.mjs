// THROWAWAY (reach spike). The same probe and browser in Linux containers, run from macOS with Docker:
//   docker build -t jaynes-reach-spike linux && node linux.mjs
// The pretend web lives on a network with no way out (`--internal`), so nothing here reaches the real internet
// except the one "open" box, which also has Docker's ordinary network, to show what open means.
//
// Boxes:
//   open        Docker's ordinary network plus the pretend web: what a container is by default
//   forced      only a network whose one other member is the proxy: the proxy is the only way out
//   firewall    ordinary network plus the pretend web, and a firewall in the box's own network namespace that lets
//               out only the proxy's address; the program runs as an ordinary user with no right to change it

import { execFileSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const HERE = import.meta.dirname;
const RUNS = path.join(HERE, 'runs', 'linux');
fs.rmSync(RUNS, { recursive: true, force: true });
fs.mkdirSync(RUNS, { recursive: true });
const IMAGE = 'jaynes-reach-spike';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const docker = (...a) => execFileSync('docker', a.map(String), { stdio: ['ignore', 'pipe', 'pipe'] }).toString().trim();
const quiet = (...a) => { try { return docker(...a); } catch { return ''; } };
const NAMES = ['rs-allowed', 'rs-evil', 'rs-proxy'];
const WEB = '172.30.0.0/24';

// Two things on the Mac stand for the person's machine: a local service and a browser with its debugger open.
const HOST_PORT = 9104;
const DEBUG_PORT = 9103;
const CHROME = process.env.SPIKE_CHROME || `${os.homedir()}/Library/Caches/ms-playwright/chromium_headless_shell-1255/chrome-headless-shell-mac-arm64/chrome-headless-shell`;
const hostSide = [];

function cleanup() {
  for (const n of NAMES) quiet('rm', '-f', n);
  for (const n of ['rs-web', 'rs-box']) quiet('network', 'rm', n);
}

const hits = (name) => JSON.parse(docker('exec', name, 'node', '-e', "fetch('http://127.0.0.1/__hits').then((r) => r.text()).then(console.log)")).length;
const reset = (name) => docker('exec', name, 'node', '-e', "fetch('http://127.0.0.1/__reset')");
const proxyLog = () => quiet('exec', 'rs-proxy', 'cat', '/tmp/proxy.jsonl').split('\n').filter(Boolean).map((l) => JSON.parse(l));
const hostHits = async () => (await (await fetch(`http://127.0.0.1:${HOST_PORT}/__hits`)).json()).length;

try {
  cleanup();
  docker('network', 'create', '--internal', '--subnet', WEB, 'rs-web');
  docker('network', 'create', '--internal', '--subnet', '172.31.0.0/24', 'rs-box');
  const mount = ['-v', `${HERE}:/spike:ro`, '-e', 'BIND=0.0.0.0'];
  docker('run', '-d', '--name', 'rs-allowed', '--network', 'rs-web', '--network-alias', 'allowed.test', ...mount, IMAGE, 'node', 'web.mjs', 'allowed', 80, 'http://evil.test');
  docker('run', '-d', '--name', 'rs-evil', '--network', 'rs-web', '--network-alias', 'evil.test', ...mount, IMAGE, 'node', 'web.mjs', 'evil', 80);
  docker('create', '--name', 'rs-proxy', '--network', 'rs-box', '--network-alias', 'proxy', ...mount, IMAGE, 'node', 'proxy.mjs',
    '--host', '0.0.0.0', '--port', 3128, '--allow', 'allowed.test:80,rebind.test:80', '--map', 'rebind.test=127.0.0.1:80', '--open', WEB, '--log', '/tmp/proxy.jsonl');
  docker('network', 'connect', 'rs-web', 'rs-proxy');
  docker('start', 'rs-proxy');
  const ip = (name, net) => docker('inspect', '-f', `{{(index .NetworkSettings.Networks "${net}").IPAddress}}`, name);
  const EVIL_IP = ip('rs-evil', 'rs-web');
  const PROXY_WEB = ip('rs-proxy', 'rs-web');
  const PROXY_BOX = ip('rs-proxy', 'rs-box');

  hostSide.push(spawn(process.execPath, ['web.mjs', 'host', HOST_PORT], { cwd: HERE, stdio: 'ignore', detached: true }));
  const dbg = fs.mkdtempSync(path.join(os.tmpdir(), 'reach-debugger-'));
  hostSide.push(spawn(CHROME, ['--use-mock-keychain', '--password-store=basic', '--disable-background-networking', `--remote-debugging-port=${DEBUG_PORT}`, `--user-data-dir=${dbg}`, 'about:blank'],
    { stdio: 'ignore', detached: true, env: { ...process.env, CFFIXED_USER_HOME: dbg } }));
  await sleep(2500);

  /// Run `argv` in a box: a container on the box's networks, with the firewall first when the box has one.
  const inBox = (box, argv, env) => {
    const name = `rs-run-${process.pid}`;
    const e = Object.entries(env).flatMap(([k, v]) => ['-e', `${k}=${v}`]);
    let cmd = argv;
    const extra = [];
    if (box === 'firewall') {
      extra.push('--cap-add', 'NET_ADMIN');
      const rules = [
        'iptables -P OUTPUT DROP', 'iptables -A OUTPUT -o lo -j ACCEPT',
        'iptables -A OUTPUT -m state --state ESTABLISHED,RELATED -j ACCEPT',
        `iptables -A OUTPUT -p tcp -d ${PROXY_WEB} --dport 3128 -j ACCEPT`,
        'ip6tables -P OUTPUT DROP 2>/dev/null || true',
      ].join(' && ');
      // the rules are set as root, then the program runs as an ordinary user with every capability dropped
      cmd = ['sh', '-c', `${rules} && exec setpriv --reuid=node --regid=node --clear-groups --inh-caps=-all --bounding-set=-all ${argv.map((a) => `'${a}'`).join(' ')}`];
    }
    if (box.startsWith('bwrap')) {
      // An ordinary user starts bwrap with a network namespace of its own: inside there is only a loopback. The proxy is
      // reached through a socket file: socat outside turns it into the proxy's TCP, socat inside offers it on the
      // box's own 127.0.0.1:3128. This is how a box with no network still gets one door. No new /proc and no process
      // namespace: inside a container a new /proc cannot be mounted over the one the container masks.
      if (box === 'bwrap, seccomp off') extra.push('--security-opt', 'seccomp=unconfined');
      const inside = `socat TCP-LISTEN:3128,bind=127.0.0.1,fork,reuseaddr UNIX-CONNECT:/tmp/door/proxy.sock & sleep 0.5; ${argv.map((a) => `'${a}'`).join(' ')}`;
      cmd = ['sh', '-c', `mkdir -p /tmp/door && chown node /tmp/door && (socat UNIX-LISTEN:/tmp/door/proxy.sock,fork,mode=777 TCP:${PROXY_WEB}:3128 &) && sleep 0.3 && exec setpriv --reuid=node --regid=node --clear-groups --inh-caps=-all --bounding-set=-all `
        + `bwrap --unshare-net --die-with-parent --ro-bind / / --dev-bind /dev /dev --bind /tmp/door /tmp/door --tmpfs /home sh -c "${inside.replaceAll('"', '\\"')}" 2>&1`];
    }
    docker('create', '--name', name, '--network', box === 'forced' ? 'rs-box' : 'bridge', '--add-host', 'host.docker.internal:host-gateway', ...extra, '-v', `${HERE}:/spike:ro`, ...e, IMAGE, ...cmd);
    if (box !== 'forced') docker('network', 'connect', 'rs-web', name);
    // A program that is killed at the end, as the browser is, makes the container end in error; what it printed counts.
    try { return execFileSync('docker', ['start', '-a', name], { timeout: 120000, stdio: ['ignore', 'pipe', 'pipe'] }).toString(); }
    catch (e) { if (!String(e.stdout || '').trim()) throw new Error(`${box}: ${String(e.stderr || e.message).slice(0, 400)}`); return String(e.stdout); }
    finally { quiet('rm', '-f', name); }
  };

  const BOXES = ['open', 'forced', 'firewall', 'bwrap', 'bwrap, seccomp off'];
  const table = {};
  for (const box of BOXES) {
    reset('rs-evil');
    await fetch(`http://127.0.0.1:${HOST_PORT}/__reset`);
    const before = proxyLog().filter((l) => l.verdict === 'refused').length;
    const proxy = box.startsWith('bwrap') ? '127.0.0.1:3128' : box === 'forced' ? `${PROXY_BOX}:3128` : `${PROXY_WEB}:3128`;
    const out = inBox(box, ['node', '/spike/probe.mjs'], { PROXY: proxy, ALLOWED: 'allowed.test:80', EVIL: 'evil.test:80', EVIL_IP: `${EVIL_IP}:80`,
      HOST_SERVICE: `host.docker.internal:${HOST_PORT}`, DEBUGGER: `host.docker.internal:${DEBUG_PORT}` });
    const last = out.trim().split('\n').pop();
    if (!last.startsWith('{')) { table[box] = { failed: out.trim().slice(0, 300) }; continue; }
    table[box] = { tries: JSON.parse(last), far: { elsewhere: hits('rs-evil'), 'person service': await hostHits() },
      proxyRefused: proxyLog().filter((l) => l.verdict === 'refused').length - before };
  }

  // The browser, forced: its page tries elsewhere by name and by address, and posts what it saw to its own origin.
  const browsers = {};
  for (const [name, evil] of [['forced, elsewhere by name', 'http://evil.test'], ['forced, elsewhere by address', `http://${EVIL_IP}`]]) {
    reset('rs-evil');
    reset('rs-allowed');
    const before = proxyLog().filter((l) => l.verdict === 'refused').length;
    const page = `http://allowed.test/page?evil=${encodeURIComponent(evil)}`;
    const script = `chromium --headless=new --no-sandbox --disable-gpu --disable-dev-shm-usage --disable-background-networking --disable-component-update --user-data-dir=/tmp/p --proxy-server=http://${PROXY_BOX}:3128 '${page}' >/dev/null 2>/tmp/err & sleep 15; kill %1 2>/dev/null; echo ended; grep -v dbus /tmp/err | tail -4`;
    const said = inBox('forced', ['sh', '-c', script], {});
    const seen = docker('exec', 'rs-allowed', 'node', '-e', "fetch('http://127.0.0.1/__result').then((r) => r.text()).then(console.log)");
    browsers[name] = { page: seen ? JSON.parse(seen) : null, said: seen ? undefined : said.trim(), far: { elsewhere: hits('rs-evil') }, proxyRefused: proxyLog().filter((l) => l.verdict === 'refused').length - before };
  }
  fs.writeFileSync(path.join(RUNS, 'proxy.jsonl'), proxyLog().map((l) => JSON.stringify(l)).join('\n') + '\n');
  fs.writeFileSync(path.join(RUNS, 'result.json'), JSON.stringify({ table, browsers }, null, 2));
  console.log(docker('run', '--rm', IMAGE, 'sh', '-c', 'uname -sm; chromium --version'));
  const ran = BOXES.filter((b) => table[b].tries);
  console.log(['try', ...ran].join(' | '));
  for (const t of Object.keys(table.open.tries)) console.log([t, ...ran.map((b) => `${table[b].tries[t].out}${table[b].tries[t].code ? ' ' + table[b].tries[t].code : ''}`)].join(' | '));
  for (const b of BOXES) console.log(b, table[b].failed ? `FAILED: ${table[b].failed}` : `far: ${JSON.stringify(table[b].far)} proxy refused: ${table[b].proxyRefused}`);
  for (const [n, b] of Object.entries(browsers)) console.log('BROWSER', n, JSON.stringify(b));
  fs.rmSync(dbg, { recursive: true, force: true });
} finally {
  for (const c of hostSide) { try { process.kill(-c.pid, 'SIGKILL'); } catch {} }
  cleanup();
  await sleep(300);
  const left = execFileSync('ps', ['-axo', 'pid=,command=']).toString().split('\n').filter((l) => l.includes('reach-debugger-') || l.includes(`web.mjs host ${HOST_PORT}`));
  for (const l of left) { try { process.kill(Number(l.trim().split(' ')[0]), 'SIGKILL'); } catch {} }
  console.log(`processes of this spike left alive: ${left.length}; containers left: ${quiet('ps', '-aq', '--filter', 'name=rs-').split('\n').filter(Boolean).length}`);
}
