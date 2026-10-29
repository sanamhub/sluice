// Decides whether the hero gets its moving scene, and loads it only then. The drawing in the HTML
// is the scene's still frame, so a reader without WebGL, with reduced motion, or with data saving
// on sees the same picture and downloads nothing more.

interface NetworkInformation {
  saveData?: boolean;
}

function webgl2(): boolean {
  try {
    return document.createElement("canvas").getContext("webgl2") !== null;
  } catch {
    return false;
  }
}

function allowed(): boolean {
  if (matchMedia("(prefers-reduced-motion: reduce)").matches) return false;
  const connection = (
    navigator as Navigator & { connection?: NetworkInformation }
  ).connection;
  if (connection?.saveData) return false;
  return webgl2();
}

function whenIdle(run: () => void) {
  if ("requestIdleCallback" in window)
    requestIdleCallback(run, { timeout: 1500 });
  else setTimeout(run, 300);
}

const host = document.querySelector<HTMLElement>("[data-hero-scene]");
if (host && allowed()) {
  const start = () =>
    whenIdle(() => {
      import("./scene")
        .then((scene) => scene.mount(host))
        .catch(() => {
          // The still frame stays; nothing else depends on the scene.
        });
    });
  if (document.readyState === "complete") start();
  else addEventListener("load", start, { once: true });
}
