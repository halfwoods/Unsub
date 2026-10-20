import { WAVEFORM_LENGTH, BAND_COUNT } from "../feed";
import {
  COMMON_GLSL,
  FULLSCREEN_VS,
  PingPong,
  Program,
  VERSION,
  bindTexture,
  resetState,
  type StyleHost,
} from "../gl";
import type { RenderInfo, StyleRenderer } from "../styles";

/** The feedback buffer is at most this wide: trails are soft, so they do not need more. */
const MAX_FEEDBACK_WIDTH = 1280;
const SECTOR_CHOICES = [6, 4, 8, 5, 6, 10];
const BEATS_PER_SECTOR_CHANGE = 32;

const WARP_FS = `${VERSION}
${COMMON_GLSL}
uniform sampler2D uPrev;
uniform vec2 uAspect, uTexel;
uniform float uTime, uZoom, uRot, uSwirl, uFade, uHue, uSectors, uFloor;
in vec2 vUv;
out vec4 o;

vec3 hueRotate(vec3 c, float a) {
  // Rotates chroma in YIQ space: cheap and keeps brightness.
  const mat3 toYiq = mat3(0.299, 0.596, 0.211, 0.587, -0.274, -0.523, 0.114, -0.322, 0.312);
  const mat3 toRgb = mat3(1.0, 1.0, 1.0, 0.956, -0.272, -1.106, 0.621, -0.647, 1.703);
  vec3 q = toYiq * c;
  float cs = cos(a), sn = sin(a);
  q.yz = vec2(q.y * cs - q.z * sn, q.y * sn + q.z * cs);
  return toRgb * q;
}

void main() {
  vec2 p = (vUv - 0.5) * uAspect;
  float r = length(p);
  // Fold the plane into mirrored wedges, so everything drawn is reflected around the centre.
  float seg = TAU / uSectors;
  float a = atan(p.y, p.x);
  a = abs(mod(a + seg * 0.5, seg) - seg * 0.5);
  p = r * vec2(cos(a), sin(a));
  // Zoom out of the picture, turn it, and swirl it more towards the edge.
  p = rot(uRot + uSwirl * r) * p * uZoom;
  // A slow current, so trails bend instead of running straight.
  vec2 flow = vec2(vnoise(p * 2.3 + uTime * 0.11), vnoise(p * 2.3 - uTime * 0.13 + 7.0)) - 0.5;
  p += flow * 0.006;
  vec2 uv = p / uAspect + 0.5;
  vec3 c = texture(uPrev, uv).rgb;
  // A touch of blur keeps the trails from turning to pixels.
  vec3 around = texture(uPrev, uv + vec2(uTexel.x, 0.0)).rgb + texture(uPrev, uv - vec2(uTexel.x, 0.0)).rgb
              + texture(uPrev, uv + vec2(0.0, uTexel.y)).rgb + texture(uPrev, uv - vec2(0.0, uTexel.y)).rgb;
  c = mix(c, 0.25 * around, 0.35);
  c = hueRotate(c, uHue);
  // Whatever is drawn at the centre is drawn there every frame, so the centre fades faster or it
  // would pile up to white.
  float fade = uFade - 0.06 * exp(-r * r * 38.0);
  c = max(c * fade - uFloor, 0.0);
  o = vec4(c, 1.0);
}`;

const RIBBON_VS = `${VERSION}
uniform sampler2D uAudio;
uniform int uRow;
uniform vec2 uAspect;
uniform float uAmp, uWidth, uY, uXSpan;
out float vSide;
out float vT;

vec2 point(int i) {
  float t = float(i) / 255.0;
  return vec2((t * 2.0 - 1.0) * uXSpan * uAspect.x, texelFetch(uAudio, ivec2(clamp(i, 0, 255), uRow), 0).r * uAmp + uY);
}
void main() {
  int i = gl_VertexID >> 1;
  float side = float((gl_VertexID & 1) * 2 - 1);
  vec2 p = point(i);
  vec2 tangent = normalize(point(min(i + 1, 255)) - point(max(i - 1, 0)) + vec2(1e-5));
  vec2 n = vec2(-tangent.y, tangent.x);
  p += n * side * uWidth;
  gl_Position = vec4(p / uAspect, 0.0, 1.0);
  vSide = side;
  vT = float(i) / 255.0;
}`;

const RIBBON_FS = `${VERSION}
uniform vec3 uColor;
uniform float uGain;
in float vSide;
in float vT;
out vec4 o;
void main() {
  float edge = 1.0 - abs(vSide);
  float a = edge * edge;
  float ends = smoothstep(0.0, 0.08, vT) * smoothstep(1.0, 0.92, vT);
  o = vec4(uColor * a * ends * uGain, 1.0);
}`;

const RADIAL_VS = `${VERSION}
${COMMON_GLSL}
uniform sampler2D uAudio;
uniform vec2 uAspect;
uniform float uInner, uLength, uRotation;
out float vWhere;
out float vAngle;
void main() {
  int k = gl_VertexID >> 1;        // 0..128 around the ring
  bool outer = (gl_VertexID & 1) == 1;
  int band = k < 64 ? k : 128 - k; // mirrored halves
  float level = texelFetch(uAudio, ivec2(min(band, 63), 2), 0).r;
  float angle = float(k) / 128.0 * TAU + uRotation;
  float r = uInner + (outer ? level * uLength : 0.0);
  gl_Position = vec4(vec2(cos(angle), sin(angle)) * r / uAspect, 0.0, 1.0);
  vWhere = outer ? 1.0 : 0.0;
  vAngle = float(k) / 128.0;
}`;

const RADIAL_FS = `${VERSION}
${COMMON_GLSL}
uniform vec3 uColorA, uColorB;
uniform float uGain;
in float vWhere;
in float vAngle;
out vec4 o;
void main() {
  vec3 c = mix(uColorA, uColorB, 0.5 + 0.5 * sin(vAngle * TAU * 2.0));
  o = vec4(c * (1.0 - vWhere * 0.75) * uGain, 1.0);
}`;

const ORB_FS = `${VERSION}
uniform vec2 uAspect;
uniform float uRadius, uGain;
uniform vec3 uColor;
in vec2 vUv;
out vec4 o;
void main() {
  vec2 p = (vUv - 0.5) * uAspect;
  float d = length(p) / uRadius;
  float g = exp(-d * d * 2.5) + 0.35 * exp(-d * 1.2);
  o = vec4(uColor * g * uGain, 1.0);
}`;

const PRESENT_FS = `${VERSION}
${COMMON_GLSL}
uniform sampler2D uTex;
uniform float uFlare;
in vec2 vUv;
out vec4 o;
void main() {
  vec3 c = texture(uTex, vUv).rgb;
  // Fold bright values over instead of clipping them.
  c = 1.0 - exp(-c * 1.35);
  vec2 q = vUv - 0.5;
  c *= 1.0 - 0.55 * dot(q, q) * 1.6;
  c += uFlare * vec3(0.05, 0.04, 0.08) * (1.0 - length(q) * 1.4);
  o = vec4(c, 1.0);
}`;

/**
 * Flowing, mirrored colour trails: each frame the last is zoomed, turned and folded into wedges,
 * faded a little, and drawn over with the waveform, a ring of the spectrum and a glow in the
 * middle. Colours drift slowly round the wheel and jump on a beat.
 */
export class ClassicStyle implements StyleRenderer {
  private readonly warp: Program;
  private readonly ribbon: Program;
  private readonly radial: Program;
  private readonly orb: Program;
  private readonly present: Program;
  private feedback: PingPong | null = null;
  private lastBeat = -1;
  private hue = Math.random();
  private hueTarget = 0;
  private hueOffset = 0;
  private rotation = 0;
  private sectors = SECTOR_CHOICES[0];
  private readonly colorA = new Float32Array(3);
  private readonly colorB = new Float32Array(3);
  private readonly colorC = new Float32Array(3);

  constructor(private readonly host: StyleHost) {
    const gl = host.gl;
    this.warp = new Program(gl, FULLSCREEN_VS, WARP_FS);
    this.ribbon = new Program(gl, RIBBON_VS, RIBBON_FS);
    this.radial = new Program(gl, RADIAL_VS, RADIAL_FS);
    this.orb = new Program(gl, FULLSCREEN_VS, ORB_FS);
    this.present = new Program(gl, FULLSCREEN_VS, PRESENT_FS);
  }

  resize(width: number, height: number): void {
    this.feedback?.dispose();
    const scale = Math.min(1, MAX_FEEDBACK_WIDTH / width);
    this.feedback = new PingPong(
      this.host.gl,
      Math.max(2, Math.round(width * scale)),
      Math.max(2, Math.round(height * scale)),
      this.host.feedbackFormat,
    );
  }

  render(info: RenderInfo): void {
    const { gl, emptyVao, scene } = this.host;
    const feedback = this.feedback;
    if (!feedback) return;
    const { audio, time, dt } = info;
    const aspect = info.width / info.height;

    if (audio.beatCount !== this.lastBeat) {
      if (this.lastBeat >= 0) {
        this.hueTarget += 0.06;
        if (audio.beatCount % BEATS_PER_SECTOR_CHANGE === 0) {
          this.sectors = SECTOR_CHOICES[(audio.beatCount / BEATS_PER_SECTOR_CHANGE) % SECTOR_CHOICES.length];
        }
      }
      this.lastBeat = audio.beatCount;
    }
    this.hueOffset += (this.hueTarget - this.hueOffset) * (1 - Math.exp(-dt / 0.12));
    this.hue += dt * 0.012;
    const hue = this.hue + this.hueOffset;
    hsv(hue, 0.75, 1, this.colorA);
    hsv(hue + 0.33, 0.7, 1, this.colorB);
    hsv(hue + 0.62, 0.55, 1, this.colorC);
    this.rotation += dt * (0.1 + 0.5 * audio.mid);

    gl.bindVertexArray(emptyVao);

    // 1. The last frame, moved and faded.
    feedback.write.bind();
    this.warp.use();
    bindTexture(gl, 0, feedback.read.texture);
    gl.uniform1i(this.warp.u("uPrev"), 0);
    gl.uniform2f(this.warp.u("uAspect"), aspect, 1);
    gl.uniform2f(this.warp.u("uTexel"), 1 / feedback.width, 1 / feedback.height);
    gl.uniform1f(this.warp.u("uTime"), time);
    const perFrame = dt * 60;
    gl.uniform1f(this.warp.u("uZoom"), 1 - (0.0045 + 0.012 * audio.beat + 0.006 * audio.low) * perFrame);
    gl.uniform1f(this.warp.u("uRot"), (0.002 + 0.004 * Math.sin(time * 0.17) + 0.006 * audio.mid) * perFrame);
    gl.uniform1f(this.warp.u("uSwirl"), 0.02 + 0.05 * audio.high);
    const fade = 0.955 + 0.03 * Math.min(1, audio.rms * 2);
    gl.uniform1f(this.warp.u("uFade"), Math.pow(fade, perFrame));
    gl.uniform1f(this.warp.u("uHue"), 0.012 * perFrame + audio.kick * 0.05);
    gl.uniform1f(this.warp.u("uSectors"), this.sectors);
    gl.uniform1f(this.warp.u("uFloor"), this.host.feedbackIs8Bit ? 1.5 / 255 : 0.0002);
    gl.drawArrays(gl.TRIANGLES, 0, 3);

    // 2. What is playing, added on top.
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE);
    const wide = 0.007 + 0.012 * audio.rms + 0.01 * audio.beat;
    this.drawRibbon(0, aspect, 0.3 + 0.2 * audio.rms, wide, 0.12, this.colorA, 0.9);
    this.drawRibbon(1, aspect, 0.3 + 0.2 * audio.rms, wide, -0.12, this.colorB, 0.9);

    this.radial.use();
    bindTexture(gl, 0, this.host.audioTexture);
    gl.uniform1i(this.radial.u("uAudio"), 0);
    gl.uniform2f(this.radial.u("uAspect"), aspect, 1);
    gl.uniform1f(this.radial.u("uInner"), 0.1 + 0.05 * audio.beat);
    gl.uniform1f(this.radial.u("uLength"), 0.32);
    gl.uniform1f(this.radial.u("uRotation"), this.rotation);
    gl.uniform3fv(this.radial.u("uColorA"), this.colorB);
    gl.uniform3fv(this.radial.u("uColorB"), this.colorC);
    gl.uniform1f(this.radial.u("uGain"), 0.42);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, (BAND_COUNT * 2 + 1) * 2);

    this.orb.use();
    gl.uniform2f(this.orb.u("uAspect"), aspect, 1);
    gl.uniform1f(this.orb.u("uRadius"), 0.05 + 0.035 * audio.beat + 0.05 * audio.kick);
    gl.uniform3fv(this.orb.u("uColor"), this.colorC);
    gl.uniform1f(this.orb.u("uGain"), 0.1 + 0.28 * audio.beat);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
    resetState(gl);
    feedback.swap();

    // 3. Shown.
    scene.bind();
    this.present.use();
    bindTexture(gl, 0, feedback.read.texture);
    gl.uniform1i(this.present.u("uTex"), 0);
    gl.uniform1f(this.present.u("uFlare"), audio.beat);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }

  private drawRibbon(
    row: number,
    aspect: number,
    amp: number,
    width: number,
    y: number,
    color: Float32Array,
    gain: number,
  ): void {
    const gl = this.host.gl;
    this.ribbon.use();
    bindTexture(gl, 0, this.host.audioTexture);
    gl.uniform1i(this.ribbon.u("uAudio"), 0);
    gl.uniform1i(this.ribbon.u("uRow"), row);
    gl.uniform2f(this.ribbon.u("uAspect"), aspect, 1);
    gl.uniform1f(this.ribbon.u("uAmp"), amp);
    gl.uniform1f(this.ribbon.u("uWidth"), width);
    gl.uniform1f(this.ribbon.u("uY"), y);
    gl.uniform1f(this.ribbon.u("uXSpan"), 0.92);
    gl.uniform3fv(this.ribbon.u("uColor"), color);
    gl.uniform1f(this.ribbon.u("uGain"), gain);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, WAVEFORM_LENGTH * 2);
  }

  dispose(): void {
    this.feedback?.dispose();
    this.warp.dispose();
    this.ribbon.dispose();
    this.radial.dispose();
    this.orb.dispose();
    this.present.dispose();
  }
}

/** HSV to RGB, into `out`. Hue wraps. */
export function hsv(h: number, s: number, v: number, out: Float32Array): void {
  const hh = (((h % 1) + 1) % 1) * 6;
  const c = v * s;
  const x = c * (1 - Math.abs((hh % 2) - 1));
  const m = v - c;
  const sector = Math.floor(hh);
  out[0] = m + (sector === 0 || sector === 5 ? c : sector === 1 || sector === 4 ? x : 0);
  out[1] = m + (sector === 1 || sector === 2 ? c : sector === 0 || sector === 3 ? x : 0);
  out[2] = m + (sector === 3 || sector === 4 ? c : sector === 2 || sector === 5 ? x : 0);
}
