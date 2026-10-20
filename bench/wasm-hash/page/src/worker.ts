/**
 * One job per worker: the page starts a fresh worker for the self-check and for each case. A
 * finished worker is terminated, which is the only way to give WASM memory back, and that lets a
 * phone that cannot hold the in-memory case's buffers twice still run the next case.
 */

import {
  BLOCK_BYTES,
  fillPattern,
  OFFICIAL_ROOTS,
  patternBlob,
  patternBlock,
  withHalving,
} from "./data";
import scalarWasmUrl from "./generated/scalar/sluice_wasm_hash_bg.wasm?url";
import simdWasmUrl from "./generated/simd/sluice_wasm_hash_bg.wasm?url";
import {
  type CaseId,
  type CaseResult,
  CHUNK_BYTES,
  type JobRequest,
  type KnownRoot,
  type Params,
  type Variant,
  WARMUP_RUNS,
  type WorkerScope,
} from "./protocol";
import { run, summarize } from "./stats";

type Wasm = typeof import("./generated/simd/sluice_wasm_hash.js");

interface Measured {
  seconds: number;
  chunks: number;
  root: string;
}

interface Subject {
  bytes: number;
  failed: number[];
  chunkBytes: number | null;
  measure(): Promise<Measured>;
  dispose(): void;
}

const scope = self as unknown as WorkerScope;

async function load(variant: Variant): Promise<Wasm> {
  const [glue, url] =
    variant === "simd"
      ? [await import("./generated/simd/sluice_wasm_hash.js"), simdWasmUrl]
      : [await import("./generated/scalar/sluice_wasm_hash.js"), scalarWasmUrl];
  const bytes = await (await fetch(url)).arrayBuffer();
  await glue.default({ module_or_path: bytes });
  return glue;
}

function seconds(from: number): number {
  return (performance.now() - from) / 1000;
}

/** The root of `bytes` bytes of the input, fed from a JavaScript block. Not timed. */
function referenceRoot(wasm: Wasm, bytes: number): string {
  const block = patternBlock();
  const hasher = new wasm.Hasher();
  for (let left = bytes; left > 0; left -= BLOCK_BYTES) {
    hasher.update(block.subarray(0, Math.min(left, BLOCK_BYTES)));
  }
  const root = hasher.finalize_hex();
  hasher.free();
  return root;
}

function inMemory(wasm: Wasm, requested: number): Subject | { error: string; failed: number[] } {
  const made = withHalving(requested, (bytes) => new wasm.Pattern(bytes));
  if ("error" in made) {
    return made;
  }
  const { value: pattern, bytes } = made;
  return {
    bytes,
    failed: made.failed,
    chunkBytes: CHUNK_BYTES,
    async measure() {
      const start = performance.now();
      const root = pattern.hash(CHUNK_BYTES);
      return { seconds: seconds(start), chunks: Math.ceil(bytes / CHUNK_BYTES), root };
    },
    dispose: () => pattern.free(),
  };
}

function copiedIn(wasm: Wasm, requested: number): Subject | { error: string; failed: number[] } {
  const made = withHalving(requested, (bytes) => {
    const buffer = new Uint8Array(bytes);
    fillPattern(buffer);
    return buffer;
  });
  if ("error" in made) {
    return made;
  }
  const { value: buffer, bytes } = made;
  return {
    bytes,
    failed: made.failed,
    chunkBytes: CHUNK_BYTES,
    async measure() {
      const hasher = new wasm.Hasher();
      const start = performance.now();
      let chunks = 0;
      for (let offset = 0; offset < bytes; offset += CHUNK_BYTES) {
        hasher.update(buffer.subarray(offset, Math.min(offset + CHUNK_BYTES, bytes)));
        chunks++;
      }
      const root = hasher.finalize_hex();
      const elapsed = seconds(start);
      hasher.free();
      return { seconds: elapsed, chunks, root };
    },
    dispose() {},
  };
}

function fromFile(wasm: Wasm, requested: number): Subject | { error: string; failed: number[] } {
  const made = withHalving(requested, patternBlob);
  if ("error" in made) {
    return made;
  }
  const { value: blob, bytes } = made;
  return {
    bytes,
    failed: made.failed,
    chunkBytes: null,
    async measure() {
      const hasher = new wasm.Hasher();
      const reader = blob.stream().getReader();
      const start = performance.now();
      let chunks = 0;
      let next = await reader.read();
      while (!next.done) {
        hasher.update(next.value);
        chunks++;
        next = await reader.read();
      }
      const root = hasher.finalize_hex();
      const elapsed = seconds(start);
      hasher.free();
      return { seconds: elapsed, chunks, root };
    },
    dispose() {},
  };
}

async function runCase(wasm: Wasm, id: CaseId, params: Params): Promise<CaseResult> {
  const requested = id === "from_file" ? params.fileBytes : params.memBytes;
  const prepare = { in_memory: inMemory, copied_in: copiedIn, from_file: fromFile }[id];
  const subject = prepare(wasm, requested);
  if ("error" in subject) {
    return {
      case: id,
      requested_bytes: requested,
      bytes: null,
      failed_sizes: subject.failed,
      chunk_bytes: null,
      runs: [],
      stats: null,
      error: `no size from ${requested} down to the 64 MiB floor could be allocated: ${subject.error}`,
    };
  }

  const expected = referenceRoot(wasm, subject.bytes);
  const runs = [];
  for (let i = 0; i < WARMUP_RUNS + params.runs; i++) {
    const warmup = i < WARMUP_RUNS;
    scope.postMessage({
      type: "progress",
      text: `${id}: ${warmup ? "warm-up run" : `run ${i - WARMUP_RUNS + 1} of ${params.runs}`}`,
    });
    const measured = await subject.measure();
    if (measured.root !== expected) {
      throw new Error(`${id} hashed to ${measured.root}, expected ${expected}`);
    }
    runs.push(run(measured.seconds, subject.bytes, measured.chunks, measured.root, warmup));
  }
  subject.dispose();
  return {
    case: id,
    requested_bytes: requested,
    bytes: subject.bytes,
    failed_sizes: subject.failed,
    chunk_bytes: subject.chunkBytes,
    runs,
    stats: summarize(runs),
    error: null,
  };
}

function selfCheck(wasm: Wasm, variant: Variant): KnownRoot[] {
  return OFFICIAL_ROOTS.map(({ bytes, root }) => {
    const resident = new wasm.Pattern(bytes);
    const viaMemory = resident.hash(CHUNK_BYTES);
    resident.free();

    const buffer = new Uint8Array(bytes);
    fillPattern(buffer);
    const hasher = new wasm.Hasher();
    for (let offset = 0; offset < bytes; offset += 1000) {
      hasher.update(buffer.subarray(offset, Math.min(offset + 1000, bytes)));
    }
    const viaCopy = hasher.finalize_hex();
    hasher.free();

    for (const [path, got] of [
      ["in WASM memory", viaMemory],
      ["copied in", viaCopy],
    ]) {
      if (got !== root) {
        throw new Error(
          `${variant} build hashed ${bytes} bytes ${path} to ${got}, and the official root is ${root}`,
        );
      }
    }
    return { bytes, root };
  });
}

async function handle(job: JobRequest): Promise<void> {
  const wasm = await load(job.variant);
  if (job.kind === "self_check") {
    scope.postMessage({ type: "self_check", roots: selfCheck(wasm, job.variant) });
  } else {
    scope.postMessage({ type: "case", result: await runCase(wasm, job.case, job.params) });
  }
}

scope.onmessage = (event) => {
  handle(event.data).catch((error: unknown) => {
    const message = error instanceof Error ? `${error.name}: ${error.message}` : String(error);
    scope.postMessage({ type: "error", message });
  });
};
