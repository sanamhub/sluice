import "./style.css";
import sizes from "./generated/sizes.json";
import { readParams } from "./params";
import {
  type CaseId,
  CHUNK_BYTES,
  type JobReply,
  type JobRequest,
  type PageResult,
  type Params,
  TARGETS_MBIT_PER_S,
  type Variant,
  type VariantResult,
  WARMUP_RUNS,
} from "./protocol";
import { copyJson, downloadJson, renderEnvironment, renderTable } from "./render";

const CASES: CaseId[] = ["in_memory", "copied_in", "from_file"];

// A module with one v128 instruction, which `WebAssembly.validate` accepts only where SIMD exists.
const SIMD_PROBE = new Uint8Array([
  0, 97, 115, 109, 1, 0, 0, 0, 1, 5, 1, 96, 0, 1, 123, 3, 2, 1, 0, 10, 10, 1, 8, 0, 65, 0, 253, 15,
  253, 98, 11,
]);

type Finished = Extract<JobReply, { type: "self_check" | "case" }>;

function element<T extends HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (found === null) {
    throw new Error(`index.html has no #${id}`);
  }
  return found as T;
}

function simdSupported(): boolean {
  try {
    return WebAssembly.validate(SIMD_PROBE);
  } catch {
    return false;
  }
}

function runJob(request: JobRequest, onProgress: (text: string) => void): Promise<Finished> {
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
    const fail = (message: string) => {
      worker.terminate();
      reject(new Error(message));
    };
    worker.onmessage = (event: MessageEvent<JobReply>) => {
      const reply = event.data;
      if (reply.type === "progress") {
        onProgress(reply.text);
      } else if (reply.type === "error") {
        fail(reply.message);
      } else {
        worker.terminate();
        resolve(reply);
      }
    };
    worker.onerror = (event) => fail(event.message || "the worker could not start");
    worker.onmessageerror = () => fail("the worker sent a message the page could not read");
    worker.postMessage(request);
  });
}

async function measureVariant(
  variant: Variant,
  params: Params,
  status: (text: string) => void,
): Promise<VariantResult> {
  status(`${variant}: checking the build against the official roots`);
  const check = await runJob({ kind: "self_check", variant }, status);
  if (check.type !== "self_check") {
    throw new Error("the worker answered the self-check with a case result");
  }
  const cases = [];
  for (const id of CASES) {
    const reply = await runJob({ kind: "case", variant, case: id, params }, (text) =>
      status(`${variant}, ${text}`),
    );
    if (reply.type !== "case") {
      throw new Error("the worker answered a case with a self-check result");
    }
    cases.push(reply.result);
  }
  return { variant, self_check: check.roots, cases };
}

/** The two builds must agree on every root they both produced at the same size. */
function crossCheck(variants: VariantResult[]): void {
  for (const id of CASES) {
    const seen = new Map<number, string>();
    for (const v of variants) {
      const c = v.cases.find((x) => x.case === id);
      const root = c?.runs[0]?.root;
      if (c?.bytes == null || root === undefined) {
        continue;
      }
      const other = seen.get(c.bytes);
      if (other !== undefined && other !== root) {
        throw new Error(`${id}: the builds disagree on the root of ${c.bytes} bytes`);
      }
      seen.set(c.bytes, root);
    }
  }
}

async function measure(params: Params, status: (text: string) => void): Promise<PageResult> {
  const startedAt = new Date().toISOString();
  const simd = simdSupported();
  const builds: Variant[] = simd ? ["simd", "scalar"] : ["scalar"];
  const variants: VariantResult[] = [];
  for (const build of builds) {
    variants.push(await measureVariant(build, params, status));
  }
  crossCheck(variants);
  return {
    schema: 1,
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
    variants,
  };
}

function setState(state: "idle" | "running" | "done" | "error"): void {
  document.body.dataset.state = state;
}

function main(): void {
  const runButton = element<HTMLButtonElement>("run");
  const status = element("status");
  const failure = element("error");
  const output = element("output");
  const json = element<HTMLTextAreaElement>("json");
  const params = readParams(location.search);

  element("plan").textContent =
    `Runs: 1 warm-up that is not counted, then ${params.runs} that are. ` +
    `In memory and copied in hash ${params.memBytes / 1048576} MiB, ` +
    `from a file hashes ${params.fileBytes / 1048576} MiB.`;

  async function start(): Promise<void> {
    runButton.disabled = true;
    failure.textContent = "";
    output.hidden = true;
    setState("running");
    try {
      const result = await measure(params, (text) => {
        status.textContent = text;
      });
      const text = `${JSON.stringify(result, null, 2)}\n`;
      json.value = text;
      element("table").replaceChildren(renderTable(result));
      element("environment").textContent = renderEnvironment(result);
      element("warning").hidden = !result.cross_origin_isolated;
      element("download").onclick = () => downloadJson(text, result.started_at);
      element("copy").onclick = async () => {
        element("copied").textContent = (await copyJson(json))
          ? "Copied."
          : "Copy failed. Select the text instead.";
      };
      output.hidden = false;
      status.textContent = "Done.";
      setState("done");
    } catch (error) {
      failure.textContent = error instanceof Error ? error.message : String(error);
      status.textContent = "Stopped.";
      setState("error");
    } finally {
      runButton.disabled = false;
    }
  }

  runButton.onclick = () => void start();
  setState("idle");
  if (params.autorun) {
    void start();
  }
}

try {
  main();
} catch (error) {
  // A bad query string is the only thing that throws before the page is usable.
  element("error").textContent = error instanceof Error ? error.message : String(error);
  setState("error");
}
