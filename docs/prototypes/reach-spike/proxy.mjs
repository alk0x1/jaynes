// THROWAWAY (reach spike). The one door out of a box: a proxy that lets through only the origins of a list. It
// resolves names itself, refuses any address in a closed range (loopback, private, link-local) unless it is one
// of the `open` exceptions, and writes one line per request, let through or refused.
//   node proxy.mjs --port 9100 --allow allowed.test:80 [--map name=ip:port] [--open 10.0.0.0/8|ip:port] --log file

import dns from 'node:dns/promises';
import fs from 'node:fs';
import http from 'node:http';
import net from 'node:net';

const args = process.argv.slice(2);
const many = (flag) => args.flatMap((a, i) => (args[i - 1] === flag ? a.split(',') : [])).filter(Boolean);
const one = (flag, d) => (args.includes(flag) ? args[args.indexOf(flag) + 1] : d);
const PORT = Number(one('--port', 9100));
const HOST = one('--host', '127.0.0.1');
const ALLOW = new Set(many('--allow').map((o) => o.toLowerCase()));
const MAP = Object.fromEntries(many('--map').map((m) => m.split('=')));
const OPEN = many('--open');
const LOG = one('--log', 'proxy.jsonl');

const v4 = (ip) => ip.split('.').reduce((n, p) => n * 256 + Number(p), 0);
const inCidr = (ip, cidr) => {
  const [base, len] = cidr.split('/');
  const size = 2 ** (32 - Number(len));
  return net.isIPv4(ip) && Math.floor(v4(ip) / size) === Math.floor(v4(base) / size);
};
const CLOSED = ['0.0.0.0/8', '10.0.0.0/8', '100.64.0.0/10', '127.0.0.0/8', '169.254.0.0/16', '172.16.0.0/12', '192.168.0.0/16'];
const closed = (ip) => (net.isIPv6(ip) ? /^(::1$|::$|f[cd]|fe[89ab]|::ffff:)/i.test(ip) : CLOSED.some((c) => inCidr(ip, c)));
const opened = (ip, port) => OPEN.some((o) => (o.includes('/') ? inCidr(ip, o) : o === `${ip}:${port}`));

const log = (fact) => fs.appendFileSync(LOG, JSON.stringify({ at: new Date().toISOString(), ...fact }) + '\n');

/// Where `host:port` may be reached, or why not. Names only: the list holds origins, never addresses.
async function decide(host, port) {
  const origin = `${host.toLowerCase()}:${port}`;
  if (!ALLOW.has(origin)) return { refused: 'not on the list' };
  let ip = host;
  let to = port;
  if (MAP[host]) [ip, to] = MAP[host].split(':');
  else if (!net.isIP(host)) {
    try { ip = (await dns.lookup(host)).address; } catch (e) { return { refused: `does not resolve: ${e.code}` }; }
  }
  // An allowed name that points into a closed range (a rebinding, or a name for the person's own machine) is refused.
  if (closed(ip) && !opened(ip, Number(to))) return { refused: `resolves to ${ip}, a closed address` };
  return { ip, port: Number(to) };
}

const server = http.createServer(async (req, res) => {
  let url;
  try { url = new URL(req.url); } catch { res.writeHead(400).end(); return log({ kind: 'http', target: req.url, verdict: 'refused', why: 'not a proxy request' }); }
  const port = Number(url.port || 80);
  const d = await decide(url.hostname, port);
  const target = `${req.method} ${url.href}`;
  if (d.refused) {
    log({ kind: 'http', target, verdict: 'refused', why: d.refused });
    return res.writeHead(403, { 'content-type': 'text/plain' }).end(`refused by the reach: ${d.refused}\n`);
  }
  log({ kind: 'http', target, verdict: 'allowed', to: `${d.ip}:${d.port}` });
  const up = http.request({ host: d.ip, port: d.port, method: req.method, path: url.pathname + url.search, headers: { ...req.headers, host: url.host } }, (r) => {
    res.writeHead(r.statusCode, r.headers);
    r.pipe(res);
  });
  up.on('error', (e) => res.writeHead(502).end(String(e.code)));
  req.pipe(up);
});

// HTTPS and WebSocket go through CONNECT: the proxy sees the name and the port, never what is inside.
const tunnel = async (req, client, head, kind) => {
  const [host, p] = kind === 'connect' ? req.url.split(':') : [new URL(req.url).hostname, new URL(req.url).port || 80];
  const d = await decide(host, Number(p));
  if (d.refused) {
    log({ kind, target: req.url, verdict: 'refused', why: d.refused });
    return client.end('HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\n\r\n');
  }
  log({ kind, target: req.url, verdict: 'allowed', to: `${d.ip}:${d.port}` });
  const up = net.connect(d.port, d.ip, () => {
    if (kind === 'connect') client.write('HTTP/1.1 200 Connection Established\r\n\r\n');
    else up.write(`${req.method} ${new URL(req.url).pathname} HTTP/1.1\r\n${Object.entries(req.headers).map(([k, v]) => `${k}: ${v}`).join('\r\n')}\r\n\r\n`);
    up.write(head);
    up.pipe(client).pipe(up);
  });
  up.on('error', () => client.destroy());
  client.on('error', () => up.destroy());
};
server.on('connect', (req, client, head) => tunnel(req, client, head, 'connect'));
server.on('upgrade', (req, client, head) => tunnel(req, client, head, 'upgrade'));

server.listen(PORT, HOST, () => console.log(`reach proxy on ${HOST}:${PORT}, allows ${[...ALLOW].join(' ')}`));
