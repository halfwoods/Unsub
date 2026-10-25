import { AudioState } from "./audio";
import { createBrowserClock, type Clock } from "./clock";
import { VisualizerEngine, type EngineFrame } from "./engine";
import type { VisualizerFeed, VisualizerFrame } from "./feed";
import { SoftwareVisualizer } from "./software";
import { DEFAULT_GASLIGHT, normalizeGaslight, type GaslightSettings } from "./gaslightSettings";
import type { Palette, ScopeMode, VisualizerStyle } from "./styles";

/** When nothing is playing the picture drifts, drawn this often (milliseconds apart). */
const IDLE_INTERVAL_MS = 66;
/** Longest step the animation takes, so a stall does not make it jump. */
const MAX_STEP_S = 0.1;
/** Most pixels drawn per frame: about 1440p, past which the cost is not worth the sharpness. */
const MAX_PIXELS = 2560 * 1440 * 1.12;
const MAX_DPR = 2;

export type RenderMode = "webgl2" | "canvas2d" | "lost";

export interface RuntimeOptions {
  style: VisualizerStyle;
  palette: Palette;
  scopeMode: ScopeMode;
  limitFlashing: boolean;
  gaslight: GaslightSettings;
}

export interface RuntimeStats {
  mode: RenderMode;
  actualStyle: VisualizerStyle;
  /** Frames drawn a second. */
  fps: number;
  /** Frames received a second from the feed. */
  feedHz: number;
  width: number;
  height: number;
  gpuMs: number;
  gpuTiming: boolean;
  /** The flash limiter's current gain (1 = not acting) and veil. */
  limiterGain: number;
  limiterVeil: number;
  idle: boolean;
}

/**
 * Everything that is not React: the canvas and its context, the loop, the smoothing of incoming
 * frames, sizing, pausing and recovering. It draws with WebGL2 when it can, and falls back to a
 * plain canvas showing Gaslight or Spectrum when it cannot.
 */
export class VisualizerRuntime {
  readonly stats: RuntimeStats = {
    mode: "webgl2",
    actualStyle: "classic",
    fps: 0,
    feedHz: 0,
    width: 0,
    height: 0,
    gpuMs: 0,
    gpuTiming: false,
    limiterGain: 1,
    limiterVeil: 0,
    idle: true,
  };

  private readonly audio = new AudioState();
  private options: RuntimeOptions = {
    style: "classic",
    palette: "cream",
    scopeMode: "both",
    limitFlashing: true,
    gaslight: { ...DEFAULT_GASLIGHT },
  };
  private engine: VisualizerEngine | null = null;
  private fallback: SoftwareVisualizer | null = null;
  private unsubscribe: (() => void) | null = null;
  private resizeObserver: ResizeObserver | null = null;
  private running = false;
  private lastMs = 0;
  private cssWidth = 0;
  private cssHeight = 0;
  private forcedSize: { width: number; height: number } | null = null;
  private nowPlaying: { url: string | null; key: string } = { url: null, key: "" };
  private frames = 0;
  private fed = 0;
  private windowStartMs = 0;
  private measuredAtMs = 0;
  private reportedError = false;
  /** Reused for every frame drawn. */
  private readonly engineFrame: EngineFrame;

  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly host: HTMLElement,
    private readonly clock: Clock = createBrowserClock(),
    private readonly onMode?: (mode: RenderMode) => void,
  ) {
    canvas.addEventListener("webglcontextlost", this.contextLost);
    canvas.addEventListener("webglcontextrestored", this.contextRestored);
    this.engineFrame = {
      style: this.options.style,
      time: 0,
      dt: 0,
      audio: this.audio,
      palette: this.options.palette,
      scopeMode: this.options.scopeMode,
      limitFlashing: this.options.limitFlashing,
      gaslight: this.options.gaslight,
    };
    this.open();
    if (typeof ResizeObserver !== "undefined") {
      this.resizeObserver = new ResizeObserver(() => this.measure());
      this.resizeObserver.observe(host);
    }
    document.addEventListener("visibilitychange", this.visibilityChanged);
    this.measure();
  }

  setFeed(feed: VisualizerFeed | null): void {
    this.unsubscribe?.();
    this.unsubscribe = feed
      ? feed.onFrame((frame: VisualizerFrame) => {
          this.audio.receive(frame, this.clock.now());
          this.fed += 1;
        })
      : null;
  }

  setOptions(options: Partial<RuntimeOptions>): void {
    this.options = { ...this.options, ...options, gaslight: normalizeGaslight(options.gaslight ?? this.options.gaslight) };
  }

  setNowPlaying(artworkUrl: string | null, key: string): void {
    this.nowPlaying = { url: artworkUrl, key };
    this.engine?.setNowPlaying(artworkUrl, key);
  }

  /** The graphics hardware, for benchmark reports. */
  get renderer(): string {
    return this.engine?.renderer ?? "none";
  }

  /** Draws at exactly this size, in pixels, instead of following the container. For benchmarks. */
  forceSize(width: number, height: number): void {
    this.forcedSize = { width, height };
    this.measure();
  }

  start(): void {
    if (this.running) return;
    this.running = true;
    this.lastMs = this.clock.now();
    this.windowStartMs = this.lastMs;
    this.clock.schedule(this.frame, 0);
  }

  stop(): void {
    this.running = false;
    this.clock.cancel();
  }

  dispose(): void {
    this.stop();
    this.unsubscribe?.();
    this.resizeObserver?.disconnect();
    document.removeEventListener("visibilitychange", this.visibilityChanged);
    this.canvas.removeEventListener("webglcontextlost", this.contextLost);
    this.canvas.removeEventListener("webglcontextrestored", this.contextRestored);
    this.engine?.dispose();
    this.engine = null;
    this.fallback?.dispose();
    this.fallback = null;
  }

  /** Draws one frame now, for a clock that is driven by hand. */
  private readonly frame = (nowMs: number): void => {
    if (!this.running) return;
    const step = Math.min(MAX_STEP_S, Math.max(0, (nowMs - this.lastMs) / 1000));
    this.lastMs = nowMs;
    const audio = this.audio;
    audio.update(step, nowMs);

    const drawable = !document.hidden && this.stats.width > 0 && this.stats.mode !== "lost";
    if (drawable) {
      if (this.engine) {
        const frame = this.engineFrame;
        frame.style = this.options.style;
        frame.time = audio.time;
        frame.dt = step;
        frame.palette = this.options.palette;
        frame.scopeMode = this.options.scopeMode;
        frame.limitFlashing = this.options.limitFlashing;
        frame.gaslight = this.options.gaslight;
        try {
          this.engine.render(frame);
        } catch (error) {
          // Keep the loop going; one bad frame is not the end of the view.
          if (!this.reportedError) console.error("visualizer: a frame failed to draw", error);
          this.reportedError = true;
        }
        const stats = this.engine.stats;
        this.stats.actualStyle = stats.actualStyle;
        this.stats.gpuMs = stats.gpuMs;
        this.stats.gpuTiming = stats.gpuTiming;
        this.stats.limiterGain = stats.gain;
        this.stats.limiterVeil = stats.veil;
      } else if (this.fallback) {
        this.fallback.render(audio, step, this.options.style, this.options.palette, this.options.gaslight, this.options.limitFlashing);
        this.stats.actualStyle = this.options.style === "gaslight" ? "gaslight" : "spectrum";
        this.stats.limiterGain = this.fallback.limiter.gain;
        this.stats.limiterVeil = this.fallback.limiter.veil;
      }
      this.frames += 1;
    }

    if (nowMs - this.windowStartMs >= 500) {
      const seconds = (nowMs - this.windowStartMs) / 1000;
      this.stats.fps = this.frames / seconds;
      this.stats.feedHz = this.fed / seconds;
      this.frames = 0;
      this.fed = 0;
      this.windowStartMs = nowMs;
    }
    this.stats.idle = audio.idle;
    // Moving to a screen of another density changes the size without the container changing.
    if (nowMs - this.measuredAtMs > 1000) {
      this.measuredAtMs = nowMs;
      this.measure();
    }
    // With no music, or nothing on screen to see, there is no need to draw 60 times a second.
    this.clock.schedule(this.frame, audio.idle || !drawable ? IDLE_INTERVAL_MS : 0);
  };

  private open(): void {
    const engine = VisualizerEngine.tryCreate(this.canvas);
    if (engine) {
      this.engine = engine;
      this.fallback = null;
      engine.setNowPlaying(this.nowPlaying.url, this.nowPlaying.key);
      this.setMode("webgl2");
      return;
    }
    // WebGL2 is not available (or has been refused): use the shared field on a 2D canvas.
    const context = this.canvas.getContext("2d");
    if (context) {
      this.fallback = new SoftwareVisualizer(context);
      this.setMode("canvas2d");
    } else {
      this.setMode("lost");
    }
  }

  private setMode(mode: RenderMode): void {
    this.stats.mode = mode;
    this.onMode?.(mode);
  }

  private measure(): void {
    const rect = this.host.getBoundingClientRect();
    this.cssWidth = rect.width;
    this.cssHeight = rect.height;
    let width: number;
    let height: number;
    if (this.forcedSize) {
      ({ width, height } = this.forcedSize);
    } else {
      const dpr = Math.min(window.devicePixelRatio || 1, MAX_DPR);
      width = this.cssWidth * dpr;
      height = this.cssHeight * dpr;
      const pixels = width * height;
      if (pixels > MAX_PIXELS) {
        const shrink = Math.sqrt(MAX_PIXELS / pixels);
        width *= shrink;
        height *= shrink;
      }
    }
    width = Math.floor(width);
    height = Math.floor(height);
    if (width < 2 || height < 2) {
      this.stats.width = 0;
      this.stats.height = 0;
      return;
    }
    if (width === this.stats.width && height === this.stats.height) return;
    this.stats.width = width;
    this.stats.height = height;
    if (this.engine) {
      this.engine.resize(width, height);
    } else if (this.fallback) {
      this.canvas.width = width;
      this.canvas.height = height;
      this.fallback.resize(width, height);
    }
  }

  private readonly contextLost = (event: Event): void => {
    // Without this the browser does not try to give the context back.
    event.preventDefault();
    try {
      this.engine?.dispose();
    } catch {
      // The context is already gone; there is nothing left to free.
    }
    this.engine = null;
    this.setMode("lost");
  };

  private readonly contextRestored = (): void => {
    this.stats.width = 0;
    this.stats.height = 0;
    this.open();
    this.measure();
  };

  private readonly visibilityChanged = (): void => {
    if (!document.hidden && this.running) {
      // Don't treat the time away as one long frame.
      this.lastMs = this.clock.now();
    }
  };
}
