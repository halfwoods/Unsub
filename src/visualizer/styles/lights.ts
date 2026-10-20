import { COMMON_GLSL, FULLSCREEN_VS, Program, VERSION, resetState, type StyleHost } from "../gl";
import type { RenderInfo, StyleRenderer } from "../styles";
import { hsv } from "./classic";

const PARTICLES = 16384;
const EMITTERS = 5;
/** Floats per particle: position, velocity, life, seed. */
const STRIDE = 6;
const BYTES = STRIDE * 4;
/** The glow pass draws a quarter of the particles, much bigger and fainter. */
const GLOW_SHARE = 4;

const UPDATE_VS = `${VERSION}
${COMMON_GLSL}
layout(location = 0) in vec4 aPosVel;
layout(location = 1) in vec2 aLife;
uniform float uDt, uTime, uKick, uBeat, uAspect;
uniform vec3 uEmit[${EMITTERS}];
out vec4 vPosVel;
out vec2 vLife;

void main() {
  vec2 pos = aPosVel.xy;
  vec2 vel = aPosVel.zw;
  float life = aLife.x;
  float seed = aLife.y;
  int e = int(floor(hash11(seed * 91.7) * float(${EMITTERS})));
  vec3 em = uEmit[e];
  life -= uDt * mix(0.22, 0.55, hash11(seed * 7.13));
  if (life <= 0.0) {
    // Born at an emitter, thrown outward harder on a beat.
    seed = hash21(vec2(seed, uTime));
    int e2 = int(floor(hash11(seed * 91.7) * float(${EMITTERS})));
    vec3 born = uEmit[e2];
    vec2 h = hash22(vec2(seed * 31.7, float(gl_VertexID) * 0.013 + uTime));
    float a = h.x * TAU;
    float r = sqrt(h.y) * 0.05;
    pos = born.xy + vec2(cos(a), sin(a)) * r;
    float speed = (0.05 + 0.4 * hash11(seed * 3.3)) * (0.5 + 1.8 * uBeat + 0.6 * born.z);
    float b = hash11(seed * 5.9) * TAU;
    vel = vec2(cos(b), sin(b)) * speed;
    life = 0.6 + 0.4 * hash11(seed * 17.0);
  } else {
    vec2 d = em.xy - pos;
    float r2 = dot(d, d) + 0.02;
    // Pulled towards the emitter and swirled around it; the pull is what the music strengthens.
    vec2 acc = d / r2 * (0.07 + 0.2 * em.z) + vec2(-d.y, d.x) / r2 * (0.06 + 0.12 * em.z);
    // A push outward when the beat lands.
    vec2 out_ = pos - em.xy;
    acc += out_ / (length(out_) + 0.05) * uKick * 7.0;
    vel += acc * uDt;
    vel *= exp(-uDt * 0.9);
    pos += vel * uDt;
    // Anything that wanders off is brought back in on the other side.
    pos.x = abs(pos.x) > uAspect * 1.15 ? -sign(pos.x) * uAspect * 1.1 : pos.x;
    pos.y = abs(pos.y) > 1.15 ? -sign(pos.y) * 1.1 : pos.y;
  }
  vPosVel = vec4(pos, vel);
  vLife = vec2(life, seed);
  gl_Position = vec4(0.0);
}`;

const UPDATE_FS = `${VERSION}
out vec4 o;
void main() { o = vec4(0.0); }`;

const DRAW_VS = `${VERSION}
${COMMON_GLSL}
layout(location = 0) in vec4 aPosVel;
layout(location = 1) in vec2 aLife;
uniform float uAspect, uSize, uGlow, uHue, uBeat, uRms;
out vec2 vCorner;
out vec3 vColor;
out float vAlpha;

void main() {
  vec2 corner = vec2(float(gl_VertexID & 1), float(gl_VertexID >> 1)) * 2.0 - 1.0;
  float life = aLife.x;
  float seed = aLife.y;
  vec2 vel = aPosVel.zw;
  float speed = length(vel);
  vec2 dir = speed > 1e-4 ? vel / speed : vec2(1.0, 0.0);
  float born = smoothstep(1.0, 0.9, life);
  float dying = smoothstep(0.0, 0.3, life);
  float size = uSize * (0.55 + 0.9 * hash11(seed * 13.1)) * (0.4 + 0.6 * life) * (1.0 + 0.7 * uBeat);
  size *= uGlow > 0.5 ? 7.0 : 1.0;
  float stretch = 1.0 + (uGlow > 0.5 ? 0.3 : 2.5) * min(speed * 2.5, 2.0);
  vec2 offset = (dir * corner.x * stretch + vec2(-dir.y, dir.x) * corner.y) * size;
  vec2 world = aPosVel.xy + offset;
  gl_Position = vec4(world.x / uAspect, world.y, 0.0, 1.0);
  float hue = fract(uHue + hash11(seed * 3.7) * 0.28 + float(int(floor(hash11(seed * 91.7) * float(${EMITTERS})))) * 0.11);
  vColor = hsv2rgb(vec3(hue, mix(0.85, 0.35, hash11(seed * 9.1) * 0.5), 1.0));
  vAlpha = born * dying * (uGlow > 0.5 ? 0.075 : 1.0) * (0.75 + 0.6 * uRms + 0.5 * uBeat);
  vCorner = corner;
}`;

const DRAW_FS = `${VERSION}
in vec2 vCorner;
in vec3 vColor;
in float vAlpha;
out vec4 o;
void main() {
  float d = dot(vCorner, vCorner);
  if (d > 1.0) discard;
  float soft = exp(-d * 3.2);
  float core = exp(-d * 12.0);
  o = vec4(vColor * (soft * 0.6 + core * 0.9) * vAlpha, 1.0);
}`;

const BACKDROP_FS = `${VERSION}
${COMMON_GLSL}
uniform vec2 uAspect;
uniform vec3 uEmit[${EMITTERS}];
uniform vec3 uColors[${EMITTERS}];
uniform float uBeat, uTime;
in vec2 vUv;
out vec4 o;
void main() {
  vec2 p = (vUv - 0.5) * vec2(uAspect.x * 2.0, 2.0);
  float r = length(vUv - 0.5);
  vec3 c = mix(vec3(0.012, 0.014, 0.03), vec3(0.0, 0.0, 0.006), smoothstep(0.0, 0.7, r));
  for (int k = 0; k < ${EMITTERS}; k++) {
    vec2 d = p - uEmit[k].xy;
    float g = exp(-dot(d, d) * (6.0 + 3.0 * float(k % 2)));
    c += uColors[k] * g * (0.05 + 0.4 * uEmit[k].z * (0.4 + uBeat));
  }
  o = vec4(c, 1.0);
}`;

/**
 * Thousands of points of light, kept in orbit round a few emitters that wander the screen and
 * pulse with the music. A beat throws the points outward; between beats they curl back in. The
 * whole simulation runs on the GPU: positions are advanced by a vertex shader that writes its
 * results back to a buffer (transform feedback), and then drawn as instanced, stretched, glowing
 * quads.
 */
export class LightsStyle implements StyleRenderer {
  private readonly update: Program;
  private readonly draw: Program;
  private readonly backdrop: Program;
  private readonly buffers: [WebGLBuffer, WebGLBuffer];
  private readonly updateVaos: [WebGLVertexArrayObject, WebGLVertexArrayObject];
  private readonly drawVaos: [WebGLVertexArrayObject, WebGLVertexArrayObject];
  private readonly feedback: WebGLTransformFeedback;
  private source = 0;
  private hue = 0.58;
  private hueKick = 0;
  private lastBeat = -1;
  private readonly emitters = new Float32Array(EMITTERS * 3);
  private readonly colors = new Float32Array(EMITTERS * 3);
  private readonly scratch = new Float32Array(3);

  constructor(private readonly host: StyleHost) {
    const gl = host.gl;
    this.update = new Program(gl, UPDATE_VS, UPDATE_FS, ["vPosVel", "vLife"]);
    this.draw = new Program(gl, DRAW_VS, DRAW_FS);
    this.backdrop = new Program(gl, FULLSCREEN_VS, BACKDROP_FS);

    // Every particle starts dead, so the first update gives it a place and a life.
    const initial = new Float32Array(PARTICLES * STRIDE);
    for (let i = 0; i < PARTICLES; i++) {
      initial[i * STRIDE + 4] = 0;
      initial[i * STRIDE + 5] = (i + 0.5) / PARTICLES;
    }
    const a = gl.createBuffer();
    const b = gl.createBuffer();
    const feedback = gl.createTransformFeedback();
    if (!a || !b || !feedback) throw new Error("could not create the particle buffers");
    this.buffers = [a, b];
    this.feedback = feedback;
    for (const buffer of this.buffers) {
      gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
      gl.bufferData(gl.ARRAY_BUFFER, initial, gl.DYNAMIC_COPY);
    }
    this.updateVaos = [this.vao(a, false), this.vao(b, false)];
    this.drawVaos = [this.vao(a, true), this.vao(b, true)];
    gl.bindBuffer(gl.ARRAY_BUFFER, null);
    gl.bindVertexArray(null);
  }

  resize(): void {}

  render(info: RenderInfo): void {
    const { gl, emptyVao, scene } = this.host;
    const { audio, time, dt } = info;
    const aspect = info.width / info.height;

    if (audio.beatCount !== this.lastBeat) {
      if (this.lastBeat >= 0) this.hueKick += 0.045;
      this.lastBeat = audio.beatCount;
    }
    this.hue += dt * 0.01 + (this.hueKick > 0 ? Math.min(this.hueKick, dt * 0.6) : 0);
    this.hueKick = Math.max(0, this.hueKick - dt * 0.6);
    this.placeEmitters(audio.bands, audio.beat, time, aspect);

    // Advance the particles: read one buffer, write the other.
    const destination = 1 - this.source;
    this.update.use();
    gl.uniform1f(this.update.u("uDt"), Math.min(dt, 0.05));
    gl.uniform1f(this.update.u("uTime"), time);
    gl.uniform1f(this.update.u("uKick"), audio.kick);
    gl.uniform1f(this.update.u("uBeat"), audio.beat);
    gl.uniform1f(this.update.u("uAspect"), aspect);
    gl.uniform3fv(this.update.u("uEmit"), this.emitters);
    gl.bindVertexArray(this.updateVaos[this.source]);
    gl.bindTransformFeedback(gl.TRANSFORM_FEEDBACK, this.feedback);
    gl.bindBufferBase(gl.TRANSFORM_FEEDBACK_BUFFER, 0, this.buffers[destination]);
    gl.enable(gl.RASTERIZER_DISCARD);
    gl.beginTransformFeedback(gl.POINTS);
    gl.drawArrays(gl.POINTS, 0, PARTICLES);
    gl.endTransformFeedback();
    gl.disable(gl.RASTERIZER_DISCARD);
    gl.bindBufferBase(gl.TRANSFORM_FEEDBACK_BUFFER, 0, null);
    gl.bindTransformFeedback(gl.TRANSFORM_FEEDBACK, null);
    this.source = destination;

    // Draw them.
    scene.bind();
    gl.bindVertexArray(emptyVao);
    this.backdrop.use();
    gl.uniform2f(this.backdrop.u("uAspect"), aspect, 1);
    gl.uniform3fv(this.backdrop.u("uEmit"), this.emitters);
    gl.uniform3fv(this.backdrop.u("uColors"), this.colors);
    gl.uniform1f(this.backdrop.u("uBeat"), audio.beat);
    gl.uniform1f(this.backdrop.u("uTime"), time);
    gl.drawArrays(gl.TRIANGLES, 0, 3);

    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE);
    this.draw.use();
    gl.bindVertexArray(this.drawVaos[this.source]);
    gl.uniform1f(this.draw.u("uAspect"), aspect);
    gl.uniform1f(this.draw.u("uSize"), 0.0085);
    gl.uniform1f(this.draw.u("uHue"), this.hue);
    gl.uniform1f(this.draw.u("uBeat"), audio.beat);
    gl.uniform1f(this.draw.u("uRms"), audio.rms);
    gl.uniform1f(this.draw.u("uGlow"), 1);
    gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, PARTICLES / GLOW_SHARE);
    gl.uniform1f(this.draw.u("uGlow"), 0);
    gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, PARTICLES);
    resetState(gl);
    gl.bindVertexArray(null);
  }

  dispose(): void {
    const gl = this.host.gl;
    this.update.dispose();
    this.draw.dispose();
    this.backdrop.dispose();
    for (const buffer of this.buffers) gl.deleteBuffer(buffer);
    for (const vao of [...this.updateVaos, ...this.drawVaos]) gl.deleteVertexArray(vao);
    gl.deleteTransformFeedback(this.feedback);
  }

  private vao(buffer: WebGLBuffer, instanced: boolean): WebGLVertexArrayObject {
    const gl = this.host.gl;
    const vao = gl.createVertexArray();
    if (!vao) throw new Error("could not create a vertex array");
    gl.bindVertexArray(vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 4, gl.FLOAT, false, BYTES, 0);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 2, gl.FLOAT, false, BYTES, 16);
    if (instanced) {
      gl.vertexAttribDivisor(0, 1);
      gl.vertexAttribDivisor(1, 1);
    }
    return vao;
  }

  /** Emitters wander on slow Lissajous paths, spreading outward on a beat; each listens to a slice. */
  private placeEmitters(bands: Float32Array, beat: number, time: number, aspect: number): void {
    const slice = Math.floor(bands.length / EMITTERS);
    for (let k = 0; k < EMITTERS; k++) {
      let energy = 0;
      for (let i = k * slice; i < (k + 1) * slice; i++) energy += bands[i];
      energy /= slice;
      const spread = 1 + 0.35 * beat;
      const phase = k * 1.37;
      this.emitters[k * 3] = aspect * 0.55 * spread * Math.sin(time * (0.21 + 0.043 * k) + phase * 1.9);
      this.emitters[k * 3 + 1] = 0.5 * spread * Math.sin(time * (0.17 + 0.037 * k) + phase * 2.3);
      this.emitters[k * 3 + 2] = Math.min(1, energy * 2.2);
      hsv(this.hue + k * 0.11, 0.8, 1, this.scratch);
      this.colors.set(this.scratch, k * 3);
    }
  }
}
