import { Artwork } from "./artwork";
import { AUDIO_TEXTURE_ROWS, AudioState } from "./audio";
import { WAVEFORM_LENGTH } from "./feed";
import { FlashLimiter } from "./flash";
import {
  COMMON_GLSL,
  FULLSCREEN_VS,
  Program,
  RenderTarget,
  VERSION,
  bindTexture,
  resetState,
  type StyleHost,
} from "./gl";
import type { Palette, RenderInfo, ScopeMode, StyleRenderer, VisualizerStyle } from "./styles";
import { AlbumStyle } from "./styles/album";
import { ClassicStyle } from "./styles/classic";
import { LightsStyle } from "./styles/lights";
import { LinesStyle } from "./styles/lines";
import { PipesStyle } from "./styles/pipes";
import { ScopeStyle } from "./styles/scope";
import { SpectrumStyle } from "./styles/spectrum";
import { StarfieldStyle } from "./styles/starfield";

/** How long, in seconds of animation, a style that is no longer shown keeps its GPU memory. */
const STYLE_KEPT_FOR_S = 30;

/** The picture is measured for flashing on a coarse grid. */
const METER_COLUMNS = 16;
const METER_ROWS = 9;

const COMPOSITE_FS = `${VERSION}
${COMMON_GLSL}
uniform sampler2D uScene;
uniform float uLimit, uGain, uVeil, uSeed;
in vec2 vUv;
out vec4 o;
void main() {
  vec3 c = texture(uScene, vUv).rgb;
  if (uLimit > 0.5) {
    // The flash limiter works in linear light, like the guideline's relative luminance.
    vec3 lin = pow(c, vec3(2.2)) * uGain + uVeil;
    c = pow(max(lin, 0.0), vec3(1.0 / 2.2));
  }
  // A little noise so slow gradients do not band.
  c += (hash21(gl_FragCoord.xy + uSeed) - 0.5) / 255.0;
  o = vec4(c, 1.0);
}`;

/** Averages the scene's linear luminance over each cell of a coarse grid, to 16 bits in RG. */
const METER_FS = `${VERSION}
uniform sampler2D uScene;
uniform int uLod;
out vec4 o;
void main() {
  ivec2 size = textureSize(uScene, uLod);
  ivec2 cell = ivec2(gl_FragCoord.xy);
  ivec2 span = max(size / ivec2(${METER_COLUMNS}, ${METER_ROWS}), ivec2(1));
  float sum = 0.0;
  for (int y = 0; y < span.y; y++) {
    for (int x = 0; x < span.x; x++) {
      vec3 c = texelFetch(uScene, min(cell * span + ivec2(x, y), size - 1), uLod).rgb;
      sum += dot(pow(c, vec3(2.2)), vec3(0.2126, 0.7152, 0.0722));
    }
  }
  float v = clamp(sum / float(span.x * span.y), 0.0, 1.0);
  float hi = floor(v * 255.0) / 255.0;
  o = vec4(hi, fract(v * 255.0), 0.0, 1.0);
}`;

export interface EngineFrame {
  style: VisualizerStyle;
  time: number;
  dt: number;
  audio: AudioState;
  palette: Palette;
  scopeMode: ScopeMode;
  limitFlashing: boolean;
}

export interface EngineStats {
  /** Smoothed GPU time per frame, in milliseconds; 0 until measured or if unsupported. */
  gpuMs: number;
  /** Whether GPU timing is available (the timer query extension). */
  gpuTiming: boolean;
  /** The flash limiter's current adjustment. */
  gain: number;
  veil: number;
  /** Measured average luminance of the last frame, before limiting. */
  luminance: number;
}

/** WebGL2 renderer for the styles. Owns the context's resources; `dispose` releases them. */
export class VisualizerEngine {
  static tryCreate(canvas: HTMLCanvasElement): VisualizerEngine | null {
    const gl = canvas.getContext("webgl2", {
      alpha: false,
      antialias: false,
      depth: false,
      stencil: false,
      powerPreference: "high-performance",
      preserveDrawingBuffer: false,
    });
    if (!gl) return null;
    try {
      return new VisualizerEngine(canvas, gl);
    } catch (error) {
      console.error("visualizer: WebGL2 setup failed", error);
      return null;
    }
  }

  readonly stats: EngineStats = { gpuMs: 0, gpuTiming: false, gain: 1, veil: 0, luminance: 0 };
  readonly artwork: Artwork;
  /** The graphics hardware, as the browser names it. */
  readonly renderer: string;

  private readonly emptyVao: WebGLVertexArrayObject;
  private readonly audioTexture: WebGLTexture;
  private readonly composite: Program;
  private readonly meter: Program;
  private readonly meterTarget: RenderTarget;
  private readonly meterBytes = new Uint8Array(METER_COLUMNS * METER_ROWS * 4);
  private readonly limiter = new FlashLimiter();
  private readonly styles = new Map<VisualizerStyle, StyleRenderer>();
  private readonly timer: GpuTimer | null;
  private readonly feedbackFormat: number;
  private readonly feedbackIs8Bit: boolean;
  private scene: RenderTarget | null = null;
  /** Reused every frame, so drawing allocates nothing. */
  private readonly renderInfo: RenderInfo;
  private readonly lastUsed = new Map<VisualizerStyle, number>();
  private nextRetireCheck = 0;
  private width = 0;
  private height = 0;
  private frameCount = 0;
  private meterLod = 0;

  private constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly gl: WebGL2RenderingContext,
  ) {
    const vao = gl.createVertexArray();
    const audio = gl.createTexture();
    if (!vao || !audio) throw new Error("could not create the base resources");
    this.emptyVao = vao;
    this.audioTexture = audio;
    gl.bindTexture(gl.TEXTURE_2D, audio);
    gl.texStorage2D(gl.TEXTURE_2D, 1, gl.R32F, WAVEFORM_LENGTH, AUDIO_TEXTURE_ROWS);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);

    // Feedback needs more than 8 bits to fade slowly without getting stuck; use half floats
    // where they can be drawn to.
    const half = gl.getExtension("EXT_color_buffer_float") ?? gl.getExtension("EXT_color_buffer_half_float");
    this.feedbackFormat = half ? gl.RGBA16F : gl.RGBA8;
    this.feedbackIs8Bit = !half;

    this.composite = new Program(gl, FULLSCREEN_VS, COMPOSITE_FS);
    this.meter = new Program(gl, FULLSCREEN_VS, METER_FS);
    this.meterTarget = new RenderTarget(gl, METER_COLUMNS, METER_ROWS, gl.RGBA8, { linear: false });
    this.artwork = new Artwork(gl);
    this.artwork.show(null, "");
    this.renderInfo = {
      time: 0,
      dt: 0,
      width: 0,
      height: 0,
      audio: new AudioState(),
      palette: "cream",
      scopeMode: "both",
    };
    this.timer = GpuTimer.create(gl);
    this.stats.gpuTiming = this.timer !== null;
    const debugInfo = gl.getExtension("WEBGL_debug_renderer_info");
    this.renderer = String(
      gl.getParameter(debugInfo ? debugInfo.UNMASKED_RENDERER_WEBGL : gl.RENDERER),
    );
  }

  /** Sets the size of the drawing buffer, in device pixels. */
  resize(width: number, height: number): void {
    width = Math.max(2, Math.round(width));
    height = Math.max(2, Math.round(height));
    if (width === this.width && height === this.height && this.scene) return;
    this.width = width;
    this.height = height;
    this.canvas.width = width;
    this.canvas.height = height;
    this.scene?.dispose();
    this.scene = new RenderTarget(this.gl, width, height, this.gl.RGBA8, { mipmaps: true });
    this.meterLod = Math.max(0, Math.floor(Math.log2(width / METER_COLUMNS)) - 2);
    for (const style of this.styles.values()) style.resize(width, height);
  }

  setNowPlaying(artworkUrl: string | null, key: string): void {
    this.artwork.show(artworkUrl, key);
  }

  render(frame: EngineFrame): void {
    const gl = this.gl;
    const scene = this.scene;
    if (!scene || gl.isContextLost()) return;
    this.timer?.begin();
    this.artwork.tick(frame.dt);

    gl.bindTexture(gl.TEXTURE_2D, this.audioTexture);
    gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, WAVEFORM_LENGTH, AUDIO_TEXTURE_ROWS, gl.RED, gl.FLOAT, frame.audio.texture);

    const style = this.styleFor(frame.style);
    const info = this.info(frame);
    scene.bind();
    gl.disable(gl.BLEND);
    style.render(info);
    resetState(gl);
    this.retireIdleStyles(frame.style, frame.time);

    let gain = 1;
    let veil = 0;
    if (frame.limitFlashing) {
      const luminance = this.measure(scene);
      const limit = this.limiter.update(frame.dt * 1000, luminance);
      gain = limit.gain;
      veil = limit.veil;
      this.stats.luminance = luminance;
    } else {
      this.limiter.reset();
    }
    this.stats.gain = gain;
    this.stats.veil = veil;

    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    gl.viewport(0, 0, this.width, this.height);
    gl.bindVertexArray(this.emptyVao);
    this.composite.use();
    bindTexture(gl, 0, scene.texture);
    gl.uniform1i(this.composite.u("uScene"), 0);
    gl.uniform1f(this.composite.u("uLimit"), frame.limitFlashing && (gain !== 1 || veil !== 0) ? 1 : 0);
    gl.uniform1f(this.composite.u("uGain"), gain);
    gl.uniform1f(this.composite.u("uVeil"), veil);
    gl.uniform1f(this.composite.u("uSeed"), (this.frameCount++ % 64) * 7.31);
    gl.drawArrays(gl.TRIANGLES, 0, 3);

    this.timer?.end();
    if (this.timer) this.stats.gpuMs = this.timer.smoothedMs;
  }

  dispose(): void {
    for (const style of this.styles.values()) style.dispose();
    this.styles.clear();
    this.lastUsed.clear();
    this.artwork.dispose();
    this.scene?.dispose();
    this.meterTarget.dispose();
    this.composite.dispose();
    this.meter.dispose();
    this.timer?.dispose();
    this.gl.deleteTexture(this.audioTexture);
    this.gl.deleteVertexArray(this.emptyVao);
  }

  /** Average linear luminance of the scene, read back from a coarse grid. */
  private measure(scene: RenderTarget): number {
    const gl = this.gl;
    gl.bindTexture(gl.TEXTURE_2D, scene.texture);
    gl.generateMipmap(gl.TEXTURE_2D);
    this.meterTarget.bind();
    gl.bindVertexArray(this.emptyVao);
    this.meter.use();
    bindTexture(gl, 0, scene.texture);
    gl.uniform1i(this.meter.u("uScene"), 0);
    gl.uniform1i(this.meter.u("uLod"), this.meterLod);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
    // A synchronous read of 144 pixels: it waits for this frame, but that is the point, since
    // the limit must apply to the frame it measures, not to the one before.
    gl.readPixels(0, 0, METER_COLUMNS, METER_ROWS, gl.RGBA, gl.UNSIGNED_BYTE, this.meterBytes);
    let sum = 0;
    const bytes = this.meterBytes;
    for (let i = 0; i < bytes.length; i += 4) sum += bytes[i] / 255 + bytes[i + 1] / 255 / 255;
    return sum / (METER_COLUMNS * METER_ROWS);
  }

  private info(frame: EngineFrame): RenderInfo {
    const info = this.renderInfo;
    info.time = frame.time;
    info.dt = frame.dt;
    info.width = this.width;
    info.height = this.height;
    info.audio = frame.audio;
    info.palette = frame.palette;
    info.scopeMode = frame.scopeMode;
    return info;
  }

  /** A style not shown for a while gives back its buffers; it is rebuilt if it is chosen again. */
  private retireIdleStyles(current: VisualizerStyle, time: number): void {
    this.lastUsed.set(current, time);
    // Looking is cheap, but not free, and there is no hurry.
    if (time < this.nextRetireCheck) return;
    this.nextRetireCheck = time + 5;
    for (const [id, style] of this.styles) {
      if (id !== current && time - (this.lastUsed.get(id) ?? 0) > STYLE_KEPT_FOR_S) {
        style.dispose();
        this.styles.delete(id);
        this.lastUsed.delete(id);
      }
    }
  }

  private styleFor(id: VisualizerStyle): StyleRenderer {
    let style = this.styles.get(id);
    if (!style) {
      try {
        style = createStyle(id, this.host());
      } catch (error) {
        // A shader some graphics driver will not take should not take the whole view with it.
        if (id === "spectrum") throw error;
        console.error(`visualizer: the ${id} style could not be built; showing spectrum`, error);
        return this.styleFor("spectrum");
      }
      style.resize(this.width, this.height);
      this.styles.set(id, style);
    }
    return style;
  }

  private host(): StyleHost {
    const current = (): RenderTarget => {
      if (!this.scene) throw new Error("no scene");
      return this.scene;
    };
    return {
      gl: this.gl,
      emptyVao: this.emptyVao,
      audioTexture: this.audioTexture,
      // The scene is made again whenever the size changes, and a style lives through that.
      get scene() {
        return current();
      },
      feedbackFormat: this.feedbackFormat,
      feedbackIs8Bit: this.feedbackIs8Bit,
      artwork: this.artwork,
    };
  }
}

function createStyle(id: VisualizerStyle, host: StyleHost): StyleRenderer {
  switch (id) {
    case "classic":
      return new ClassicStyle(host);
    case "lights":
      return new LightsStyle(host);
    case "spectrum":
      return new SpectrumStyle(host);
    case "scope":
      return new ScopeStyle(host);
    case "album":
      return new AlbumStyle(host);
    case "pipes":
      return new PipesStyle(host);
    case "starfield":
      return new StarfieldStyle(host);
    case "lines":
      return new LinesStyle(host);
  }
}

interface TimerExtension {
  TIME_ELAPSED_EXT: number;
  GPU_DISJOINT_EXT: number;
}

/** GPU time per frame from the timer query extension, where the browser offers it. */
class GpuTimer {
  static create(gl: WebGL2RenderingContext): GpuTimer | null {
    const extension = gl.getExtension("EXT_disjoint_timer_query_webgl2") as TimerExtension | null;
    return extension ? new GpuTimer(gl, extension) : null;
  }

  smoothedMs = 0;
  private readonly queries: WebGLQuery[] = [];
  private readonly pending: boolean[] = [];
  private next = 0;

  private constructor(
    private readonly gl: WebGL2RenderingContext,
    private readonly extension: TimerExtension,
  ) {
    for (let i = 0; i < 4; i++) {
      const query = gl.createQuery();
      if (query) {
        this.queries.push(query);
        this.pending.push(false);
      }
    }
  }

  begin(): void {
    if (this.queries.length === 0 || this.pending[this.next]) return;
    this.gl.beginQuery(this.extension.TIME_ELAPSED_EXT, this.queries[this.next]);
  }

  end(): void {
    const gl = this.gl;
    if (this.queries.length === 0) return;
    if (!this.pending[this.next]) {
      gl.endQuery(this.extension.TIME_ELAPSED_EXT);
      this.pending[this.next] = true;
      this.next = (this.next + 1) % this.queries.length;
    }
    // Collect whichever finished.
    const disjoint = gl.getParameter(this.extension.GPU_DISJOINT_EXT) as boolean;
    for (let i = 0; i < this.queries.length; i++) {
      if (!this.pending[i]) continue;
      if (!gl.getQueryParameter(this.queries[i], gl.QUERY_RESULT_AVAILABLE)) continue;
      this.pending[i] = false;
      if (disjoint) continue;
      const ms = (gl.getQueryParameter(this.queries[i], gl.QUERY_RESULT) as number) / 1e6;
      this.smoothedMs = this.smoothedMs === 0 ? ms : this.smoothedMs + (ms - this.smoothedMs) * 0.1;
    }
  }

  dispose(): void {
    for (const query of this.queries) this.gl.deleteQuery(query);
  }
}
