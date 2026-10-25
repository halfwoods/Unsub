import { getCurrentWindow } from "@tauri-apps/api/window";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useEffect, useMemo, useRef, useState } from "react";
import { artworkUrl, onVisualizerEnded, visualizerStart, visualizerStop } from "../api/commands";
import { useApp } from "../state/store";
import { TauriFeed } from "./feed";
import { attachVisualizerFocus } from "./focus";
import { Visualizer } from "./Visualizer";
import "./VisualizerPane.css";

const appWindow = getCurrentWindow();
const BACKEND = { start: visualizerStart, stop: visualizerStop };
/** Big enough for the Album style to stay sharp on a full screen. */
const COVER_SIZE = 1024;

/**
 * The visualizer in the app: it covers the window's body, leaving the toolbar and LCD, and fills
 * the screen with F. Esc closes it, and puts the window back if F had filled the screen. Showing
 * it starts the backend's analysis and hiding it stops it, so the microphone is only ever open
 * while it is on screen.
 */
export function VisualizerPane() {
  const paneRef = useRef<HTMLDivElement>(null);
  const { style, limitFlashing, microphone, microphoneDevice, gaslight } = useApp((s) => s.visualizer);
  const setVisualizer = useApp((s) => s.setVisualizer);
  const showNotice = useApp((s) => s.showNotice);
  const nowPlaying = useApp((s) => s.nowPlaying);
  const castTarget = useApp((s) => s.castTarget);
  // Ids of CD and shared songs mean other songs in the library, so only library songs have art.
  const fromLibrary = useApp((s) => s.playingSource.type === "library");
  // Whether the picture fills the screen, toolbar included: only after F, not just because the
  // window happens to be in native full screen already.
  const [fullscreen, setFullscreen] = useState(false);
  // The window went full screen because of F, so leaving puts it back; otherwise it was the user's.
  const enteredFullscreen = useRef(false);
  const [hidden, setHidden] = useState(document.hidden);

  const feed = useMemo(
    () =>
      new TauriFeed(BACKEND, microphone ? { type: "microphone", device: microphoneDevice } : { type: "music" }, (error) => {
        showNotice(String(error));
        // The microphone is on only because it was asked for; when it can't be had, go back to the music.
        if (microphone) setVisualizer({ microphone: false });
      }),
    [microphone, microphoneDevice, showNotice, setVisualizer],
  );

  const showFullscreen = async (next: boolean) => {
    if (next) {
      enteredFullscreen.current = !(await appWindow.isFullscreen());
      if (enteredFullscreen.current) await appWindow.setFullscreen(true);
    } else if (enteredFullscreen.current) {
      enteredFullscreen.current = false;
      await appWindow.setFullscreen(false);
    }
    setFullscreen(next);
  };

  // Leaving full screen with the system's own control ends ours too.
  useEffect(() => {
    if (!fullscreen) return;
    const resized = appWindow.onResized(() => {
      void appWindow.isFullscreen().then((full) => {
        if (!full) {
          enteredFullscreen.current = false;
          setFullscreen(false);
        }
      });
    });
    return () => void resized.then((unlisten) => unlisten());
  }, [fullscreen]);

  // Hiding the visualizer, with Esc or the menu, puts the window back as it was.
  useEffect(
    () => () => {
      if (enteredFullscreen.current) void appWindow.setFullscreen(false);
    },
    [],
  );

  // The microphone can fail after it started, for instance by being unplugged; the analysis then
  // says so and the music takes over.
  useEffect(() => {
    const ended = onVisualizerEnded((reason) => {
      showNotice(reason);
      setVisualizer({ microphone: false });
    });
    return () => void ended.then((unlisten) => unlisten());
  }, [showNotice, setVisualizer]);

  // Minimized or covered, nobody sees the picture, so the analysis (and the microphone) rests.
  useEffect(() => {
    const sync = () => setHidden(document.hidden);
    document.addEventListener("visibilitychange", sync);
    return () => document.removeEventListener("visibilitychange", sync);
  }, []);

  // The arrow keys change the style here. Keep the webview as the native first responder
  // through fullscreen entry/exit, but never take focus from another app, editor or sheet.
  useEffect(() => {
    if (!document.querySelector('[role="dialog"], [role="alertdialog"], [role="menu"]')
      && document.activeElement instanceof HTMLElement) document.activeElement.blur();
    return attachVisualizerFocus({
      onResize: (callback) => appWindow.onResized(callback),
      isWindowFocused: () => appWindow.isFocused(),
      canRestore: () => !document.hidden
        && !document.querySelector('[role="dialog"], [role="alertdialog"], [role="menu"]')
        && !(document.activeElement instanceof HTMLElement
          && (document.activeElement.isContentEditable || document.activeElement.closest('input, textarea, select, button, a[href]'))),
      focusWebview: () => getCurrentWebview().setFocus(),
      focusContent: () => paneRef.current?.focus({ preventScroll: true }),
      onError: (error) => console.warn("visualizer: could not restore keyboard focus", error),
    });
  }, []);

  return (
    <div ref={paneRef} className="visualizer-pane" data-fullscreen={fullscreen} tabIndex={-1}>
      <Visualizer
        feed={hidden ? null : feed}
        nowPlaying={
          nowPlaying && {
            title: nowPlaying.title,
            artist: nowPlaying.artist,
            artworkUrl: fromLibrary ? artworkUrl(nowPlaying.id, { type: "library" }, COVER_SIZE) : null,
          }
        }
        // A speaker's sound never passes through here, so there is nothing for the other styles to move with.
        style={castTarget ? "album" : style}
        onStyleChange={(next) => !castTarget && setVisualizer({ style: next })}
        limitFlashing={limitFlashing}
        gaslight={gaslight}
        onToggleFullscreen={() => void showFullscreen(!fullscreen)}
        onClose={() => setVisualizer({ open: false })}
      />
      {castTarget && <p className="visualizer-note">Playing on {castTarget.name}. The visualizer can’t hear it.</p>}
    </div>
  );
}
