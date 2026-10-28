import type { VideoAddKind, VideoAddPreview } from "../api/types";

export const MAX_VIDEO_ADD_FILES = 500;
let nextSessionId = 0;

export interface VideoAddApi {
  preview: (deviceId: string, paths: string[], kind: VideoAddKind, requestId: string) => Promise<VideoAddPreview>;
  cancelPreview: (requestId: string) => Promise<void>;
  start: (deviceId: string, paths: string[], kind: VideoAddKind) => Promise<void>;
}
export interface VideoAddState {
  paths: string[];
  kind: VideoAddKind;
  preview: VideoAddPreview | null;
  supported: boolean | null;
  checking: boolean;
  starting: boolean;
  error: string | null;
}

/** One sheet's selection. A changed selection or category immediately invalidates approval to start. */
export function createVideoAddSession(deviceId: string, api: VideoAddApi) {
  let state: VideoAddState = { paths: [], kind: "movie", preview: null, supported: null,
    checking: true, starting: false, error: null };
  let active = true, generation = 0;
  // Keep this compatible with High Sierra, whose WebKit has no crypto.randomUUID.
  const sessionId = `video-add-${Date.now().toString(36)}-${++nextSessionId}-${Math.random().toString(36).slice(2)}`;
  let currentRequestId: string | null = null;
  const listeners = new Set<() => void>();
  const update = (patch: Partial<VideoAddState>) => {
    if (!active) return;
    state = { ...state, ...patch };
    listeners.forEach(listener => listener());
  };
  const preview = async () => {
    const request = ++generation;
    const requestId = `${sessionId}-${request}`;
    currentRequestId = requestId;
    const { paths, kind } = state;
    update({ preview: null, error: null, checking: true });
    try {
      const result = await api.preview(deviceId, [...paths], kind, requestId);
      if (active && request === generation) update({ preview: result, supported: result.supported, checking: false });
    } catch (error) {
      if (active && request === generation) update({ error: String(error), checking: false });
    } finally {
      if (currentRequestId === requestId) currentRequestId = null;
    }
  };
  const cancelPreview = () => {
    const requestId = currentRequestId;
    currentRequestId = null;
    if (requestId) void api.cancelPreview(requestId).catch(() => {});
  };
  const acceptedPaths = () => {
    const selected = new Set(state.paths);
    return [...new Set(state.preview?.items.filter(item => selected.has(item.path) && item.reason === null && !item.alreadyPresent)
      .map(item => item.path) ?? [])];
  };
  const canStart = () => active && !state.checking && !state.starting && !!state.preview?.supported &&
    !!state.preview.fits && acceptedPaths().length > 0;
  return {
    getSnapshot: () => state,
    subscribe: (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; },
    async load() {
      // React StrictMode may dispose and reopen the same session before its first request finishes.
      active = true;
      if (!state.starting) await preview();
    },
    select(paths: string[]) {
      if (!active || state.starting) return;
      const unique = [...new Set(paths)].filter(path => path.length > 0);
      if (unique.length > MAX_VIDEO_ADD_FILES) {
        generation++;
        cancelPreview();
        update({ preview: null, checking: false, error: `Choose up to ${MAX_VIDEO_ADD_FILES} videos at a time. Your previous selection has not changed.` });
        return;
      }
      update({ paths: unique });
      void preview();
    },
    category(kind: VideoAddKind) {
      if (!active || state.starting || kind === state.kind) return;
      update({ kind });
      void preview();
    },
    async retry() { if (active && !state.starting) await preview(); },
    acceptedPaths,
    canStart,
    async start() {
      if (!canStart()) return false;
      const paths = acceptedPaths(), kind = state.kind;
      update({ starting: true, error: null });
      try {
        await api.start(deviceId, paths, kind);
        return true;
      } catch (error) {
        update({ starting: false, preview: null, error: String(error) });
        return false;
      }
    },
    dispose() { active = false; generation++; cancelPreview(); },
  };
}
