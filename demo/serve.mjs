// Static server for the WASI demo. The wasm is built for
// wasm32-wasip1-threads, so it asks for a shared WebAssembly.Memory --
// which needs SharedArrayBuffer, which needs cross-origin isolation.
// Hence the two headers below: without them the module fails to instantiate.
import { createServer } from 'node:http';
import { readFile, readdir } from 'node:fs/promises';
import { extname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('.', import.meta.url));  // see render.mjs
const types = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript',
  '.mjs': 'text/javascript',
  '.wasm': 'application/wasm',
  '.svg': 'image/svg+xml',
  '.json': 'application/json',
  '.ttf': 'font/ttf',
};

const server = createServer(async (req, res) => {
  const url = req.url.split('?')[0];
  // The page's example picker: whatever is in examples/, no list to keep in sync.
  if (url === '/examples/') {
    const files = (await readdir(join(root, 'examples'))).filter((f) => f.endsWith('.svg')).sort();
    // A fragment (`badge.svg`) has no root <svg> and is not an example: the
    // template that renders it pulls it in by name.
    const heads = await Promise.all(files.map((f) =>
      readFile(join(root, 'examples', f), 'utf8').then((t) => t.slice(0, 400))));
    const names = files.filter((_, i) => /<svg[\s>]/.test(heads[i]));
    res.writeHead(200, {
      'content-type': 'application/json',
      'cross-origin-opener-policy': 'same-origin',
      'cross-origin-embedder-policy': 'require-corp',
      'cache-control': 'no-store',
    });
    res.end(JSON.stringify(names));
    return;
  }
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

const port = Number(process.env.PORT ?? 8787);
server.listen(port, () => console.log(`demo on http://localhost:${port}/`));
