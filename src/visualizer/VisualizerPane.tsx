import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useMemo, useRef, useState } from "react";
import { artworkUrl, onVisualizerEnded, visualizerStart, visualizerStop } from "../api/commands";
import { useApp } from "../state/store";
import { TauriFeed } from "./feed";
import { Visualizer } from "./Visualizer";
import "./VisualizerPane.css";

const appWindow = getCurrentWindow();
const BACKEND = { start: visualizerStart, stop: visualizerStop };
/** Big enough for the Album style to stay sharp on a full screen. */
const COVER_SIZE = 1024;

/**
 * The visualizer in the app: it covers the window's body, leaving the toolbar and LCD, and fills
 * the screen with F, Esc going back. Showing it starts the backend's analysis and hiding it stops
 * it, so the microphone is only ever open while it is on screen.
 */
export function VisualizerPane() {
  const { style, limitFlashing, microphone, microphoneDevice } = useApp((s) => s.visualizer);
  const setVisualizer = useApp((s) => s.setVisualizer);
  const showNotice = useApp((s) => s.showNotice);
  const nowPlaying = useApp((s) => s.nowPlaying);
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
      if (enteredFullscreen.current) void appWindow.isFullscreen().then((full) => !full && setFullscreen(false));
    });
    return () => void resized.then((unlisten) => unlisten());
  }, [fullscreen]);

  // Hiding the visualizer puts the window back as it was.
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

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || !fullscreen) return;
      event.preventDefault();
      void showFullscreen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [fullscreen]);

  // Minimized or covered, nobody sees the picture, so the analysis (and the microphone) rests.
  useEffect(() => {
    const sync = () => setHidden(document.hidden);
    document.addEventListener("visibilitychange", sync);
    return () => document.removeEventListener("visibilitychange", sync);
  }, []);

  // The arrow keys change the style here, not the selection in a list underneath.
  useEffect(() => {
    if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
  }, []);

  return (
    <div className="visualizer-pane" data-fullscreen={fullscreen}>
      <Visualizer
        feed={hidden ? null : feed}
        nowPlaying={
          nowPlaying && {
            title: nowPlaying.title,
            artist: nowPlaying.artist,
            artworkUrl: fromLibrary ? artworkUrl(nowPlaying.id, { type: "library" }, COVER_SIZE) : null,
          }
        }
        style={style}
        onStyleChange={(next) => setVisualizer({ style: next })}
        limitFlashing={limitFlashing}
        onToggleFullscreen={() => void showFullscreen(!fullscreen)}
      />
    </div>
  );
}
