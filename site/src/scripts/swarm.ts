// The hero of version 5. A file is drawn out of points of light: a page with a folded corner and
// lines of text. The points leave it in a stream, squeeze through a brass ring, turn brass, and draw
// the same page again on the other side. Every point's whole path is worked out on the graphics
// card from one number, the place in the loop, so the page's script only advances a clock. The
// points move away from the pointer, and scatter as the hero scrolls out of view.
import {
  AdditiveBlending,
  BufferAttribute,
  BufferGeometry,
  Color,
  Mesh,
  MeshBasicMaterial,
  NormalBlending,
  PerspectiveCamera,
  Points,
  Scene,
  ShaderMaterial,
  TorusGeometry,
  Vector2,
  Vector3,
  WebGLRenderer,
} from "three";
import { isLight, readPalette } from "./palette";

const LOOP = 11; // seconds for one whole transfer
const PAGE_W = 2.3;
const PAGE_H = 3;
const FOLD = 0.62;

const vertex = /* glsl */ `
uniform float uLoop;
uniform float uTime;
uniform float uScatter;
uniform float uSize;
uniform vec3 uFrom;
uniform vec3 uTo;
uniform vec2 uPointer;
attribute vec3 aSeed;
attribute float aOrder;
varying float vTurn;
varying float vAlpha;

void main() {
    // Each point leaves in its own turn, the far edge of the page first.
    float start = 0.1 + aOrder * 0.42;
    float l = clamp((uLoop - start) / 0.34, 0.0, 1.0);

    vec3 from = position + uFrom;
    vec3 to = position + uTo;
    vec3 ring = vec3(0.0, (aSeed.x - 0.5) * 0.5, (aSeed.y - 0.5) * 0.5);
    ring = mix(ring.xyz, ring.yxz, step(0.5, abs(normalize(uTo - uFrom).y)));
    ring += (uFrom + uTo) * 0.5;
    // Into the ring, then out of it: every point passes through the same small opening.
    float in1 = clamp(l * 2.0, 0.0, 1.0);
    float out1 = clamp(l * 2.0 - 1.0, 0.0, 1.0);
    in1 = in1 * in1 * (3.0 - 2.0 * in1);
    out1 = out1 * out1 * (3.0 - 2.0 * out1);
    vec3 p = mix(mix(from, ring, in1), to, out1);

    // Loose while it travels, still at both ends.
    float loose = sin(l * 3.14159);
    p.y += sin(uTime * 1.7 + aSeed.z * 40.0 + l * 9.0) * 0.16 * loose;
    p.z += cos(uTime * 1.3 + aSeed.x * 40.0 + l * 7.0) * 0.22 * loose;
    // A slow breath while it rests in a page.
    p += (aSeed - 0.5) * 0.035 * sin(uTime * 0.8 + aSeed.y * 20.0) * (1.0 - loose);

    vec2 away = p.xy - uPointer;
    float near = smoothstep(1.3, 0.0, length(away));
    p.xy += normalize(away + 0.0001) * near * 0.55;
    p += (aSeed - 0.5) * uScatter * 9.0;

    vTurn = smoothstep(0.45, 0.55, l);
    // The first page fades in as the last one fades out, so the loop has no jump.
    vAlpha = smoothstep(0.0, 0.06, uLoop) * (1.0 - smoothstep(0.93, 1.0, uLoop)) * (1.0 - uScatter);

    vec4 mv = modelViewMatrix * vec4(p, 1.0);
    gl_Position = projectionMatrix * mv;
    gl_PointSize = uSize * (0.55 + aSeed.z * 0.9) * (1.0 + loose * 0.6) / -mv.z;
}`;

const fragment = /* glsl */ `
uniform vec3 uWater;
uniform vec3 uBrass;
uniform float uSolid;
varying float vTurn;
varying float vAlpha;

void main() {
    float d = length(gl_PointCoord - 0.5);
    float disc = smoothstep(0.5, mix(0.05, 0.38, uSolid), d);
    gl_FragColor = vec4(mix(uWater, uBrass, vTurn), disc * vAlpha * mix(0.8, 0.95, uSolid));
    #include <colorspace_fragment>
}`;

/** Points that draw a page: its outline, its folded corner and its lines of text. */
function page(count: number) {
  const spots = new Float32Array(count * 3);
  const seeds = new Float32Array(count * 3);
  const order = new Float32Array(count);
  const lines = 9;
  for (let i = 0; i < count; i++) {
    let x = 0;
    let y = 0;
    const kind = Math.random();
    if (kind < 0.2) {
      // The outline, with the corner cut.
      const t = Math.random() * (PAGE_W * 2 + PAGE_H * 2);
      if (t < PAGE_W) [x, y] = [t, 0];
      else if (t < PAGE_W + PAGE_H) [x, y] = [PAGE_W, t - PAGE_W];
      else if (t < PAGE_W * 2 + PAGE_H) [x, y] = [t - PAGE_W - PAGE_H, PAGE_H];
      else [x, y] = [0, t - PAGE_W * 2 - PAGE_H];
      if (x > PAGE_W - FOLD && y > PAGE_H - FOLD) {
        const along = Math.random() * FOLD;
        const side = Math.random();
        if (side < 0.34) [x, y] = [PAGE_W - FOLD + along, PAGE_H - along];
        else if (side < 0.67) [x, y] = [PAGE_W - FOLD, PAGE_H - along];
        else [x, y] = [PAGE_W - FOLD + along, PAGE_H - FOLD];
      }
    } else if (kind < 0.86) {
      // A line of text. The last line is short.
      const line = Math.floor(Math.random() * lines);
      const reach = line === lines - 1 ? 0.45 : line < 2 ? 0.56 : 0.78;
      x = 0.3 + Math.random() * (PAGE_W - 0.6) * (reach / 0.78);
      y = PAGE_H - 0.55 - line * 0.27 + (Math.random() - 0.5) * 0.07;
    } else {
      // A faint fill, so the page reads as a sheet.
      do {
        x = Math.random() * PAGE_W;
        y = Math.random() * PAGE_H;
      } while (
        x > PAGE_W - FOLD &&
        y > PAGE_H - FOLD &&
        x - (PAGE_W - FOLD) + (y - (PAGE_H - FOLD)) > FOLD
      );
    }
    spots[i * 3] = x - PAGE_W / 2;
    spots[i * 3 + 1] = y - PAGE_H / 2;
    spots[i * 3 + 2] = (Math.random() - 0.5) * 0.16;
    seeds[i * 3] = Math.random();
    seeds[i * 3 + 1] = Math.random();
    seeds[i * 3 + 2] = Math.random();
    // The edge nearest the ring leaves first, with a little disorder.
    order[i] = Math.min(
      1,
      Math.max(0, 1 - x / PAGE_W) * 0.8 + Math.random() * 0.2,
    );
  }
  const geometry = new BufferGeometry();
  geometry.setAttribute("position", new BufferAttribute(spots, 3));
  geometry.setAttribute("aSeed", new BufferAttribute(seeds, 3));
  geometry.setAttribute("aOrder", new BufferAttribute(order, 1));
  return geometry;
}

export function mount(host: HTMLElement, still: boolean) {
  const canvas = document.createElement("canvas");
  canvas.setAttribute("aria-hidden", "true");
  const renderer = new WebGLRenderer({ canvas, antialias: false, alpha: true });
  const ratio = Math.min(devicePixelRatio, 2);
  renderer.setPixelRatio(ratio);
  host.prepend(canvas);

  const scene = new Scene();
  const camera = new PerspectiveCamera(35, 1, 0.1, 60);
  camera.position.set(0, 0, 11.5);

  const uniforms = {
    uLoop: { value: still ? 0.5 : 0 },
    uTime: { value: 0 },
    uScatter: { value: 0 },
    uSize: { value: 30 },
    uFrom: { value: new Vector3() },
    uTo: { value: new Vector3() },
    uPointer: { value: new Vector2(99, 99) },
    uWater: { value: new Color() },
    uBrass: { value: new Color() },
    uSolid: { value: 0 },
  };
  const material = new ShaderMaterial({
    uniforms,
    vertexShader: vertex,
    fragmentShader: fragment,
    transparent: true,
    depthTest: false,
    depthWrite: false,
  });
  const points = new Points(page(innerWidth < 700 ? 14000 : 30000), material);
  points.frustumCulled = false;
  const ringMaterial = new MeshBasicMaterial({ toneMapped: false });
  const ring = new Mesh(new TorusGeometry(0.62, 0.035, 12, 96), ringMaterial);
  scene.add(points, ring);

  const paint = () => {
    const palette = readPalette(host);
    const light = isLight();
    uniforms.uWater.value.copy(palette.water);
    uniforms.uBrass.value.copy(palette.brass);
    uniforms.uSolid.value = light ? 1 : 0;
    material.blending = light ? NormalBlending : AdditiveBlending;
    material.needsUpdate = true;
    ringMaterial.color.copy(palette.brass);
  };
  paint();

  let half = new Vector2(1, 1);
  const resize = () => {
    const { width, height } = host.getBoundingClientRect();
    if (width === 0 || height === 0) return;
    renderer.setSize(width, height, false);
    camera.aspect = width / height;
    const tall = camera.aspect < 0.9;
    // Side by side on a wide screen, one above the other on a tall one. On a tall screen the
    // camera steps back so both pages fit the height.
    camera.position.z = tall ? 15.5 : Math.max(11.5, 15.5 / camera.aspect);
    camera.updateProjectionMatrix();
    const reach = Math.tan((camera.fov * Math.PI) / 360) * camera.position.z;
    half = new Vector2(reach * camera.aspect, reach);
    if (tall) {
      uniforms.uFrom.value.set(0, 2.9, 0);
      uniforms.uTo.value.set(0, -2.9, 0);
      ring.rotation.set(Math.PI / 2 - 0.35, 0, 0);
    } else {
      const apart = Math.min(4.4, half.x - PAGE_W / 2 - 0.5);
      uniforms.uFrom.value.set(-apart, -0.5, 0);
      uniforms.uTo.value.set(apart, -0.5, 0);
      ring.rotation.set(0, Math.PI / 2 - 0.45, 0);
    }
    ring.position.y = tall ? 0 : -0.5;
    uniforms.uSize.value = (height * ratio) / 34;
    // Where each page sits, for the captions the hero puts under them.
    for (const [name, at] of [
      ["from", uniforms.uFrom.value],
      ["to", uniforms.uTo.value],
    ] as const) {
      host.style.setProperty(
        `--${name}-x`,
        `${(50 + (at.x / half.x) * 50).toFixed(2)}%`,
      );
      host.style.setProperty(
        `--${name}-y`,
        `${(50 - ((at.y - PAGE_H / 2 - 0.3) / half.y) * 50).toFixed(2)}%`,
      );
    }
    if (still) renderer.render(scene, camera);
  };
  new ResizeObserver(resize).observe(host);
  resize();

  const repaint = () => {
    paint();
    if (still) renderer.render(scene, camera);
  };
  new MutationObserver(repaint).observe(document.documentElement, {
    attributeFilter: ["data-theme"],
  });
  matchMedia("(prefers-color-scheme: light)").addEventListener(
    "change",
    repaint,
  );

  if (still) {
    // Reduced motion: one frame, caught half way across.
    renderer.render(scene, camera);
    host.dataset.live = "true";
    return;
  }

  host.addEventListener(
    "pointermove",
    (event) => {
      const box = host.getBoundingClientRect();
      uniforms.uPointer.value.set(
        ((event.clientX - box.left) / box.width - 0.5) * 2 * half.x,
        -((event.clientY - box.top) / box.height - 0.5) * 2 * half.y,
      );
    },
    { passive: true },
  );
  host.addEventListener("pointerleave", () =>
    uniforms.uPointer.value.set(99, 99),
  );

  let visible = true;
  let running = false;
  let clock = 0;
  let last = 0;
  let frames = 0;
  const loop = (now: number) => {
    if (!visible || document.hidden) {
      running = false;
      return;
    }
    clock += Math.min(0.05, (now - last) / 1000 || 0);
    last = now;
    uniforms.uTime.value = clock;
    uniforms.uLoop.value = (clock / LOOP) % 1;
    const box = host.getBoundingClientRect();
    uniforms.uScatter.value = Math.min(
      1,
      Math.max(0, -box.top / (box.height * 0.8)),
    );
    renderer.render(scene, camera);
    if (frames++ === 1) host.dataset.live = "true";
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
  canvas.addEventListener("webglcontextlost", () => {
    visible = false;
    delete host.dataset.live;
  });
  run();
}
