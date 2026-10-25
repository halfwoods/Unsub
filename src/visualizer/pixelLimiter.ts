import { FlashLimiter } from "./flash.ts";

const LINEAR = Float32Array.from({ length: 256 }, (_, value) => (value / 255) ** 2.2);

/** The GPU compositor's linear-light transform, applied to bounded software-renderer pixels. */
export class PixelLimiter {
  private readonly limiter = new FlashLimiter();
  private readonly transfer = new Uint8ClampedArray(256);
  gain = 1;
  veil = 0;
  luminance = 0;

  apply(pixels: Uint8ClampedArray, dt: number, enabled: boolean): void {
    this.gain = 1;
    this.veil = 0;
    if (!enabled) { this.limiter.reset(); return; }
    let sum = 0;
    for (let i = 0; i < pixels.length; i += 4) {
      sum += LINEAR[pixels[i]] * 0.2126 + LINEAR[pixels[i + 1]] * 0.7152 + LINEAR[pixels[i + 2]] * 0.0722;
    }
    this.luminance = sum / (pixels.length / 4);
    const limit = this.limiter.update(dt * 1000, this.luminance);
    this.gain = limit.gain;
    this.veil = limit.veil;
    if (this.gain === 1 && this.veil === 0) return;
    for (let i = 0; i < 256; i++) this.transfer[i] = 255 * (LINEAR[i] * this.gain + this.veil) ** (1 / 2.2);
    for (let i = 0; i < pixels.length; i += 4) {
      pixels[i] = this.transfer[pixels[i]];
      pixels[i + 1] = this.transfer[pixels[i + 1]];
      pixels[i + 2] = this.transfer[pixels[i + 2]];
    }
  }
}
