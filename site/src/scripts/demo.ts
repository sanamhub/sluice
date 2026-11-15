// Plays the handoff in version 2's hero. The demo is one element with a data-stage from 0 to 5,
// and the stylesheet decides what each stage shows, so this script only moves the number: on a
// timer while the demo is on screen, or straight to a stage when one of the step buttons is
// pressed. With reduced motion nothing plays by itself and the demo rests on its last stage.

const demo = document.querySelector<HTMLElement>("[data-demo]");

if (demo) {
  const steps = [...demo.querySelectorAll<HTMLButtonElement>("[data-go]")];
  const toggle = demo.querySelector<HTMLButtonElement>("[data-demo-toggle]");
  const calm = matchMedia("(prefers-reduced-motion: reduce)");
  // How long each stage holds before the next one.
  const HOLD = [1500, 1100, 2200, 2400, 2600, 3200];
  const LAST = HOLD.length - 1;
  let stage = Number(demo.dataset.stage);
  let timer = 0;
  let playing = !calm.matches;
  let onScreen = false;

  function show(next: number) {
    stage = next;
    demo!.dataset.stage = String(stage);
    for (const step of steps) {
      const from = Number(step.dataset.go);
      const until = Number(step.dataset.until);
      step.setAttribute(
        "aria-pressed",
        String(stage >= from && stage <= until),
      );
    }
  }

  function schedule() {
    clearTimeout(timer);
    if (!playing || !onScreen) return;
    timer = window.setTimeout(() => {
      show(stage === LAST ? 0 : stage + 1);
      schedule();
    }, HOLD[stage]);
  }

  function label() {
    if (!toggle) return;
    toggle.textContent = playing ? "Pause" : "Play";
  }

  for (const step of steps) {
    step.addEventListener("click", () => {
      playing = false;
      label();
      show(Number(step.dataset.go));
      schedule();
    });
  }

  if (toggle) {
    toggle.hidden = false;
    toggle.addEventListener("click", () => {
      playing = !playing;
      label();
      if (playing && stage === LAST) show(0);
      schedule();
    });
  }

  new IntersectionObserver(
    ([entry]) => {
      onScreen = entry.isIntersecting;
      schedule();
    },
    { threshold: 0.35 },
  ).observe(demo);

  show(playing ? 0 : LAST);
  label();
}

export {};
