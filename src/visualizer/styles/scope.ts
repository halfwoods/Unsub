import { WAVEFORM_LENGTH } from "../feed";
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
import type { RenderInfo, ScopeMode, StyleRenderer } from "../styles";

const MAX_WIDTH = 1920;
/** Samples of the waveform shown across a trace: a window that starts at the trigger. */
const TRACE_SAMPLES = 128;
/** How long a trace takes to fade to a thousandth of itself: the screen's persistence. */
const PERSISTENCE_S = 0.5;
const MODE_FADE_S = 0.35;

const DECAY_FS = `${VERSION}
uniform sampler2D uPrev;
uniform vec2 uTexel;
uniform float uDecay, uFloor;
in vec2 vUv;
out vec4 o;
void main() {
  vec3 c = texture(uPrev, vUv).rgb;
  // Phosphor spreads a little as it fades.
  vec3 around = texture(uPrev, vUv + vec2(uTexel.x, 0.0)).rgb + texture(uPrev, vUv - vec2(uTexel.x, 0.0)).rgb
              + texture(uPrev, vUv + vec2(0.0, uTexel.y)).rgb + texture(uPrev, vUv - vec2(0.0, uTexel.y)).rgb;
  c = mix(c, 0.25 * around, 0.3);
  o = vec4(max(c * uDecay - uFloor, 0.0), 1.0);
}`;

/** One channel of the waveform as a line, starting at the trigger point. */
const TRACE_VS = `${VERSION}
uniform sampler2D uAudio;
uniform int uRow, uOffset;
uniform vec2 uAspect;
uniform float uX0, uX1, uY, uAmp, uWidth;
out float vSide;
out float vT;
out float vSpeed;
vec2 point(int i) {
  float t = float(i) / ${TRACE_SAMPLES - 1}.0;
  float s = texelFetch(uAudio, ivec2(clamp(uOffset + i, 0, 255), uRow), 0).r;
  return vec2(mix(uX0, uX1, t), uY + s * uAmp);
}
void main() {
  int i = gl_VertexID >> 1;
  float side = float((gl_VertexID & 1) * 2 - 1);
  vec2 p = point(i);
  vec2 tangent = normalize(point(min(i + 1, ${TRACE_SAMPLES - 1})) - point(max(i - 1, 0)) + vec2(1e-5));
  p += vec2(-tangent.y, tangent.x) * side * uWidth;
  gl_Position = vec4(p / uAspect, 0.0, 1.0);
  vSide = side;
  vT = float(i) / ${TRACE_SAMPLES - 1}.0;
  vSpeed = 0.0;
}`;

/** The left channel against the right: a closed shape for a stereo image, a diagonal for mono. */
const XY_VS = `${VERSION}
uniform sampler2D uAudio;
uniform vec2 uAspect, uCentre;
uniform float uScale, uWidth;
out float vSide;
out float vT;
out float vSpeed;
vec2 point(int i) {
  i = clamp(i, 0, 255);
  // Rotated a quarter turn so a mono signal runs up the middle, as on a vectorscope.
  float l = texelFetch(uAudio, ivec2(i, 0), 0).r;
  float r = texelFetch(uAudio, ivec2(i, 1), 0).r;
  return uCentre + vec2(l - r, l + r) * 0.7071 * uScale;
}
void main() {
  int i = gl_VertexID >> 1;
  float side = float((gl_VertexID & 1) * 2 - 1);
  vec2 p = point(i);
  vec2 before = point(i - 1);
  vec2 after = point(i + 1);
  vec2 tangent = normalize(after - before + vec2(1e-5));
  p += vec2(-tangent.y, tangent.x) * side * uWidth;
  gl_Position = vec4(p / uAspect, 0.0, 1.0);
  vSide = side;
  vT = float(i) / 255.0;
  // The beam is brighter where it moves slowly, as on a real tube.
  vSpeed = length(after - before);
}`;

const LINE_FS = `${VERSION}
uniform vec3 uColor;
uniform float uGain;
uniform float uSpeedKnee;
in float vSide;
in float vT;
in float vSpeed;
out vec4 o;
void main() {
  float edge = 1.0 - abs(vSide);
  float beam = edge * edge;
  float bright = uSpeedKnee > 0.0 ? clamp(uSpeedKnee / (vSpeed + 0.004), 0.12, 1.0) : 1.0;
  o = vec4(uColor * beam * bright * uGain, 1.0);
}`;

const PRESENT_FS = `${VERSION}
${COMMON_GLSL}
uniform sampler2D uTex;
uniform vec2 uAspect, uTexel;
uniform float uGridGain;
in vec2 vUv;
out vec4 o;

float grid(vec2 p, float divisions) {
  vec2 g = abs(fract(p * divisions + 0.5) - 0.5) / fwidth(p * divisions);
  return 1.0 - min(min(g.x, g.y), 1.0);
}
void main() {
  vec3 c = texture(uTex, vUv).rgb;
  // A halo: the phosphor glows past the line.
  vec3 halo = vec3(0.0);
  for (int i = 0; i < 8; i++) {
    float a = float(i) * TAU / 8.0;
    halo += texture(uTex, vUv + vec2(cos(a), sin(a)) * uTexel * 5.0).rgb;
  }
  c += halo * 0.07;
  c = 1.0 - exp(-c * 1.5);
  vec2 p = (vUv - 0.5) * uAspect * 2.0;
  float lines = grid(p * 0.5, 8.0) * 0.5 + grid(p * 0.5, 40.0) * 0.12;
  c += vec3(0.02, 0.12, 0.05) * lines * uGridGain;
  vec2 q = vUv - 0.5;
  c *= 1.0 - dot(q, q) * 1.3;
  c += vec3(0.0, 0.012, 0.005);
  // Faint scanlines.
  c *= 0.94 + 0.06 * sin(vUv.y * 1080.0 * PI);
  o = vec4(c, 1.0);
}`;

/**
 * An oscilloscope with a long-persistence green phosphor: the two channels as traces that wait
 * for a rising edge so they hold still, the left against the right as a Lissajous figure, or both.
 * Lines are drawn into a buffer that fades slowly instead of clearing, which is where the glow
 * and the afterimage come from.
 */
export class ScopeStyle implements StyleRenderer {
  private readonly decay: Program;
  private readonly trace: Program;
  private readonly xy: Program;
  private readonly present: Program;
  private screen: PingPong | null = null;
  private waveGain = 1;
  private xyGain = 1;
  /** Slow measure of how big the signal is, so quiet music still fills the screen. */
  private level = 0.3;
  private readonly green = new Float32Array([0.16, 1.0, 0.38]);
  private readonly cyan = new Float32Array([0.2, 0.9, 0.85]);

  constructor(private readonly host: StyleHost) {
    const gl = host.gl;
    this.decay = new Program(gl, FULLSCREEN_VS, DECAY_FS);
    this.trace = new Program(gl, TRACE_VS, LINE_FS);
    this.xy = new Program(gl, XY_VS, LINE_FS);
    this.present = new Program(gl, FULLSCREEN_VS, PRESENT_FS);
  }

  resize(width: number, height: number): void {
    this.screen?.dispose();
    const scale = Math.min(1, MAX_WIDTH / width);
    this.screen = new PingPong(
      this.host.gl,
      Math.max(2, Math.round(width * scale)),
      Math.max(2, Math.round(height * scale)),
      this.host.feedbackFormat,
    );
  }

  render(info: RenderInfo): void {
    const { gl, emptyVao, scene } = this.host;
    const screen = this.screen;
    if (!screen) return;
    const { audio, dt } = info;
    const aspect = info.width / info.height;
    const mode = effectiveMode(info.scopeMode, aspect);
    const fade = 1 - Math.exp(-dt / (MODE_FADE_S / 3));
    this.waveGain += ((mode === "xy" ? 0 : 1) - this.waveGain) * fade;
    this.xyGain += ((mode === "wave" ? 0 : 1) - this.xyGain) * fade;

    gl.bindVertexArray(emptyVao);
    // 1. Let what is on the screen fade.
    screen.write.bind();
    this.decay.use();
    bindTexture(gl, 0, screen.read.texture);
    gl.uniform1i(this.decay.u("uPrev"), 0);
    gl.uniform2f(this.decay.u("uTexel"), 1 / screen.width, 1 / screen.height);
    gl.uniform1f(this.decay.u("uDecay"), Math.pow(0.001, dt / PERSISTENCE_S));
    gl.uniform1f(this.decay.u("uFloor"), this.host.feedbackIs8Bit ? 2 / 255 : 0.0003);
    gl.drawArrays(gl.TRIANGLES, 0, 3);

    // 2. Draw the beam.
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE);
    const loud = 0.8 + 0.5 * audio.rms;
    let peak = 0;
    for (let i = 0; i < WAVEFORM_LENGTH; i++) {
      peak = Math.max(peak, Math.abs(audio.waveL[i]), Math.abs(audio.waveR[i]));
    }
    // Rises quickly, falls over a few seconds.
    this.level += (peak - this.level) * (1 - Math.exp(-dt / (peak > this.level ? 0.05 : 2.5)));
    const zoom = Math.min(3.5, 0.75 / Math.max(this.level, 0.2));
    if (this.waveGain > 0.01) {
      const trigger = risingEdge(audio.waveL);
      const x1 = mode === "both" ? aspect * 0.94 : aspect * 0.92;
      const x0 = mode === "both" ? 0.08 : -aspect * 0.92;
      this.drawTrace(0, trigger, aspect, x0, x1, 0.46, 0.3 * zoom, this.green, loud * this.waveGain);
      this.drawTrace(1, trigger, aspect, x0, x1, -0.46, 0.3 * zoom, this.green, loud * this.waveGain);
    }
    if (this.xyGain > 0.01) {
      this.xy.use();
      bindTexture(gl, 0, this.host.audioTexture);
      gl.uniform1i(this.xy.u("uAudio"), 0);
      gl.uniform2f(this.xy.u("uAspect"), aspect, 1);
      const both = mode === "both";
      gl.uniform2f(this.xy.u("uCentre"), both ? -aspect * 0.46 : 0, 0);
      gl.uniform1f(this.xy.u("uScale"), (both ? 0.78 : 0.9) * Math.min(zoom, 1.9));
      gl.uniform1f(this.xy.u("uWidth"), 0.0042);
      gl.uniform3fv(this.xy.u("uColor"), this.cyan);
      gl.uniform1f(this.xy.u("uGain"), 0.9 * loud * this.xyGain);
      gl.uniform1f(this.xy.u("uSpeedKnee"), 0.012);
      gl.drawArrays(gl.TRIANGLE_STRIP, 0, WAVEFORM_LENGTH * 2);
    }
    resetState(gl);
    screen.swap();

    // 3. Through the glass.
    scene.bind();
    this.present.use();
    bindTexture(gl, 0, screen.read.texture);
    gl.uniform1i(this.present.u("uTex"), 0);
    gl.uniform2f(this.present.u("uAspect"), aspect, 1);
    gl.uniform2f(this.present.u("uTexel"), 1 / screen.width, 1 / screen.height);
    gl.uniform1f(this.present.u("uGridGain"), 0.6);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }

  private drawTrace(
    row: number,
    offset: number,
    aspect: number,
    x0: number,
    x1: number,
    y: number,
    amp: number,
    color: Float32Array,
    gain: number,
  ): void {
    const gl = this.host.gl;
    this.trace.use();
    bindTexture(gl, 0, this.host.audioTexture);
    gl.uniform1i(this.trace.u("uAudio"), 0);
    gl.uniform1i(this.trace.u("uRow"), row);
    gl.uniform1i(this.trace.u("uOffset"), offset);
    gl.uniform2f(this.trace.u("uAspect"), aspect, 1);
    gl.uniform1f(this.trace.u("uX0"), x0);
    gl.uniform1f(this.trace.u("uX1"), x1);
    gl.uniform1f(this.trace.u("uY"), y);
    gl.uniform1f(this.trace.u("uAmp"), amp);
    gl.uniform1f(this.trace.u("uWidth"), 0.0045);
    gl.uniform3fv(this.trace.u("uColor"), color);
    gl.uniform1f(this.trace.u("uGain"), gain);
    gl.uniform1f(this.trace.u("uSpeedKnee"), 0);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, TRACE_SAMPLES * 2);
  }

  dispose(): void {
    this.screen?.dispose();
    this.decay.dispose();
    this.trace.dispose();
    this.xy.dispose();
    this.present.dispose();
  }
}

/** Both side by side needs room to put them side by side. */
function effectiveMode(mode: ScopeMode, aspect: number): ScopeMode {
  return mode === "both" && aspect < 1.3 ? "xy" : mode;
}

/**
 * Where to start drawing so the trace holds still: the first place in the first half where the
 * signal crosses zero going up, as an oscilloscope's trigger does.
 */
export function risingEdge(wave: Float32Array): number {
  const limit = WAVEFORM_LENGTH - TRACE_SAMPLES;
  for (let i = 1; i < limit; i++) {
    if (wave[i - 1] < 0 && wave[i] >= 0) return i;
  }
  return 0;
}
