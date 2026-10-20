import { type Run, type Stats, TARGETS_MBIT_PER_S } from "./protocol";

export function median(values: number[]): number {
  const sorted = [...values].sort((a, b) => a - b);
  const mid = sorted.length >> 1;
  const upper = sorted[mid] ?? Number.NaN;
  return sorted.length % 2 === 1 ? upper : ((sorted[mid - 1] ?? Number.NaN) + upper) / 2;
}

export function run(
  seconds: number,
  bytes: number,
  chunks: number,
  root: string,
  warmup: boolean,
): Run {
  return {
    warmup,
    seconds,
    mb_per_s: bytes / 1e6 / seconds,
    gbit_per_s: (bytes * 8) / 1e9 / seconds,
    chunks,
    root,
  };
}

/** Statistics over the counted runs. The warm-up is left out. */
export function summarize(runs: Run[]): Stats | null {
  const counted = runs.filter((r) => !r.warmup);
  if (counted.length === 0) {
    return null;
  }
  const mb = counted.map((r) => r.mb_per_s);
  const gbit = counted.map((r) => r.gbit_per_s);
  const [target500, target1000] = TARGETS_MBIT_PER_S;
  const medianGbit = median(gbit);
  return {
    median_mb_per_s: median(mb),
    min_mb_per_s: Math.min(...mb),
    max_mb_per_s: Math.max(...mb),
    median_gbit_per_s: medianGbit,
    min_gbit_per_s: Math.min(...gbit),
    max_gbit_per_s: Math.max(...gbit),
    median_vs_500_mbit: (medianGbit * 1000) / target500,
    median_vs_1_gbit: (medianGbit * 1000) / target1000,
  };
}
