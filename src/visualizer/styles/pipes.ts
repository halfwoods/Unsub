import { BAND_COUNT } from "../feed";
import { COMMON_GLSL, FULLSCREEN_VS, Program, VERSION, resetState, type StyleHost } from "../gl";
import { MAX_JOINTS, MAX_PIPES, MAX_SEGMENTS, PIECE_STRIDE, PipeField } from "../pipefield";
import type { RenderInfo, StyleRenderer } from "../styles";
import { hsv } from "./classic";

const MULTISAMPLES = 4;
const CYLINDER_SIDES = 18;
const SPHERE_RINGS = 12;
const SPHERE_SLICES = 18;
const FIELD_OF_VIEW = (48 * Math.PI) / 180;
/** The camera's distance from the middle of the pipes, and the nearest and farthest it draws. */
const DISTANCE = 16.5;
/** The loudest band is looked for in the spectrum as it has been over this long, so the colour wanders rather than flickers. */
const SPECTRUM_AVERAGE_S = 1.5;
const HUE_FOLLOW_S = 1.2;
const NEAR = 1;
const FAR = 60;

const BACKDROP_FS = `${VERSION}
uniform vec3 uColor;
uniform float uFade;
in vec2 vUv;
out vec4 o;
void main() {
  float r = length(vUv - 0.5);
  o = vec4(uColor * mix(1.6, 0.25, smoothstep(0.0, 0.8, r)) * uFade, 1.0);
}`;

/**
 * A piece is a unit cylinder along z or a unit ball, set in place by its instance: a cylinder from
 * `aStart` to `aEnd`, a ball at `aStart` with the radius in its w. The hue is in `aEnd.w`, and
 * becomes the metal's colour here.
 */
const PIPE_VS = `${VERSION}
${COMMON_GLSL}
layout(location = 0) in vec3 aLocal;
layout(location = 1) in vec4 aStart;
layout(location = 2) in vec4 aEnd;
uniform mat4 uViewProj;
uniform float uBall;
out vec3 vWorld;
out vec3 vNormal;
out vec3 vColor;
void main() {
  vec3 normal;
  vec3 world;
  if (uBall > 0.5) {
    normal = aLocal;
    world = aStart.xyz + aLocal * aStart.w;
  } else {
    vec3 axis = aEnd.xyz - aStart.xyz;
    float len = length(axis);
    vec3 z = len > 1e-5 ? axis / len : vec3(0.0, 0.0, 1.0);
    vec3 side = abs(z.y) < 0.9 ? vec3(0.0, 1.0, 0.0) : vec3(1.0, 0.0, 0.0);
    vec3 x = normalize(cross(side, z));
    vec3 y = cross(z, x);
    normal = x * aLocal.x + y * aLocal.y;
    world = aStart.xyz + normal * aStart.w + z * (aLocal.z * len);
  }
  vWorld = world;
  vNormal = normal;
  vColor = hsv2rgb(vec3(aEnd.w, 0.8, 1.0));
  gl_Position = uViewProj * vec4(world, 1.0);
}`;

const PIPE_FS = `${VERSION}
uniform vec3 uEye, uBackdrop;
uniform float uFade, uSpark;
in vec3 vWorld;
in vec3 vNormal;
in vec3 vColor;
out vec4 o;

// A small studio for polished metal to reflect: a bright sky, a dark floor and two strip lights.
vec3 studio(vec3 r) {
  float up = r.y * 0.5 + 0.5;
  vec3 c = mix(vec3(0.03, 0.035, 0.05), vec3(0.6, 0.66, 0.8), smoothstep(0.15, 1.0, up));
  c += 1.3 * smoothstep(0.84, 0.95, dot(r, normalize(vec3(0.6, 0.55, 0.5))));
  c += 0.8 * smoothstep(0.88, 0.97, dot(r, normalize(vec3(-0.7, 0.35, -0.4))));
  return c;
}

void main() {
  vec3 n = normalize(vNormal);
  vec3 v = normalize(uEye - vWorld);
  vec3 key = normalize(vec3(0.45, 0.85, 0.3));
  float lit = max(dot(n, key), 0.0);
  float fill = max(dot(n, normalize(vec3(-0.6, 0.1, -0.7))), 0.0);
  float shine = pow(max(dot(n, normalize(key + v)), 0.0), 140.0);
  float rim = pow(1.0 - max(dot(n, v), 0.0), 3.0);
  vec3 mirror = studio(reflect(-v, n));
  float sheen = pow(max(dot(n, normalize(key + v)), 0.0), 14.0);
  vec3 c = vColor * (0.2 + 0.85 * lit + 0.3 * fill);
  c += mirror * mix(vec3(0.5), vColor * 1.2 + 0.2, 0.6) * (0.35 + 0.65 * rim);
  c += (shine * 1.3 + sheen * 0.12) * (1.0 + 1.5 * uSpark);
  c = 1.0 - exp(-c * 1.7);
  // Farther pipes sink into the dark.
  c = mix(uBackdrop, c, exp(-length(uEye - vWorld) * 0.016));
  o = vec4(c * uFade, 1.0);
}`;

/**
 * Shiny pipes growing through a grid in space, turning at right angles with a ball at every
 * corner, a few at a time in different colours. When the space is full they fade and it starts
 * again. The growth is in `pipefield.ts`; this draws it as instanced cylinders and balls into a
 * multisampled buffer with a depth test, and the music steers it: the loudness sets how fast the
 * pipes grow, a beat starts a pipe or makes one turn, and the hue of what grows follows the band
 * that is loudest.
 */
export class PipesStyle implements StyleRenderer {
  private readonly backdrop: Program;
  private readonly pipes: Program;
  private readonly field = new PipeField((Math.random() * 0x7fffffff) | 0);
  private readonly meshes: WebGLBuffer[] = [];
  private readonly indexBuffers: WebGLBuffer[] = [];
  /** Instances: finished cylinders, finished balls, cylinders still growing, balls on their tips. */
  private readonly instances: WebGLBuffer[];
  private readonly arrays: WebGLVertexArrayObject[] = [];
  private readonly cylinderIndices: number;
  private readonly ballIndices: number;
  private uploadedSegments = 0;
  private uploadedJoints = 0;
  private target: MultisampledTarget | null = null;

  private lastBeat = -1;
  private hue = 0.55;
  private yaw = 0.6;
  private readonly spectrum = new Float32Array(BAND_COUNT);
  private readonly viewProjection = new Float32Array(16);
  private readonly eye = new Float32Array(3);
  private readonly backdropColor = new Float32Array(3);

  constructor(private readonly host: StyleHost) {
    const gl = host.gl;
    this.backdrop = new Program(gl, FULLSCREEN_VS, BACKDROP_FS);
    this.pipes = new Program(gl, PIPE_VS, PIPE_FS);

    const cylinder = cylinderMesh();
    const ball = ballMesh();
    this.cylinderIndices = cylinder.indices.length;
    this.ballIndices = ball.indices.length;
    const meshes = [cylinder, ball];
    const sizes = [MAX_SEGMENTS, MAX_JOINTS, MAX_PIPES, MAX_PIPES];
    this.instances = sizes.map((count) => this.makeBuffer(count * PIECE_STRIDE * 4));
    for (const mesh of meshes) {
      const vertices = this.makeBuffer(0);
      gl.bufferData(gl.ARRAY_BUFFER, mesh.vertices, gl.STATIC_DRAW);
      const indices = this.makeIndexBuffer(mesh.indices);
      this.meshes.push(vertices);
      this.indexBuffers.push(indices);
    }
    // Finished and growing cylinders share a mesh, as do balls: four arrays in all.
    const mesh = [0, 1, 0, 1];
    sizes.forEach((_, index) => this.arrays.push(this.makeArray(this.meshes[mesh[index]], this.indexBuffers[mesh[index]], this.instances[index])));
    gl.bindVertexArray(null);
    gl.bindBuffer(gl.ARRAY_BUFFER, null);
  }

  resize(width: number, height: number): void {
    this.target?.dispose(this.host.gl);
    this.target = MultisampledTarget.create(this.host.gl, width, height, MULTISAMPLES);
  }

  render(info: RenderInfo): void {
    const { gl, emptyVao, scene } = this.host;
    const target = this.target;
    if (!target) return;
    const { audio, dt } = info;
    const aspect = info.width / info.height;

    if (audio.beatCount !== this.lastBeat) {
      if (this.lastBeat >= 0) this.field.beat();
      this.lastBeat = audio.beatCount;
    }
    // The hue goes to the loudest band: red for the bass, through green, to violet for the treble.
    const average = 1 - Math.exp(-dt / SPECTRUM_AVERAGE_S);
    let loudest = 0;
    for (let band = 0; band < BAND_COUNT; band++) {
      this.spectrum[band] += (audio.bands[band] - this.spectrum[band]) * average;
      if (this.spectrum[band] > this.spectrum[loudest]) loudest = band;
    }
    const wanted = 0.02 + 0.78 * (loudest / (BAND_COUNT - 1));
    this.hue += (wanted - this.hue) * (1 - Math.exp(-dt / HUE_FOLLOW_S));
    // Slow in a quiet passage, quick in a loud one, and a thump of the bass hurries it for a moment.
    this.field.step(Math.min(dt, 0.05), 0.8 + 5.5 * audio.energy + 2.5 * audio.thump, this.hue);
    this.upload(gl);

    this.placeCamera(aspect, dt, audio.energy, audio.kick);
    hsv(this.hue, 0.6, 0.1, this.backdropColor);

    target.bind(gl);
    gl.clearColor(0, 0, 0, 1);
    gl.clearDepth(1);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    gl.bindVertexArray(emptyVao);
    this.backdrop.use();
    gl.uniform3fv(this.backdrop.u("uColor"), this.backdropColor);
    gl.uniform1f(this.backdrop.u("uFade"), this.field.fade);
    gl.drawArrays(gl.TRIANGLES, 0, 3);

    gl.enable(gl.DEPTH_TEST);
    this.pipes.use();
    gl.uniformMatrix4fv(this.pipes.u("uViewProj"), false, this.viewProjection);
    gl.uniform3fv(this.pipes.u("uEye"), this.eye);
    gl.uniform3fv(this.pipes.u("uBackdrop"), this.backdropColor);
    gl.uniform1f(this.pipes.u("uFade"), this.field.fade);
    gl.uniform1f(this.pipes.u("uSpark"), audio.spark);
    this.draw(gl, 0, this.cylinderIndices, this.field.segmentCount, 0);
    this.draw(gl, 1, this.ballIndices, this.field.jointCount, 1);
    this.draw(gl, 2, this.cylinderIndices, this.field.headCount, 0);
    this.draw(gl, 3, this.ballIndices, this.field.headCount, 1);
    gl.bindVertexArray(null);
    resetState(gl);

    target.resolveInto(gl, scene.framebuffer);
  }

  dispose(): void {
    const gl = this.host.gl;
    this.backdrop.dispose();
    this.pipes.dispose();
    this.target?.dispose(gl);
    for (const buffer of [...this.meshes, ...this.indexBuffers, ...this.instances]) gl.deleteBuffer(buffer);
    for (const array of this.arrays) gl.deleteVertexArray(array);
  }

  private draw(gl: WebGL2RenderingContext, array: number, indices: number, instances: number, ball: number): void {
    if (instances === 0) return;
    gl.bindVertexArray(this.arrays[array]);
    gl.uniform1f(this.pipes.u("uBall"), ball);
    gl.drawElementsInstanced(gl.TRIANGLES, indices, gl.UNSIGNED_SHORT, 0, instances);
  }

  /** Sends what has been added to the finished pieces since last time, and all of what is growing. */
  private upload(gl: WebGL2RenderingContext): void {
    const field = this.field;
    // Restarting empties the lists, so what was sent is no longer there.
    if (field.segmentCount < this.uploadedSegments) this.uploadedSegments = 0;
    if (field.jointCount < this.uploadedJoints) this.uploadedJoints = 0;
    this.send(gl, this.instances[0], field.segments, this.uploadedSegments, field.segmentCount);
    this.send(gl, this.instances[1], field.joints, this.uploadedJoints, field.jointCount);
    this.send(gl, this.instances[2], field.heads, 0, field.headCount);
    this.send(gl, this.instances[3], field.tips, 0, field.headCount);
    this.uploadedSegments = field.segmentCount;
    this.uploadedJoints = field.jointCount;
  }

  private send(gl: WebGL2RenderingContext, buffer: WebGLBuffer, data: Float32Array, from: number, to: number): void {
    if (to <= from) return;
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.bufferSubData(gl.ARRAY_BUFFER, from * PIECE_STRIDE * 4, data, from * PIECE_STRIDE, (to - from) * PIECE_STRIDE);
  }

  /** The camera circles the pipes slowly, faster when it is loud, and leans in a little on a kick. */
  private placeCamera(aspect: number, dt: number, energy: number, kick: number): void {
    this.yaw += dt * (0.045 + 0.09 * energy);
    const pitch = 0.42 + 0.1 * Math.sin(this.yaw * 0.7);
    // A narrow window needs the camera farther back to keep the pipes in view.
    const distance = (DISTANCE - 0.7 * kick) * Math.max(1, 1.3 / aspect);
    const eye = this.eye;
    eye[0] = distance * Math.cos(pitch) * Math.sin(this.yaw);
    eye[1] = distance * Math.sin(pitch);
    eye[2] = distance * Math.cos(pitch) * Math.cos(this.yaw);
    lookAtOrigin(this.viewProjection, eye, aspect);
  }

  private makeBuffer(bytes: number): WebGLBuffer {
    const gl = this.host.gl;
    const buffer = gl.createBuffer();
    if (!buffer) throw new Error("could not create a buffer");
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    if (bytes > 0) gl.bufferData(gl.ARRAY_BUFFER, bytes, gl.DYNAMIC_DRAW);
    return buffer;
  }

  private makeIndexBuffer(indices: Uint16Array): WebGLBuffer {
    const gl = this.host.gl;
    const buffer = gl.createBuffer();
    if (!buffer) throw new Error("could not create a buffer");
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, buffer);
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, indices, gl.STATIC_DRAW);
    return buffer;
  }

  private makeArray(mesh: WebGLBuffer, indices: WebGLBuffer, instances: WebGLBuffer): WebGLVertexArrayObject {
    const gl = this.host.gl;
    const array = gl.createVertexArray();
    if (!array) throw new Error("could not create a vertex array");
    gl.bindVertexArray(array);
    gl.bindBuffer(gl.ARRAY_BUFFER, mesh);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 12, 0);
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, indices);
    gl.bindBuffer(gl.ARRAY_BUFFER, instances);
    const stride = PIECE_STRIDE * 4;
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 4, gl.FLOAT, false, stride, 0);
    gl.vertexAttribDivisor(1, 1);
    gl.enableVertexAttribArray(2);
    gl.vertexAttribPointer(2, 4, gl.FLOAT, false, stride, 16);
    gl.vertexAttribDivisor(2, 1);
    return array;
  }
}

/** The picture is drawn here with its edges smoothed and its depth known, then copied to the scene. */
class MultisampledTarget {
  /**
   * Edges smoothed if the graphics can do it. A driver that offers fewer samples than wanted, or
   * refuses the framebuffer, gets plain buffers instead: jagged edges are better than no picture.
   */
  static create(gl: WebGL2RenderingContext, width: number, height: number, wanted: number): MultisampledTarget {
    const samples = Math.min(wanted, gl.getParameter(gl.MAX_SAMPLES) as number);
    try {
      return new MultisampledTarget(gl, width, height, samples);
    } catch (error) {
      if (samples === 0) throw error;
      console.warn("visualizer: multisampling is not available for the pipes", error);
      return new MultisampledTarget(gl, width, height, 0);
    }
  }

  private readonly framebuffer: WebGLFramebuffer;
  private readonly colour: WebGLRenderbuffer;
  private readonly depth: WebGLRenderbuffer;

  private constructor(
    gl: WebGL2RenderingContext,
    private readonly width: number,
    private readonly height: number,
    samples: number,
  ) {
    const framebuffer = gl.createFramebuffer();
    const colour = gl.createRenderbuffer();
    const depth = gl.createRenderbuffer();
    if (!framebuffer || !colour || !depth) throw new Error("could not create the pipes' buffers");
    gl.bindRenderbuffer(gl.RENDERBUFFER, colour);
    gl.renderbufferStorageMultisample(gl.RENDERBUFFER, samples, gl.RGBA8, width, height);
    gl.bindRenderbuffer(gl.RENDERBUFFER, depth);
    gl.renderbufferStorageMultisample(gl.RENDERBUFFER, samples, gl.DEPTH_COMPONENT24, width, height);
    gl.bindFramebuffer(gl.FRAMEBUFFER, framebuffer);
    gl.framebufferRenderbuffer(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.RENDERBUFFER, colour);
    gl.framebufferRenderbuffer(gl.FRAMEBUFFER, gl.DEPTH_ATTACHMENT, gl.RENDERBUFFER, depth);
    const status = gl.checkFramebufferStatus(gl.FRAMEBUFFER);
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    this.framebuffer = framebuffer;
    this.colour = colour;
    this.depth = depth;
    if (status !== gl.FRAMEBUFFER_COMPLETE) {
      this.dispose(gl);
      throw new Error(`the pipes' framebuffer is incomplete (0x${status.toString(16)})`);
    }
  }

  bind(gl: WebGL2RenderingContext): void {
    gl.bindFramebuffer(gl.FRAMEBUFFER, this.framebuffer);
    gl.viewport(0, 0, this.width, this.height);
  }

  /** Averages the samples into `destination`, which is the same size. */
  resolveInto(gl: WebGL2RenderingContext, destination: WebGLFramebuffer): void {
    gl.bindFramebuffer(gl.READ_FRAMEBUFFER, this.framebuffer);
    gl.bindFramebuffer(gl.DRAW_FRAMEBUFFER, destination);
    gl.blitFramebuffer(0, 0, this.width, this.height, 0, 0, this.width, this.height, gl.COLOR_BUFFER_BIT, gl.NEAREST);
    gl.bindFramebuffer(gl.FRAMEBUFFER, destination);
  }

  dispose(gl: WebGL2RenderingContext): void {
    gl.deleteFramebuffer(this.framebuffer);
    gl.deleteRenderbuffer(this.colour);
    gl.deleteRenderbuffer(this.depth);
  }
}

interface Mesh {
  /** x, y, z per vertex. */
  vertices: Float32Array;
  indices: Uint16Array;
}

/** A tube of radius 1 from z = 0 to z = 1; its vertices' x and y are also its normals. */
function cylinderMesh(): Mesh {
  const vertices = new Float32Array((CYLINDER_SIDES + 1) * 2 * 3);
  const indices = new Uint16Array(CYLINDER_SIDES * 6);
  for (let side = 0; side <= CYLINDER_SIDES; side++) {
    const angle = (side / CYLINDER_SIDES) * Math.PI * 2;
    for (let end = 0; end < 2; end++) {
      vertices.set([Math.cos(angle), Math.sin(angle), end], (side * 2 + end) * 3);
    }
  }
  for (let side = 0; side < CYLINDER_SIDES; side++) {
    const base = side * 2;
    indices.set([base, base + 1, base + 2, base + 1, base + 3, base + 2], side * 6);
  }
  return { vertices, indices };
}

/** A ball of radius 1; its vertices are also its normals. */
function ballMesh(): Mesh {
  const vertices = new Float32Array((SPHERE_RINGS + 1) * (SPHERE_SLICES + 1) * 3);
  const indices = new Uint16Array(SPHERE_RINGS * SPHERE_SLICES * 6);
  for (let ring = 0; ring <= SPHERE_RINGS; ring++) {
    const polar = (ring / SPHERE_RINGS) * Math.PI;
    for (let slice = 0; slice <= SPHERE_SLICES; slice++) {
      const azimuth = (slice / SPHERE_SLICES) * Math.PI * 2;
      vertices.set(
        [Math.sin(polar) * Math.cos(azimuth), Math.cos(polar), Math.sin(polar) * Math.sin(azimuth)],
        (ring * (SPHERE_SLICES + 1) + slice) * 3,
      );
    }
  }
  let at = 0;
  for (let ring = 0; ring < SPHERE_RINGS; ring++) {
    for (let slice = 0; slice < SPHERE_SLICES; slice++) {
      const a = ring * (SPHERE_SLICES + 1) + slice;
      const b = a + SPHERE_SLICES + 1;
      indices.set([a, b, a + 1, a + 1, b, b + 1], at);
      at += 6;
    }
  }
  return { vertices, indices };
}

/** Perspective times a view from `eye` towards the origin, column-major, into `out`. */
function lookAtOrigin(out: Float32Array, eye: Float32Array, aspect: number): void {
  const ex = eye[0];
  const ey = eye[1];
  const ez = eye[2];
  const length = Math.hypot(ex, ey, ez);
  const fx = -ex / length;
  const fy = -ey / length;
  const fz = -ez / length;
  // Right is forward x up(0, 1, 0), and the view's up is right x forward.
  const side = Math.hypot(fz, fx);
  const rx = -fz / side;
  const rz = fx / side;
  const ux = -rz * fy;
  const uy = rz * fx - rx * fz;
  const uz = rx * fy;
  const scale = 1 / Math.tan(FIELD_OF_VIEW / 2);
  const depth = NEAR - FAR;
  const clipZ = (FAR + NEAR) / depth;
  const clipW = (2 * FAR * NEAR) / depth;
  const wide = scale / aspect;
  // The columns of the view's rotation, with the projection folded in.
  out[0] = wide * rx;
  out[1] = scale * ux;
  out[2] = -clipZ * fx;
  out[3] = fx;
  out[4] = 0;
  out[5] = scale * uy;
  out[6] = -clipZ * fy;
  out[7] = fy;
  out[8] = wide * rz;
  out[9] = scale * uz;
  out[10] = -clipZ * fz;
  out[11] = fz;
  const tx = -(rx * ex + rz * ez);
  const ty = -(ux * ex + uy * ey + uz * ez);
  const tz = fx * ex + fy * ey + fz * ez;
  out[12] = wide * tx;
  out[13] = scale * ty;
  out[14] = clipZ * tz + clipW;
  out[15] = -tz;
}
