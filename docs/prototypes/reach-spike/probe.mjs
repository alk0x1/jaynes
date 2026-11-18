// THROWAWAY (reach spike). Runs inside a box and tries every way out it knows, as a program that wants out would.
// It sends nothing but a request line or an empty datagram. Prints one JSON object: each try and what came of it.
// Targets come from the environment, so the same probe runs on macOS and in Linux:
//   PROXY=host:port ALLOWED=name:port EVIL=name:port EVIL_IP=ip:port HOST_SERVICE=host:port DEBUGGER=host:port

import dgram from 'node:dgram';
import dns from 'node:dns/promises';
import http from 'node:http';
import net from 'node:net';

const env = process.env;
const split = (hp) => { const i = hp.lastIndexOf(':'); return [hp.slice(0, i), Number(hp.slice(i + 1))]; };
const [PH, PP] = split(env.PROXY || '127.0.0.1:9100');

const within = (ms, p) => Promise.race([p, new Promise((_, no) => setTimeout(() => no(Object.assign(new Error('timeout'), { code: 'TIMEOUT' })), ms))]);

/// A plain HTTP GET, to `via` (the proxy, in absolute form) or straight to the host.
const get = (url, via) => new Promise((ok, no) => {
  const u = new URL(url);
  const [host, port] = via ? split(via) : [u.hostname, Number(u.port || 80)];
  const req = http.request({ host, port, path: via ? url : u.pathname, headers: { host: u.host }, agent: false }, (res) => {
    res.resume();
    res.on('end', () => ok({ status: res.statusCode, location: res.headers.location }));
  });
  req.on('error', no);
  req.end();
});

/// A CONNECT through the proxy, then a WebSocket handshake inside it.
const websocket = (target) => new Promise((ok, no) => {
  const s = net.connect(PP, PH, () => s.write(`CONNECT ${target} HTTP/1.1\r\nhost: ${target}\r\n\r\n`));
  let buf = '';
  let stage = 'connect';
  s.on('data', (d) => {
    buf += d;
    if (stage === 'connect' && buf.includes('\r\n\r\n')) {
      if (!buf.startsWith('HTTP/1.1 200')) { s.destroy(); return no(Object.assign(new Error(buf.split('\r\n')[0]), { code: 'PROXY_403' })); }
      stage = 'ws';
      buf = '';
      s.write(`GET /ws HTTP/1.1\r\nhost: ${target}\r\nupgrade: websocket\r\nconnection: Upgrade\r\nsec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\nsec-websocket-version: 13\r\n\r\n`);
    } else if (stage === 'ws' && buf.includes('\r\n\r\n')) { s.destroy(); ok({ status: Number(buf.split(' ')[1]) }); }
  });
  s.on('error', no);
});

const connect = (hp) => new Promise((ok, no) => { const [h, p] = split(hp); const s = net.connect(p, h, () => { s.destroy(); ok({ connected: true }); }); s.on('error', no); });
const datagram = (hp) => new Promise((ok, no) => { const [h, p] = split(hp); const s = dgram.createSocket('udp4'); s.send(Buffer.alloc(0), p, h, (e) => { s.close(); e ? no(e) : ok({ sent: true }); }); });

const TRIES = {
  'allowed, through the proxy': () => get(`http://${env.ALLOWED}/`, env.PROXY),
  'elsewhere, through the proxy': () => get(`http://${env.EVIL}/`, env.PROXY),
  'redirect to elsewhere': async () => {
    const first = await get(`http://${env.ALLOWED}/redirect`, env.PROXY);
    return { first: first.status, then: (await get(first.location, env.PROXY)).status };
  },
  'websocket, allowed': () => websocket(env.ALLOWED),
  'websocket, elsewhere': () => websocket(env.EVIL),
  'allowed name pointing home (rebinding)': () => get('http://rebind.test/', env.PROXY),
  'elsewhere, straight by name': () => get(`http://${env.EVIL}/`),
  'elsewhere, straight by IP': () => get(`http://${env.EVIL_IP}/`),
  'elsewhere, UDP datagram': () => datagram(env.EVIL_IP),
  'DNS lookup of a public name': () => dns.lookup('example.com').then((a) => ({ address: a.address })),
  'public internet, TCP': () => connect('1.1.1.1:443'),
  "person's machine, a local service": () => connect(env.HOST_SERVICE),
  "person's machine, a browser's debugger": () => get(`http://${env.DEBUGGER}/json/version`).then((r) => ({ status: r.status })),
  'cloud metadata address': () => connect('169.254.169.254:80'),
};

/// What a try came to: reached the far side, refused by the proxy, or stopped by the box before leaving.
function verdict(value, error) {
  if (error) {
    const code = error.code || error.message;
    if (code === 'PROXY_403') return { out: 'refused by proxy', code };
    if (code === 'ECONNREFUSED') return { out: 'left, nothing listening', code };
    if (code === 'TIMEOUT' || code === 'ETIMEDOUT') return { out: 'no answer', code };
    return { out: 'blocked', code };
  }
  const statuses = [value.status, value.first, value.then].filter((s) => s !== undefined);
  if (statuses.includes(403)) return { out: 'refused by proxy', ...value };
  return { out: 'reached', ...value };
}

const results = {};
for (const [name, f] of Object.entries(TRIES)) {
  try { results[name] = verdict(await within(3000, f())); } catch (e) { results[name] = verdict(null, e); }
}
console.log(JSON.stringify(results));
