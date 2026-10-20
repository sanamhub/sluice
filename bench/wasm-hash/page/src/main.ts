import "./style.css";
import { measure } from "./measure";
import { readParams } from "./params";
import { copyJson, downloadJson, renderEnvironment, renderRuns, renderTable } from "./render";

function element<T extends HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (found === null) {
    throw new Error(`index.html has no #${id}`);
  }
  return found as T;
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
    `The two builds alternate run by run, and swap who goes first every round. ` +
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
      element("runs").replaceChildren(renderRuns(result));
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
