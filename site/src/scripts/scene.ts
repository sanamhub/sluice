// The hero's moving scene. Two machines stand at either end of a stone channel. A grid on the
// sender's face is the file, cut into blocks; each block leaves, rides the water, and passes under
// the sluice gate, where the brass ring flashes as it is checked; then it lights its cell on the
// receiver's face. When every cell is lit the run rests, and starts again.
import {
  ACESFilmicToneMapping,
  AdditiveBlending,
  BoxGeometry,
  BufferAttribute,
  BufferGeometry,
  Color,
  DirectionalLight,
  EdgesGeometry,
  Fog,
  Group,
  HemisphereLight,
  InstancedMesh,
  LineBasicMaterial,
  LineSegments,
  Mesh,
  NormalBlending,
  MeshBasicMaterial,
  MeshStandardMaterial,
  Object3D,
  PerspectiveCamera,
  PlaneGeometry,
  PointLight,
  Points,
  PointsMaterial,
  Scene,
  SRGBColorSpace,
  TorusGeometry,
  Vector3,
  WebGLRenderer,
} from "three";
import { RoundedBoxGeometry } from "three/addons/geometries/RoundedBoxGeometry.js";
import { isLight, readPalette, type Palette } from "./palette";
import { makeWater } from "./water";

const COLS = 6;
const ROWS = 4;
const BLOCKS = COLS * ROWS;
const GAP = 0.42; // seconds between departures
const TRAVEL = 3.1; // seconds from the sender's face to the receiver's
const REST = 1.6; // seconds with every cell lit before the run starts again
const CYCLE = GAP * (BLOCKS - 1) + TRAVEL + REST;
const START_X = -2.25;
const END_X = 2.25;
const MACHINE_X = 3.4;
const MIST = 180;

interface Built {
  scene: Scene;
  recolour(palette: Palette, light: boolean): void;
  tick(time: number): void;
}

/** A grid of cells on a machine's front face, one per block of the file. */
function faceGrid(x: number) {
  const cell = new PlaneGeometry(0.2, 0.2);
  const mesh = new InstancedMesh(
    cell,
    new MeshBasicMaterial({ toneMapped: false }),
    BLOCKS,
  );
  const holder = new Object3D();
  for (let i = 0; i < BLOCKS; i++) {
    const col = i % COLS;
    const row = Math.floor(i / COLS);
    holder.position.set(
      x + (col - (COLS - 1) / 2) * 0.27,
      1.22 - row * 0.27,
      1.111,
    );
    holder.updateMatrix();
    mesh.setMatrixAt(i, holder.matrix);
    mesh.setColorAt(i, new Color());
  }
  return mesh;
}

function machine(
  x: number,
  stone: MeshStandardMaterial,
  edge: LineBasicMaterial,
) {
  const group = new Group();
  const body = new RoundedBoxGeometry(2.2, 1.7, 2.2, 4, 0.14);
  const box = new Mesh(body, stone);
  box.position.set(x, 0.85, 0);
  const lines = new LineSegments(
    new EdgesGeometry(new BoxGeometry(2.24, 1.74, 2.24), 1),
    edge,
  );
  lines.position.copy(box.position);
  group.add(box, lines);
  return group;
}

function channel(stone: MeshStandardMaterial) {
  const group = new Group();
  const length = END_X - START_X + 0.3;
  for (const z of [-0.56, 0.56]) {
    const wall = new Mesh(new BoxGeometry(length, 0.52, 0.12), stone);
    wall.position.set(0, 0.26, z);
    group.add(wall);
  }
  const bed = new Mesh(new BoxGeometry(length, 0.08, 1.0), stone);
  bed.position.set(0, 0.04, 0);
  group.add(bed);
  return group;
}

function gate(brass: MeshStandardMaterial) {
  const group = new Group();
  for (const z of [-0.66, 0.66]) {
    const post = new Mesh(new BoxGeometry(0.14, 1.62, 0.14), brass);
    post.position.set(0, 0.81, z);
    group.add(post);
  }
  const lintel = new Mesh(new BoxGeometry(0.16, 0.14, 1.5), brass);
  lintel.position.set(0, 1.62, 0);
  const plate = new Mesh(new BoxGeometry(0.07, 0.5, 1.12), brass);
  plate.position.set(0, 1.18, 0);
  const wheel = new Mesh(new TorusGeometry(0.24, 0.035, 10, 40), brass);
  wheel.position.set(0, 1.98, 0);
  wheel.rotation.y = Math.PI / 2;
  const spindle = new Mesh(new BoxGeometry(0.05, 0.3, 0.05), brass);
  spindle.position.set(0, 1.8, 0);
  group.add(lintel, plate, wheel, spindle);
  return { group, wheel };
}

function mist(water: Color) {
  const positions = new Float32Array(MIST * 3);
  for (let i = 0; i < MIST; i++) {
    positions[i * 3] = START_X + Math.random() * (END_X - START_X);
    positions[i * 3 + 1] = 0.4 + Math.random() * 1.1;
    positions[i * 3 + 2] = (Math.random() - 0.5) * 1.1;
  }
  const geometry = new BufferGeometry();
  geometry.setAttribute("position", new BufferAttribute(positions, 3));
  const material = new PointsMaterial({
    color: water,
    size: 0.03,
    transparent: true,
    opacity: 0.55,
    depthWrite: false,
    blending: AdditiveBlending,
  });
  return { points: new Points(geometry, material), positions, material };
}

function build(host: HTMLElement): Built {
  let palette = readPalette(host);
  let light = isLight();
  const scene = new Scene();
  scene.fog = new Fog(palette.basin, 10, 22);

  const stone = new MeshStandardMaterial({
    color: palette.stone,
    roughness: 0.82,
    metalness: 0.06,
  });
  const plinthStone = new MeshStandardMaterial({
    color: palette["stone-2"],
    roughness: 0.9,
  });
  const edge = new LineBasicMaterial({
    color: palette["line-strong"],
    transparent: true,
    opacity: 0.7,
  });
  const brass = new MeshStandardMaterial({
    color: palette.brass,
    roughness: 0.38,
    metalness: 0.3,
    emissive: palette.brass,
    emissiveIntensity: 0.18,
  });

  const plinth = new Mesh(
    new RoundedBoxGeometry(11.6, 0.36, 4.4, 4, 0.1),
    plinthStone,
  );
  plinth.position.y = -0.18;
  scene.add(
    plinth,
    machine(-MACHINE_X, stone, edge),
    machine(MACHINE_X, stone, edge),
    channel(stone),
  );

  const water = makeWater(
    END_X - START_X + 0.3,
    1.0,
    palette["water-deep"],
    palette.water,
    light,
  );
  water.mesh.position.y = 0.4;
  scene.add(water.mesh);

  const { group: gateGroup, wheel } = gate(brass);
  scene.add(gateGroup);
  const ringMaterial = new MeshBasicMaterial({
    color: palette.brass,
    transparent: true,
    opacity: 0,
    toneMapped: false,
  });
  const ring = new Mesh(new TorusGeometry(0.36, 0.022, 8, 56), ringMaterial);
  ring.position.set(0, 0.58, 0);
  ring.rotation.y = Math.PI / 2;
  scene.add(ring);

  const sender = faceGrid(-MACHINE_X);
  const receiver = faceGrid(MACHINE_X);
  scene.add(sender, receiver);

  const blockMaterial = new MeshStandardMaterial({
    roughness: 0.3,
    metalness: 0.1,
    toneMapped: false,
  });
  const blocks = new InstancedMesh(
    new RoundedBoxGeometry(0.32, 0.32, 0.32, 2, 0.06),
    blockMaterial,
    BLOCKS,
  );
  blocks.frustumCulled = false;
  scene.add(blocks);

  const haze = mist(palette.water);
  scene.add(haze.points);

  const sky = new HemisphereLight(palette.ink, palette.basin, 1.1);
  const sun = new DirectionalLight(0xffffff, 1.6);
  sun.position.set(-4, 9, 6);
  const glow = new PointLight(palette.water, 6, 5, 1.6);
  glow.position.set(0, 0.9, 0.4);
  const lamp = new PointLight(palette.brass, 0, 3, 1.4);
  lamp.position.set(0, 1.1, 0.7);
  scene.add(sky, sun, glow, lamp);

  const holder = new Object3D();
  const colour = new Color();
  const hidden = new Vector3(0, -5, 0);
  let lastFlash = -10;

  return {
    scene,
    recolour(next, nextLight) {
      palette = next;
      light = nextLight;
      (scene.fog as Fog).color.copy(next.basin);
      stone.color.copy(next.stone);
      plinthStone.color.copy(next["stone-2"]);
      edge.color.copy(next["line-strong"]);
      brass.color.copy(next.brass);
      brass.emissive.copy(next.brass);
      ringMaterial.color.copy(next.brass);
      haze.material.color.copy(next.water);
      haze.material.blending = nextLight ? NormalBlending : AdditiveBlending;
      sky.color.copy(next.ink);
      sky.groundColor.copy(next.basin);
      glow.color.copy(next.water);
      lamp.color.copy(next.brass);
      water.recolour(next["water-deep"], next.water, nextLight);
    },
    tick(time) {
      const t = time % CYCLE;
      const resting = t > GAP * (BLOCKS - 1) + TRAVEL;
      for (let i = 0; i < BLOCKS; i++) {
        const p = (t - i * GAP) / TRAVEL;
        const gone = p >= 0;
        const arrived = p >= 1;
        colour.copy(gone ? palette.line : palette.water);
        sender.setColorAt(i, colour);
        if (arrived) {
          const since = (p - 1) * TRAVEL;
          colour
            .copy(palette.brass)
            .lerp(palette.water, Math.min(1, since / 0.6));
        } else colour.copy(palette.line);
        if (resting) colour.lerp(palette.water, 0.2);
        receiver.setColorAt(i, colour);

        if (!gone || arrived) {
          holder.position.copy(hidden);
          holder.scale.setScalar(0.001);
        } else {
          const x = START_X + p * (END_X - START_X);
          const lane = ((i * 7) % 3) - 1;
          holder.position.set(
            x,
            0.56 + Math.sin(time * 3 + i) * 0.03,
            lane * 0.14,
          );
          holder.rotation.set(
            Math.sin(time + i) * 0.12,
            time * 0.6 + i,
            Math.cos(time * 0.8 + i) * 0.1,
          );
          const edgeScale = Math.min(1, p / 0.06, (1 - p) / 0.06);
          holder.scale.setScalar(Math.max(0.001, edgeScale));
          const past = x - 0;
          if (past >= 0 && past < 0.05) lastFlash = time;
          if (past < 0) colour.copy(palette.water);
          else
            colour
              .copy(palette.brass)
              .lerp(palette.water, Math.min(1, past / 0.9));
          blocks.setColorAt(i, colour);
        }
        holder.updateMatrix();
        blocks.setMatrixAt(i, holder.matrix);
      }
      for (const mesh of [blocks, sender, receiver]) {
        mesh.instanceMatrix.needsUpdate = true;
        if (mesh.instanceColor) mesh.instanceColor.needsUpdate = true;
      }
      const flash = Math.max(0, 1 - (time - lastFlash) / 0.45);
      ringMaterial.opacity = flash * 0.95;
      ring.scale.setScalar(1 + (1 - flash) * 0.25);
      lamp.intensity = flash * 4;
      wheel.rotation.x = time * 0.35;
      water.tick(time);
      for (let i = 0; i < MIST; i++) {
        const y = i * 3 + 1;
        haze.positions[y] += 0.0025 + (i % 5) * 0.0006;
        if (haze.positions[y] > 1.6) haze.positions[y] = 0.42;
      }
      haze.points.geometry.attributes.position.needsUpdate = true;
    },
  };
}

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
