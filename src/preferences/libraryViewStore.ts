import { create } from "zustand";
import type { SortKey } from "../api/types";
import { changeLayout, copyLayoutToPlaylists, loadLibraryViews, normalizeLayout, restoreLayout, saveLibraryViews, type BrowsingPreferences, type ColumnLayout, type LibraryViewPrefs } from "./libraryViews";

interface LibraryViewsState {
  prefs: LibraryViewPrefs;
  optionsOpen: boolean;
  setOptionsOpen: (open: boolean) => void;
  updateLayout: (source: string, changes: Partial<ColumnLayout>, regular?: boolean) => void;
  restore: (source: string) => void;
  setSort: (source: string, sort: SortKey, descending: boolean, regular?: boolean) => void;
  setBrowsing: (changes: Partial<BrowsingPreferences>) => void;
  savePreset: (name: string, layout: ColumnLayout) => void;
  removePreset: (name: string) => void;
  copyToPlaylists: (layout: ColumnLayout, ids: number[]) => void;
  makeLibraryDefault: (layout: ColumnLayout) => void;
}
export const useLibraryViews = create<LibraryViewsState>((set, get) => {
  const update = (prefs: LibraryViewPrefs) => { saveLibraryViews(prefs); set({ prefs }); };
  return {
    prefs: loadLibraryViews(), optionsOpen: false,
    setOptionsOpen: optionsOpen => set({ optionsOpen }),
    updateLayout: (source, changes, regular = false) => update(changeLayout(get().prefs, source, changes, regular)),
    restore: source => update(restoreLayout(get().prefs, source)),
    setSort: (source, sort, descending, regular = false) => update(changeLayout(get().prefs, source, { sort, descending }, regular)),
    setBrowsing: changes => update({ ...get().prefs, browsing: { ...get().prefs.browsing, ...changes } }),
    savePreset: (name, layout) => {
      const label = name.trim().slice(0, 80); if (!label) return;
      const prefs = get().prefs;
      update({ ...prefs, customPresets: [...prefs.customPresets.filter(p => p.name !== label), { name: label, layout: normalizeLayout(layout) }] });
    },
    removePreset: name => update({ ...get().prefs, customPresets: get().prefs.customPresets.filter(p => p.name !== name) }),
    copyToPlaylists: (layout, ids) => update(copyLayoutToPlaylists(get().prefs, layout, ids)),
    makeLibraryDefault: layout => update({ ...get().prefs, libraryDefault: normalizeLayout({ ...layout, sort: layout.sort === "playlistOrder" ? "artist" : layout.sort, descending: layout.sort === "playlistOrder" ? false : layout.descending }) }),
  };
});
