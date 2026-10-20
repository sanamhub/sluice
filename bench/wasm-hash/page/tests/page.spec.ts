import { expect, type Page, test } from "@playwright/test";
import { OFFICIAL_ROOTS } from "../src/data";
import type { CaseId, PageResult } from "../src/protocol";

// The CI numbers are not results. Playwright's Chromium, Firefox and WebKit are test builds on a
// shared runner, and Playwright's WebKit is not Safari. These tests check that the page works.
const SMALL = "autorun=1&mem_mib=64&file_mib=64&runs=2";
const MIB = 1024 * 1024;
const CASES: CaseId[] = ["in_memory", "copied_in", "from_file"];

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
    expect(result.schema).toBe(1);
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
    }

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

    await expect(page.locator("#table tbody tr")).toHaveCount(result.variants.length * 3);
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

  test("a bad query stops the page with the reason", async ({ page }) => {
    await page.goto("/?mem_mib=0");
    expect(await finished(page)).toBe("error");
    await expect(page.locator("#error")).toContainText("mem_mib");
  });
});
