import type { AudioState } from "./audio";

export const VISUALIZER_STYLES = ["classic", "lights", "spectrum", "scope", "album"] as const;
export type VisualizerStyle = (typeof VISUALIZER_STYLES)[number];

export const STYLE_LABELS: Record<VisualizerStyle, string> = {
  classic: "Classic",
  lights: "Lights",
  spectrum: "Spectrum",
  scope: "Scope",
  album: "Album",
};

/** Spectrum's colours: the iTunes 10 LCD's cream, or a dark variant of it. */
export type Palette = "cream" | "dark";

/** What Scope draws: two traces, the left channel against the right, or both side by side. */
export type ScopeMode = "wave" | "xy" | "both";

/** What changes from frame to frame, shared by all styles. */
export interface RenderInfo {
  /** Seconds, advancing whether or not music plays. */
  time: number;
  /** Seconds since the last frame. */
  dt: number;
  width: number;
  height: number;
  audio: AudioState;
  palette: Palette;
  scopeMode: ScopeMode;
}

/** A style draws the whole picture into the scene target each frame. */
export interface StyleRenderer {
  /** The size of the scene, in pixels. Called before the first render and whenever it changes. */
  resize(width: number, height: number): void;
  render(info: RenderInfo): void;
  dispose(): void;
}

export function nextStyle(style: VisualizerStyle, step: 1 | -1): VisualizerStyle {
  const count = VISUALIZER_STYLES.length;
  const index = VISUALIZER_STYLES.indexOf(style);
  return VISUALIZER_STYLES[(index + step + count) % count];
}
