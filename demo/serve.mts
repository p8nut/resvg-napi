// Static server for the WASI demo. The wasm is built for
// wasm32-wasip1-threads, so it asks for a shared WebAssembly.Memory --
// which needs SharedArrayBuffer, which needs cross-origin isolation.
// Hence the two headers below: without them the module fails to instantiate.
import { createServer } from 'node:http';
import type { AddressInfo } from 'node:net';
import { readFile } from 'node:fs/promises';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('.', import.meta.url));  // see render.mjs
const types: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript',
  '.mjs': 'text/javascript',
  '.wasm': 'application/wasm',
  '.svg': 'image/svg+xml',
  '.json': 'application/json',
  '.ttf': 'font/ttf',
};

const server = createServer(async (req, res) => {
  const url = (req.url ?? '/').split('?')[0];
  const path = join(root, normalize(decodeURIComponent(url)));
  const file = path.endsWith('/') ? join(path, 'index.html') : path;
  try {
    const body = await readFile(file);
    res.writeHead(200, {
      'content-type': types[extname(file)] ?? 'application/octet-stream',
      'cross-origin-opener-policy': 'same-origin',
      'cross-origin-embedder-policy': 'require-corp',
      'cache-control': 'no-store',
    });
    res.end(body);
  } catch {
    res.writeHead(404).end('not found');
  }
});

// PORT=0 takes any free port; the line below then names the one it got, which
// is how demo/smoke.mts finds the server it started.
const port = Number(process.env.PORT ?? 8787);
server.listen(port, () => {
  const { port: bound } = server.address() as AddressInfo;
  console.log(`demo on http://localhost:${bound}/`);
});
