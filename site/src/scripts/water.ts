// The channel's water: a plane whose bands run from the sender to the receiver, with a slow cross
// ripple and brighter banks. A small shader, so the flow costs one draw call and no texture.
import { Color, DoubleSide, Mesh, PlaneGeometry, ShaderMaterial } from "three";

const vertex = /* glsl */ `
uniform float uTime;
varying vec2 vUv;
void main() {
    vUv = uv;
    vec3 p = position;
    p.z += sin(p.x * 3.2 - uTime * 2.6) * 0.012 + sin(p.y * 9.0 + uTime * 1.7) * 0.006;
    gl_Position = projectionMatrix * modelViewMatrix * vec4(p, 1.0);
}`;

const fragment = /* glsl */ `
uniform float uTime;
uniform vec3 uDeep;
uniform vec3 uWater;
uniform float uLight;
varying vec2 vUv;
void main() {
    float across = vUv.y;
    float bank = smoothstep(0.32, 0.0, across) + smoothstep(0.68, 1.0, across);
    float wobble = sin(across * 18.0 + uTime * 1.3) * 0.06;
    float band = sin((vUv.x * 26.0 - uTime * 3.4) + wobble * 6.0);
    float streak = smoothstep(0.82, 1.0, band) * (0.55 + 0.45 * sin(across * 40.0 + uTime));
    vec3 colour = mix(uDeep, uWater, 0.28 + bank * 0.32 + streak * 0.55);
    float alpha = mix(0.86, 0.94, uLight);
    gl_FragColor = vec4(colour, alpha);
    #include <colorspace_fragment>
}`;

export function makeWater(
  length: number,
  width: number,
  deep: Color,
  water: Color,
  light: boolean,
) {
  const material = new ShaderMaterial({
    uniforms: {
      uTime: { value: 0 },
      uDeep: { value: deep.clone() },
      uWater: { value: water.clone() },
      uLight: { value: light ? 1 : 0 },
    },
    vertexShader: vertex,
    fragmentShader: fragment,
    transparent: true,
    side: DoubleSide,
    depthWrite: false,
  });
  const mesh = new Mesh(new PlaneGeometry(length, width, 96, 12), material);
  mesh.rotation.x = -Math.PI / 2;
  return {
    mesh,
    tick(time: number) {
      material.uniforms.uTime.value = time;
    },
    recolour(nextDeep: Color, nextWater: Color, nextLight: boolean) {
      material.uniforms.uDeep.value.copy(nextDeep);
      material.uniforms.uWater.value.copy(nextWater);
      material.uniforms.uLight.value = nextLight ? 1 : 0;
    },
  };
}
