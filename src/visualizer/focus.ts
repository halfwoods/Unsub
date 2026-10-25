export interface VisualizerFocusHost {
  onResize(callback: () => void): Promise<() => void>;
  isWindowFocused(): Promise<boolean>;
  canRestore(): boolean;
  focusWebview(): Promise<void>;
  focusContent(): void;
  onError?(error: unknown): void;
}

/**
 * AppKit may leave the NSWindow as first responder after a fullscreen transition. Tauri emits
 * a resize after the transition finishes; focus the webview then, followed by its visualizer.
 * This is separate from making the native window key, which does not focus WKWebView.
 */
export function attachVisualizerFocus(host: VisualizerFocusHost): () => void {
  let disposed = false;
  let request = 0;
  let unsubscribe: (() => void) | null = null;
  let reported = false;
  const report = (error: unknown) => {
    if (!disposed && !reported) host.onError?.(error);
    reported = true;
  };
  const restore = async () => {
    const current = ++request;
    if (disposed || !host.canRestore()) return;
    const focused = await host.isWindowFocused();
    // Recheck after IPC: another app, a preferences sheet or an editor may now own focus.
    if (disposed || current !== request || !focused || !host.canRestore()) return;
    await host.focusWebview();
    if (!disposed && current === request && host.canRestore()) host.focusContent();
  };
  const resized = () => void restore().catch(report);
  void host.onResize(resized).then((unlisten) => {
    if (disposed) unlisten();
    else unsubscribe = unlisten;
  }, report);
  resized();
  return () => {
    disposed = true;
    request++;
    unsubscribe?.();
  };
}
