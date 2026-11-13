// "Acme", a customer's web app for the hook browser tests: one page, served
// from a real local server on another origin than litegen's. It calls litegen
// with the customer's own token (x-acme-token), never a litegen key.
// Started by playwright.hooks.config.ts.
import http from 'node:http';

const PORT = Number(process.env.ACME_PORT ?? 5299);
const LITEGEN = process.env.ACME_LITEGEN ?? 'http://127.0.0.1:5199';

const page = `<!doctype html><html><head><meta charset="utf-8"><title>Acme</title>
<style>body{font:16px system-ui;margin:40px;background:#0f141a;color:#e8edf3}
code{color:#ffd27a} #status{margin:16px 0;font-size:20px} img{width:256px;border-radius:8px}</style></head>
<body><h1>Acme app on <code>http://127.0.0.1:${PORT}</code></h1>
<p>Calls litegen at <code>${LITEGEN}</code> with its own <code>x-acme-token</code>, never a litegen key.</p>
<div id="status">…</div><img id="out" alt="">
<script>
async function generate(token) {
  const status = document.getElementById('status');
  status.textContent = 'POST /v1/images/generations with x-acme-token: ' + token;
  const res = await fetch('${LITEGEN}/v1/images/generations', {
    method: 'POST',
    headers: { 'content-type': 'application/json', 'x-acme-token': token },
    body: JSON.stringify({ model: 'mock/visual-image-gen', prompt: 'a fox' }),
  });
  const body = await res.json();
  status.textContent = res.status + ' ' + (res.ok ? 'image generated' : (body && body.error && body.error.message) || '');
  const image = body && body.data && body.data[0];
  if (image) document.getElementById('out').src = image.url || ('data:image/png;base64,' + image.b64_json);
  return { status: res.status, body };
}
</script></body></html>`;

http
  .createServer((req, res) => {
    res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
    res.end(page);
  })
  .listen(PORT, '127.0.0.1');
