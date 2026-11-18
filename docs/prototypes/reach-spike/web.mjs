// THROWAWAY (reach spike). A pretend web: `allowed` is the origin a step may reach, `evil` is anywhere else, and
// `host` stands for a service on the person's own machine. Each counts every request, connection and datagram it
// gets, so what leaves a box is also seen from the far side. The count is read at /__hits, from outside the box.
//   node web.mjs allowed|evil|host <port> [<evil origin for the page>]

import dgram from 'node:dgram';
import http from 'node:http';
import { createHash } from 'node:crypto';

const [role, portArg, evilArg] = process.argv.slice(2);
const PORT = Number(portArg);
const HOST = process.env.BIND || '127.0.0.1';
const hits = [];
let result = '';
const hit = (what) => hits.push({ at: Date.now(), what });

// The page a boxed browser loads: everything a page can do to reach elsewhere, each one written into the page.
const page = (evil) => `<!doctype html><meta charset="utf-8"><title>reach</title><body><pre id="out"></pre><script>
const out = {}; const done = () => { document.getElementById('out').textContent = JSON.stringify(out); fetch('/result', { method: 'POST', body: JSON.stringify(out) }); };
const t = (p, ms) => Promise.race([p, new Promise((_, no) => setTimeout(() => no(new Error('timeout')), ms))]);
const tries = {
  allowed_fetch: () => fetch('/').then((r) => r.status),
  evil_fetch: () => fetch('${evil}/fetch', { mode: 'no-cors' }).then(() => 'reached'),
  evil_image: () => new Promise((ok, no) => { const i = new Image(); i.onload = () => ok('reached'); i.onerror = () => no(new Error('error')); i.src = '${evil}/img'; }),
  evil_websocket: () => new Promise((ok, no) => { const w = new WebSocket('${evil.replace('http', 'ws')}/ws'); w.onopen = () => ok('reached'); w.onerror = () => no(new Error('error')); }),
  redirect_to_evil: () => fetch('/redirect', { mode: 'no-cors' }).then((r) => r.type + ' ' + r.status),
  beacon: () => Promise.resolve(navigator.sendBeacon('${evil}/beacon', 'x') ? 'queued' : 'refused'),
};
Promise.all(Object.entries(tries).map(([k, f]) => t(f(), 4000).then((v) => (out[k] = String(v)), (e) => (out[k] = 'failed: ' + e.message)))).then(() => setTimeout(done, 1500));
</script>`;

const server = http.createServer((req, res) => {
  const url = new URL(req.url, 'http://x');
  if (url.pathname.startsWith('/__')) {
    // the harness's own reading, from outside the box: its connection is not a hit
    const mine = hits.findIndex((h) => h.socket === req.socket);
    if (mine >= 0) hits.splice(mine, 1);
    if (url.pathname === '/__reset') { hits.length = 0; result = ''; }
    if (url.pathname === '/__result') return res.end(result);
    return res.end(JSON.stringify(hits.map(({ at, what }) => ({ at, what }))));
  }
  hit(`${req.method} ${req.url}`);
  if (role === 'allowed' && url.pathname === '/page') return res.writeHead(200, { 'content-type': 'text/html' }).end(page(url.searchParams.get('evil') || evilArg));
  if (role === 'allowed' && url.pathname === '/result') { let b = ''; req.on('data', (c) => (b += c)); req.on('end', () => { result = b; res.end(); }); return; }
  if (role === 'allowed' && url.pathname === '/redirect') return res.writeHead(302, { location: `${evilArg}/landed` }).end();
  res.writeHead(200, { 'content-type': 'text/plain', 'access-control-allow-origin': '*' }).end(`${role}\n`);
});
server.on('connection', (socket) => hits.push({ at: Date.now(), what: 'tcp', socket }));
server.on('upgrade', (req, socket) => {
  hit(`upgrade ${req.url}`);
  const key = createHash('sha1').update(req.headers['sec-websocket-key'] + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest('base64');
  socket.write(`HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: Upgrade\r\nsec-websocket-accept: ${key}\r\n\r\n`);
});
server.listen(PORT, HOST);
const udp = dgram.createSocket('udp4');
udp.on('message', (m) => hit(`udp ${m.length} bytes`));
udp.bind(PORT, HOST);
console.log(`${role} on ${HOST}:${PORT}`);
