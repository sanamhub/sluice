/**
 * One worker per build per case, so the page can interleave the builds run by run. The page
 * terminates a worker when its case is done, which is the only way to give WASM memory back, and
 * that lets a phone that cannot hold one case's buffers twice still run the next case.
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
  CHUNK_BYTES,
  type JobReply,
  type JobRequest,
  type KnownRoot,
  type Variant,
  type WorkerScope,
} from "./protocol";

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

/** What a case worker holds between requests. */
interface Open {
  id: CaseId;
  wasm: Wasm;
  subject: Subject;
  /** The root every run must give, computed apart from the timing. */
  expected: string;
}

let open: Open | null = null;

function prepare(wasm: Wasm, id: CaseId, requested: number) {
  return { in_memory: inMemory, copied_in: copiedIn, from_file: fromFile }[id](wasm, requested);
}

function prepared(subject: Subject | { error: string; failed: number[] }): JobReply {
  if ("error" in subject) {
    return {
      type: "prepared",
      bytes: null,
      failed_sizes: subject.failed,
      chunk_bytes: null,
      error: subject.error,
    };
  }
  return {
    type: "prepared",
    bytes: subject.bytes,
    failed_sizes: subject.failed,
    chunk_bytes: subject.chunkBytes,
    error: null,
  };
}

/** Allocates the case's data and works out the root its runs must give. */
function openCase(wasm: Wasm, id: CaseId, requested: number): JobReply {
  open?.subject.dispose();
  open = null;
  const subject = prepare(wasm, id, requested);
  if (!("error" in subject)) {
    open = { id, wasm, subject, expected: referenceRoot(wasm, subject.bytes) };
  }
  return prepared(subject);
}

async function runOnce(): Promise<JobReply> {
  if (open === null) {
    throw new Error("run asked of a worker that holds no data");
  }
  const timing = await open.subject.measure();
  if (timing.root !== open.expected) {
    throw new Error(`${open.id} hashed to ${timing.root}, expected ${open.expected}`);
  }
  return { type: "ran", ...timing };
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

async function handle(job: JobRequest): Promise<JobReply> {
  switch (job.kind) {
    case "self_check":
      return { type: "checked", roots: selfCheck(await load(job.variant), job.variant) };
    case "open":
      return openCase(await load(job.variant), job.case, job.requested);
    case "resize": {
      if (open === null) {
        throw new Error("resize asked of a worker that holds no data");
      }
      return openCase(open.wasm, open.id, job.bytes);
    }
    case "run":
      return runOnce();
  }
}

scope.onmessage = (event) => {
  handle(event.data).then(
    (reply) => scope.postMessage(reply),
    (error: unknown) => {
      const message = error instanceof Error ? `${error.name}: ${error.message}` : String(error);
      scope.postMessage({ type: "error", message });
    },
  );
};
