import { BAND_COUNT } from "../feed";
import { COMMON_GLSL, FULLSCREEN_VS, Program, VERSION, bindTexture, resetState, type StyleHost } from "../gl";
import type { RenderInfo, StyleRenderer } from "../styles";

/** The LCD's own colours (see src/theme/tokens.css), and a dark counterpart. */
const CREAM = {
  top: new Float32Array([0.941, 0.949, 0.902]),
  upper: new Float32Array([0.918, 0.929, 0.859]),
  lower: new Float32Array([0.898, 0.914, 0.82]),
  bottom: new Float32Array([0.973, 0.98, 0.894]),
  ink: new Float32Array([0.286, 0.294, 0.247]),
  inkTop: new Float32Array([0.165, 0.169, 0.141]),
  cap: new Float32Array([0.165, 0.169, 0.141]),
};
const DARK = {
  top: new Float32Array([0.055, 0.058, 0.043]),
  upper: new Float32Array([0.075, 0.08, 0.058]),
  lower: new Float32Array([0.1, 0.106, 0.078]),
  bottom: new Float32Array([0.13, 0.136, 0.1]),
  ink: new Float32Array([0.79, 0.81, 0.7]),
  inkTop: new Float32Array([0.941, 0.949, 0.902]),
  cap: new Float32Array([1, 1, 0.96]),
};

/** Reflected bars, reflected caps, bars, caps: as pairs of flip and cap. */
const PASSES = [-1, 0, -1, 1, 1, 0, 1, 1];

const BACKGROUND_FS = `${VERSION}
${COMMON_GLSL}
uniform vec3 uTop, uUpper, uLower, uBottom;
uniform float uHorizon, uDark;
in vec2 vUv;
out vec4 o;
void main() {
  // Glass: a lighter upper half, a darker reflective lower half, and a seam between them.
  float y = vUv.y;
  vec3 upper = mix(uTop, uUpper, smoothstep(1.0, uHorizon, y));
  vec3 lower = mix(uLower, uBottom, smoothstep(uHorizon, 0.0, y));
  vec3 c = y > uHorizon ? upper : lower;
  c += (uDark > 0.5 ? 0.03 : 0.025) * exp(-abs(y - uHorizon) * 60.0);
  vec2 q = vUv - 0.5;
  c *= 1.0 - dot(q, q) * (uDark > 0.5 ? 0.9 : 0.35);
  o = vec4(c, 1.0);
}`;

const BAR_VS = `${VERSION}
uniform sampler2D uAudio;
uniform float uHorizon, uMaxHeight, uMargin, uFill, uFlip, uCap, uPad;
flat out vec4 vBox;   // half width, bottom, top, level; heights are up from the baseline
out vec2 vLocal;      // x from the bar's centre, y up from the baseline
void main() {
  int i = gl_InstanceID;
  float level = texelFetch(uAudio, ivec2(i, uCap > 0.5 ? 3 : 2), 0).r;
  float slot = (2.0 - 2.0 * uMargin) / 64.0;
  float cx = -1.0 + uMargin + slot * (float(i) + 0.5);
  float hw = slot * 0.5 * uFill;
  float bottom = uCap > 0.5 ? level * uMaxHeight + 0.008 : 0.0;
  float top = uCap > 0.5 ? bottom + 0.011 : max(level * uMaxHeight, 0.006);
  float x = float(gl_VertexID & 1) * 2.0 - 1.0;
  float y = (gl_VertexID >> 1) == 1 ? top + uPad : bottom - uPad;
  vLocal = vec2(x * (hw + uPad), y);
  vBox = vec4(hw, bottom, top, level);
  gl_Position = vec4(cx + vLocal.x, (uHorizon * 2.0 - 1.0) + uFlip * y, 0.0, 1.0);
}`;

const BAR_FS = `${VERSION}
uniform vec3 uInk, uInkTop;
uniform float uFlip, uCap, uDark, uMaxHeight;
flat in vec4 vBox;
in vec2 vLocal;
out vec4 o;

float roundedBox(vec2 p, vec2 b, float r) {
  vec2 d = abs(p) - b + r;
  return length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - r;
}
void main() {
  vec2 halfSize = vec2(vBox.x, (vBox.z - vBox.y) * 0.5);
  vec2 centre = vec2(0.0, (vBox.y + vBox.z) * 0.5);
  float d = roundedBox(vLocal - centre, halfSize, min(halfSize.x, halfSize.y) * 0.6);
  float px = max(fwidth(d), 1e-5);
  float a = smoothstep(px, -px, d);
  float h = clamp(vLocal.y / uMaxHeight, 0.0, 1.0);
  vec3 c = uCap > 0.5 ? uInkTop : mix(uInk, uInkTop, smoothstep(0.0, 0.85, h));
  // A soft highlight down the left of each bar.
  float shine = smoothstep(1.0, -1.0, vLocal.x / vBox.x) * 0.1;
  c += uDark > 0.5 ? vec3(shine * 0.5) : vec3(shine);
  if (uDark > 0.5) {
    float halo = exp(-max(d, 0.0) * 70.0) * (0.25 + 0.45 * vBox.w);
    a = max(a, halo * (uCap > 0.5 ? 0.9 : 0.55));
  }
  if (uFlip < 0.0) {
    // The reflection is paler, and gone before it reaches half the bar's height.
    float fade = 1.0 - smoothstep(0.0, 0.55 * uMaxHeight + 0.04, vLocal.y);
    a *= (uDark > 0.5 ? 0.36 : 0.3) * fade;
  }
  o = vec4(c, a);
}`;

/**
 * Sixty-four bars with peak caps that fall under gravity, standing on a line with a mirrored
 * reflection, on the cream glass of the iTunes 10 display, or a dark version of it.
 */
export class SpectrumStyle implements StyleRenderer {
  private readonly background: Program;
  private readonly bars: Program;

  constructor(private readonly host: StyleHost) {
    this.background = new Program(host.gl, FULLSCREEN_VS, BACKGROUND_FS);
    this.bars = new Program(host.gl, BAR_VS, BAR_FS);
  }

  resize(): void {}

  render(info: RenderInfo): void {
    const { gl, emptyVao, scene } = this.host;
    const colors = info.palette === "dark" ? DARK : CREAM;
    const dark = info.palette === "dark" ? 1 : 0;
    const horizon = 0.4;
    scene.bind();
    gl.bindVertexArray(emptyVao);

    this.background.use();
    gl.uniform3fv(this.background.u("uTop"), colors.top);
    gl.uniform3fv(this.background.u("uUpper"), colors.upper);
    gl.uniform3fv(this.background.u("uLower"), colors.lower);
    gl.uniform3fv(this.background.u("uBottom"), colors.bottom);
    gl.uniform1f(this.background.u("uHorizon"), horizon);
    gl.uniform1f(this.background.u("uDark"), dark);
    gl.drawArrays(gl.TRIANGLES, 0, 3);

    this.bars.use();
    bindTexture(gl, 0, this.host.audioTexture);
    gl.uniform1i(this.bars.u("uAudio"), 0);
    gl.uniform1f(this.bars.u("uHorizon"), horizon);
    // Bars fill the height above the line, leaving room at the top.
    gl.uniform1f(this.bars.u("uMaxHeight"), (1 - horizon) * 2 * 0.86);
    gl.uniform1f(this.bars.u("uMargin"), 0.06);
    gl.uniform1f(this.bars.u("uFill"), 0.68);
    gl.uniform1f(this.bars.u("uDark"), dark);
    gl.uniform3fv(this.bars.u("uInk"), colors.ink);
    gl.uniform3fv(this.bars.u("uInkTop"), colors.inkTop);
    gl.enable(gl.BLEND);
    gl.blendFuncSeparate(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA, gl.ONE, gl.ONE_MINUS_SRC_ALPHA);

    // The reflection first, so the bars overlap it; then bars and caps.
    for (let pass = 0; pass < PASSES.length; pass += 2) {
      const flip = PASSES[pass];
      const cap = PASSES[pass + 1];
      gl.uniform1f(this.bars.u("uFlip"), flip);
      gl.uniform1f(this.bars.u("uCap"), cap);
      gl.uniform1f(this.bars.u("uPad"), dark ? 0.04 : 0.012);
      if (cap === 1) gl.uniform3fv(this.bars.u("uInkTop"), colors.cap);
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, BAND_COUNT);
      if (cap === 1) gl.uniform3fv(this.bars.u("uInkTop"), colors.inkTop);
    }
    resetState(gl);
  }

  dispose(): void {
    this.background.dispose();
    this.bars.dispose();
  }
}
