/**
 * A standalone page for the visualizer, driven by the mock feed.
 *
 * `pnpm dev`, then open /src/visualizer/preview.html. Addresses it understands:
 *
 *   ?style=classic|lights|spectrum|scope|album|pipes|starfield|lines   which style to start on
 *   ?palette=cream|dark   ?scope=wave|xy|both   ?limit=0   ?info=1   ?bpm=120   ?art=0
 *   ?arranged=1           play a 24-second arrangement (quiet start, loud middle, drumless
 *                         breakdown) instead of one steady groove, to see calm and lively
 *   ?seconds=8            run that many seconds at once on a hand-driven clock, then hold still
 *                         (for screenshots); the page sets data-ready="true" on <body> when done
 *   ?size=2560x1440       draw at exactly this many pixels
 *   ?bench=1              time every style at the size above (default 2560x1440) and print JSON
 *   ?flashcheck=1         feed a violent strobe to every style, with the flash limiter off and on,
 *                         and count flashes in the pictures that come out; prints JSON
 *   ?music=1              with ?flashcheck, use the mock music instead of the strobe, to see how
 *                         much ordinary music makes the pictures flash
 *   ?only=pipes,lines     limit ?bench, ?flashcheck and ?alloc to these styles
 *   ?alloc=1              count what drawing allocates (run Chrome with --js-flags="--expose-gc
 *                         --min-semi-space-size=128" and --enable-precise-memory-info)
 *   ?nogl=1               pretend WebGL2 is missing, to see the 2D fallback
 *   ?strobe=1             the strobe as the music, to watch the flash limiter work
 */
import React, { useEffect, useMemo, useRef, useState } from "react";
import ReactDOM from "react-dom/client";
import "../theme/tokens.css";
import { ManualClock } from "./clock";
import { BAND_COUNT, MockFeed, WAVEFORM_LENGTH, type VisualizerFeed, type VisualizerFrame } from "./feed";
import { worstChangesPerWindow } from "./flash";
import type { VisualizerRuntime } from "./runtime";
import { VISUALIZER_STYLES, STYLE_LABELS, type Palette, type ScopeMode, type VisualizerStyle } from "./styles";
import { Visualizer, type NowPlaying } from "./Visualizer";

const params = new URLSearchParams(location.search);
const FRAME_MS = 1000 / 60;

if (params.get("nogl") === "1") {
  const original = HTMLCanvasElement.prototype.getContext;
  HTMLCanvasElement.prototype.getContext = function (this: HTMLCanvasElement, type: string, ...rest: unknown[]) {
    if (type === "webgl2" || type === "webgl" || type === "experimental-webgl") return null;
    return (original as (...args: unknown[]) => RenderingContext | null).call(this, type, ...rest);
  } as typeof original;
}

/**
 * The worst music imaginable for a photosensitive viewer: every band and the whole waveform
 * slamming between full and nearly nothing, eight times a second.
 */
class StrobeFeed implements VisualizerFeed {
  private readonly listeners = new Set<(frame: VisualizerFrame) => void>();
  private timer: ReturnType<typeof setInterval> | null = null;
  private lastOn = false;

  onFrame(callback: (frame: VisualizerFrame) => void): () => void {
    this.listeners.add(callback);
    return () => this.listeners.delete(callback);
  }

  start(): void {
    if (this.timer === null) this.timer = setInterval(() => this.emit(performance.now()), FRAME_MS);
  }

  stop(): void {
    if (this.timer !== null) clearInterval(this.timer);
    this.timer = null;
  }

  emit(tMs: number): void {
    const on = Math.floor(tMs / 62.5) % 2 === 0;
    const level = on ? 1 : 0.02;
    const frame: VisualizerFrame = {
      tMs,
      bands: new Array<number>(BAND_COUNT).fill(level),
      peaks: new Array<number>(BAND_COUNT).fill(level),
      waveformL: Array.from({ length: WAVEFORM_LENGTH }, (_, i) => level * Math.sin(i / 4)),
      waveformR: Array.from({ length: WAVEFORM_LENGTH }, (_, i) => level * Math.cos(i / 4)),
      rms: level,
      beat: on && !this.lastOn,
      beatStrength: 1,
      onset: level,
      tempoBpm: 480,
    };
    this.lastOn = on;
    for (const listener of this.listeners) listener(frame);
  }
}

/** One prepared frame, handed out again and again, so what the feed itself allocates is nothing. */
class RepeatFeed implements VisualizerFeed {
  private readonly listeners = new Set<(frame: VisualizerFrame) => void>();
  private readonly frame = new MockFeed().frameAt(0);

  onFrame(callback: (frame: VisualizerFrame) => void): () => void {
    this.listeners.add(callback);
    return () => this.listeners.delete(callback);
  }

  emit(): void {
    this.frame.beat = false;
    for (const listener of this.listeners) listener(this.frame);
  }
}

const SONGS: NowPlaying[] = [
  { title: "Midnight Circuit", artist: "The Mock Orchestra" },
  { title: "A Rather Long Title For A Song About Nothing In Particular", artist: "Various Artists & Friends" },
  { title: "Low Tide", artist: "Harbour Lights" },
];

function isStyle(value: string | null): value is VisualizerStyle {
  return VISUALIZER_STYLES.some((style) => style === value);
}

/** The styles the scripted runs go through: those named by `?only`, or all. */
function scriptedStyles(): readonly VisualizerStyle[] {
  const named = (params.get("only") ?? "").split(",").filter(isStyle);
  return named.length > 0 ? named : VISUALIZER_STYLES;
}

function parseSize(value: string | null): { width: number; height: number } | undefined {
  const match = /^(\d+)x(\d+)$/.exec(value ?? "");
  return match ? { width: Number(match[1]), height: Number(match[2]) } : undefined;
}

/** A busy picture to stand in for cover art. */
function makeCover(seed: number): string {
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 512;
  const g = canvas.getContext("2d");
  if (!g) return "";
  const hue = (seed * 97 + 20) % 360;
  const sky = g.createLinearGradient(0, 0, 0, 512);
  sky.addColorStop(0, `hsl(${hue} 80% 58%)`);
  sky.addColorStop(0.62, `hsl(${(hue + 40) % 360} 85% 48%)`);
  sky.addColorStop(1, `hsl(${(hue + 200) % 360} 60% 14%)`);
  g.fillStyle = sky;
  g.fillRect(0, 0, 512, 512);
  g.fillStyle = `hsl(${(hue + 50) % 360} 100% 82%)`;
  g.beginPath();
  g.arc(340, 190, 80, 0, Math.PI * 2);
  g.fill();
  for (let i = 0; i < 4; i++) {
    g.fillStyle = `hsl(${(hue + 210 + i * 12) % 360} 55% ${24 - i * 4}%)`;
    g.beginPath();
    g.moveTo(0, 512);
    for (let x = 0; x <= 512; x += 16) {
      g.lineTo(x, 330 + i * 46 + Math.sin(x * 0.02 + i * 2 + seed) * 30 + Math.sin(x * 0.07 + seed * 3) * 8);
    }
    g.lineTo(512, 512);
    g.fill();
  }
  return canvas.toDataURL("image/png");
}

function Preview() {
  const startStyle = params.get("style");
  const [style, setStyle] = useState<VisualizerStyle>(isStyle(startStyle) ? startStyle : "classic");
  const [palette, setPalette] = useState<Palette>(params.get("palette") === "dark" ? "dark" : "cream");
  const [scopeMode, setScopeMode] = useState<ScopeMode>(
    ["wave", "xy", "both"].includes(params.get("scope") ?? "") ? (params.get("scope") as ScopeMode) : "both",
  );
  const [limit, setLimit] = useState(params.get("limit") !== "0");
  const [playing, setPlaying] = useState(true);
  const [songIndex, setSongIndex] = useState(0);
  const [bpm, setBpm] = useState(Number(params.get("bpm") ?? 120));
  const size = parseSize(params.get("size"));
  const seconds = Number(params.get("seconds") ?? 0);
  const bench = params.get("bench") === "1";
  const flashCheck = params.get("flashcheck") === "1";
  const alloc = params.get("alloc") === "1";
  const manual = seconds > 0 || bench || flashCheck || alloc;
  const [strobe, setStrobe] = useState(params.get("strobe") === "1");
  const [closed, setClosed] = useState(false);

  const clock = useMemo(() => (manual ? new ManualClock() : undefined), [manual]);
  const arranged = params.get("arranged") === "1";
  const music = params.get("music") === "1";
  const feed = useMemo<MockFeed | StrobeFeed | RepeatFeed>(
    () => (alloc ? new RepeatFeed() : (flashCheck && !music) || strobe ? new StrobeFeed() : new MockFeed({ bpm, arranged })),
    [bpm, arranged, strobe, flashCheck, music, alloc],
  );
  const covers = useMemo(() => SONGS.map((_, i) => makeCover(i + 1)), []);
  const song: NowPlaying = {
    ...SONGS[songIndex % SONGS.length],
    artworkUrl: params.get("art") === "0" ? null : covers[songIndex % covers.length],
  };
  const runtimeRef = useRef<VisualizerRuntime | null>(null);

  // On a real clock the mock feed runs itself; on a hand-driven one the steps below drive it.
  useEffect(() => {
    if (manual || !("start" in feed)) return;
    if (playing) feed.start();
    else feed.stop();
    return () => feed.stop();
  }, [feed, playing, manual]);

  // Handles for the scripts that test this page from outside.
  useEffect(() => {
    // The extension cannot be asked for once the context is lost, so it is kept from before.
    let extension: WEBGL_lose_context | null | undefined;
    Object.assign(window, {
      __preview: {
        loseContext: () => {
          extension = document.querySelector("canvas")?.getContext("webgl2")?.getExtension("WEBGL_lose_context");
          extension?.loseContext();
        },
        restoreContext: () => extension?.restoreContext(),
        stats: () => runtimeRef.current?.stats,
      },
    });
  }, []);

  const onRuntime = (runtime: VisualizerRuntime | null) => {
    runtimeRef.current = runtime;
    if (!runtime || !manual || !clock) return;
    // React runs effects twice in development; only the runtime that survives is scripted.
    setTimeout(() => {
      if (runtimeRef.current !== runtime || document.body.dataset.ready) return;
      const mode = flashCheck ? "flashcheck" : alloc ? "alloc" : bench ? "bench" : "still";
      void runScripted(runtime, clock, feed, seconds, mode, size);
    }, 50);
  };

  if (closed) return <p style={{ color: "#ddd", font: "14px sans-serif", margin: 16 }}>Closed with Esc. Reload to bring it back.</p>;

  return (
    <>
      <Visualizer
        feed={feed}
        nowPlaying={song}
        style={style}
        onStyleChange={setStyle}
        limitFlashing={limit}
        palette={palette}
        scopeMode={scopeMode}
        clock={clock}
        forceSize={size ?? (bench ? { width: 2560, height: 1440 } : undefined)}
        onRuntime={onRuntime}
        showInfo={params.get("info") === "1"}
        onClose={() => setClosed(true)}
        onToggleFullscreen={() => {
          if (document.fullscreenElement) void document.exitFullscreen();
          else void document.documentElement.requestFullscreen();
        }}
      />
      {manual ? null : (
        <div style={panel}>
          <div style={row}>
            {VISUALIZER_STYLES.map((id) => (
              <button key={id} style={button(id === style)} onClick={() => setStyle(id)}>
                {STYLE_LABELS[id]}
              </button>
            ))}
          </div>
          <div style={row}>
            <button style={button(false)} onClick={() => setPlaying((p) => !p)}>
              {playing ? "Pause music" : "Play music"}
            </button>
            <button style={button(false)} onClick={() => setSongIndex((i) => i + 1)}>
              Next song
            </button>
            <label>
              <input type="checkbox" checked={limit} onChange={(e) => setLimit(e.target.checked)} /> limit flashing
            </label>
            <label>
              <input type="checkbox" checked={strobe} onChange={(e) => setStrobe(e.target.checked)} /> strobe stress test
            </label>
            <label>
              <input type="checkbox" checked={palette === "dark"} onChange={(e) => setPalette(e.target.checked ? "dark" : "cream")} /> dark spectrum
            </label>
          </div>
          <div style={row}>
            <label>
              scope{" "}
              <select value={scopeMode} onChange={(e) => setScopeMode(e.target.value as ScopeMode)}>
                <option value="both">both</option>
                <option value="wave">traces</option>
                <option value="xy">X/Y</option>
              </select>
            </label>
            <label>
              tempo {bpm} <input type="range" min={60} max={480} value={bpm} onChange={(e) => setBpm(Number(e.target.value))} />
            </label>
            <span>&larr; &rarr; style &nbsp; F full screen &nbsp; M info &nbsp; Esc close</span>
          </div>
        </div>
      )}
    </>
  );
}

const panel: React.CSSProperties = {
  position: "fixed",
  right: 12,
  bottom: 12,
  display: "flex",
  flexDirection: "column",
  gap: 6,
  padding: 8,
  borderRadius: 8,
  background: "rgba(0,0,0,0.6)",
  color: "#ddd",
  font: "11px/1.3 -apple-system, sans-serif",
  opacity: 0.45,
};
const row: React.CSSProperties = { display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" };
const button = (on: boolean): React.CSSProperties => ({
  padding: "3px 9px",
  borderRadius: 4,
  border: "1px solid #666",
  background: on ? "#4a74b8" : "#2b2b2b",
  color: "#fff",
  font: "inherit",
  cursor: "pointer",
});

/** Runs the feed and the renderer by hand: for screenshots, timing and tests. */
async function runScripted(
  runtime: VisualizerRuntime,
  clock: ManualClock,
  feed: { emit(tMs: number): void },
  seconds: number,
  mode: "still" | "bench" | "flashcheck" | "alloc",
  size: { width: number; height: number } | undefined,
): Promise<void> {
  const steps = (n: number) => {
    for (let i = 0; i < n; i++) {
      clock.advance(FRAME_MS);
      feed.emit(clock.now());
    }
  };
  const pause = () => new Promise<void>((resolve) => setTimeout(resolve, 0));
  const report = (results: unknown) => {
    const output = document.createElement("pre");
    output.id = "bench";
    output.textContent = JSON.stringify(results, null, 1);
    output.style.cssText =
      "position:fixed;left:0;top:0;margin:0;padding:8px;background:#000;color:#9f9;font:12px monospace;z-index:9";
    document.body.append(output);
    document.body.dataset.ready = "true";
  };

  if (mode === "still") {
    steps(Math.round(seconds * 60));
    document.body.dataset.ready = "true";
    return;
  }

  if (mode === "alloc") {
    const gc = (window as unknown as { gc?: () => void }).gc;
    const memory = () => (performance as unknown as { memory: { usedJSHeapSize: number } }).memory.usedJSHeapSize;
    const results: Record<string, unknown> = { gcAvailable: typeof gc === "function" };
    for (const id of scriptedStyles()) {
      for (const limit of [false, true]) {
        runtime.setOptions({ style: id, limitFlashing: limit });
        steps(200); // warm up: compile shaders, grow buffers
        gc?.();
        const before = memory();
        const frames = 1500;
        steps(frames);
        results[`${id}${limit ? " + limiter" : ""}`] = { bytesPerFrame: Math.round((memory() - before) / frames) };
      }
    }
    report(results);
    return;
  }

  if (mode === "flashcheck") {
    const canvas = document.querySelector("canvas");
    if (!canvas) return report({ error: "no canvas" });
    // The picture is shrunk by halves, which averages it properly; one big step would sample a
    // few pixels and make sparse specks look like flicker.
    const stages: { canvas: HTMLCanvasElement; context: CanvasRenderingContext2D }[] = [];
    for (let width = canvas.width >> 1, height = canvas.height >> 1; ; width >>= 1, height >>= 1) {
      const stage = document.createElement("canvas");
      stage.width = Math.max(16, width);
      stage.height = Math.max(9, height);
      const context = stage.getContext("2d", { willReadFrequently: true });
      if (!context) return report({ error: "no 2d context" });
      context.imageSmoothingQuality = "high";
      stages.push({ canvas: stage, context });
      if (width <= 16) break;
    }
    const luminance = () => {
      let from: HTMLCanvasElement = canvas;
      for (const stage of stages) {
        stage.context.drawImage(from, 0, 0, stage.canvas.width, stage.canvas.height);
        from = stage.canvas;
      }
      const last = stages[stages.length - 1];
      const pixels = last.context.getImageData(0, 0, last.canvas.width, last.canvas.height).data;
      let sum = 0;
      for (let i = 0; i < pixels.length; i += 4) {
        sum +=
          0.2126 * (pixels[i] / 255) ** 2.2 + 0.7152 * (pixels[i + 1] / 255) ** 2.2 + 0.0722 * (pixels[i + 2] / 255) ** 2.2;
      }
      return sum / (pixels.length / 4);
    };
    const results: Record<string, unknown> = {
      note: "changes of 10% or more of relative luminance, the most in any second; 6 is three flashes",
    };
    for (const id of scriptedStyles()) {
      for (const palette of id === "spectrum" ? (["cream", "dark"] as const) : (["cream"] as const)) {
        for (const limit of [false, true]) {
          runtime.setOptions({ style: id, limitFlashing: limit, palette });
          steps(90);
          const samples: number[] = [];
          for (let i = 0; i < 360; i++) {
            steps(1);
            samples.push(luminance());
          }
          const label = `${id}${id === "spectrum" ? ` (${palette})` : ""}${limit ? " + limiter" : ""}`;
          results[label] = {
            changesPerSecond: worstChangesPerWindow(samples, FRAME_MS),
            luminanceRange: [Math.min(...samples), Math.max(...samples)].map((v) => round(v * 1000) / 1000),
          };
          await pause();
        }
      }
    }
    return report(results);
  }

  // A timing run. The timer queries only report back once the page has returned to its event
  // loop, so each frame is followed by a pause. `cpuMs` is the time spent in the call that draws
  // the frame, which with the limiter on includes waiting for the GPU to finish it.
  const results: Record<string, unknown> = {
    size: size ?? { width: 2560, height: 1440 },
    mode: runtime.stats.mode,
    renderer: runtime.renderer,
    gpuTiming: false,
    userAgent: navigator.userAgent,
  };
  const frames = 240;
  for (const id of scriptedStyles()) {
    for (const limit of [false, true]) {
      runtime.setOptions({ style: id, limitFlashing: limit });
      for (let i = 0; i < 120; i++) {
        steps(1);
        await pause();
      }
      const cpu: number[] = [];
      const gpu: number[] = [];
      for (let i = 0; i < frames; i++) {
        const t0 = performance.now();
        steps(1);
        cpu.push(performance.now() - t0);
        await pause();
        gpu.push(runtime.stats.gpuMs);
      }
      cpu.sort((a, b) => a - b);
      gpu.sort((a, b) => a - b);
      results[`${id}${limit ? " + limiter" : ""}`] = {
        gpuMs: round(gpu[frames >> 1]),
        gpuMsP95: round(gpu[Math.floor(frames * 0.95)]),
        cpuMs: round(cpu[frames >> 1]),
      };
      results.gpuTiming = runtime.stats.gpuTiming;
    }
  }
  report(results);
}

function round(value: number): number {
  return Math.round(value * 100) / 100;
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <Preview />
  </React.StrictMode>,
);
