// The water behind version 6's hero: one fragment shader on one triangle, with no library. Slow
// folded noise in the page's own colours, lighter where the folds meet, and warmer toward the
// receiving side. It is drawn at half size and stretched, since it has no sharp edge to lose. With
// reduced motion one frame is drawn.
const canvas = document.querySelector<HTMLCanvasElement>("[data-liquid]");
const gl = canvas?.getContext("webgl", { antialias: false, alpha: false });

const fragment = `
precision mediump float;
uniform vec2 uSize;
uniform float uTime;
uniform vec2 uPointer;
uniform vec3 uBase;
uniform vec3 uWater;
uniform vec3 uBrass;

float hash(vec2 p) { return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }
float noise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2(1.0, 0.0)), f.x), mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), f.x), f.y);
}
float fbm(vec2 p) {
    float v = 0.0;
    float a = 0.5;
    for (int i = 0; i < 4; i++) {
        v += a * noise(p);
        p = p * 2.02 + vec2(3.1, 1.7);
        a *= 0.5;
    }
    return v;
}
void main() {
    vec2 uv = gl_FragCoord.xy / uSize;
    vec2 p = uv * vec2(uSize.x / uSize.y, 1.0) * 2.1;
    float t = uTime * 0.06;
    vec2 q = vec2(fbm(p + vec2(t * 2.0, 0.0)), fbm(p + vec2(5.2, 1.3) - t));
    vec2 r = vec2(fbm(p + 3.0 * q + vec2(1.7, 9.2) + t), fbm(p + 3.0 * q + vec2(8.3, 2.8) - t * 1.3));
    float f = fbm(p + 2.5 * r + (uPointer - uv) * 0.5);
    float fold = pow(smoothstep(0.42, 0.85, f + 0.25 * r.x), 2.0);
    vec3 c = mix(uBase, uWater, clamp(f * f * 1.5, 0.0, 1.0) * 0.5);
    c = mix(c, uBrass, smoothstep(0.5, 1.0, uv.x) * r.y * 0.4);
    c = mix(c, uWater, fold * 0.28);
    gl_FragColor = vec4(c, 1.0);
}`;

if (canvas && gl) {
  const program = gl.createProgram()!;
  for (const [type, source] of [
    [
      gl.VERTEX_SHADER,
      "attribute vec2 a; void main() { gl_Position = vec4(a, 0.0, 1.0); }",
    ],
    [gl.FRAGMENT_SHADER, fragment],
  ] as const) {
    const shader = gl.createShader(type)!;
    gl.shaderSource(shader, source);
    gl.compileShader(shader);
    gl.attachShader(program, shader);
  }
  gl.bindAttribLocation(program, 0, "a");
  gl.linkProgram(program);
  gl.useProgram(program);
  gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
  gl.bufferData(
    gl.ARRAY_BUFFER,
    new Float32Array([-1, -1, 3, -1, -1, 3]),
    gl.STATIC_DRAW,
  );
  gl.enableVertexAttribArray(0);
  gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
  const at = (name: string) => gl.getUniformLocation(program, name);

  const calm = matchMedia("(prefers-reduced-motion: reduce)");
  const pointer = { x: 0.5, y: 0.5, ex: 0.5, ey: 0.5 };
  let clock = 20;
  let last = 0;
  let frame = 0;
  let onScreen = false;

  /** A token as three numbers from 0 to 1, in the theme that is showing. */
  function colour(name: string): [number, number, number] {
    canvas!.style.color = `var(${name})`;
    const [r, g, b] = (
      getComputedStyle(canvas!).color.match(/[\d.]+/g) ?? []
    ).map(Number);
    canvas!.style.color = "";
    return [r / 255, g / 255, b / 255];
  }

  function draw() {
    gl!.uniform1f(at("uTime"), clock);
    gl!.uniform2f(at("uPointer"), pointer.ex, pointer.ey);
    gl!.drawArrays(gl!.TRIANGLES, 0, 3);
  }

  function reset() {
    const box = canvas!.getBoundingClientRect();
    canvas!.width = Math.max(1, Math.round(box.width / 2));
    canvas!.height = Math.max(1, Math.round(box.height / 2));
    gl!.viewport(0, 0, canvas!.width, canvas!.height);
    gl!.uniform2f(at("uSize"), canvas!.width, canvas!.height);
    gl!.uniform3f(at("uBase"), ...colour("--basin"));
    gl!.uniform3f(at("uWater"), ...colour("--water"));
    gl!.uniform3f(at("uBrass"), ...colour("--brass"));
    draw();
    canvas!.dataset.live = "true";
  }

  function tick(now: number) {
    clock += Math.min(0.05, (now - last) / 1000);
    last = now;
    pointer.ex += (pointer.x - pointer.ex) * 0.04;
    pointer.ey += (pointer.y - pointer.ey) * 0.04;
    draw();
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
  new MutationObserver(reset).observe(document.documentElement, {
    attributeFilter: ["data-theme"],
  });
  matchMedia("(prefers-color-scheme: light)").addEventListener("change", reset);
  addEventListener(
    "pointermove",
    (event) => {
      pointer.x = event.clientX / innerWidth;
      pointer.y = 1 - event.clientY / innerHeight;
    },
    { passive: true },
  );
}

export {};
