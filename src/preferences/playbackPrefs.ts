import { VISUALIZER_STYLES, type VisualizerStyle } from "../visualizer/styles.ts";

import { DEFAULT_GASLIGHT, normalizeGaslight, type GaslightSettings } from "../visualizer/gaslightSettings.ts";

/** Playback choices are per Mac (its audio device, its speakers), so they live in localStorage. */
function load(key: string): boolean {
  try {
    return localStorage.getItem(key) === "true";
  } catch {
    return false;
  }
}

function save(key: string, value: unknown) {
  try {
    localStorage.setItem(key, typeof value === "string" ? value : JSON.stringify(value));
  } catch {
    // Storage unavailable: the setting still applies until the app quits.
  }
}

function loadJson<T>(key: string, fallback: T): T {
  try {
    const stored = localStorage.getItem(key);
    return stored ? { ...fallback, ...(JSON.parse(stored) as Partial<T>) } : fallback;
  } catch {
    return fallback;
  }
}

export const loadBitPerfect = () => load("unsub.bitPerfect");
export const saveBitPerfect = (enabled: boolean) => save("unsub.bitPerfect", enabled);
export const loadSoundCheck = () => load("unsub.soundCheck");
export const saveSoundCheck = (enabled: boolean) => save("unsub.soundCheck", enabled);

export interface EqualizerPrefs {
  on: boolean;
  /** The preset the sliders came from; null once they're moved by hand ("Manual"). */
  preset: string | null;
  preampDb: number;
  bands: number[];
  preventClipping: boolean;
}

const FLAT: EqualizerPrefs = { on: false, preset: "Flat", preampDb: 0, bands: Array(10).fill(0), preventClipping: true };
export const loadEqualizer = () => loadJson("unsub.equalizer", FLAT);
export const saveEqualizer = (prefs: EqualizerPrefs) => save("unsub.equalizer", prefs);

/** Crossfade Songs' length in seconds, or null when it's off. */
export const loadCrossfade = () => loadJson<{ seconds: number | null }>("unsub.crossfade", { seconds: null }).seconds;
export const saveCrossfade = (seconds: number | null) => save("unsub.crossfade", { seconds });

export interface VisualizerPrefs {
  style: VisualizerStyle;
  /** Keeps the picture to three flashes a second; on unless a viewer who can take it turns it off. */
  limitFlashing: boolean;
  /** Listens through the microphone instead of the music. Off until chosen, as it prompts macOS. */
  microphone: boolean;
  /** The microphone to listen to; null for the system's default. */
  microphoneDevice: string | null;
  gaslight: GaslightSettings;
}

const VISUALIZER: VisualizerPrefs = { style: "classic", limitFlashing: true, microphone: false, microphoneDevice: null, gaslight: { ...DEFAULT_GASLIGHT } };
export function loadVisualizerPrefs(): VisualizerPrefs {
  const prefs = loadJson("unsub.visualizer", VISUALIZER);
  // A style from some other version must not reach the renderer.
  return {
    style: VISUALIZER_STYLES.includes(prefs.style) ? prefs.style : VISUALIZER.style,
    limitFlashing: typeof prefs.limitFlashing === "boolean" ? prefs.limitFlashing : true,
    microphone: typeof prefs.microphone === "boolean" ? prefs.microphone : false,
    microphoneDevice: typeof prefs.microphoneDevice === "string" ? prefs.microphoneDevice : null,
    gaslight: normalizeGaslight(prefs.gaslight),
  };
}
export const saveVisualizerPrefs = (prefs: VisualizerPrefs) => save("unsub.visualizer", prefs);
