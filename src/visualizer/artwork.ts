import type { ArtworkSource } from "./gl";

/** How much of the cover's colour range to pay attention to: vivid pixels count more. */
const TINT_SAMPLE_SIZE = 16;

/**
 * The cover art of the song playing, as a texture, with its average colour. Without a cover (or
 * until it has loaded) there is a stand-in whose colours come from the title and artist, so every
 * song looks like itself.
 */
export class Artwork implements ArtworkSource {
  texture: WebGLTexture | null = null;
  readonly tint = new Float32Array([0.5, 0.5, 0.5]);
  age = 0;
  private request = 0;

  constructor(private readonly gl: WebGL2RenderingContext) {}

  /** Seconds since the cover last changed; the album style fades the new one in with it. */
  tick(dt: number): void {
    this.age += dt;
  }

  /**
   * Shows `url` when it has loaded, and meanwhile a stand-in made from `key` (the title and
   * artist). A newer call supersedes an older one that has not finished.
   */
  show(url: string | null, key: string): void {
    const request = ++this.request;
    const stand = standIn(key);
    this.replace(stand, stand.getContext("2d"));
    if (!url) return;
    const image = new Image();
    image.crossOrigin = "anonymous";
    image.decoding = "async";
    image.onload = () => {
      if (request !== this.request) return;
      this.replace(image, null);
    };
    image.src = url;
  }

  private replace(source: HTMLImageElement | HTMLCanvasElement, context: CanvasRenderingContext2D | null): void {
    const gl = this.gl;
    const texture = gl.createTexture();
    if (!texture) return;
    gl.bindTexture(gl.TEXTURE_2D, texture);
    gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, true);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, gl.RGBA, gl.UNSIGNED_BYTE, source);
    gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, false);
    gl.generateMipmap(gl.TEXTURE_2D);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    if (this.texture) gl.deleteTexture(this.texture);
    this.texture = texture;
    this.age = 0;
    this.measureTint(source, context);
  }

  private measureTint(source: HTMLImageElement | HTMLCanvasElement, context: CanvasRenderingContext2D | null): void {
    try {
      const sampler = context ? null : document.createElement("canvas");
      let reader = context;
      if (sampler) {
        sampler.width = sampler.height = TINT_SAMPLE_SIZE;
        reader = sampler.getContext("2d", { willReadFrequently: true });
        reader?.drawImage(source, 0, 0, TINT_SAMPLE_SIZE, TINT_SAMPLE_SIZE);
      }
      if (!reader) return;
      const size = sampler ? TINT_SAMPLE_SIZE : (source as HTMLCanvasElement).width;
      const pixels = reader.getImageData(0, 0, size, size).data;
      let r = 0;
      let g = 0;
      let b = 0;
      let weight = 0;
      for (let i = 0; i < pixels.length; i += 4) {
        const max = Math.max(pixels[i], pixels[i + 1], pixels[i + 2]);
        const min = Math.min(pixels[i], pixels[i + 1], pixels[i + 2]);
        // Vivid, not-too-dark pixels say more about a cover's colour than grey ones.
        const w = 0.05 + (max - min) / 255 + (max > 40 ? 0.15 : 0);
        r += pixels[i] * w;
        g += pixels[i + 1] * w;
        b += pixels[i + 2] * w;
        weight += w;
      }
      this.tint[0] = r / weight / 255;
      this.tint[1] = g / weight / 255;
      this.tint[2] = b / weight / 255;
    } catch {
      // A cover from another origin without permission cannot be read back; keep the last tint.
    }
  }

  dispose(): void {
    this.request += 1;
    if (this.texture) this.gl.deleteTexture(this.texture);
    this.texture = null;
  }
}

/** A square of colour from `key`: two hues and a note, so a missing cover is not just grey. */
function standIn(key: string): HTMLCanvasElement {
  let hash = 2166136261;
  for (let i = 0; i < key.length; i++) hash = Math.imul(hash ^ key.charCodeAt(i), 16777619) >>> 0;
  const hueA = hash % 360;
  const hueB = (hueA + 40 + ((hash >>> 9) % 80)) % 360;
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 256;
  const context = canvas.getContext("2d", { willReadFrequently: true });
  if (!context) return canvas;
  const fill = context.createLinearGradient(0, 0, 256, 256);
  fill.addColorStop(0, `hsl(${hueA} 62% 46%)`);
  fill.addColorStop(1, `hsl(${hueB} 70% 22%)`);
  context.fillStyle = fill;
  context.fillRect(0, 0, 256, 256);
  context.fillStyle = "rgba(255,255,255,0.22)";
  context.font = "bold 150px serif";
  context.textAlign = "center";
  context.textBaseline = "middle";
  context.fillText("♫", 128, 138);
  return canvas;
}
