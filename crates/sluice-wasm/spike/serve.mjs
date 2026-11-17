// Serves the spike's page, the module `wasm-bindgen --target web` writes beside it, and the config
// the native peer wrote; then prints the one result the page posts and exits.
//
//   node serve.mjs [--port 8811] [--config config.json] [--out result.json]
//
// The page is loaded by a browser started at http://127.0.0.1:PORT/; nothing here drives the
// browser, so the same script serves Chromium, Firefox or anything else that can open a page.

import { readFile, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const argv = process.argv.slice(2);
const argument = (name, fallback) => {
  const index = argv.indexOf(name);
  return index >= 0 && index + 1 < argv.length ? argv[index + 1] : fallback;
};

const port = Number(argument('--port', '8811'));
const configPath = resolve(argument('--config', 'config.json'));
const outPath = resolve(argument('--out', 'result.json'));
const root = dirname(fileURLToPath(import.meta.url));

const TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.json': 'application/json',
  '.wasm': 'application/wasm',
};

const server = createServer(async (request, response) => {
  try {
    if (request.method === 'POST' && request.url === '/result') {
      const chunks = [];
      for await (const chunk of request) chunks.push(chunk);
      const body = Buffer.concat(chunks).toString('utf8');
      await writeFile(outPath, `${body}\n`);
      console.log(body);
      response.writeHead(204).end();
      server.close();
      return;
    }
    const wanted = request.url === '/' ? '/page.html' : request.url.split('?')[0];
    const path = wanted === '/config.json' ? configPath : join(root, wanted.replace(/^\/+/, ''));
    const type = TYPES[path.slice(path.lastIndexOf('.'))] ?? 'application/octet-stream';
    const body = await readFile(path);
    response.writeHead(200, { 'content-type': type }).end(body);
  } catch (error) {
    response.writeHead(404, { 'content-type': 'text/plain' }).end(String(error));
  }
});

server.listen(port, '127.0.0.1', () => {
  console.log(`serving ${root} on http://127.0.0.1:${port}/ with ${configPath}`);
});
