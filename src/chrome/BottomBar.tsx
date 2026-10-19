import { isPlaylistSource, useApp, type SourceId } from "../state/store";
import { formatBytes, formatTotalDuration, pluralize } from "../util/format";
import "./BottomBar.css";

const NOUN: Partial<Record<SourceId, [string, string]>> = {
  music: ["song", "songs"],
  podcasts: ["episode", "episodes"],
  audiobooks: ["audiobook", "audiobooks"],
};

function Icon({ d, label, on, onClick }: { d: string; label: string; on?: boolean; onClick?: () => void }) {
  return (
    <button
      className="bottom-button"
      aria-label={label}
      aria-pressed={on}
      title={label}
      onClick={onClick}
    >
      <svg width={16} height={13} viewBox="0 0 16 13" aria-hidden="true">
        <path d={d} />
      </svg>
    </button>
  );
}

const PLUS = "M8 2v9M3.5 6.5h9";
const SHUFFLE = "M1 3.5h3l6 6h4M12 7.5l2 2-2 2M1 9.5h3l1.8-1.8M8.2 5.3L10 3.5h4M12 1.5l2 2-2 2";
const REPEAT = "M3 8.5V5.5a2 2 0 0 1 2-2h8M11 1.5l2 2-2 2M13 4.5v3a2 2 0 0 1-2 2H3M5 11.5l-2-2 2-2";
const ARTWORK = "M1.5 1.5h13v10h-13zM1.5 9l4-4 3 3 2-2 4 4";

export function BottomBar() {
  const stats = useApp((s) => s.stats);
  const source = useApp((s) => s.source);
  const shuffle = useApp((s) => s.shuffle);
  const repeat = useApp((s) => s.repeat);
  const showArtwork = useApp((s) => s.showArtwork);
  const toggleShuffle = useApp((s) => s.toggleShuffle);
  const cycleRepeat = useApp((s) => s.cycleRepeat);
  const toggleArtwork = useApp((s) => s.toggleArtwork);
  const createPlaylist = useApp((s) => s.createPlaylist);
  const [one, many] = NOUN[source] ?? (isPlaylistSource(source) ? ["song", "songs"] : ["item", "items"]);

  return (
    <footer className="bottom-bar chrome-surface" data-tauri-drag-region>
      <div className="bottom-left">
        <Icon d={PLUS} label="Create a playlist" onClick={() => void createPlaylist("regular")} />
        <Icon d={SHUFFLE} label="Shuffle" on={shuffle} onClick={toggleShuffle} />
        <span className={repeat === "one" ? "bottom-repeat bottom-repeat--one" : "bottom-repeat"}>
          <Icon d={REPEAT} label={`Repeat: ${repeat}`} on={repeat !== "off"} onClick={cycleRepeat} />
        </span>
        <Icon d={ARTWORK} label="Show artwork" on={showArtwork} onClick={toggleArtwork} />
      </div>
      <div className="bottom-status" data-tauri-drag-region>
        {stats && stats.count > 0
          ? `${pluralize(stats.count, one, many)}, ${formatTotalDuration(stats.totalDurationMs)}, ${formatBytes(stats.totalBytes)}`
          : ""}
      </div>
    </footer>
  );
}
