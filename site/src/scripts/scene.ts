// The hero's scene, mounted and animated. `build` makes the machines, the channel and the gate; this
// file owns the renderer, the camera, the pointer and the loop that moves the water.
import {
  ACESFilmicToneMapping,
  PerspectiveCamera,
  SRGBColorSpace,
  Vector3,
  WebGLRenderer,
} from "three";
import { isLight, readPalette } from "./palette";
import { build } from "./scene/build";

export function mount(host: HTMLElement) {
  const canvas = document.createElement("canvas");
  canvas.className = "hero-canvas";
  canvas.setAttribute("aria-hidden", "true");
  const renderer = new WebGLRenderer({
    canvas,
    antialias: true,
    alpha: true,
    powerPreference: "low-power",
  });
  renderer.setPixelRatio(Math.min(devicePixelRatio, 1.75));
  renderer.outputColorSpace = SRGBColorSpace;
  renderer.toneMapping = ACESFilmicToneMapping;
  renderer.toneMappingExposure = 1.05;
  host.append(canvas);

  const built = build(host);
  const camera = new PerspectiveCamera(26, 1, 0.1, 60);
  const target = new Vector3(0, 0.55, 0);
  const pointer = { x: 0, y: 0, ex: 0, ey: 0 };
  let distance = 13;

  const resize = () => {
    const { width, height } = host.getBoundingClientRect();
    if (width === 0 || height === 0) return;
    renderer.setSize(width, height, false);
    camera.aspect = width / height;
    // Keep both machines in view on a narrow screen by stepping back, not by shrinking the fov.
    distance = 11.6 * Math.min(1.9, Math.max(1, 1.62 / camera.aspect));
    camera.updateProjectionMatrix();
  };
  new ResizeObserver(resize).observe(host);
  resize();

  addEventListener(
    "pointermove",
    (event) => {
      pointer.x = event.clientX / innerWidth - 0.5;
      pointer.y = event.clientY / innerHeight - 0.5;
    },
    { passive: true },
  );

  const recolour = () => built.recolour(readPalette(host), isLight());
  new MutationObserver(recolour).observe(document.documentElement, {
    attributeFilter: ["data-theme"],
  });
  matchMedia("(prefers-color-scheme: light)").addEventListener(
    "change",
    recolour,
  );

  let visible = true;
  let running = false;
  let frame = 0;
  let clock = 0;
  let last = 0;
  const loop = (now: number) => {
    if (!visible || document.hidden) {
      running = false;
      return;
    }
    clock += Math.min(0.05, (now - last) / 1000 || 0);
    last = now;
    pointer.ex += (pointer.x - pointer.ex) * 0.04;
    pointer.ey += (pointer.y - pointer.ey) * 0.04;
    // A near-frontal view from above, turned a little toward the receiver, so both machines stand
    // the same size either side of the gate and the channel's depth still reads. The pointer and a
    // slow sway turn it a little.
    const azimuth = 0.2 + Math.sin(clock * 0.12) * 0.04 + pointer.ex * 0.1;
    const elevation = 0.5 - pointer.ey * 0.06;
    camera.position.set(
      Math.sin(azimuth) * Math.cos(elevation) * distance,
      Math.sin(elevation) * distance,
      Math.cos(azimuth) * Math.cos(elevation) * distance,
    );
    camera.lookAt(target);
    built.tick(clock);
    renderer.render(built.scene, camera);
    if (frame++ === 1) host.dataset.live = "true";
    requestAnimationFrame(loop);
  };
  const run = () => {
    if (running || !visible || document.hidden) return;
    running = true;
    last = performance.now();
    requestAnimationFrame(loop);
  };
  new IntersectionObserver(([entry]) => {
    visible = entry.isIntersecting;
    run();
  }).observe(host);
  document.addEventListener("visibilitychange", run);

  const motion = matchMedia("(prefers-reduced-motion: reduce)");
  motion.addEventListener("change", () => {
    if (!motion.matches) return;
    visible = false;
    delete host.dataset.live;
    renderer.dispose();
    canvas.remove();
  });
  canvas.addEventListener("webglcontextlost", () => {
    visible = false;
    delete host.dataset.live;
  });
  run();
}
