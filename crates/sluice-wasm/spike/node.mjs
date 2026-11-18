// The Node half of the post-quantum spike: the handshake both ways, then the blob, from the module
// `wasm-bindgen --target nodejs` writes.
//
//   node node.mjs [config.json]
//
// The numbers come from the module, not from the clock here: a handshake is timed around `connect`
// and a fetch around the download, so neither includes binding an endpoint or waiting for a relay.

import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const config = JSON.parse(readFileSync(process.argv[2] ?? 'config.json', 'utf8'));
const wasm = require('./out/node/sluice_wasm.js');

const ROUNDS = Number(process.env.ROUNDS ?? 50);
const FETCHES = Number(process.env.FETCHES ?? 5);
const MODES = ['classical', 'hybrid'];

function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
}

function stats(values) {
  return {
    median: Number(median(values).toFixed(3)),
    min: Number(Math.min(...values).toFixed(3)),
    max: Number(Math.max(...values).toFixed(3)),
  };
}

const report = {
  runtime: `node ${process.version}`,
  rounds: ROUNDS,
  fetches: FETCHES,
  size: config.size,
  handshake: {},
  fetch: {},
};

for (const mode of MODES) {
  // Two that are not counted: the first handshake pays for a certificate verifier and a relay
  // registration that later ones do not.
  await wasm.pq_handshake(config.relay, config.endpoint, mode);
  await wasm.pq_handshake(config.relay, config.endpoint, mode);
  const times = [];
  let group = 'none';
  for (let round = 0; round < ROUNDS; round += 1) {
    const result = JSON.parse(await wasm.pq_handshake(config.relay, config.endpoint, mode));
    times.push(result.handshake_ms);
    group = result.group;
  }
  report.handshake[mode] = { group, ...stats(times) };
  console.log(JSON.stringify({ event: 'handshake', mode, ...report.handshake[mode] }));
}

for (const mode of MODES) {
  const warm = await wasm.fetch_blob_pq(
    config.relay,
    config.endpoint,
    config.root,
    config.size,
    mode,
  );
  if (warm.length !== config.size) {
    throw new Error(`the warmup fetched ${warm.length} bytes, not ${config.size}`);
  }
  const transfers = [];
  const walls = [];
  for (let round = 0; round < FETCHES; round += 1) {
    const started = performance.now();
    const bytes = await wasm.fetch_blob_pq(
      config.relay,
      config.endpoint,
      config.root,
      config.size,
      mode,
    );
    const wall = performance.now() - started;
    if (bytes.length !== config.size) {
      throw new Error(`the fetch got ${bytes.length} bytes, not ${config.size}`);
    }
    transfers.push(wasm.pq_last_fetch_ms());
    walls.push(wall);
  }
  const seconds = median(transfers) / 1000;
  report.fetch[mode] = {
    ...stats(transfers),
    wall_ms: Number(median(walls).toFixed(3)),
    mib_per_second: Number((config.size / 1048576 / seconds).toFixed(1)),
  };
  console.log(JSON.stringify({ event: 'fetch', mode, ...report.fetch[mode] }));
  console.log(
    JSON.stringify({
      event: 'fetch-memory',
      mode,
      rss_mib: Number((process.memoryUsage().rss / 1048576).toFixed(1)),
    }),
  );
}

console.log(JSON.stringify({ event: 'done', report }));
