import { useState } from "react";
import { radioOpenStream } from "../api/commands";
import { useApp } from "../state/store";
import "./RadioView.css";

/** File › Open Stream… (⌘U): plays a stream address, or a .pls or .m3u naming one, and keeps it in My Stations. */
export function OpenStream() {
  const close = () => useApp.setState({ openingStream: false });
  const [url, setUrl] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const play = async () => {
    setBusy(true);
    setError(null);
    try {
      await radioOpenStream(url);
      close();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };
  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="sheet stream-sheet" role="dialog" aria-label="Open Stream">
        <label className="stream-sheet-label" htmlFor="stream-url">
          Enter the URL of an Internet stream:
        </label>
        <input
          id="stream-url"
          className="smart-input stream-url"
          autoFocus
          placeholder="http://"
          spellCheck={false}
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && url.trim() && !busy && void play()}
        />
        {error && <p className="smart-error">{error}</p>}
        <div className="sheet-buttons">
          <button className="push-button" onClick={close}>
            Cancel
          </button>
          <button className="push-button push-button--default" disabled={!url.trim() || busy} onClick={() => void play()}>
            Play
          </button>
        </div>
      </div>
    </div>
  );
}
