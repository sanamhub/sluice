// Sets the stage of the "How a file crosses" drawing. Beside the steps, on a wide screen, the stage
// is the step in the middle of the window. Above them, on a narrow one, the drawing plays its
// stages in turn while it is on screen. With reduced motion it stays on the last stage, a still.

const figure = document.querySelector<HTMLElement>("[data-channel]");
const steps = [...document.querySelectorAll<HTMLElement>(".how-step")];
const wide = matchMedia("(min-width: 56rem)");
const still = matchMedia("(prefers-reduced-motion: reduce)");
const STAGE_MS = 4200;

if (figure && steps.length === 3) {
  const show = (stage: number) => {
    if (figure.dataset.stage !== String(stage))
      figure.dataset.stage = String(stage);
  };

  // Wide: follow the reader.
  const reading = new IntersectionObserver(
    (entries) => {
      if (!wide.matches) return;
      for (const entry of entries) {
        if (entry.isIntersecting)
          show(steps.indexOf(entry.target as HTMLElement) + 1);
      }
    },
    { rootMargin: "-45% 0px -45% 0px" },
  );
  steps.forEach((step) => reading.observe(step));

  // Narrow: play the stages in turn while the drawing is on screen.
  let timer = 0;
  const play = () => {
    clearInterval(timer);
    if (wide.matches || still.matches) return;
    show(1);
    timer = window.setInterval(
      () => show((Number(figure.dataset.stage) % 3) + 1),
      STAGE_MS,
    );
  };
  new IntersectionObserver(([entry]) => {
    if (entry.isIntersecting) play();
    else clearInterval(timer);
  }).observe(figure);
  wide.addEventListener("change", play);
}
