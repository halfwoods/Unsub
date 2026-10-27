import type { DeviceVideos, VideoExportPreview } from "../api/types";

export interface VideoExportApi {
  list: (deviceId: string) => Promise<DeviceVideos>;
  preview: (deviceId: string, ids: string[], destination: string) => Promise<VideoExportPreview>;
  start: (deviceId: string, ids: string[], destination: string) => Promise<void>;
}

export interface VideoExportState {
  listing: DeviceVideos | null;
  selected: string[];
  destination: string;
  preview: VideoExportPreview | null;
  loading: boolean;
  counting: boolean;
  starting: boolean;
  error: string | null;
}

/** Owns one open sheet. Every selection/destination change invalidates its old preview immediately. */
export function createVideoExportSession(deviceId: string, api: VideoExportApi) {
  let state: VideoExportState = { listing: null, selected: [], destination: "", preview: null,
    loading: true, counting: false, starting: false, error: null };
  let generation = 0, active = true;
  const listeners = new Set<() => void>();
  const update = (patch: Partial<VideoExportState>) => {
    if (!active) return;
    state = { ...state, ...patch };
    listeners.forEach(listener => listener());
  };
  const preview = async () => {
    const request = ++generation;
    const { destination, selected } = state;
    update({ preview: null, error: null, counting: !!destination && selected.length > 0 });
    if (!destination || selected.length === 0) return;
    try {
      const result = await api.preview(deviceId, [...selected], destination);
      if (active && request === generation) update({ preview: result, counting: false });
    } catch (error) {
      if (active && request === generation) update({ error: String(error), counting: false });
    }
  };
  const canStart = () => active && !state.loading && !state.counting && !state.starting &&
    state.selected.length > 0 && !!state.destination && !!state.preview?.fits &&
    state.preview.videos > state.preview.missing;
  return {
    getSnapshot: () => state,
    subscribe: (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; },
    async load() {
      active = true;
      const request = ++generation;
      update({ listing: null, selected: [], preview: null, loading: true, counting: false, error: null });
      try {
        const listing = await api.list(deviceId);
        if (!active || request !== generation) return;
        update({ listing, selected: listing.items.map(item => item.id), loading: false });
        await preview();
      } catch (error) {
        if (active && request === generation) update({ error: String(error), loading: false });
      }
    },
    select(ids: string[]) {
      if (!active || state.starting || state.loading) return;
      const known = new Set(state.listing?.items.map(item => item.id));
      update({ selected: [...new Set(ids)].filter(id => known.has(id)) });
      void preview();
    },
    destination(destination: string) {
      if (!active || state.starting || state.loading) return;
      update({ destination });
      void preview();
    },
    canStart,
    async start() {
      if (!canStart()) return false;
      const { selected, destination } = state;
      update({ starting: true, error: null });
      try {
        await api.start(deviceId, [...selected], destination);
        return true;
      } catch (error) {
        update({ starting: false, preview: null, error: String(error) });
        return false;
      }
    },
    dispose() { active = false; generation++; },
  };
}
