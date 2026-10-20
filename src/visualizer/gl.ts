/** Small helpers over raw WebGL2: programs, render targets, and a fullscreen triangle. */

export const VERSION = "#version 300 es\nprecision highp float;\nprecision highp int;\nprecision highp sampler2D;\n";

/** A triangle that covers the screen, from the vertex index alone. Bind any VAO and draw 3. */
export const FULLSCREEN_VS = `${VERSION}
out vec2 vUv;
void main() {
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
  vUv = p;
  gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}`;

/** Functions several shaders share. */
export const COMMON_GLSL = `
const float PI = 3.14159265359;
const float TAU = 6.28318530718;

vec3 hsv2rgb(vec3 c) {
  vec3 p = abs(fract(c.xxx + vec3(0.0, 2.0 / 3.0, 1.0 / 3.0)) * 6.0 - 3.0);
  return c.z * mix(vec3(1.0), clamp(p - 1.0, 0.0, 1.0), c.y);
}
float hash11(float p) {
  p = fract(p * 0.1031);
  p *= p + 33.33;
  p *= p + p;
  return fract(p);
}
float hash21(vec2 p) {
  vec3 p3 = fract(vec3(p.xyx) * 0.1031);
  p3 += dot(p3, p3.yzx + 33.33);
  return fract((p3.x + p3.y) * p3.z);
}
vec2 hash22(vec2 p) {
  vec3 p3 = fract(vec3(p.xyx) * vec3(0.1031, 0.1030, 0.0973));
  p3 += dot(p3, p3.yzx + 33.33);
  return fract((p3.xx + p3.yz) * p3.zy);
}
float vnoise(vec2 p) {
  vec2 i = floor(p);
  vec2 f = fract(p);
  f = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash21(i), hash21(i + vec2(1.0, 0.0)), f.x),
             mix(hash21(i + vec2(0.0, 1.0)), hash21(i + vec2(1.0, 1.0)), f.x), f.y);
}
mat2 rot(float a) {
  float c = cos(a), s = sin(a);
  return mat2(c, -s, s, c);
}
// The audio texture: 256 x 4, rows are left waveform, right waveform, bands (64), peaks (64).
float waveAt(sampler2D audio, int row, int i) { return texelFetch(audio, ivec2(clamp(i, 0, 255), row), 0).r; }
`;

export class GlError extends Error {}

export class Program {
  readonly handle: WebGLProgram;
  private readonly locations = new Map<string, WebGLUniformLocation | null>();

  constructor(
    private readonly gl: WebGL2RenderingContext,
    vertex: string,
    fragment: string,
    feedbackVaryings?: string[],
  ) {
    const vs = compile(gl, gl.VERTEX_SHADER, vertex);
    const fs = compile(gl, gl.FRAGMENT_SHADER, fragment);
    const program = gl.createProgram();
    if (!program) throw new GlError("could not create a program");
    gl.attachShader(program, vs);
    gl.attachShader(program, fs);
    if (feedbackVaryings) {
      gl.transformFeedbackVaryings(program, feedbackVaryings, gl.INTERLEAVED_ATTRIBS);
    }
    gl.linkProgram(program);
    gl.deleteShader(vs);
    gl.deleteShader(fs);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
      const log = gl.getProgramInfoLog(program) ?? "";
      gl.deleteProgram(program);
      throw new GlError(`program failed to link: ${log}`);
    }
    this.handle = program;
  }

  use(): void {
    this.gl.useProgram(this.handle);
  }

  /** The location of a uniform, looked up once. */
  u(name: string): WebGLUniformLocation | null {
    let location = this.locations.get(name);
    if (location === undefined) {
      location = this.gl.getUniformLocation(this.handle, name);
      this.locations.set(name, location);
    }
    return location;
  }

  dispose(): void {
    this.gl.deleteProgram(this.handle);
  }
}

function compile(gl: WebGL2RenderingContext, type: number, source: string): WebGLShader {
  const shader = gl.createShader(type);
  if (!shader) throw new GlError("could not create a shader");
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    const log = gl.getShaderInfoLog(shader) ?? "";
    gl.deleteShader(shader);
    const kind = type === gl.VERTEX_SHADER ? "vertex" : "fragment";
    throw new GlError(`${kind} shader failed to compile: ${log}\n${numbered(source)}`);
  }
  return shader;
}

function numbered(source: string): string {
  return source
    .split("\n")
    .map((line, i) => `${String(i + 1).padStart(3)}  ${line}`)
    .join("\n");
}

/** A texture with a framebuffer to draw into it. */
export class RenderTarget {
  readonly texture: WebGLTexture;
  readonly framebuffer: WebGLFramebuffer;

  constructor(
    private readonly gl: WebGL2RenderingContext,
    readonly width: number,
    readonly height: number,
    readonly internalFormat: number,
    options: { linear?: boolean; mipmaps?: boolean } = {},
  ) {
    const texture = gl.createTexture();
    const framebuffer = gl.createFramebuffer();
    if (!texture || !framebuffer) throw new GlError("could not create a render target");
    gl.bindTexture(gl.TEXTURE_2D, texture);
    const levels = options.mipmaps ? Math.floor(Math.log2(Math.max(width, height))) + 1 : 1;
    gl.texStorage2D(gl.TEXTURE_2D, levels, internalFormat, width, height);
    const filter = options.linear === false ? gl.NEAREST : gl.LINEAR;
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, options.mipmaps ? gl.LINEAR_MIPMAP_LINEAR : filter);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, filter);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.bindFramebuffer(gl.FRAMEBUFFER, framebuffer);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, texture, 0);
    const status = gl.checkFramebufferStatus(gl.FRAMEBUFFER);
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    if (status !== gl.FRAMEBUFFER_COMPLETE) {
      gl.deleteTexture(texture);
      gl.deleteFramebuffer(framebuffer);
      throw new GlError(`framebuffer incomplete (0x${status.toString(16)})`);
    }
    this.texture = texture;
    this.framebuffer = framebuffer;
  }

  bind(): void {
    this.gl.bindFramebuffer(this.gl.FRAMEBUFFER, this.framebuffer);
    this.gl.viewport(0, 0, this.width, this.height);
  }

  clear(r = 0, g = 0, b = 0, a = 1): void {
    this.bind();
    this.gl.clearColor(r, g, b, a);
    this.gl.clear(this.gl.COLOR_BUFFER_BIT);
  }

  dispose(): void {
    this.gl.deleteTexture(this.texture);
    this.gl.deleteFramebuffer(this.framebuffer);
  }
}

/** A pair of render targets to draw from one into the other, then swap: for feedback. */
export class PingPong {
  private flipped = false;
  private readonly a: RenderTarget;
  private readonly b: RenderTarget;

  constructor(gl: WebGL2RenderingContext, width: number, height: number, format: number) {
    this.a = new RenderTarget(gl, width, height, format);
    try {
      this.b = new RenderTarget(gl, width, height, format);
    } catch (error) {
      this.a.dispose();
      throw error;
    }
    this.a.clear(0, 0, 0, 1);
    this.b.clear(0, 0, 0, 1);
  }

  /** What was drawn last. */
  get read(): RenderTarget {
    return this.flipped ? this.b : this.a;
  }

  /** Where to draw next. */
  get write(): RenderTarget {
    return this.flipped ? this.a : this.b;
  }

  swap(): void {
    this.flipped = !this.flipped;
  }

  get width(): number {
    return this.a.width;
  }

  get height(): number {
    return this.a.height;
  }

  dispose(): void {
    this.a.dispose();
    this.b.dispose();
  }
}

export function bindTexture(gl: WebGL2RenderingContext, unit: number, texture: WebGLTexture | null): void {
  gl.activeTexture(gl.TEXTURE0 + unit);
  gl.bindTexture(gl.TEXTURE_2D, texture);
}

/** What a style is given to draw with. */
export interface StyleHost {
  gl: WebGL2RenderingContext;
  /** A vertex array with nothing in it, for draws that make their vertices from the index. */
  emptyVao: WebGLVertexArrayObject;
  /** The 256 x 4 audio texture; see `COMMON_GLSL`. */
  audioTexture: WebGLTexture;
  /** The target every style draws its finished picture into. */
  scene: RenderTarget;
  /** A format for feedback buffers: half-float when it can be drawn to, else plain 8-bit. */
  feedbackFormat: number;
  /** Whether `feedbackFormat` is 8-bit, which needs a fade that is sure to reach black. */
  feedbackIs8Bit: boolean;
  /** The album cover, or a stand-in. */
  artwork: ArtworkSource;
}

export interface ArtworkSource {
  readonly texture: WebGLTexture | null;
  /** The cover's average colour, linear 0..1. */
  readonly tint: Float32Array;
  /** Seconds since this cover appeared. */
  readonly age: number;
}

/** Common draw state to return to after a style has used blending. */
export function resetState(gl: WebGL2RenderingContext): void {
  gl.disable(gl.BLEND);
  gl.disable(gl.DEPTH_TEST);
  gl.disable(gl.CULL_FACE);
  gl.colorMask(true, true, true, true);
}
