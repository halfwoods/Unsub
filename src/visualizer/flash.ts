/**
 * Photosensitive-viewer safety: keeps the picture from flashing more than three times a second.
 *
 * WCAG 2.3.1 calls a flash a pair of opposing changes in relative luminance of 10% or more of the
 * maximum (when the darker state is below 0.8), over a large part of the screen. This limits the
 * thing directly. Every frame the average relative luminance L of the whole picture is measured,
 * and the picture is shown with that average clamped to within `deadband` of a slow average of
 * what was shown. A strobe can then swing the picture's average by at most twice the deadband,
 * under the 10% that makes a change count, however fast it goes; a swell or fade that takes a
 * few tenths of a second or more still passes, because the slow average follows it. The price is
 * that the average cannot change faster than `deadband / windowMs` (about 0.1 a second).
 *
 * Too bright is fixed by scaling the picture down (`gain`), too dark by lifting it with a flat
 * `veil`, both applied in linear light in the final compositing pass, so every style obeys it.
 * It is a limit on the average of the whole picture, not of each region, so a small bright area
 * can still flash; that is also what the guideline allows.
 */
export interface FlashLimit {
  /** Multiplies linear light. 1 when nothing needs doing. */
  gain: number;
  /** Added to linear light after the gain. 0 when nothing needs doing. */
  veil: number;
}

export interface FlashLimiterOptions {
  /** Half the swing allowed around the slow average, in relative luminance. */
  deadband?: number;
  /** Time constant of the slow average, in milliseconds. */
  windowMs?: number;
  /** The picture is never dimmed below this share of itself, or lifted by more than `maxVeil`. */
  minGain?: number;
  maxVeil?: number;
}

export class FlashLimiter {
  private readonly deadband: number;
  private readonly windowMs: number;
  private readonly minGain: number;
  private readonly maxVeil: number;
  private average = -1;
  /** Reused: `update` returns this object. */
  private readonly limit: FlashLimit = { gain: 1, veil: 0 };

  constructor(options: FlashLimiterOptions = {}) {
    this.deadband = options.deadband ?? 0.035;
    this.windowMs = options.windowMs ?? 300;
    this.minGain = options.minGain ?? 0.2;
    this.maxVeil = options.maxVeil ?? 0.3;
  }

  reset(): void {
    this.average = -1;
    this.limit.gain = 1;
    this.limit.veil = 0;
  }

  /**
   * `luminance` is the average relative luminance (0..1, linear light) of the frame about to be
   * shown, before limiting. Returns how to adjust it; the returned object is reused.
   */
  update(dtMs: number, luminance: number): FlashLimit {
    if (this.average < 0) this.average = luminance;

    const high = this.average + this.deadband;
    const low = this.average - this.deadband;
    const limit = this.limit;
    limit.gain = 1;
    limit.veil = 0;
    if (luminance > high && luminance > 0) {
      limit.gain = Math.max(this.minGain, high / luminance);
    } else if (luminance < low) {
      limit.veil = Math.min(this.maxVeil, low - luminance);
    }
    // The slow average follows what is shown, not what was asked for, so a strobe that is being
    // held down does not drag it back and forth.
    const shown = FlashLimiter.shown(luminance, limit);
    this.average += (shown - this.average) * (1 - Math.exp(-dtMs / this.windowMs));
    return limit;
  }

  /** The luminance a frame of `luminance` is shown with. */
  static shown(luminance: number, limit: FlashLimit): number {
    return luminance * limit.gain + limit.veil;
  }
}

/**
 * Counts the changes in a series of luminance samples that the guideline would count: swings of
 * at least `threshold`, alternating up and down. Returns the most such changes in any window of
 * `windowMs`. A flash is a pair of opposing changes, so three flashes a second is six changes.
 * For tests.
 */
export function worstChangesPerWindow(
  samples: ArrayLike<number>,
  sampleMs: number,
  windowMs = 1000,
  threshold = 0.1,
): number {
  const times: number[] = [];
  // 0 until a first swing of `threshold` has been seen, then the direction of the last swing.
  let direction = 0;
  let high = samples[0] ?? 0;
  let low = high;
  let highAt = 0;
  let lowAt = 0;
  for (let i = 1; i < samples.length; i++) {
    const value = samples[i];
    if (direction === 0) {
      if (value > high) [high, highAt] = [value, i];
      if (value < low) [low, lowAt] = [value, i];
      if (high - low >= threshold) {
        times.push(i * sampleMs);
        direction = highAt > lowAt ? 1 : -1;
        // From here on `high` is the extreme of a rise and `low` of a fall.
        if (direction === 1) low = value;
        else high = value;
      }
    } else if (direction === 1) {
      high = Math.max(high, value);
      if (high - value >= threshold) {
        times.push(i * sampleMs);
        direction = -1;
        low = value;
      }
    } else {
      low = Math.min(low, value);
      if (value - low >= threshold) {
        times.push(i * sampleMs);
        direction = 1;
        high = value;
      }
    }
  }
  let worst = 0;
  let start = 0;
  for (let end = 0; end < times.length; end++) {
    while (times[end] - times[start] >= windowMs) start += 1;
    worst = Math.max(worst, end - start + 1);
  }
  return worst;
}
