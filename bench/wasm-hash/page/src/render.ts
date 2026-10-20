import type { CaseId, PageResult, Stats } from "./protocol";

const CASE_LABELS: Record<CaseId, string> = {
  in_memory: "In memory",
  copied_in: "Copied in",
  memory_blob: "From a memory-backed Blob",
  disk_file: "From a file on disk",
};

const MIB_LABEL = (bytes: number) => `${Number((bytes / 1048576).toFixed(1))} MiB`;

function cell(row: HTMLTableRowElement, text: string, header = false): void {
  const element = document.createElement(header ? "th" : "td");
  element.textContent = text;
  row.append(element);
}

function range(low: number, high: number, digits: number): string {
  return `${low.toFixed(digits)} to ${high.toFixed(digits)}`;
}

function statsCells(row: HTMLTableRowElement, s: Stats): void {
  cell(row, s.median_mb_per_s.toFixed(0));
  cell(row, range(s.min_mb_per_s, s.max_mb_per_s, 0));
  cell(row, s.median_gbit_per_s.toFixed(2));
  cell(row, range(s.min_gbit_per_s, s.max_gbit_per_s, 2));
  cell(row, `${s.median_vs_500_mbit.toFixed(2)}x`);
  cell(row, `${s.median_vs_1_gbit.toFixed(2)}x`);
}

export function renderTable(result: PageResult): HTMLTableElement {
  const table = document.createElement("table");
  const head = table.createTHead().insertRow();
  for (const title of [
    "Build",
    "Case",
    "Size",
    "Median MB/s",
    "Range MB/s",
    "Median Gbit/s",
    "Range Gbit/s",
    "vs 500 Mbit/s",
    "vs 1 Gbit/s",
  ]) {
    cell(head, title, true);
  }
  const body = table.createTBody();
  for (const variant of result.variants) {
    for (const c of variant.cases) {
      const row = body.insertRow();
      cell(row, variant.variant);
      cell(row, CASE_LABELS[c.case]);
      const retried = c.failed_sizes.length > 0 ? " (retried smaller)" : "";
      cell(row, c.bytes === null ? "none" : `${MIB_LABEL(c.bytes)}${retried}`);
      if (c.stats === null) {
        const note = row.insertCell();
        note.colSpan = 6;
        note.textContent = c.error ?? "no runs";
      } else {
        statsCells(row, c.stats);
      }
    }
  }
  return table;
}

/** Only the disk case has a first read to name. Its other reads may be served by the OS cache. */
function runNote(id: CaseId, firstRead: boolean): string {
  if (id !== "disk_file") {
    return "";
  }
  return firstRead ? "first read, from disk" : "may come from the OS cache";
}

/** Every run of every case, in the order it happened, so thermal drift can be read off. */
export function renderRuns(result: PageResult): HTMLTableElement {
  const table = document.createElement("table");
  const head = table.createTHead().insertRow();
  for (const title of ["Case", "Order", "Round", "Build", "MB/s", "Gbit/s", "Note"]) {
    cell(head, title, true);
  }
  const body = table.createTBody();
  const caseIds = result.variants[0]?.cases.map((c) => c.case) ?? [];
  for (const id of caseIds) {
    const runs = result.variants.flatMap((v) =>
      (v.cases.find((c) => c.case === id)?.runs ?? []).map((r) => ({ build: v.variant, ...r })),
    );
    for (const r of runs.sort((a, b) => a.order - b.order)) {
      const row = body.insertRow();
      cell(row, CASE_LABELS[id]);
      cell(row, String(r.order));
      cell(row, r.warmup ? "warm-up" : String(r.round));
      cell(row, r.build);
      cell(row, r.mb_per_s.toFixed(0));
      cell(row, r.gbit_per_s.toFixed(2));
      cell(row, runNote(id, r.first_read === true));
    }
  }
  return table;
}

export function renderEnvironment(result: PageResult): string {
  const lines = [
    `crossOriginIsolated: ${result.cross_origin_isolated}`,
    `WebAssembly SIMD: ${result.simd_supported ? "supported" : "not supported"}`,
    `hardwareConcurrency: ${result.hardware_concurrency}`,
    ...result.skipped.map((s) => `Skipped ${s.variant}: ${s.reason}`),
    result.user_agent,
  ];
  return lines.join("\n");
}

function copyWithSelection(area: HTMLTextAreaElement): boolean {
  area.focus();
  area.select();
  return document.execCommand("copy");
}

/**
 * `navigator.clipboard` exists only on secure origins, and the LAN preview a phone opens is plain
 * HTTP. The fallback selects the text and copies it the old way.
 */
export async function copyJson(area: HTMLTextAreaElement): Promise<boolean> {
  if (window.isSecureContext && navigator.clipboard) {
    try {
      await navigator.clipboard.writeText(area.value);
      return true;
    } catch {
      // Permission denied. The selection fallback below can still work.
    }
  }
  return copyWithSelection(area);
}

export function downloadJson(text: string, startedAt: string): void {
  const link = document.createElement("a");
  const url = URL.createObjectURL(new Blob([text], { type: "application/json" }));
  link.href = url;
  link.download = `sluice-wasm-hash-${startedAt.replaceAll(":", "-")}.json`;
  link.click();
  // Safari can start the download after click() returns, and a revoked URL would cancel it.
  setTimeout(() => URL.revokeObjectURL(url), 30_000);
}
