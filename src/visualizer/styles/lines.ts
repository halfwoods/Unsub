import { COMMON_GLSL, Program, VERSION, resetState, type StyleHost } from "../gl";
import { EDGE_STRIDE, MAX_EDGES, POLYGON_SIDES, Polygons } from "../polygons";
import type { RenderInfo, StyleRenderer } from "../styles";

const POLYGONS = POLYGON_SIDES.length;
/** How thick a line is, in picture heights, and how much a beat adds. */
const WIDTH = 0.0032;
const WIDTH_PER_BEAT = 0.9;
/** Seconds between the copies of a polygon that make its trail; see `polygons.ts`. */
const COPY_EVERY_S = 0.02;

const EDGE_VS = `${VERSION}
layout(location = 0) in vec4 aEdge;
layout(location = 1) in vec2 aMeta;
uniform vec2 uAspect;
uniform float uWidth;
out vec2 vLocal;
out float vAge;
flat out float vPolygon;
void main() {
  vec2 a = aEdge.xy;
  vec2 b = aEdge.zw;
  vec2 axis = b - a;
  float len = max(length(axis), 1e-5);
  vec2 t = axis / len;
  vec2 n = vec2(-t.y, t.x);
  vec2 corner = vec2(float(gl_VertexID & 1), float(gl_VertexID >> 1)) * 2.0 - 1.0;
  // The quad reaches a line's width past each end, so a corner is a round dot, not a notch.
  vec2 p = mix(a, b, corner.x * 0.5 + 0.5) + (t * corner.x + n * corner.y) * uWidth * 2.0;
  gl_Position = vec4(p / uAspect, 0.0, 1.0);
  vLocal = vec2(corner.x, corner.y * 2.0);
  vAge = aMeta.x;
  vPolygon = aMeta.y;
}`;

const EDGE_FS = `${VERSION}
${COMMON_GLSL}
uniform float uHue[${POLYGONS}];
uniform float uGain[${POLYGONS}];
in vec2 vLocal;
in float vAge;
flat in float vPolygon;
out vec4 o;
void main() {
  int polygon = int(vPolygon + 0.5);
  float across = vLocal.y;
  float line = exp(-across * across * 3.2);
  // Older copies fade quickly and drift round the colour wheel.
  float fade = (1.0 - vAge) * (1.0 - vAge);
  vec3 c = hsv2rgb(vec3(uHue[polygon] + vAge * 0.12, 0.7, 1.0));
  o = vec4(c * line * fade * uGain[polygon], 1.0);
}`;

/**
 * Polygons whose corners drift across the picture, bounce off its edges and leave fading trails,
 * their colours slowly cycling. How fast the corners move follows the loudness, a beat kicks them
 * into a new heading, and each polygon's brightness follows a part of the spectrum: the bass, the
 * middle and the treble.
 */
export class LinesStyle implements StyleRenderer {
  private readonly program: Program;
  private readonly vao: WebGLVertexArrayObject;
  private readonly buffer: WebGLBuffer;
  private readonly edges = new Float32Array(MAX_EDGES * EDGE_STRIDE);
  private readonly polygons = new Polygons();
  private readonly hues = new Float32Array(POLYGONS);
  private readonly gains = new Float32Array(POLYGONS);
  private lastBeat = -1;

  constructor(private readonly host: StyleHost) {
    const gl = host.gl;
    this.program = new Program(gl, EDGE_VS, EDGE_FS);
    const vao = gl.createVertexArray();
    const buffer = gl.createBuffer();
    if (!vao || !buffer) throw new Error("could not create the line buffers");
    this.vao = vao;
    this.buffer = buffer;
    gl.bindVertexArray(vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.bufferData(gl.ARRAY_BUFFER, this.edges.byteLength, gl.DYNAMIC_DRAW);
    const stride = EDGE_STRIDE * 4;
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 4, gl.FLOAT, false, stride, 0);
    gl.vertexAttribDivisor(0, 1);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 2, gl.FLOAT, false, stride, 16);
    gl.vertexAttribDivisor(1, 1);
    gl.bindVertexArray(null);
    gl.bindBuffer(gl.ARRAY_BUFFER, null);
  }

  resize(): void {}

  render(info: RenderInfo): void {
    const { gl, scene } = this.host;
    const { audio, dt, time } = info;
    const aspect = info.width / info.height;

    if (audio.beatCount !== this.lastBeat) {
      if (this.lastBeat >= 0) this.polygons.kick(Math.min(1, Math.max(0.4, audio.kick)));
      this.lastBeat = audio.beatCount;
    }
    // Quiet music lets the corners drift; loud music sends them across the picture.
    const speed = 0.09 + 0.7 * audio.energy + 0.25 * audio.thump;
    this.polygons.step(Math.min(dt, 0.05), aspect, speed);
    const count = this.polygons.fill(this.edges);

    // A tightly packed trail would pile its copies on top of each other and burn white, so each
    // copy is dimmed by how much of the line's width the corners move between them.
    const width = WIDTH * (1 + WIDTH_PER_BEAT * audio.beat);
    const packing = Math.min(1, Math.max(0.12, (speed * COPY_EVERY_S) / (2 * width)));
    // What each polygon listens to, against how loud that part of the spectrum usually is.
    for (let polygon = 0; polygon < POLYGONS; polygon++) {
      const part = polygon === 0 ? audio.low / 0.6 : polygon === 1 ? audio.mid / 0.4 : audio.high / 0.25;
      this.hues[polygon] = time * 0.025 + polygon * 0.31;
      this.gains[polygon] = packing * (0.4 + 0.9 * Math.min(1, part) + 0.4 * audio.kick + 0.25 * audio.spark) * 1.5;
    }

    scene.clear(0.004, 0.005, 0.01);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.ONE, gl.ONE);
    this.program.use();
    gl.uniform2f(this.program.u("uAspect"), aspect, 1);
    gl.uniform1f(this.program.u("uWidth"), width);
    gl.uniform1fv(this.program.u("uHue"), this.hues);
    gl.uniform1fv(this.program.u("uGain"), this.gains);
    gl.bindVertexArray(this.vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.buffer);
    gl.bufferSubData(gl.ARRAY_BUFFER, 0, this.edges, 0, count * EDGE_STRIDE);
    gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, count);
    gl.bindVertexArray(null);
    gl.bindBuffer(gl.ARRAY_BUFFER, null);
    resetState(gl);
  }

  dispose(): void {
    this.program.dispose();
    this.host.gl.deleteVertexArray(this.vao);
    this.host.gl.deleteBuffer(this.buffer);
  }
}
