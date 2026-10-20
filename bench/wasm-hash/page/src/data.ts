/** The known roots, the test-vector input and the Blob the from-file case reads. */

import { MIB } from "./protocol";

/** Byte `i` of the BLAKE3 test-vector input is `i % 251`. */
export const PERIOD = 251;

/**
 * Whole periods, so repeating the block repeats the input. It is a little over 1 MiB for that
 * reason.
 */
export const BLOCK_BYTES = PERIOD * 4178;

/** `hash` of the input at each length in test_vectors/test_vectors.json of BLAKE3-team/BLAKE3. */
export const OFFICIAL_ROOTS = [
  { bytes: 1025, root: "d00278ae47eb27b34faecf67b4fe263f82d5412916c1ffd97c8cb7fb814b8444" },
  { bytes: 102_400, root: "bc3e3d41a1146b069abffad3c0d44860cf664390afce4d9661f7902e7943e085" },
] as const;

export function fillPattern(buf: Uint8Array): void {
  const seed = Math.min(buf.length, PERIOD * 4096);
  for (let i = 0; i < seed; i++) {
    buf[i] = i % PERIOD;
  }
  // The seed is a whole number of periods, so each doubling stays in phase.
  for (let filled = seed; filled < buf.length; filled *= 2) {
    buf.copyWithin(filled, 0, Math.min(filled, buf.length - filled));
  }
}

export function patternBlock(): Uint8Array {
  const block = new Uint8Array(BLOCK_BYTES);
  fillPattern(block);
  return block;
}

/** A Blob of `bytes` bytes of the input, as a dropped file would be. */
export function patternBlob(bytes: number): Blob {
  const block = patternBlock();
  const parts: Uint8Array[] = [];
  let left = bytes;
  for (; left >= BLOCK_BYTES; left -= BLOCK_BYTES) {
    parts.push(block);
  }
  if (left > 0) {
    parts.push(block.slice(0, left));
  }
  return new Blob(parts as BlobPart[]);
}

/**
 * Runs `make` at `requested` bytes, then at half, down to 64 MiB, until an allocation works. A phone
 * cannot always give a desktop's 1 GiB, and the size that worked is part of the result.
 */
export function withHalving<T>(
  requested: number,
  make: (bytes: number) => T,
): { value: T; bytes: number; failed: number[] } | { error: string; failed: number[] } {
  const floor = Math.min(64 * MIB, requested);
  const failed: number[] = [];
  for (let bytes = requested; ; bytes = Math.max(floor, Math.floor(bytes / 2))) {
    try {
      return { value: make(bytes), bytes, failed };
    } catch (error) {
      failed.push(bytes);
      if (bytes <= floor) {
        return { error: `${bytes} bytes: ${String(error)}`, failed };
      }
    }
  }
}
