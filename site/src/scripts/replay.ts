// Replays a recorded run inside [data-replay]: each [data-step] line appears in order, and a
// command is typed out first. The lines are in the page already, so with no script, or with
// reduced motion, the whole run is simply there. The replay starts once, when the block scrolls
// into view, and the button runs it again.

const reduced = matchMedia("(prefers-reduced-motion: reduce)");

function wait(ms: number) {
  return new Promise((done) => setTimeout(done, ms));
}

for (const block of document.querySelectorAll<HTMLElement>("[data-replay]")) {
  const lines = [...block.querySelectorAll<HTMLElement>("[data-step]")].sort(
    (a, b) => Number(a.dataset.step) - Number(b.dataset.step),
  );
  const button = block.querySelector<HTMLButtonElement>("[data-replay-again]");
  if (
    lines.length === 0 ||
    reduced.matches ||
    !("IntersectionObserver" in window)
  )
    continue;

  const texts = lines.map((line) => line.textContent ?? "");
  let run = 0;

  async function play() {
    const mine = ++run;
    for (const line of lines) line.classList.add("is-waiting");
    for (const [at, line] of lines.entries()) {
      await wait(line.dataset.kind === "cmd" ? 500 : 260);
      if (mine !== run) return;
      line.classList.remove("is-waiting");
      if (line.dataset.kind !== "cmd") continue;
      for (let n = 1; n <= texts[at].length; n++) {
        line.textContent = texts[at].slice(0, n);
        await wait(18);
        if (mine !== run) {
          line.textContent = texts[at];
          return;
        }
      }
    }
  }

  const seen = new IntersectionObserver(
    (entries) => {
      if (!entries.some((entry) => entry.isIntersecting)) return;
      seen.disconnect();
      void play();
    },
    { threshold: 0.4 },
  );
  seen.observe(block);

  if (button) {
    button.hidden = false;
    button.addEventListener("click", () => {
      lines.forEach((line, at) => (line.textContent = texts[at]));
      void play();
    });
  }
}
