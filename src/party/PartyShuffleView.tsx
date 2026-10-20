import { useEffect, useState, type DragEvent, type KeyboardEvent } from "react";
import { onPartyChanged, onPlayerEvent, partyAdd, partyMove, partyPlay, partyRemove, partyReshuffle, partySetSettings, partyView } from "../api/commands";
import type { PartySettings, PartyView, Track } from "../api/types";
import { NowPlayingIcon } from "../browser/SongList";
import { TRACKS_MIME } from "../sidebar/PlaylistRows";
import { useApp } from "../state/store";
import { formatClock } from "../util/format";
import "./PartyShuffleView.css";

const ROWS_MIME = "application/x-unsub-party-rows";
const COUNTS = [0, 5, 10, 15, 25, 50];

type Row = { track: Track; part: "history" | "current" | "upcoming"; upcomingIndex: number };

function rowsOf(view: PartyView): Row[] {
  return [
    ...view.history.map((track) => ({ track, part: "history" as const, upcomingIndex: -1 })),
    ...(view.current ? [{ track: view.current, part: "current" as const, upcomingIndex: -1 }] : []),
    ...view.upcoming.map((track, upcomingIndex) => ({ track, part: "upcoming" as const, upcomingIndex })),
  ];
}

/** iTunes' Party Shuffle: recent songs grayed, the current one, then the queue that keeps refilling. */
export function PartyShuffleView() {
  const [view, setView] = useState<PartyView | null>(null);
  const [selected, setSelected] = useState<ReadonlySet<number>>(new Set());
  const [dropAt, setDropAt] = useState<number | null>(null);
  const playlists = useApp((s) => s.playlists);
  const playing = useApp((s) => s.playback.state === "playing");

  useEffect(() => {
    const refresh = () => void partyView().then(setView);
    refresh();
    const subscriptions = [onPartyChanged(refresh), onPlayerEvent((e) => e.type === "trackStarted" && refresh())];
    return () => subscriptions.forEach((s) => void s.then((unlisten) => unlisten()));
  }, []);

  if (!view) return <div className="party" />;
  const rows = rowsOf(view);
  const settings = view.settings;
  const update = (next: Partial<PartySettings>) => void partySetSettings({ ...settings, ...next }).then(setView);

  const onKeyDown = (e: KeyboardEvent) => {
    if ((e.key === "Backspace" || e.key === "Delete") && selected.size > 0) {
      e.preventDefault();
      void partyRemove([...selected]).then(setView);
      setSelected(new Set());
    }
  };

  const onDrop = (e: DragEvent) => {
    e.preventDefault();
    const at = dropAt ?? view.upcoming.length;
    setDropAt(null);
    const moved = e.dataTransfer.getData(ROWS_MIME);
    const added = e.dataTransfer.getData(TRACKS_MIME);
    if (moved) void partyMove(JSON.parse(moved) as number[], at).then(setView);
    // Songs dropped from elsewhere join the queue where they land.
    else if (added) void partyAdd(JSON.parse(added) as number[], at).then(setView);
  };

  return (
    <div className="party">
      <div className="song-list-header party-header" role="row">
        <div className="song-list-header-cell" />
        {["Name", "Time", "Artist", "Album"].map((title) => (
          <div key={title} className="song-list-header-cell">
            {title}
          </div>
        ))}
      </div>
      <div
        className="song-list-body party-body"
        tabIndex={0}
        onKeyDown={onKeyDown}
        onDragOver={(e) => (e.dataTransfer.types.includes(TRACKS_MIME) || e.dataTransfer.types.includes(ROWS_MIME)) && e.preventDefault()}
        onDrop={onDrop}
      >
        {rows.map((row, i) => {
          const isSelected = row.part === "upcoming" && selected.has(row.upcomingIndex);
          const classes = ["party-row", `party-row--${row.part}`, i % 2 ? "song-row--odd" : "", isSelected ? "song-row--selected" : ""];
          return (
            <div
              key={`${row.part}-${i}-${row.track.id}`}
              className={classes.join(" ")}
              draggable={row.part === "upcoming"}
              onDragStart={(e) => e.dataTransfer.setData(ROWS_MIME, JSON.stringify(selected.has(row.upcomingIndex) ? [...selected] : [row.upcomingIndex]))}
              onDragOver={(e) => {
                if (row.part !== "upcoming") return;
                const rect = e.currentTarget.getBoundingClientRect();
                setDropAt(e.clientY - rect.top > rect.height / 2 ? row.upcomingIndex + 1 : row.upcomingIndex);
              }}
              onMouseDown={(e) => {
                if (row.part !== "upcoming") return;
                const next = new Set(e.metaKey || e.ctrlKey ? selected : []);
                if (next.has(row.upcomingIndex)) next.delete(row.upcomingIndex);
                else next.add(row.upcomingIndex);
                setSelected(next);
              }}
              onDoubleClick={() => row.part === "upcoming" && void partyPlay(row.upcomingIndex).then(setView)}
            >
              <div className="song-cell song-cell--status">{row.part === "current" && <NowPlayingIcon playing={playing} />}</div>
              <div className="song-cell">{row.track.title}</div>
              <div className="song-cell party-time">{formatClock(row.track.durationMs)}</div>
              <div className="song-cell">{row.track.artist}</div>
              <div className="song-cell">{row.track.album}</div>
              {dropAt === row.upcomingIndex && row.part === "upcoming" && <div className="song-drop-line party-drop-line" />}
            </div>
          );
        })}
        {rows.length === 0 && <div className="party-empty">Your library has nothing to shuffle yet. Add some music first.</div>}
      </div>
      <div className="party-bar">
        <label>
          Source:
          <select
            className="smart-popup"
            value={settings.source.type === "library" ? "library" : String(settings.source.id)}
            onChange={(e) => update({ source: e.target.value === "library" ? { type: "library" } : { type: "playlist", id: Number(e.target.value) } })}
          >
            <option value="library">Music</option>
            {playlists
              .filter((p) => p.kind !== "folder")
              .map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
          </select>
        </label>
        <label>
          Display:
          <select className="smart-popup" value={settings.recent} onChange={(e) => update({ recent: Number(e.target.value) })}>
            {COUNTS.map((n) => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
          </select>
          recently played songs, and
          <select className="smart-popup" value={settings.upcoming} onChange={(e) => update({ upcoming: Number(e.target.value) })}>
            {COUNTS.filter((n) => n > 0).map((n) => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
          </select>
          upcoming songs
        </label>
        <label>
          <input type="checkbox" checked={settings.favorRated} onChange={(e) => update({ favorRated: e.target.checked })} />
          Play higher rated songs more often
        </label>
        <button className="push-button" onClick={() => void partyReshuffle().then(setView)}>
          Refresh
        </button>
      </div>
    </div>
  );
}
