/** The messages between the page and the worker, and the shape of the result object. */

export type Variant = "simd" | "scalar";
export type CaseId = "in_memory" | "copied_in" | "from_file";

export const MIB = 1024 * 1024;
export const CHUNK_BYTES = MIB;
export const WARMUP_RUNS = 1;
export const TARGETS_MBIT_PER_S = [500, 1000] as const;

export interface Params {
  /** Bytes hashed by the in-memory and copied-in cases. */
  memBytes: number;
  /** Bytes in the Blob of the from-file case. */
  fileBytes: number;
  /** Runs that count, after the warm-up. */
  runs: number;
  autorun: boolean;
}

export interface WasmSize {
  raw_bytes: number;
  brotli_bytes: number;
}

export interface Stats {
  median_mb_per_s: number;
  min_mb_per_s: number;
  max_mb_per_s: number;
  median_gbit_per_s: number;
  min_gbit_per_s: number;
  max_gbit_per_s: number;
  /** The median as a multiple of 500 Mbit/s, the browser target in docs/performance.md. */
  median_vs_500_mbit: number;
  median_vs_1_gbit: number;
}

export interface Run {
  /** The first run of a build in a case. It is in the JSON and not in the statistics. */
  warmup: boolean;
  /** 0 for the warm-up round, then 1 to `counted_runs`. Both builds run once per round. */
  round: number;
  /** Position among all runs of the case, both builds together, from 1. Shows who ran when. */
  order: number;
  seconds: number;
  mb_per_s: number;
  gbit_per_s: number;
  /** Calls into the hasher: the 1 MiB chunks, or the chunks the Blob stream delivered. */
  chunks: number;
  root: string;
}

export interface CaseResult {
  case: CaseId;
  requested_bytes: number;
  /** The size actually hashed, after any retry at half the size. Null if every size failed. */
  bytes: number | null;
  /** Sizes whose allocation failed, largest first. */
  failed_sizes: number[];
  /** Null for the from-file case, where the stream decides. */
  chunk_bytes: number | null;
  runs: Run[];
  stats: Stats | null;
  error: string | null;
}

export interface KnownRoot {
  bytes: number;
  root: string;
}

export interface VariantResult {
  variant: Variant;
  self_check: KnownRoot[];
  cases: CaseResult[];
}

export interface PageResult {
  schema: 2;
  started_at: string;
  user_agent: string;
  hardware_concurrency: number;
  cross_origin_isolated: boolean;
  simd_supported: boolean;
  warmup_runs: number;
  counted_runs: number;
  chunk_bytes: number;
  targets_mbit_per_s: readonly number[];
  wasm: Record<Variant, WasmSize>;
  /** A variant the browser cannot run, and why. */
  skipped: { variant: Variant; reason: string }[];
  variants: VariantResult[];
}

/**
 * A self-check worker answers once. A case worker lives for the whole case: it is opened, may be
 * resized, runs once per request, and is terminated by the page.
 */
export type JobRequest =
  | { kind: "self_check"; variant: Variant }
  | { kind: "open"; variant: Variant; case: CaseId; requested: number }
  | { kind: "resize"; bytes: number }
  | { kind: "run" };

export type JobReply =
  | { type: "checked"; roots: KnownRoot[] }
  | {
      type: "prepared";
      /** Null if no size down to the floor could be had, and `error` says why. */
      bytes: number | null;
      failed_sizes: number[];
      chunk_bytes: number | null;
      error: string | null;
    }
  | { type: "ran"; seconds: number; chunks: number; root: string }
  | { type: "error"; message: string };

/** The part of the worker's global scope the worker uses. The DOM and WebWorker libs do not mix. */
export interface WorkerScope {
  postMessage(message: JobReply): void;
  onmessage: ((event: MessageEvent<JobRequest>) => void) | null;
}
