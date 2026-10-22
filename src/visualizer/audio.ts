import { BAND_COUNT, WAVEFORM_LENGTH, type VisualizerFrame } from "./feed.ts";

/** Frames older than this count as no frames: the music has stopped or been paused. */
const STALE_MS = 250;
/**
 * How long the picture takes to settle into its idle drift, and how long to come back from it.
 * The way back is quick so that a song starting is answered at once; the way down is slow so a
 * pause does not look like a crash.
 */
const LIVE_FALL_S = 0.6;
const LIVE_RISE_S = 0.12;

const BAND_ATTACK_S = 0.022;
const BAND_RELEASE_S = 0.13;
const PEAK_HOLD_S = 0.2;
const PEAK_GRAVITY = 2.4;
const BEAT_DECAY_S = 0.24;
const KICK_DECAY_S = 0.075;

/**
 * Loudness is read on a decibel scale and then squared, so a quiet verse and a loud chorus are far
 * apart: a track's rms of 0.3 gives 0.9, 0.1 gives 0.5 and 0.03 gives 0.25. It swells quickly and
 * settles slowly, as loudness is heard.
 */
const ENERGY_FLOOR_DB = -52;
const ENERGY_CEIL_DB = -8;
const ENERGY_ATTACK_S = 0.09;
const ENERGY_RELEASE_S = 0.5;
/** The picture idles at this energy: calm, but not asleep. */
const IDLE_ENERGY = 0.12;
/** The bass is a thump when it stands this far (times the gain) above its average of the last second or so. */
const THUMP_AVERAGE_S = 1.2;
const THUMP_GAIN = 5;
/**
 * An onset is a spark when it stands out of the onsets around it: dense music has a steady onset
 * level of 0.05 to 0.1 and its hits reach 0.3 to 0.9.
 */
const SPARK_AVERAGE_S = 1.5;
const SPARK_DEAD_ZONE = 0.05;
const SPARK_GAIN = 2.2;
const SPARK_DECAY_S = 0.1;

/** Rows of the audio texture: waveform L, waveform R, bands, peaks. */
export const AUDIO_TEXTURE_ROWS = 4;

/**
 * The latest frame, smoothed. Frames arrive at about 60 a second and the screen draws at its own
 * rate, so nothing is drawn straight from a frame: bands rise quickly and fall slowly, peaks hold
 * and then fall under gravity, and beats become envelopes that decay between them. When frames
 * stop, the values ease into a slow drift so the picture idles rather than freezing.
 *
 * Everything lives in preallocated typed arrays; `update` allocates nothing.
 */
export class AudioState {
  readonly bands = new Float32Array(BAND_COUNT);
  readonly peaks = new Float32Array(BAND_COUNT);
  readonly waveL = new Float32Array(WAVEFORM_LENGTH);
  readonly waveR = new Float32Array(WAVEFORM_LENGTH);
  /** The four rows the shaders read, as one texture upload. */
  readonly texture = new Float32Array(WAVEFORM_LENGTH * AUDIO_TEXTURE_ROWS);

  rms = 0;
  /** 1 at a beat, decaying over about a quarter second. */
  beat = 0;
  /** 1 at a beat, decaying within a tenth of a second: for sudden pushes. */
  kick = 0;
  /** Beats seen so far, to key things that change on a beat. */
  beatCount = 0;
  onset = 0;
  /**
   * How loud the music is, 0..1 and perceptual: near 0 for a whisper, 0.5 for a quiet verse, near
   * 0.9 for a loud chorus. What speed and brightness follow, so quiet is calm and loud is lively.
   */
  energy = 0;
  /** A kick or a bass note: the lowest bands standing above their recent average, 0..1. Follows the sound itself, so it works when no beat was detected. */
  thump = 0;
  /** A sudden change anywhere in the sound (a snare, a strum), 1 when it lands and gone within a fifth of a second. */
  spark = 0;
  tempoBpm = 0;
  /** Mean of the lowest, middle and highest bands. */
  low = 0;
  mid = 0;
  high = 0;
  /** 1 while frames are arriving, 0 once the picture has settled into idling. */
  live = 0;
  /** Seconds of animation, advancing whether or not music plays. */
  time = 0;

  private lastFrame: VisualizerFrame | null = null;
  /** The biggest onset received since the last update, so a frame between two draws is not lost. */
  private pendingOnset = 0;
  private slowLow = 0;
  private onsetAverage = 0;
  private lastFrameAtMs = -Infinity;
  private readonly holdLeft = new Float32Array(BAND_COUNT);
  private readonly fall = new Float32Array(BAND_COUNT);
  /** How loud each band idles at: a gentle slope, louder in the bass. */
  private readonly idleShape = new Float32Array(BAND_COUNT);

  constructor() {
    for (let i = 0; i < BAND_COUNT; i++) {
      this.idleShape[i] = 0.05 + 0.1 * Math.pow(1 - i / (BAND_COUNT - 1), 1.3);
    }
  }

  /**
   * Whether music has stopped for long enough that the picture can draw less often: the
   * drifting that replaces it is slow, so a fifth of the frames look the same.
   */
  get idle(): boolean {
    return this.live < 0.25;
  }

  /** Notes that `frame` has arrived. Call from the feed's callback. */
  receive(frame: VisualizerFrame, nowMs: number): void {
    this.lastFrame = frame;
    this.lastFrameAtMs = nowMs;
    this.pendingOnset = Math.max(this.pendingOnset, clamp01(frame.onset));
    if (frame.beat) {
      this.beatCount += 1;
      const strength = Math.min(1, Math.max(0.35, frame.beatStrength));
      this.kick = Math.max(this.kick, strength);
      this.beat = Math.max(this.beat, strength);
    }
  }

  /** Advances by `dt` seconds towards the latest frame, or towards idling if there is none. */
  update(dt: number, nowMs: number): void {
    const frame = this.lastFrame;
    const fresh = frame !== null && nowMs - this.lastFrameAtMs < STALE_MS;
    this.live += ((fresh ? 1 : 0) - this.live) * (1 - Math.exp(-dt / (fresh ? LIVE_RISE_S : LIVE_FALL_S)));
    if (!fresh && this.live < 0.002) this.live = 0;
    this.time += dt;

    const attack = 1 - Math.exp(-dt / BAND_ATTACK_S);
    const release = 1 - Math.exp(-dt / BAND_RELEASE_S);
    const live = this.live;
    const idleShare = 1 - live;
    const t = this.time;
    // Each source is only consulted while it counts, which keeps the common case (music playing)
    // free of the idle drift's trigonometry.
    const heard = frame !== null && live > 0.001 ? frame : null;
    const drifting = idleShare > 0.001;

    let low = 0;
    let mid = 0;
    let high = 0;
    for (let i = 0; i < BAND_COUNT; i++) {
      let target = 0;
      if (heard !== null) {
        const value = heard.bands[i];
        target = (value > 1 ? 1 : value > 0 ? value : 0) * live;
      }
      if (drifting) {
        target += this.idleShape[i] * (0.4 + 0.3 * Math.sin(t * 0.35 + i * 0.21) + 0.3) * idleShare;
      }
      const current = this.bands[i];
      this.bands[i] = current + (target - current) * (target > current ? attack : release);

      // Peak caps hold, then fall faster the longer they fall.
      const level = this.bands[i];
      if (level >= this.peaks[i]) {
        this.peaks[i] = level;
        this.holdLeft[i] = PEAK_HOLD_S;
        this.fall[i] = 0;
      } else if (this.holdLeft[i] > 0) {
        this.holdLeft[i] -= dt;
      } else {
        this.fall[i] += PEAK_GRAVITY * dt;
        const fallen = this.peaks[i] - this.fall[i] * dt;
        this.peaks[i] = fallen > level ? fallen : level;
      }

      if (i < 8) low += level;
      else if (i < 32) mid += level;
      else high += level;
    }
    this.low = low / 8;
    this.mid = mid / 24;
    this.high = high / 32;
    // A song starting is not a thump: until the music is fully in, the average is the bass itself.
    this.slowLow = live < 0.9 ? this.low : this.slowLow + (this.low - this.slowLow) * (1 - Math.exp(-dt / THUMP_AVERAGE_S));
    this.thump = clamp01((this.low - this.slowLow) * THUMP_GAIN);

    const waveRate = 1 - Math.exp(-dt / 0.014);
    for (let i = 0; i < WAVEFORM_LENGTH; i++) {
      let targetL = 0;
      let targetR = 0;
      if (heard !== null) {
        targetL = heard.waveformL[i] * live;
        targetR = heard.waveformR[i] * live;
      }
      if (drifting) {
        // Two slow standing waves, a little out of step between the channels.
        const x = i * (Math.PI * 4 / WAVEFORM_LENGTH);
        targetL += 0.07 * Math.sin(x + t * 0.9) * Math.sin(t * 0.21) * idleShare;
        targetR += 0.07 * Math.sin(x + t * 0.9 + 1.3) * Math.sin(t * 0.21 + 1.3) * idleShare;
      }
      this.waveL[i] += (targetL - this.waveL[i]) * waveRate;
      this.waveR[i] += (targetR - this.waveR[i]) * waveRate;
    }

    const heardRms = heard !== null ? clamp01(heard.rms) : 0;
    const rmsTarget = heardRms * live + (drifting ? (0.05 + 0.02 * Math.sin(t * 0.6)) * idleShare : 0);
    this.rms += (rmsTarget - this.rms) * (rmsTarget > this.rms ? attack : release);
    const heardOnset = heard !== null ? clamp01(heard.onset) * live : 0;
    this.onset += (heardOnset - this.onset) * (heardOnset > this.onset ? attack : release);
    this.tempoBpm = frame?.tempoBpm ?? 0;

    const loudness = heardRms > 0 ? (20 * Math.log10(heardRms) - ENERGY_FLOOR_DB) / (ENERGY_CEIL_DB - ENERGY_FLOOR_DB) : 0;
    const heardEnergy = clamp01(loudness) ** 2;
    const energyTarget = heardEnergy * live + IDLE_ENERGY * idleShare;
    this.energy += (energyTarget - this.energy) * (1 - Math.exp(-dt / (energyTarget > this.energy ? ENERGY_ATTACK_S : ENERGY_RELEASE_S)));

    const spark = clamp01((this.pendingOnset * live - this.onsetAverage - SPARK_DEAD_ZONE) * SPARK_GAIN);
    this.pendingOnset = 0;
    this.spark = Math.max(this.spark * Math.exp(-dt / SPARK_DECAY_S), spark);
    this.onsetAverage += (heardOnset - this.onsetAverage) * (1 - Math.exp(-dt / SPARK_AVERAGE_S));

    this.beat *= Math.exp(-dt / BEAT_DECAY_S);
    this.kick *= Math.exp(-dt / KICK_DECAY_S);

    this.pack();
  }

  private pack(): void {
    const data = this.texture;
    data.set(this.waveL, 0);
    data.set(this.waveR, WAVEFORM_LENGTH);
    data.set(this.bands, WAVEFORM_LENGTH * 2);
    data.set(this.peaks, WAVEFORM_LENGTH * 3);
  }
}

function clamp01(value: number): number {
  return value < 0 ? 0 : value > 1 ? 1 : value;
}
