import type { AudioState } from "./audio";
import { BAND_COUNT } from "./feed";
import type { Palette } from "./styles";

const CREAM = {
  top: "#f0f2e6",
  upper: "#eaeddb",
  lower: "#e5e9d1",
  bottom: "#f8fae4",
  bar: "#494b3f",
  barTop: "#2a2b24",
  cap: "#2a2b24",
};
const DARK = {
  top: "#0e0f0b",
  upper: "#13140f",
  lower: "#1a1b14",
  bottom: "#21221a",
  bar: "#c9cdb3",
  barTop: "#f0f2e6",
  cap: "#fffef4",
};

/**
 * The Spectrum style on a plain 2D canvas, for when WebGL2 is not there. Same bars, caps and
 * reflection, without the shader polish. Composited by SoftwareVisualizer.
 */
export class Spectrum2D {
  private width = 0;
  private height = 0;
  private background: CanvasGradient | null = null;
  private palette: Palette | null = null;

  constructor(private readonly context: CanvasRenderingContext2D) {}

  resize(width: number, height: number): void {
    this.width = width;
    this.height = height;
    this.palette = null;
  }

  render(audio: AudioState, palette: Palette): void {
    const context = this.context;
    const { width, height } = this;
    if (width === 0) return;
    const colors = palette === "dark" ? DARK : CREAM;
    if (this.palette !== palette) {
      this.palette = palette;
      const horizon = height * 0.6;
      const background = context.createLinearGradient(0, 0, 0, height);
      background.addColorStop(0, colors.top);
      background.addColorStop(horizon / height - 0.001, colors.upper);
      background.addColorStop(horizon / height, colors.lower);
      background.addColorStop(1, colors.bottom);
      this.background = background;
    }
    context.globalAlpha = 1;
    context.fillStyle = this.background ?? colors.top;
    context.fillRect(0, 0, width, height);

    const horizon = height * 0.6;
    const margin = width * 0.03;
    const slot = (width - margin * 2) / BAND_COUNT;
    const barWidth = slot * 0.68;
    const maxHeight = horizon * 0.86;
    for (let i = 0; i < BAND_COUNT; i++) {
      const x = margin + slot * i + (slot - barWidth) / 2;
      const barHeight = Math.max(2, audio.bands[i] * maxHeight);
      const capHeight = Math.max(2, audio.peaks[i] * maxHeight);
      context.fillStyle = colors.bar;
      // The reflection: paler, and shorter than the bar.
      context.globalAlpha = 0.16;
      context.fillRect(x, horizon, barWidth, barHeight * 0.55);
      context.globalAlpha = 1;
      context.fillStyle = colors.bar;
      context.fillRect(x, horizon - barHeight, barWidth, barHeight);
      context.fillStyle = colors.barTop;
      context.fillRect(x, horizon - barHeight, barWidth, Math.min(barHeight, 3));
      context.fillStyle = colors.cap;
      context.fillRect(x, horizon - capHeight - 7, barWidth, 3);
    }
  }
}
