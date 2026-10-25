/** Small, validated preferences kept separate from the lazily loaded renderer. */
export const GASLIGHT_PALETTES = ["ember", "aurora", "inverted"] as const;
export type GaslightPalette = (typeof GASLIGHT_PALETTES)[number];
export interface GaslightSettings {
  palette: GaslightPalette;
  /** How long the spectrum leaves a trail, 0..1. */
  trails: number;
  glow: number;
  height: number;
  /** Slowly varies the shape and glow within the selected palette. */
  autopilot: boolean;
}

export const DEFAULT_GASLIGHT: Readonly<GaslightSettings> = Object.freeze({
  palette: "ember", trails: 0.7, glow: 0.65, height: 0.7, autopilot: false,
});

/** Also used at the storage boundary: old, partial and malformed preferences are harmless. */
export function normalizeGaslight(value: unknown): GaslightSettings {
  const input = value && typeof value === "object" ? value as Partial<GaslightSettings> : {};
  const number = (value: unknown, fallback: number) =>
    typeof value === "number" && Number.isFinite(value) ? Math.max(0, Math.min(1, value)) : fallback;
  return {
    palette: GASLIGHT_PALETTES.includes(input.palette as GaslightPalette) ? input.palette! : DEFAULT_GASLIGHT.palette,
    trails: number(input.trails, DEFAULT_GASLIGHT.trails),
    glow: number(input.glow, DEFAULT_GASLIGHT.glow),
    height: number(input.height, DEFAULT_GASLIGHT.height),
    autopilot: typeof input.autopilot === "boolean" ? input.autopilot : DEFAULT_GASLIGHT.autopilot,
  };
}
