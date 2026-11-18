// THROWAWAY (target spike). The test page of case B, served on 127.0.0.1 only: one setting kept by the server, a
// Save button, and a second port standing for "somewhere else" the session must not reach.
// The /__test paths are the harness's: it arranges the page and reads the truth. The scripts never use them.

import http from 'node:http';

export const PORT = Number(process.env.SPIKE_PORT || 8793);
export const ELSEWHERE = PORT + 1;
export const ORIGIN = `http://127.0.0.1:${PORT}`;

export const VARIANTS = ['normal', 'moved', 'restyled', 'renamed', 'extra-field', 'broken-save'];

const state = { digest: false, variant: 'normal', saves: 0, elsewhere: 0 };

function html() {
  const v = state.variant;
  const label = v === 'renamed' ? 'Summary e-mail' : 'Weekly digest';
  const name = v === 'renamed' ? 'summary' : 'digest';
  const dark = v === 'restyled';
  return `<!doctype html><html><head><meta charset="utf-8"><title>Notification settings</title><style>
  html, body { margin: 0; background: ${dark ? '#1c1f24' : '#ffffff'}; color: ${dark ? '#e8e8e8' : '#111111'}; font: ${dark ? 18 : 16}px/1 Helvetica, Arial, sans-serif; }
  body { padding: 30px 40px; }
  h1 { margin: 0 0 50px; height: 40px; font-size: 28px; line-height: 40px; }
  .banner { height: 60px; line-height: 60px; }
  .row { display: flex; align-items: center; height: 36px; margin: 0 0 34px; }
  .row input { appearance: none; margin: 0 14px 0 0; width: 28px; height: 28px; border: 2px solid ${dark ? '#9aa3ad' : '#333333'}; background: ${dark ? '#1c1f24' : '#ffffff'}; }
  .row input:checked { background: ${dark ? '#58c98a' : '#1a8f4c'}; border-color: ${dark ? '#58c98a' : '#1a8f4c'}; }
  button { width: 120px; height: 40px; border: 0; background: ${dark ? '#7d6cf0' : '#1f5fd1'}; color: #ffffff; font: inherit; }
  #status { display: inline-block; margin-left: 20px; }
  </style></head><body>
  ${v === 'moved' ? '<div class="banner">New: settings now apply to every device.</div>' : ''}
  <h1>Notification settings</h1>
  <form id="settings">
    <label class="row"><input type="checkbox" name="${name}" ${state.digest ? 'checked' : ''}>${label}</label>
    ${v === 'extra-field' ? '<label class="row"><input type="checkbox" name="share">Share usage data</label>' : ''}
    <button type="submit">Save</button><span id="status"></span>
  </form>
  <img src="http://127.0.0.1:${ELSEWHERE}/pixel" width="1" height="1" alt="">
  <script>
  document.getElementById('settings').addEventListener('submit', async (e) => {
    e.preventDefault();
    const box = document.querySelector('.row input');
    const r = await fetch('/save', { method: 'POST', body: JSON.stringify({ digest: box.checked }) });
    document.getElementById('status').textContent = r.ok ? 'Saved' : 'Not saved';
  });
  </script></body></html>`;
}

const body = (req) => new Promise((done) => { let b = ''; req.on('data', (c) => (b += c)); req.on('end', () => done(b)); });

export function serve() {
  const page = http.createServer(async (req, res) => {
    const url = new URL(req.url, ORIGIN);
    if (url.pathname === '/') {
      res.writeHead(200, { 'content-type': 'text/html', 'cache-control': 'no-store' }).end(html());
    } else if (url.pathname === '/save' && req.method === 'POST') {
      const sent = JSON.parse(await body(req));
      state.saves += 1;
      if (state.variant !== 'broken-save') state.digest = !!sent.digest; // broken: answers 200 and keeps nothing
      res.writeHead(200, { 'content-type': 'application/json' }).end('{}');
    } else if (url.pathname === '/__test/set') {
      if (url.searchParams.has('digest')) state.digest = url.searchParams.get('digest') === '1';
      if (url.searchParams.has('variant')) state.variant = url.searchParams.get('variant');
      state.saves = 0;
      state.elsewhere = 0;
      res.end(JSON.stringify(state));
    } else if (url.pathname === '/__test/state') {
      res.end(JSON.stringify(state));
    } else res.writeHead(404).end();
  });
  const elsewhere = http.createServer((req, res) => {
    state.elsewhere += 1;
    res.writeHead(200, { 'content-type': 'image/gif' }).end(Buffer.from('R0lGODlhAQABAAAAACwAAAAAAQABAAA=', 'base64'));
  });
  return new Promise((done) => page.listen(PORT, '127.0.0.1', () => elsewhere.listen(ELSEWHERE, '127.0.0.1', () => done({ close: () => { page.close(); elsewhere.close(); page.closeAllConnections(); elsewhere.closeAllConnections(); } }))));
}

/// The harness's hands: arrange the page, and read what the server really holds.
export const arrange = async (set) => (await fetch(`${ORIGIN}/__test/set?${new URLSearchParams(set)}`)).json();
export const truth = async () => (await fetch(`${ORIGIN}/__test/state`)).json();

if (import.meta.filename === process.argv[1]) serve().then(() => console.log(`test page at ${ORIGIN}/ (elsewhere at ${ELSEWHERE})`));
