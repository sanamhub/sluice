// The hero of version 1: blocks stream across a canvas, loose and wavering until they reach the
// gate, then brass and in line once they are through it. It is a drawing of what a transfer does,
// not a measurement. The loop runs only while the canvas is on screen and the tab is visible, the
// backing store is capped at twice the CSS size, and with reduced motion one still frame is drawn.

interface Block {
  x: number;
  lane: number;
  size: number;
  speed: number;
  phase: number;
  depth: number;
  flash: number;
}

const canvas = document.querySelector<HTMLCanvasElement>("[data-stream]");
const ctx = canvas?.getContext("2d");

if (canvas && ctx) {
  const calm = matchMedia("(prefers-reduced-motion: reduce)");
  const LANE = 26;
  let width = 0;
  let height = 0;
  let gate = 0;
  let blocks: Block[] = [];
  let water = "#7adce8";
  let brass = "#e0b26a";
  let pointerX = -1e4;
  let pointerY = -1e4;
  let last = 0;
  let frame = 0;
  let onScreen = false;

  /** A custom property as the colour it resolves to in the current theme. */
  function resolved(name: string): string {
    canvas!.style.color = `var(${name})`;
    const value = getComputedStyle(canvas!).color;
    canvas!.style.color = "";
    return value;
  }

  function palette() {
    water = resolved("--water");
    brass = resolved("--brass");
  }

  function layout() {
    const box = canvas!.getBoundingClientRect();
    const ratio = Math.min(devicePixelRatio || 1, 2);
    width = box.width;
    height = box.height;
    canvas!.width = Math.round(width * ratio);
    canvas!.height = Math.round(height * ratio);
    ctx!.setTransform(ratio, 0, 0, ratio, 0, 0);
    gate = width * (width < 700 ? 0.8 : 0.72);
    const lanes = Math.max(1, Math.floor(height / LANE));
    const perLane = Math.max(2, Math.round(width / 210));
    blocks = [];
    for (let lane = 0; lane < lanes; lane++) {
      for (let n = 0; n < perLane; n++) {
        const depth = 0.25 + Math.random() * 0.75;
        blocks.push({
          x: Math.random() * (width + 40) - 20,
          lane,
          size: 4 + depth * 7,
          speed: 90 + depth * 330,
          phase: Math.random() * Math.PI * 2,
          depth,
          flash: 0,
        });
      }
    }
  }

  function draw(now: number, dt: number) {
    ctx!.clearRect(0, 0, width, height);
    const top = (height - Math.floor(height / LANE) * LANE) / 2 + LANE / 2;
    for (const block of blocks) {
      const before = block.x < gate;
      block.x += block.speed * dt;
      if (before && block.x >= gate) block.flash = 1;
      if (block.x > width + 20) block.x = -20;
      block.flash = Math.max(0, block.flash - dt * 3);

      const through = block.x >= gate;
      let y = top + block.lane * LANE;
      // Loose before the gate, calmer the closer it gets, and in line after it.
      if (!through)
        y +=
          Math.sin(block.phase + now * 0.0016 * (0.6 + block.depth)) *
          9 *
          (1 - (block.x / gate) * 0.7);
      const dx = block.x - pointerX;
      const dy = y - pointerY;
      const near = Math.hypot(dx, dy);
      if (near < 110) y += (dy < 0 ? -1 : 1) * (1 - near / 110) * 22;

      ctx!.fillStyle = through ? brass : water;
      const tail = block.speed * 0.14;
      ctx!.globalAlpha = block.depth * 0.22;
      ctx!.fillRect(
        block.x - tail,
        y - block.size * 0.14,
        tail,
        block.size * 0.28,
      );
      ctx!.globalAlpha = 0.25 + block.depth * 0.75;
      ctx!.beginPath();
      ctx!.roundRect(
        block.x,
        y - block.size / 2,
        block.size,
        block.size,
        block.size * 0.25,
      );
      ctx!.fill();
      if (block.flash > 0) {
        ctx!.globalAlpha = block.flash * 0.8;
        ctx!.fillStyle = brass;
        ctx!.fillRect(gate - 1.5, y - 12, 3, 24);
      }
    }
    ctx!.globalAlpha = 1;
  }

  function tick(now: number) {
    const dt = Math.min(0.05, (now - last) / 1000);
    last = now;
    draw(now, dt);
    frame = requestAnimationFrame(tick);
  }

  function start() {
    if (frame || calm.matches || !onScreen || document.hidden) return;
    last = performance.now();
    frame = requestAnimationFrame(tick);
  }

  function stop() {
    cancelAnimationFrame(frame);
    frame = 0;
  }

  function reset() {
    palette();
    layout();
    if (calm.matches) draw(0, 0);
  }

  reset();
  new ResizeObserver(reset).observe(canvas);
  new IntersectionObserver(([entry]) => {
    onScreen = entry.isIntersecting;
    if (onScreen) start();
    else stop();
  }).observe(canvas);
  document.addEventListener("visibilitychange", () =>
    document.hidden ? stop() : start(),
  );
  calm.addEventListener("change", () => (calm.matches ? stop() : start()));
  new MutationObserver(reset).observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["data-theme"],
  });
  matchMedia("(prefers-color-scheme: light)").addEventListener("change", reset);

  const host = canvas.parentElement ?? canvas;
  host.addEventListener(
    "pointermove",
    (event) => {
      const box = canvas.getBoundingClientRect();
      pointerX = event.clientX - box.left;
      pointerY = event.clientY - box.top;
    },
    { passive: true },
  );
  host.addEventListener("pointerleave", () => {
    pointerX = -1e4;
    pointerY = -1e4;
  });
}

export {};
