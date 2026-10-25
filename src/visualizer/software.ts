import type { AudioState } from "./audio";
import { GaslightField } from "./gaslight";
import type { GaslightSettings } from "./gaslightSettings";
import { PixelLimiter } from "./pixelLimiter";
import { Spectrum2D } from "./spectrum2d";
import type { Palette, VisualizerStyle } from "./styles";

/** Gaslight and Spectrum stay available without WebGL2. All output passes through the limiter. */
export class SoftwareVisualizer {
  readonly limiter = new PixelLimiter();
  private readonly field = new GaslightField();
  private readonly stage = document.createElement("canvas");
  private readonly stageContext: CanvasRenderingContext2D;
  private readonly image: ImageData;
  private readonly spectrum: Spectrum2D;
  private width = 0;
  private height = 0;

  constructor(private readonly context: CanvasRenderingContext2D) {
    this.stage.width = this.field.width;
    this.stage.height = this.field.height;
    const stageContext = this.stage.getContext("2d");
    if (!stageContext) throw new Error("could not create the software visualizer");
    this.stageContext = stageContext;
    this.image = stageContext.createImageData(this.field.width, this.field.height);
    this.spectrum = new Spectrum2D(stageContext);
    this.spectrum.resize(this.field.width, this.field.height);
  }

  resize(width: number, height: number): void { this.width = width; this.height = height; }

  render(audio: AudioState, dt: number, style: VisualizerStyle, palette: Palette, settings: GaslightSettings, limit: boolean): void {
    if (style === "gaslight") {
      this.image.data.set(this.field.render(dt, audio, settings));
      this.limiter.apply(this.image.data, dt, limit);
      this.stageContext.putImageData(this.image, 0, 0);
    } else {
      this.spectrum.render(audio, palette);
      if (limit) {
        const pixels = this.stageContext.getImageData(0, 0, this.stage.width, this.stage.height);
        this.limiter.apply(pixels.data, dt, true);
        this.stageContext.putImageData(pixels, 0, 0);
      } else {
        this.limiter.apply(this.image.data, dt, false);
      }
    }
    this.context.imageSmoothingEnabled = true;
    this.context.drawImage(this.stage, 0, 0, this.width, this.height);
  }

  dispose(): void {
    this.stage.width = this.stage.height = 0;
  }
}
