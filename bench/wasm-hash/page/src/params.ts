import { MIB, type Params } from "./protocol";

const MAX_MEM_MIB = 4095;

function mib(query: URLSearchParams, name: string, fallback: number, max: number): number {
  const raw = query.get(name);
  if (raw === null) {
    return fallback;
  }
  const value = Number(raw);
  if (!Number.isInteger(value) || value < 1 || value > max) {
    throw new Error(`?${name}= must be a whole number from 1 to ${max}, got "${raw}"`);
  }
  return value;
}

/**
 * The defaults are the sizes ADR 0014's measurement calls for. The query exists so the tests can run
 * the page small, and so a device that cannot hold 1 GiB can start lower.
 *
 * `mem_mib` is capped at 4095 because the WASM side takes the length as a `u32`.
 */
export function readParams(search: string): Params {
  const query = new URLSearchParams(search);
  return {
    memBytes: mib(query, "mem_mib", 1024, MAX_MEM_MIB) * MIB,
    blobBytes: mib(query, "blob_mib", 256, MAX_MEM_MIB) * MIB,
    runs: mib(query, "runs", 5, 100),
    autorun: query.get("autorun") === "1",
  };
}
