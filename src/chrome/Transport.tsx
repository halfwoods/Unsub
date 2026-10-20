import { playerNext, playerPrevious } from "../api/commands";
import { useApp } from "../state/store";
import "./Transport.css";

function Glyph({ d, width, height, nudge = false }: { d: string; width: number; height: number; nudge?: boolean }) {
  return (
    <svg className={nudge ? "gloss-glyph gloss-glyph--nudge" : "gloss-glyph"} width={width} height={height} viewBox={`0 0 ${width} ${height}`} aria-hidden="true">
      <defs>
        <linearGradient id="glyph-fill" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor="var(--glyph-a)" />
          <stop offset="1" stopColor="var(--glyph-b)" />
        </linearGradient>
      </defs>
      <path d={d} fill="url(#glyph-fill)" />
    </svg>
  );
}

const REWIND = "M7 0v8L0 4zM14 0v8L7 4z";
const FORWARD = "M0 0v8l7-4zM7 0v8l7-4z";
const PLAY = "M0 0v13l11-6.5z";
const PAUSE = "M0 0h4v12H0zM7 0h4v12H7z";

export function Transport() {
  const state = useApp((s) => s.playback.state);
  const hasTracks = useApp((s) => s.tracks.length > 0);
  const hasCurrent = useApp((s) => s.playback.trackId !== null);
  // A station has no previous or next, and once paused it can still be tuned in again.
  const live = useApp((s) => s.playback.live);
  const hasStation = useApp((s) => s.liveStation !== null);
  const connecting = useApp((s) => s.liveLink === "connecting");
  const togglePlay = useApp((s) => s.togglePlay);
  // Pause also gives up on a station that is still being reached.
  const playing = state === "playing" || connecting;

  return (
    <div className="transport">
      <button className="gloss-button gloss-button--small" aria-label="Previous" disabled={!hasCurrent || live} onClick={() => void playerPrevious()}>
        <Glyph d={REWIND} width={14} height={8} />
      </button>
      <button
        className="gloss-button gloss-button--large"
        aria-label={playing ? "Pause" : "Play"}
        disabled={!hasTracks && !hasCurrent && !hasStation}
        onClick={() => void togglePlay()}
      >
        {playing ? <Glyph d={PAUSE} width={11} height={12} /> : <Glyph d={PLAY} width={11} height={13} nudge />}
      </button>
      <button className="gloss-button gloss-button--small" aria-label="Next" disabled={!hasCurrent || live} onClick={() => void playerNext()}>
        <Glyph d={FORWARD} width={14} height={8} />
      </button>
    </div>
  );
}
