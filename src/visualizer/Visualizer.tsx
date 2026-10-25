import { useEffect, useRef, useState } from "react";
import type { Clock } from "./clock";
import type { VisualizerFeed } from "./feed";
import { DEFAULT_GASLIGHT, type GaslightSettings } from "./gaslightSettings";
import { VisualizerRuntime, type RenderMode, type RuntimeStats } from "./runtime";
import { STYLE_LABELS, nextStyle, type Palette, type ScopeMode, type VisualizerStyle } from "./styles";
import "./Visualizer.css";

export interface NowPlaying {
  title: string;
  artist: string;
  /** Anything an `<img>` can load. Without it, a stand-in made from the title is shown. */
  artworkUrl?: string | null;
}

export interface VisualizerProps {
  /** Where analysis frames come from, about 60 a second. `null` while nothing is playing. */
  feed: VisualizerFeed | null;
  nowPlaying: NowPlaying | null;
  style: VisualizerStyle;
  onStyleChange: (style: VisualizerStyle) => void;
  /**
   * Keeps the picture from flashing more than three times a second (WCAG 2.3.1), by clamping
   * how fast its overall brightness can swing. See `flash.ts`. On unless set to `false`.
   */
  limitFlashing?: boolean;
  /** Called for the F key. The app does the real full screen. */
  onToggleFullscreen?: () => void;
  /** Called for Esc, unless a sheet or menu is open above the picture. The app hides the visualizer. */
  onClose?: () => void;
  /** Spectrum's colours: the LCD's cream (the default) or a dark variant. */
  palette?: Palette;
  /** What Scope draws; both channels' traces beside the stereo figure by default. */
  scopeMode?: ScopeMode;
  gaslight?: GaslightSettings;
  /** For tests and benchmarks: supply the clock and force a size. */
  clock?: Clock;
  forceSize?: { width: number; height: number };
  /** Lets a benchmark reach the running renderer. */
  onRuntime?: (runtime: VisualizerRuntime | null) => void;
  /** Shows the info corner from the start. */
  showInfo?: boolean;
}

/**
 * The visualizer. It fills its container, draws at the display's refresh rate, and listens for
 * the left and right arrow keys (change style), F (full screen), M (an info corner) and Esc (close).
 */
export function Visualizer({
  feed,
  nowPlaying,
  style,
  onStyleChange,
  limitFlashing = true,
  onToggleFullscreen,
  onClose,
  palette = "cream",
  scopeMode = "both",
  gaslight = DEFAULT_GASLIGHT,
  clock,
  forceSize,
  onRuntime,
  showInfo = false,
}: VisualizerProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [runtime, setRuntime] = useState<VisualizerRuntime | null>(null);
  const [mode, setMode] = useState<RenderMode>("webgl2");
  const [info, setInfo] = useState(showInfo);
  const [fallbackStyle, setFallbackStyle] = useState<VisualizerStyle | null>(null);

  // The key handler is attached once and reads the latest of these.
  const latest = useRef({ style, onStyleChange, onToggleFullscreen, onClose });
  latest.current = { style, onStyleChange, onToggleFullscreen, onClose };

  useEffect(() => {
    const host = hostRef.current;
    const canvas = canvasRef.current;
    if (!host || !canvas) return;
    const created = new VisualizerRuntime(canvas, host, clock, setMode);
    if (forceSize) created.forceSize(forceSize.width, forceSize.height);
    created.start();
    setRuntime(created);
    onRuntime?.(created);
    return () => {
      created.dispose();
      setRuntime(null);
      onRuntime?.(null);
    };
    // The runtime is made once; everything else reaches it through the effects below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => runtime?.setFeed(feed), [runtime, feed]);
  useEffect(
    () => runtime?.setOptions({ style, palette, scopeMode, limitFlashing, gaslight }),
    [runtime, style, palette, scopeMode, limitFlashing, gaslight],
  );

  // Shader fallback can happen after context setup. Keep labels honest without React work
  // on every animation frame. Software rendering resolves immediately from the chosen style.
  useEffect(() => {
    setFallbackStyle(null);
    if (!runtime) return;
    const timer = setInterval(() => {
      const actual = runtime.stats.actualStyle;
      setFallbackStyle(actual === style ? null : actual);
    }, 500);
    return () => clearInterval(timer);
  }, [runtime, style]);

  const songKey = nowPlaying ? `${nowPlaying.title}\u0000${nowPlaying.artist}` : "";
  const artworkUrl = nowPlaying?.artworkUrl ?? null;
  useEffect(() => runtime?.setNowPlaying(artworkUrl, songKey), [runtime, artworkUrl, songKey]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey || event.defaultPrevented) return;
      if (isTyping(event.target)) return;
      const { style: current, onStyleChange: change, onToggleFullscreen: fullscreen, onClose: close } = latest.current;
      switch (event.key) {
        case "ArrowLeft":
          change(nextStyle(current, -1));
          break;
        case "ArrowRight":
          change(nextStyle(current, 1));
          break;
        case "f":
        case "F":
          fullscreen?.();
          break;
        case "m":
        case "M":
          setInfo((shown) => !shown);
          break;
        case "Escape":
          if (!close || isOverlayOpen()) return;
          close();
          break;
        default:
          return;
      }
      event.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const plainCanvas = mode === "canvas2d";
  const actualStyle = plainCanvas ? (style === "gaslight" ? "gaslight" : "spectrum") : fallbackStyle ?? style;
  const onLight = (actualStyle === "spectrum" && palette === "cream") || (actualStyle === "gaslight" && gaslight.palette === "inverted");
  const label = `Music visualizer, ${STYLE_LABELS[actualStyle]}`;

  return (
    <div ref={hostRef} className="visualizer" role="img" aria-label={label}>
      <canvas ref={canvasRef} className="visualizer-canvas" />
      {nowPlaying ? (
        <div key={songKey} className={`visualizer-overlay${onLight ? " is-on-light" : ""}`}>
          <div className="visualizer-overlay-title">{nowPlaying.title}</div>
          {nowPlaying.artist ? <div className="visualizer-overlay-artist">{nowPlaying.artist}</div> : null}
        </div>
      ) : null}
      {mode !== "lost" && actualStyle !== style ? (
        <div className="visualizer-note">
          This display can&rsquo;t draw {STYLE_LABELS[style]}, so it is showing {STYLE_LABELS[actualStyle]}.
        </div>
      ) : null}
      {mode === "lost" ? (
        <div className="visualizer-note">The graphics were interrupted. Waiting for them to come back&hellip;</div>
      ) : null}
      {info && runtime ? <InfoCorner runtime={runtime} /> : null}
    </div>
  );
}

/** Style, frame rate and the like, refreshed twice a second. */
function InfoCorner({ runtime }: { runtime: VisualizerRuntime }) {
  const [text, setText] = useState("");
  useEffect(() => {
    const update = () => setText(describe(runtime.stats));
    update();
    const timer = setInterval(update, 500);
    return () => clearInterval(timer);
  }, [runtime]);
  return <div className="visualizer-info">{text}</div>;
}

function describe(stats: RuntimeStats): string {
  const lines = [
    `${STYLE_LABELS[stats.actualStyle]}${stats.mode === "canvas2d" ? " (software)" : ""}`,
    `${stats.fps.toFixed(0)} fps${stats.idle ? " (idle)" : ""}   feed ${stats.feedHz.toFixed(0)} Hz`,
    `${stats.width} × ${stats.height}`,
  ];
  if (stats.gpuTiming) lines.push(`GPU ${stats.gpuMs.toFixed(2)} ms`);
  if (stats.limiterGain !== 1 || stats.limiterVeil !== 0) {
    lines.push(`flash limit  gain ${stats.limiterGain.toFixed(2)}  veil ${stats.limiterVeil.toFixed(2)}`);
  }
  return lines.join("\n");
}

/** A sheet, panel or menu is open over the picture, and Esc is for it: it dismisses itself, or is read by the system. */
function isOverlayOpen(): boolean {
  return document.querySelector('[role="dialog"], [role="alertdialog"], [role="menu"]') !== null;
}

function isTyping(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName);
}
