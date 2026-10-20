import { expect, type Page, test } from "@playwright/test";
import { OFFICIAL_ROOTS } from "../src/data";
import type { CaseId, PageResult } from "../src/protocol";

// The CI numbers are not results. Playwright's Chromium, Firefox and WebKit are test builds on a
// shared runner, and Playwright's WebKit is not Safari. These tests check that the page works.
const SMALL = "autorun=1&mem_mib=64&blob_mib=64&runs=2";
const MIB = 1024 * 1024;
const CASES: CaseId[] = ["in_memory", "copied_in", "memory_blob"];

async function finished(page: Page): Promise<string> {
  await page.waitForFunction(() => ["done", "error"].includes(document.body.dataset.state ?? ""), {
    timeout: 4 * 60_000,
  });
  return (await page.evaluate(() => document.body.dataset.state)) ?? "";
}

test.describe("page (Playwright engines, WebKit here is not Safari)", () => {
  test("both builds finish with matching roots and a result of the right shape", async ({
    page,
  }) => {
    const pageErrors: string[] = [];
    page.on("pageerror", (error) => pageErrors.push(error.message));

    const response = await page.goto(`/?${SMALL}`);
    // ADR 0014: the page needs neither header.
    expect(response?.headers()["cross-origin-opener-policy"]).toBeUndefined();
    expect(response?.headers()["cross-origin-embedder-policy"]).toBeUndefined();

    expect(await finished(page), (await page.textContent("#error")) ?? "").toBe("done");
    expect(pageErrors).toEqual([]);

    const result: PageResult = JSON.parse(await page.inputValue("#json"));
    expect(result.schema).toBe(2);
    expect(result.cross_origin_isolated).toBe(false);
    expect(result.user_agent).toContain("Mozilla");
    expect(result.hardware_concurrency).toBeGreaterThan(0);
    expect(result.warmup_runs).toBe(1);
    expect(result.counted_runs).toBe(2);
    expect(result.chunk_bytes).toBe(MIB);
    expect(result.targets_mbit_per_s).toEqual([500, 1000]);
    for (const variant of ["simd", "scalar"] as const) {
      expect(result.wasm[variant].raw_bytes).toBeGreaterThan(0);
      expect(result.wasm[variant].brotli_bytes).toBeGreaterThan(0);
      expect(result.wasm[variant].target_features.length).toBeGreaterThan(0);
    }
    expect(result.wasm.simd.target_features).toContain("simd128");
    expect(result.wasm.scalar.target_features).not.toContain("simd128");

    const built = result.variants.map((v) => v.variant);
    if (result.simd_supported) {
      expect(built).toEqual(["simd", "scalar"]);
      expect(result.skipped).toEqual([]);
    } else {
      expect(built).toEqual(["scalar"]);
      expect(result.skipped.map((s) => s.variant)).toEqual(["simd"]);
    }

    for (const variant of result.variants) {
      expect(variant.self_check).toEqual(OFFICIAL_ROOTS);
      expect(variant.cases.map((c) => c.case)).toEqual(CASES);
      for (const c of variant.cases) {
        expect(c.error).toBeNull();
        expect(c.bytes).toBe(64 * MIB);
        expect(c.failed_sizes).toEqual([]);
        expect(c.runs.map((r) => r.warmup)).toEqual([true, false, false]);
        expect(new Set(c.runs.map((r) => r.root)).size).toBe(1);
        for (const r of c.runs) {
          expect(r.seconds).toBeGreaterThan(0);
          expect(r.mb_per_s).toBeGreaterThan(0);
          expect(r.gbit_per_s).toBeCloseTo((r.mb_per_s * 8) / 1000, 6);
        }
        const s = c.stats;
        expect(s).not.toBeNull();
        expect(s?.min_mb_per_s).toBeLessThanOrEqual(s?.median_mb_per_s ?? 0);
        expect(s?.median_mb_per_s).toBeLessThanOrEqual(s?.max_mb_per_s ?? 0);
        expect(s?.median_vs_500_mbit).toBeCloseTo((s?.median_gbit_per_s ?? 0) * 2, 6);
        expect(s?.median_vs_1_gbit).toBeCloseTo(s?.median_gbit_per_s ?? 0, 6);
      }
    }

    // The same bytes give the same root in every case and in both builds.
    const roots = new Set(result.variants.flatMap((v) => v.cases.map((c) => c.runs[0]?.root)));
    expect(roots.size).toBe(1);

    // Round 0 is the warm-up. The builds alternate, and simd goes first in the even rounds.
    for (const id of CASES) {
      const inOrder = result.variants
        .flatMap((v) =>
          (v.cases.find((c) => c.case === id)?.runs ?? []).map((r) => ({ ...r, build: v.variant })),
        )
        .sort((a, b) => a.order - b.order);
      expect(inOrder.map((r) => r.order)).toEqual(inOrder.map((_, i) => i + 1));
      const expected = result.simd_supported
        ? [0, 1, 2].flatMap((round) => (round % 2 === 0 ? ["simd", "scalar"] : ["scalar", "simd"]))
        : ["scalar", "scalar", "scalar"];
      expect(inOrder.map((r) => r.build)).toEqual(expected);
      expect(inOrder.map((r) => r.round)).toEqual(
        result.simd_supported ? [0, 0, 1, 1, 2, 2] : [0, 1, 2],
      );
    }

    await expect(page.locator("#table tbody tr")).toHaveCount(result.variants.length * 3);
    await expect(page.locator("#runs tbody tr")).toHaveCount(result.variants.length * 3 * 3);
  });

  test("the page contacts only the server that serves it", async ({ page, baseURL }) => {
    const foreign: string[] = [];
    page.on("request", (request) => {
      const url = request.url();
      if (!url.startsWith(`${baseURL}/`) && !/^(blob|data):/.test(url)) {
        foreign.push(url);
      }
    });
    await page.goto(`/?${SMALL}`);
    expect(await finished(page)).toBe("done");
    expect(foreign).toEqual([]);
  });

  test("a picked file adds a disk case for both builds, with the first read marked", async ({
    page,
  }) => {
    // 4 MiB of the pattern, so its root is the root of the in-memory case at 4 MiB.
    const buffer = Buffer.alloc(4 * MIB);
    for (let i = 0; i < buffer.length; i++) {
      buffer[i] = i % 251;
    }
    await page.goto("/?mem_mib=4&blob_mib=4&runs=2");
    await page.setInputFiles("#disk-file", {
      name: "private-name.bin",
      mimeType: "application/x-private-type",
      buffer,
    });
    await page.click("#run");
    expect(await finished(page), (await page.textContent("#error")) ?? "").toBe("done");

    const text = await page.inputValue("#json");
    // Only the size and the times are recorded.
    expect(text).not.toContain("private-name");
    expect(text).not.toContain("x-private-type");
    expect(await page.textContent("body")).not.toContain("private-name");

    const result: PageResult = JSON.parse(text);
    for (const variant of result.variants) {
      expect(variant.cases.map((c) => c.case)).toEqual([...CASES, "disk_file"]);
    }
    const disk = result.variants.flatMap((v) =>
      (v.cases.find((c) => c.case === "disk_file")?.runs ?? []).map((r) => ({
        ...r,
        build: v.variant,
      })),
    );
    disk.sort((a, b) => a.order - b.order);
    expect(disk.map((r) => r.order)).toEqual(disk.map((_, i) => i + 1));
    expect(disk.filter((r) => r.first_read)).toHaveLength(1);
    expect(disk[0]?.first_read).toBe(true);
    expect(disk[0]?.warmup).toBe(true);

    const memory = result.variants[0]?.cases.find((c) => c.case === "in_memory");
    for (const variant of result.variants) {
      const c = variant.cases.find((x) => x.case === "disk_file");
      expect(c?.bytes).toBe(4 * MIB);
      expect(c?.error).toBeNull();
      expect(c?.stats).not.toBeNull();
    }
    expect(new Set(disk.map((r) => r.root))).toEqual(new Set([memory?.runs[0]?.root]));
    await expect(page.locator("#runs td", { hasText: "first read, from disk" })).toHaveCount(1);
  });

  test("a file under 1 MiB is refused before anything runs", async ({ page }) => {
    await page.goto("/");
    await page.setInputFiles("#disk-file", {
      name: "a.bin",
      mimeType: "x/y",
      buffer: Buffer.alloc(10),
    });
    await page.click("#run");
    await expect(page.locator("#error")).toContainText("1 MiB");
    expect(await page.evaluate(() => document.body.dataset.state)).toBe("idle");
  });

  test("a bad query stops the page with the reason", async ({ page }) => {
    await page.goto("/?mem_mib=0");
    expect(await finished(page)).toBe("error");
    await expect(page.locator("#error")).toContainText("mem_mib");
  });
});
