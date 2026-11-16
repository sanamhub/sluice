// The scene of version 4: one long canal, with the sending machine at one end, the receiving
// machine at the other and the gate in the middle. Blocks leave the sender's face, ride above the
// water, turn brass as they pass the ring, and land on the receiver's face. Scrolling does not move
// the page's picture, it moves the camera: six stops along one curve, from a view of the sender to
// the whole canal seen from above.
import {
  ACESFilmicToneMapping,
  AdditiveBlending,
  BoxGeometry,
  BufferAttribute,
  BufferGeometry,
  CatmullRomCurve3,
  CircleGeometry,
  Color,
  DirectionalLight,
  DoubleSide,
  Fog,
  HemisphereLight,
  InstancedMesh,
  Mesh,
  MeshBasicMaterial,
  MeshStandardMaterial,
  NormalBlending,
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

const HALF = 30; // the gate is at x = 0 and each machine's face is this far from it
const GRID = 8;
const CELLS = GRID * GRID;
const CROSSING = 9; // seconds a block takes from face to face
const RIBS = 31;

// The camera's stops: where it stands and what it looks at. One stop per screen of scrolling.
const STOPS: [number, number, number, number, number, number][] = [
  [-47, 6.4, 13.5, -31, 2.4, -3.5], // the sender, and the canal leading away
  [-23.5, 3.1, 5.6, -30, 2.6, 0], // back at the sender's face, where the blocks leave
  [-16, 2.5, 3.9, -3, 0.9, -1.2], // over the bank, travelling with the blocks
  [-5.2, 2.2, 2.3, 3, 0.85, 1.6], // at the ring
  [21.5, 3.3, 6.2, 30, 2.5, 0], // the receiver's face filling
  [-12, 12, 33, 3, 8.5, 0], // all of it at once
];

function faceCells(x: number, turn: number) {
  const mesh = new InstancedMesh(
    new PlaneGeometry(0.4, 0.4),
    new MeshBasicMaterial({ toneMapped: false, side: DoubleSide }),
    CELLS,
  );
  const holder = new Object3D();
  holder.rotation.y = turn;
  for (let i = 0; i < CELLS; i++) {
    holder.position.set(
      x,
      0.95 + Math.floor(i / GRID) * 0.52,
      ((i % GRID) - (GRID - 1) / 2) * 0.52,
    );
    holder.updateMatrix();
    mesh.setMatrixAt(i, holder.matrix);
    mesh.setColorAt(i, new Color());
  }
  return mesh;
}

function build(host: HTMLElement, count: number) {
  let palette = readPalette(host);
  let light = isLight();
  const scene = new Scene();
  scene.fog = new Fog(palette.basin, 16, 70);

  const stone = new MeshStandardMaterial({
    color: palette.stone,
    roughness: 0.8,
    metalness: 0.08,
  });
  const ground = new MeshStandardMaterial({
    color: palette["stone-2"],
    roughness: 0.92,
  });
  const brass = new MeshStandardMaterial({
    color: palette.brass,
    roughness: 0.34,
    metalness: 0.35,
    emissive: palette.brass,
    emissiveIntensity: 0.22,
  });

  const slab = new Mesh(new BoxGeometry(HALF * 2 + 26, 0.4, 16), ground);
  slab.position.y = -0.2;
  scene.add(slab);

  for (const side of [-1, 1]) {
    const machine = new Mesh(
      new RoundedBoxGeometry(5.2, 5.6, 5.2, 4, 0.22),
      stone,
    );
    machine.position.set(side * (HALF + 2.7), 2.8, 0);
    const wall = new Mesh(new BoxGeometry(HALF * 2, 0.7, 0.22), stone);
    wall.position.set(0, 0.35, side * 1.25);
    scene.add(machine, wall);
  }
  const bed = new Mesh(new BoxGeometry(HALF * 2, 0.1, 2.4), stone);
  bed.position.y = 0.05;
  scene.add(bed);

  // Posts along both banks, each with a lamp: water-coloured before the gate, brass after it.
  const posts = new InstancedMesh(
    new BoxGeometry(0.26, 1.15, 0.26),
    stone,
    RIBS * 2,
  );
  const lamps = new InstancedMesh(
    new BoxGeometry(0.12, 0.12, 0.12),
    new MeshBasicMaterial({ toneMapped: false }),
    RIBS * 2,
  );
  const holder = new Object3D();
  for (let i = 0; i < RIBS * 2; i++) {
    const x = -HALF + (Math.floor(i / 2) + 0.5) * ((HALF * 2) / RIBS);
    const z = (i % 2 ? 1 : -1) * 1.48;
    holder.position.set(x, 0.575, z);
    holder.updateMatrix();
    posts.setMatrixAt(i, holder.matrix);
    holder.position.y = 1.24;
    holder.updateMatrix();
    lamps.setMatrixAt(i, holder.matrix);
  }
  scene.add(posts, lamps);
  const paintLamps = () => {
    for (let i = 0; i < RIBS * 2; i++)
      lamps.setColorAt(
        i,
        Math.floor(i / 2) < RIBS / 2 ? palette.water : palette.brass,
      );
    if (lamps.instanceColor) lamps.instanceColor.needsUpdate = true;
  };
  paintLamps();

  const water = makeWater(
    HALF * 2,
    2.28,
    palette["water-deep"],
    palette.water,
    light,
  );
  water.mesh.position.y = 0.46;
  scene.add(water.mesh);

  // The gate: two posts, a lintel, and the ring every block passes through.
  for (const z of [-1.9, 1.9]) {
    const post = new Mesh(new BoxGeometry(0.34, 4.4, 0.34), brass);
    post.position.set(0, 2.2, z);
    scene.add(post);
  }
  const lintel = new Mesh(new BoxGeometry(0.4, 0.34, 4.5), brass);
  lintel.position.y = 4.4;
  const ring = new Mesh(new TorusGeometry(1.12, 0.07, 14, 80), brass);
  ring.position.y = 1.0;
  ring.rotation.y = Math.PI / 2;
  const flashMaterial = new MeshBasicMaterial({
    color: palette.brass,
    transparent: true,
    opacity: 0,
    toneMapped: false,
  });
  const flash = new Mesh(new TorusGeometry(1.2, 0.03, 8, 80), flashMaterial);
  flash.position.copy(ring.position);
  flash.rotation.y = Math.PI / 2;
  const veilMaterial = new MeshBasicMaterial({
    color: palette.brass,
    transparent: true,
    opacity: 0.08,
    side: DoubleSide,
    depthWrite: false,
    toneMapped: false,
  });
  const veil = new Mesh(new CircleGeometry(1.08, 64), veilMaterial);
  veil.position.copy(ring.position);
  veil.rotation.y = Math.PI / 2;
  scene.add(lintel, ring, flash, veil);

  const sender = faceCells(-HALF - 0.08, Math.PI / 2);
  const receiver = faceCells(HALF + 0.08, -Math.PI / 2);
  scene.add(sender, receiver);

  const blocks = new InstancedMesh(
    new RoundedBoxGeometry(0.34, 0.34, 0.34, 2, 0.07),
    new MeshStandardMaterial({
      roughness: 0.32,
      metalness: 0.1,
      toneMapped: false,
    }),
    count,
  );
  const tailMaterial = new MeshBasicMaterial({
    transparent: true,
    opacity: 0.3,
    depthWrite: false,
    toneMapped: false,
    blending: light ? NormalBlending : AdditiveBlending,
  });
  const tails = new InstancedMesh(
    new BoxGeometry(1, 0.05, 0.05),
    tailMaterial,
    count,
  );
  blocks.frustumCulled = false;
  tails.frustumCulled = false;
  scene.add(blocks, tails);
  const seeds = Array.from({ length: count }, (_, i) => ({
    at: (i + Math.random() * 0.7) / count,
    z: (Math.random() - 0.5) * 1.3,
    y: 0.78 + Math.random() * 0.5,
    spin: Math.random() * 6,
    cell: Math.floor(Math.random() * CELLS),
  }));

  // A haze of motes over the whole length.
  const MOTES = 700;
  const spots = new Float32Array(MOTES * 3);
  for (let i = 0; i < MOTES; i++) {
    spots[i * 3] = (Math.random() - 0.5) * (HALF * 2 + 12);
    spots[i * 3 + 1] = 0.5 + Math.random() * 5;
    spots[i * 3 + 2] = (Math.random() - 0.5) * 9;
  }
  const hazeGeometry = new BufferGeometry();
  hazeGeometry.setAttribute("position", new BufferAttribute(spots, 3));
  const hazeMaterial = new PointsMaterial({
    color: palette.water,
    size: 0.05,
    transparent: true,
    opacity: 0.5,
    depthWrite: false,
    blending: light ? NormalBlending : AdditiveBlending,
  });
  scene.add(new Points(hazeGeometry, hazeMaterial));

  const sky = new HemisphereLight(palette.ink, palette.basin, 1.9);
  const sun = new DirectionalLight(0xffffff, 1.5);
  sun.position.set(-8, 14, 10);
  const sendGlow = new PointLight(palette.water, 30, 14, 1.5);
  sendGlow.position.set(-HALF + 1.5, 2.6, 0);
  const gateGlow = new PointLight(palette.brass, 10, 10, 1.5);
  gateGlow.position.set(0.6, 1.4, 0);
  const landGlow = new PointLight(palette.water, 30, 14, 1.5);
  landGlow.position.set(HALF - 1.5, 2.6, 0);
  scene.add(sky, sun, sendGlow, gateGlow, landGlow);

  const colour = new Color();
  const lit = new Float32Array(CELLS).fill(-10); // when each receiver cell was last lit
  const gone = new Float32Array(CELLS).fill(-10); // when each sender cell last sent a block
  let lastFlash = -10;
  let before = 0;

  return {
    scene,
    recolour(next: Palette, nextLight: boolean) {
      palette = next;
      light = nextLight;
      (scene.fog as Fog).color.copy(next.basin);
      stone.color.copy(next.stone);
      ground.color.copy(next["stone-2"]);
      brass.color.copy(next.brass);
      brass.emissive.copy(next.brass);
      flashMaterial.color.copy(next.brass);
      veilMaterial.color.copy(next.brass);
      hazeMaterial.color.copy(next.water);
      hazeMaterial.blending = tailMaterial.blending = nextLight
        ? NormalBlending
        : AdditiveBlending;
      sky.color.copy(next.ink);
      sky.groundColor.copy(next.basin);
      sendGlow.color.copy(next.water);
      landGlow.color.copy(next.water);
      gateGlow.color.copy(next.brass);
      water.recolour(next["water-deep"], next.water, nextLight);
      paintLamps();
    },
    tick(time: number) {
      const step = time - before;
      before = time;
      for (let i = 0; i < count; i++) {
        const seed = seeds[i];
        const p = (time / CROSSING + seed.at) % 1;
        const was = (p - step / CROSSING + 1) % 1;
        const x = -HALF + p * HALF * 2;
        if (was > p) {
          // The block wrapped: it has just landed, and a new one leaves the sender.
          lit[seed.cell] = time;
          seed.cell = Math.floor(Math.random() * CELLS);
          gone[seed.cell] = time;
        }
        if (was < 0.5 && p >= 0.5) lastFlash = time;
        const ends = Math.min(1, p / 0.03, (1 - p) / 0.03);
        const pull = 1 - Math.min(1, Math.abs(x) / 5) ** 2; // toward the ring's centre near it
        const y =
          seed.y +
          (1.0 - seed.y) * pull +
          Math.sin(time * 2 + seed.spin) * 0.04;
        const z = seed.z * (1 - pull * 0.75);
        holder.position.set(x, y, z);
        holder.rotation.set(time * 0.7 + seed.spin, time * 0.5 + seed.spin, 0);
        holder.scale.setScalar(Math.max(0.001, ends));
        holder.updateMatrix();
        blocks.setMatrixAt(i, holder.matrix);
        colour.copy(x < 0 ? palette.water : palette.brass);
        blocks.setColorAt(i, colour);
        tails.setColorAt(i, colour);
        holder.position.x = x - 0.95;
        holder.rotation.set(0, 0, 0);
        holder.scale.set(1.7 * ends, 1, 1);
        holder.updateMatrix();
        tails.setMatrixAt(i, holder.matrix);
      }
      for (let i = 0; i < CELLS; i++) {
        const sent = Math.min(1, (time - gone[i]) / 1.4);
        colour.copy(palette.line).lerp(palette.water, 0.35 + sent * 0.65);
        sender.setColorAt(i, colour);
        const since = time - lit[i];
        colour.copy(palette.line).lerp(palette.water, 0.3);
        if (since < 4)
          colour.lerp(
            since < 0.7 ? palette.brass : palette.water,
            1 - since / 4,
          );
        receiver.setColorAt(i, colour);
      }
      for (const mesh of [blocks, tails, sender, receiver]) {
        mesh.instanceMatrix.needsUpdate = true;
        if (mesh.instanceColor) mesh.instanceColor.needsUpdate = true;
      }
      const glow = Math.max(0, 1 - (time - lastFlash) / 0.5);
      flashMaterial.opacity = glow * 0.9;
      flash.scale.setScalar(1 + (1 - glow) * 0.18);
      veilMaterial.opacity = 0.07 + glow * 0.16;
      gateGlow.intensity = 10 + glow * 26;
      water.tick(time);
      for (let i = 0; i < MOTES; i++) {
        spots[i * 3] += 0.012;
        if (spots[i * 3] > HALF + 6) spots[i * 3] = -HALF - 6;
      }
      hazeGeometry.attributes.position.needsUpdate = true;
    },
  };
}

/** Starts the scene in `stage` and ties the camera to how far `track` has scrolled. */
export function mount(stage: HTMLElement, track: HTMLElement, still: boolean) {
  const canvas = document.createElement("canvas");
  canvas.setAttribute("aria-hidden", "true");
  const renderer = new WebGLRenderer({ canvas, antialias: true, alpha: true });
  renderer.setPixelRatio(Math.min(devicePixelRatio, 1.75));
  renderer.outputColorSpace = SRGBColorSpace;
  renderer.toneMapping = ACESFilmicToneMapping;
  renderer.toneMappingExposure = 1.08;
  stage.prepend(canvas);

  const small = innerWidth < 700;
  const built = build(stage, small ? 60 : 110);
  const camera = new PerspectiveCamera(40, 1, 0.1, 160);
  const eyes = new CatmullRomCurve3(
    STOPS.map((s) => new Vector3(s[0], s[1], s[2])),
    false,
    "catmullrom",
    0.35,
  );
  const aims = new CatmullRomCurve3(
    STOPS.map((s) => new Vector3(s[3], s[4], s[5])),
    false,
    "catmullrom",
    0.35,
  );
  const eye = new Vector3();
  const aim = new Vector3();
  const pointer = { x: 0, y: 0, ex: 0, ey: 0 };
  let wanted = still ? 1 : 0;
  let shown = wanted;

  const resize = () => {
    const { width, height } = stage.getBoundingClientRect();
    if (width === 0 || height === 0) return;
    renderer.setSize(width, height, false);
    camera.aspect = width / height;
    // A tall screen sees less to either side, so it gets a wider lens.
    camera.fov = camera.aspect < 0.8 ? 62 : camera.aspect < 1.3 ? 50 : 40;
    camera.updateProjectionMatrix();
  };
  new ResizeObserver(resize).observe(stage);
  resize();

  const measure = () => {
    if (still) return;
    const box = track.getBoundingClientRect();
    const range = box.height - innerHeight;
    wanted = range > 0 ? Math.min(1, Math.max(0, -box.top / range)) : 0;
  };
  addEventListener("scroll", measure, { passive: true });
  addEventListener("resize", measure);
  measure();
  shown = wanted;

  addEventListener(
    "pointermove",
    (event) => {
      pointer.x = event.clientX / innerWidth - 0.5;
      pointer.y = event.clientY / innerHeight - 0.5;
    },
    { passive: true },
  );

  const recolour = () => built.recolour(readPalette(stage), isLight());
  new MutationObserver(recolour).observe(document.documentElement, {
    attributeFilter: ["data-theme"],
  });
  matchMedia("(prefers-color-scheme: light)").addEventListener("change", () => {
    recolour();
    if (still) draw(0);
  });

  const draw = (clock: number) => {
    shown += (wanted - shown) * 0.09;
    pointer.ex += (pointer.x - pointer.ex) * 0.05;
    pointer.ey += (pointer.y - pointer.ey) * 0.05;
    eyes.getPoint(shown, eye);
    aims.getPoint(shown, aim);
    camera.position.set(
      eye.x + pointer.ex * 1.1,
      eye.y - pointer.ey * 0.7,
      eye.z,
    );
    camera.lookAt(aim);
    built.tick(clock);
    renderer.render(built.scene, camera);
    track.style.setProperty("--p", shown.toFixed(4));
  };

  if (still) {
    // Reduced motion: one frame of the whole canal, drawn again only when the theme changes.
    for (let n = 0; n < 40; n++) built.tick(n * 0.25);
    draw(10);
    new MutationObserver(() => draw(10)).observe(document.documentElement, {
      attributeFilter: ["data-theme"],
    });
    stage.dataset.live = "true";
    return;
  }

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
    draw(clock);
    if (frames++ === 1) stage.dataset.live = "true";
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
  }).observe(stage);
  document.addEventListener("visibilitychange", run);
  canvas.addEventListener("webglcontextlost", () => {
    visible = false;
    delete stage.dataset.live;
  });
  run();
}
