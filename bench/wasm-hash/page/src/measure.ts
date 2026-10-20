import { Channel } from "./channel";
import sizes from "./generated/sizes.json";
import {
  type CaseId,
  type CaseResult,
  CHUNK_BYTES,
  type JobReply,
  type KnownRoot,
  type PageResult,
  type Params,
  type Run,
  TARGETS_MBIT_PER_S,
  type Variant,
  type VariantResult,
  WARMUP_RUNS,
} from "./protocol";
import { run, summarize } from "./stats";

export const CASES: CaseId[] = ["in_memory", "copied_in", "memory_blob"];

// A module with one v128 instruction, which `WebAssembly.validate` accepts only where SIMD exists.
const SIMD_PROBE = new Uint8Array([
  0, 97, 115, 109, 1, 0, 0, 0, 1, 5, 1, 96, 0, 1, 123, 3, 2, 1, 0, 10, 10, 1, 8, 0, 65, 0, 253, 15,
  253, 98, 11,
]);

type Prepared = Extract<JobReply, { type: "prepared" }>;
type Status = (text: string) => void;

interface Session {
  variant: Variant;
  channel: Channel;
  prepared: Prepared;
  runs: Run[];
}

function simdSupported(): boolean {
  try {
    return WebAssembly.validate(SIMD_PROBE);
  } catch {
    return false;
  }
}

async function selfCheck(variant: Variant, status: Status): Promise<KnownRoot[]> {
  status(`${variant}: checking the build against the official roots`);
  const channel = new Channel();
  try {
    return (await channel.ask({ kind: "self_check", variant }, "checked")).roots;
  } finally {
    channel.close();
  }
}

/**
 * Opens both builds on the case, and brings them to the same size. Each worker retries on its own
 * when an allocation fails, so they can land on different sizes, and a rate compared across sizes
 * would compare the sizes.
 */
async function openSessions(
  id: CaseId,
  requested: number,
  file: File | null,
  variants: Variant[],
): Promise<Session[]> {
  const sessions: Session[] = variants.map((variant) => ({
    variant,
    channel: new Channel(),
    prepared: { type: "prepared", bytes: null, failed_sizes: [], chunk_bytes: null, error: null },
    runs: [],
  }));
  await Promise.all(
    sessions.map(async (s) => {
      s.prepared = await s.channel.ask(
        { kind: "open", variant: s.variant, case: id, requested, file },
        "prepared",
      );
    }),
  );
  for (;;) {
    const landed = sessions.map((s) => s.prepared.bytes);
    if (landed.some((bytes) => bytes === null)) {
      return sessions;
    }
    const smallest = Math.min(...landed.map((bytes) => bytes ?? 0));
    if (landed.every((bytes) => bytes === smallest)) {
      return sessions;
    }
    await Promise.all(
      sessions.map(async (s) => {
        s.prepared = await s.channel.ask({ kind: "resize", bytes: smallest }, "prepared");
      }),
    );
  }
}

function failedCase(id: CaseId, requested: number, s: Session): CaseResult {
  return {
    case: id,
    requested_bytes: requested,
    bytes: null,
    failed_sizes: s.prepared.failed_sizes,
    chunk_bytes: null,
    runs: [],
    stats: null,
    error: `no size from ${requested} bytes down to the 64 MiB floor could be allocated (${s.prepared.error})`,
  };
}

/**
 * One run of each build per round, swapping who goes first every round. Round 0 is the warm-up.
 * Thermal drift and a boost clock that fades both favour whoever runs first, so alternating
 * spreads that over the builds. An odd count of counted rounds cannot split evenly.
 */
async function runRounds(
  id: CaseId,
  sessions: Session[],
  rounds: number,
  status: Status,
): Promise<void> {
  let order = 0;
  for (let round = 0; round <= rounds; round++) {
    const turn = round % 2 === 0 ? sessions : [...sessions].reverse();
    for (const s of turn) {
      order++;
      status(`${id}: ${round === 0 ? "warm-up" : `round ${round} of ${rounds}`}, ${s.variant}`);
      const timing = await s.channel.ask({ kind: "run" }, "ran");
      const finished = run(timing, s.prepared.bytes ?? 0, round, order);
      // Whoever runs first reads the file cold. Everything after may come from the OS cache.
      if (id === "disk_file" && order === 1) {
        finished.first_read = true;
      }
      s.runs.push(finished);
    }
  }
}

async function runCase(
  id: CaseId,
  variants: Variant[],
  params: Params,
  file: File | null,
  status: Status,
): Promise<Map<Variant, CaseResult>> {
  const requested = {
    in_memory: params.memBytes,
    copied_in: params.memBytes,
    memory_blob: params.blobBytes,
    disk_file: file?.size ?? 0,
  }[id];
  const results = new Map<Variant, CaseResult>();
  let sessions: Session[] = [];
  try {
    sessions = await openSessions(id, requested, file, variants);
    const failed = sessions.find((s) => s.prepared.bytes === null);
    if (failed !== undefined) {
      for (const s of sessions) {
        results.set(s.variant, failedCase(id, requested, failed));
      }
      return results;
    }
    await runRounds(id, sessions, params.runs, status);
    const roots = new Set(sessions.flatMap((s) => s.runs.map((r) => r.root)));
    if (roots.size !== 1) {
      throw new Error(`${id}: the runs gave ${roots.size} different roots for the same bytes`);
    }
    for (const s of sessions) {
      results.set(s.variant, {
        case: id,
        requested_bytes: requested,
        bytes: s.prepared.bytes,
        failed_sizes: s.prepared.failed_sizes,
        chunk_bytes: s.prepared.chunk_bytes,
        runs: s.runs,
        stats: summarize(s.runs),
        error: null,
      });
    }
    return results;
  } finally {
    for (const s of sessions) {
      s.channel.close();
    }
  }
}

/** `file` adds the disk case as a fourth. Its name and type are never read. */
export async function measure(
  params: Params,
  file: File | null,
  status: Status,
): Promise<PageResult> {
  const startedAt = new Date().toISOString();
  const simd = simdSupported();
  const variants: Variant[] = simd ? ["simd", "scalar"] : ["scalar"];
  const results: VariantResult[] = [];
  for (const variant of variants) {
    results.push({ variant, self_check: await selfCheck(variant, status), cases: [] });
  }
  for (const id of file === null ? CASES : [...CASES, "disk_file" as const]) {
    const byVariant = await runCase(id, variants, params, file, status);
    for (const result of results) {
      const found = byVariant.get(result.variant);
      if (found !== undefined) {
        result.cases.push(found);
      }
    }
  }
  return {
    schema: 2,
    started_at: startedAt,
    user_agent: navigator.userAgent,
    hardware_concurrency: navigator.hardwareConcurrency,
    cross_origin_isolated: self.crossOriginIsolated,
    simd_supported: simd,
    warmup_runs: WARMUP_RUNS,
    counted_runs: params.runs,
    chunk_bytes: CHUNK_BYTES,
    targets_mbit_per_s: TARGETS_MBIT_PER_S,
    wasm: sizes,
    skipped: simd
      ? []
      : [{ variant: "simd", reason: "WebAssembly.validate rejects SIMD instructions here" }],
    variants: results,
  };
}
