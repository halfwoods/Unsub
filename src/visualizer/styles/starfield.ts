import { COMMON_GLSL, FULLSCREEN_VS, Program, VERSION, bindTexture, resetState, type StyleHost } from "../gl";
import type { RenderInfo, StyleRenderer } from "../styles";
import { hsv } from "./classic";

const STARS = 3200;

const BACKDROP_FS = `${VERSION}
uniform vec2 uAspect, uCentre;
uniform vec3 uTint;
uniform float uGlow;
in vec2 vUv;
out vec4 o;
void main() {
  vec2 p = (vUv - 0.5) * uAspect - uCentre * 0.4;
  float r = length(p);
  vec3 c = mix(vec3(0.006, 0.008, 0.02), vec3(0.0, 0.0, 0.004), smoothstep(0.0, 1.0, length((vUv - 0.5) * uAspect)));
  // A glow where the stars come from, in the colour of the sound.
  c += uTint * uGlow * exp(-r * r * 6.0);
  o = vec4(c, 1.0);
}`;

/**
 * Stars are not stored. Each is made from its number alone: a place in the field, a depth that
 * shrinks as the camera travels, and a band of the spectrum that it listens to. The streak behind
 * a star is the same star a little farther away, so it lengthens with the speed.
 */
const STAR_VS = `${VERSION}
${COMMON_GLSL}
uniform sampler2D uAudio;
uniform vec2 uAspect, uCam;
uniform float uTravel, uStreak, uSize, uRoll, uBright;
out vec2 vLocal;
flat out float vLength;
out vec3 vColor;
out float vAlpha;

// A star is 0 deep where it reaches the viewer and 1 deep where it begins.
vec2 project(vec2 lateral, float z) {
  return (lateral + uCam) / mix(0.035, 1.0, z);
}

void main() {
  float id = float(gl_InstanceID);
  vec3 h = vec3(hash11(id * 0.713 + 0.1), hash11(id * 1.377 + 7.3), hash11(id * 2.911 + 3.9));
  float pick = hash11(id * 5.31 + 1.7);
  float angle = h.x * TAU;
  vec2 lateral = rot(uRoll) * (vec2(cos(angle), sin(angle)) * mix(0.04, 1.0, sqrt(h.y)) * 0.85);
  float z = fract(h.z - uTravel);
  vec2 head = project(lateral, z);
  vec2 tail = project(lateral, min(z + uStreak, 1.0));

  float near = 1.0 - z;
  float r = uSize * (0.6 + 0.8 * pick) * (0.7 + 1.8 * near);
  vec2 axis = head - tail;
  float len = length(axis);
  vec2 t = len > 1e-5 ? axis / len : vec2(1.0, 0.0);
  vec2 n = vec2(-t.y, t.x);
  vec2 corner = vec2(float(gl_VertexID & 1), float(gl_VertexID >> 1)) * 2.0 - 1.0;
  vec2 pos = (corner.x > 0.0 ? head : tail) + t * corner.x * r + n * corner.y * r;
  gl_Position = vec4(pos / uAspect, 0.0, 1.0);
  vLocal = vec2(corner.x > 0.0 ? len / r + 1.0 : -1.0, corner.y);
  vLength = len / r;

  // Each star listens to one band, and takes its colour from where that band sits in the
  // spectrum: warm for the bass, cool for the treble. A loud band lights its stars.
  int band = int(floor(pick * 63.99));
  float level = texelFetch(uAudio, ivec2(band, 2), 0).r;
  vec3 tint = hsv2rgb(vec3(mix(0.0, 0.78, float(band) / 63.0), 0.85, 1.0));
  vColor = mix(vec3(1.0), tint, 0.25 + 0.5 * level);
  // Dim and small far away, so a star does not pop into being.
  vAlpha = smoothstep(1.0, 0.85, z) * (0.25 + 1.4 * near * near) * (0.4 + 1.1 * level) * uBright
         * (1.0 + min(len / r, 12.0) * 0.12);
}`;

const STAR_FS = `${VERSION}
in vec2 vLocal;
flat in float vLength;
in vec3 vColor;
in float vAlpha;
out vec4 o;
void main() {
  // How far this pixel is from the star's path, in radii.
  float along = clamp(vLocal.x, 0.0, vLength);
  float d = length(vec2(vLocal.x - along, vLocal.y));
  if (d > 1.0) discard;
  float core = exp(-d * d * 7.0);
  float halo = exp(-d * d * 1.8) * 0.3;
  o = vec4(vColor * (core + halo) * vAlpha, 1.0);
}`;

/**
 * Flying forward through a field of stars. The loudness sets the speed, each beat is a brief surge
 * that stretches the stars into streaks, and every star is tinted by, and lit by, one band of the
 * spectrum. The camera drifts and rolls slowly, and a glow where the stars come from takes the
 * colour of the sound.
 */
export class StarfieldStyle implements StyleRenderer {
  private readonly backdrop: Program;
  private readonly stars: Program;
  private travel = 0;
  private roll = 0;
  private readonly tint = new Float32Array(3);

  constructor(private readonly host: StyleHost) {
    this.backdrop = new Program(host.gl, FULLSCREEN_VS, BACKDROP_FS);
    this.stars = new Program(host.gl, STAR_VS, STAR_FS);
  }

  resize(): void {}

  render(info: RenderInfo): void {
    const { gl, emptyVao, scene, audioTexture } = this.host;
    const { audio, dt, time } = info;
    const aspect = info.width / info.height;

    // Slow when it is quiet, and a beat throws the camera forward for a moment.
    const speed = 0.04 + 0.34 * audio.energy + 0.8 * audio.beat;
    this.travel += dt * speed;
    this.roll += dt * (0.012 + 0.035 * audio.energy);
    const camX = 0.1 * Math.sin(time * 0.11);
    const camY = 0.07 * Math.sin(time * 0.17 + 1);

    // Where the sound sits in the spectrum, as a colour.
    const total = audio.low + audio.mid + audio.high + 1e-4;
    hsv(0.02 + 0.76 * ((0.5 * audio.mid + audio.high) / total), 0.7, 1, this.tint);

    scene.bind();
    gl.bindVertexArray(emptyVao);
    this.backdrop.use();
    gl.uniform2f(this.backdrop.u("uAspect"), aspect, 1);
    gl.uniform2f(this.backdrop.u("uCentre"), camX, camY);
    gl.uniform3fv(this.backdrop.u("uTint"), this.tint);
    gl.uniform1f(this.backdrop.u("uGlow"), 0.012 + 0.05 * audio.energy + 0.06 * audio.beat);
    gl.drawArrays(gl.TRIANGLES, 0, 3);

    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE);
    this.stars.use();
    bindTexture(gl, 0, audioTexture);
    gl.uniform1i(this.stars.u("uAudio"), 0);
    gl.uniform2f(this.stars.u("uAspect"), aspect, 1);
    gl.uniform2f(this.stars.u("uCam"), camX, camY);
    gl.uniform1f(this.stars.u("uTravel"), this.travel);
    gl.uniform1f(this.stars.u("uStreak"), 0.004 + 0.07 * speed);
    gl.uniform1f(this.stars.u("uSize"), 0.0035);
    gl.uniform1f(this.stars.u("uRoll"), this.roll);
    gl.uniform1f(this.stars.u("uBright"), 0.8 + 0.5 * audio.energy + 0.5 * audio.spark);
    gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, STARS);
    resetState(gl);
  }

  dispose(): void {
    this.backdrop.dispose();
    this.stars.dispose();
  }
}
