import { create } from "zustand";

export type Appearance = "classic" | "itunes12";
const KEY = "unsub-appearance";

function initialAppearance(): Appearance {
  try { return localStorage.getItem(KEY) === "itunes12" ? "itunes12" : "classic"; }
  catch { return "classic"; }
}

export const useAppearance = create<{ appearance: Appearance; setAppearance: (appearance: Appearance) => void }>((set) => ({
  appearance: initialAppearance(),
  setAppearance: (appearance) => {
    try { localStorage.setItem(KEY, appearance); } catch { /* Keep the choice for this session. */ }
    set({ appearance });
  },
}));
