/** One analysis of the audio going to the speakers, about 60 a second. */
export interface VisualizerFrame {
  /** Milliseconds since playback began, lined up with what is audible. */
  tMs: number;
  /** 64 log-spaced bands, each 0..1. */
  bands: number[];
  /** 64 peak holds, each 0..1. */
  peaks: number[];
  /** 256 samples of the left channel, -1..1. */
  waveformL: number[];
  waveformR: number[];
  /** Loudness, 0..1. */
  rms: number;
  beat: boolean;
  /** 0..1; meaningful when `beat` is set. */
  beatStrength: number;
  /** Spectral flux, 0..1. */
  onset: number;
  tempoBpm: number | null;
}

export interface VisualizerFeed {
  /** Calls `callback` for every frame until the returned function is called. */
  onFrame(callback: (frame: VisualizerFrame) => void): () => void;
}

/** Where the app's analysis listens: what the player is playing, or the microphone. */
export type VisualizerSource = { type: "music" } | { type: "microphone"; device: string | null };

/** What `TauriFeed` asks of the app's backend. */
export interface FeedBackend {
  /** Starts, or restarts, the analysis; `onFrame` is called for each frame it makes. */
  start(onFrame: (frame: VisualizerFrame) => void, source: VisualizerSource): Promise<void>;
  stop(): Promise<void>;
}

export const BAND_COUNT = 64;
export const WAVEFORM_LENGTH = 256;

export interface MockFeedOptions {
  bpm?: number;
  /** Frames a second. */
  fps?: number;
  /**
   * Plays a 24-second arrangement instead of one steady groove: a quiet start, a loud middle and a
   * breakdown with no drums, to see the pictures calm down and pick up.
   */
  arranged?: boolean;
}

/** How full the arrangement is at `t` seconds: 0.2 in the quiet parts, 1 in the loud one. */
function fullness(t: number): number {
  const phase = t % 24;
  const ramp = (from: number, to: number) => {
    const x = Math.min(1, Math.max(0, (phase - from) / (to - from)));
    return x * x * (3 - 2 * x);
  };
  return 0.2 + 0.8 * ramp(3, 5) * (1 - ramp(17, 19));
}

/**
 * A stand-in for the analyzer: a beat at 120 BPM with a stronger one every fourth, a spectrum
 * that drifts, hi-hats between the beats, and a stereo pair of waveforms that are not quite alike.
 * Frames are a function of time only, so `frameAt` gives the same frame for the same time.
 */
export class MockFeed implements VisualizerFeed {
  private readonly listeners = new Set<(frame: VisualizerFrame) => void>();
  private readonly bpm: number;
  private readonly arranged: boolean;
  private readonly interval: number;
  private timer: ReturnType<typeof setInterval> | null = null;
  private startedAt = 0;

  constructor(options: MockFeedOptions = {}) {
    this.bpm = options.bpm ?? 120;
    this.arranged = options.arranged ?? false;
    this.interval = 1000 / (options.fps ?? 60);
  }

  onFrame(callback: (frame: VisualizerFrame) => void): () => void {
    this.listeners.add(callback);
    return () => {
      this.listeners.delete(callback);
    };
  }

  /** Starts emitting frames on a timer, as the real analyzer does. */
  start(): void {
    if (this.timer !== null) return;
    this.startedAt = performance.now();
    this.timer = setInterval(() => this.emit(performance.now() - this.startedAt), this.interval);
  }

  /** Stops, as when playback pauses: no frames arrive at all. */
  stop(): void {
    if (this.timer === null) return;
    clearInterval(this.timer);
    this.timer = null;
  }

  get running(): boolean {
    return this.timer !== null;
  }

  /** Delivers the frame for `tMs` to the listeners. Used by `start`, and directly by tests. */
  emit(tMs: number): void {
    if (this.listeners.size === 0) return;
    const frame = this.frameAt(tMs);
    for (const listener of this.listeners) listener(frame);
  }

  frameAt(tMs: number): VisualizerFrame {
    const t = tMs / 1000;
    const beats = (t * this.bpm) / 60;
    const beatIndex = Math.floor(beats);
    const phase = beats - beatIndex;
    // A beat is on the frame in which the beat counter ticks over.
    const before = Math.floor((((tMs - this.interval) / 1000) * this.bpm) / 60);
    const full = this.arranged ? fullness(t) : 1;
    // The drums come in as the arrangement fills.
    const drums = this.arranged ? Math.min(1, Math.max(0, (full - 0.5) / 0.3)) : 1;
    const isBeat = beatIndex !== before && drums > 0.5;
    const accent = beatIndex % 4 === 0 ? 1 : 0.7;
    const kick = Math.exp(-phase * 5) * accent * drums;
    // Hi-hats on the off-beats.
    const offPhase = (phase + 0.5) % 1;
    const hat = Math.exp(-offPhase * 14) * 0.6 * drums;

    const bands: number[] = new Array<number>(BAND_COUNT);
    const peaks: number[] = new Array<number>(BAND_COUNT);
    for (let i = 0; i < BAND_COUNT; i++) {
      const f = i / (BAND_COUNT - 1);
      const tilt = 0.8 * Math.pow(1 - f, 1.4);
      const drift = 0.5 + 0.5 * Math.sin(t * (0.7 + f * 1.9) + i * 0.55);
      const wander = 0.5 + 0.5 * Math.sin(t * 0.23 + i * 0.12 + Math.sin(t * 0.11) * 3);
      const bass = f < 0.18 ? kick * (1 - f / 0.18) : 0;
      const air = f > 0.7 ? hat * ((f - 0.7) / 0.3) : 0;
      const grain = 0.04 * Math.sin(t * 37 + i * 12.9898);
      const value = tilt * (0.25 + 0.45 * drift * wander) * (0.4 + 0.6 * full) + bass * 0.65 + air * 0.5 + grain;
      bands[i] = Math.min(1, Math.max(0, value));
      peaks[i] = Math.min(1, bands[i] + 0.05 + 0.05 * drift);
    }

    // A low note that changes each bar, with overtones; the right channel is a little detuned.
    const note = [55, 65.4, 73.4, 49][Math.floor(beats / 4) % 4];
    const cycles = note / 110;
    const left: number[] = new Array<number>(WAVEFORM_LENGTH);
    const right: number[] = new Array<number>(WAVEFORM_LENGTH);
    const amp = (0.25 + 0.5 * (0.35 + kick * 0.65)) * (0.4 + 0.6 * full);
    for (let k = 0; k < WAVEFORM_LENGTH; k++) {
      const x = k / WAVEFORM_LENGTH;
      const base = x * Math.PI * 2 * cycles * 4 + t * 3;
      const l =
        Math.sin(base) + 0.45 * Math.sin(base * 2.01 + 0.6) + 0.25 * Math.sin(base * 3.97 + t);
      const r =
        Math.sin(base + 0.9 + 0.4 * Math.sin(t * 0.5)) +
        0.4 * Math.sin(base * 2.0 + 1.3) +
        0.3 * Math.sin(base * 4.03 + t * 1.3);
      const shimmer = hat * 0.35 * Math.sin(x * 190 + t * 90);
      left[k] = Math.max(-1, Math.min(1, (l * amp * 0.55 + shimmer) * 0.9));
      right[k] = Math.max(-1, Math.min(1, (r * amp * 0.55 + shimmer * 0.8) * 0.9));
    }

    return {
      tMs,
      bands,
      peaks,
      waveformL: left,
      waveformR: right,
      rms: Math.min(1, (0.14 + 0.3 * kick + 0.1 * hat) * (0.1 + 0.9 * full ** 1.5)),
      beat: isBeat,
      beatStrength: isBeat ? accent : 0,
      onset: Math.min(1, kick * 0.8 + hat * 0.5),
      // The real analyzer needs a few seconds of music before it commits to a tempo.
      tempoBpm: t < 2 ? null : this.bpm,
    };
  }
}

/**
 * The app's real feed: frames from the backend's analyzer over a channel. The backend works only
 * while somebody is listening, so a hidden visualizer costs nothing and the microphone is never
 * opened for a picture that is not showing.
 */
export class TauriFeed implements VisualizerFeed {
  private readonly listeners = new Set<(frame: VisualizerFrame) => void>();
  private readonly backend: FeedBackend;
  private readonly source: VisualizerSource;
  private readonly onError: (error: unknown) => void;
  /** Starts and stops run one after another: the backend answers them in whatever order they land. */
  private pending: Promise<void> = Promise.resolve();

  constructor(backend: FeedBackend, source: VisualizerSource, onError: (error: unknown) => void) {
    this.backend = backend;
    this.source = source;
    this.onError = onError;
  }

  onFrame(callback: (frame: VisualizerFrame) => void): () => void {
    this.listeners.add(callback);
    if (this.listeners.size === 1) this.run(() => this.backend.start((frame) => this.deliver(frame), this.source));
    return () => {
      if (this.listeners.delete(callback) && this.listeners.size === 0) this.run(() => this.backend.stop());
    };
  }

  private deliver(frame: VisualizerFrame): void {
    for (const listener of this.listeners) listener(frame);
  }

  private run(task: () => Promise<void>): void {
    this.pending = this.pending.then(task).catch(this.onError);
  }
}
