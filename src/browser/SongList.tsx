import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useRef, type CSSProperties, type KeyboardEvent } from "react";
import { playSourceOf, sameSource, useApp } from "../state/store";
import { COLUMNS, STATUS_COLUMN_WIDTH } from "./columns";
import "./SongList.css";

const ROW_HEIGHT = 18;

const GRID_TEMPLATE = [
  `${STATUS_COLUMN_WIDTH}px`,
  ...COLUMNS.map((c) => (c.flex ? `minmax(${Math.round(c.width / 3)}px, ${c.width}fr)` : `${c.width}px`)),
].join(" ");

function SortArrow({ descending }: { descending: boolean }) {
  return (
    <svg className="sort-arrow" width={7} height={5} viewBox="0 0 7 5" aria-hidden="true">
      <path d={descending ? "M0 0h7L3.5 5z" : "M0 5h7L3.5 0z"} />
    </svg>
  );
}

function NowPlayingIcon({ playing }: { playing: boolean }) {
  return (
    <svg className={playing ? "now-playing now-playing--on" : "now-playing"} width={12} height={10} viewBox="0 0 12 10" aria-label={playing ? "Playing" : "Paused"}>
      <path d="M0 3h2.3L5.5 0.3v9.4L2.3 7H0z" />
      <path className="now-playing-wave" d="M7.3 3.2q1.2 1.8 0 3.6M9.2 1.6q2.4 3.4 0 6.8" />
    </svg>
  );
}

export function SongList() {
  const tracks = useApp((s) => s.tracks);
  const query = useApp((s) => s.query);
  const selection = useApp((s) => s.selection);
  const anchor = useApp((s) => s.selectionAnchor);
  // Library and iPod track ids can collide, so the speaker only shows in the list playback came from.
  const nowPlayingId = useApp((s) =>
    sameSource(s.playingSource, playSourceOf(s.source)) ? s.playback.trackId : null,
  );
  const playing = useApp((s) => s.playback.state === "playing");
  const sortBy = useApp((s) => s.sortBy);
  const selectRow = useApp((s) => s.selectRow);
  const playRow = useApp((s) => s.playRow);

  const scrollRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: tracks.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 24,
  });

  useEffect(() => {
    if (anchor !== null) virtualizer.scrollToIndex(anchor, { align: "auto" });
  }, [anchor, virtualizer]);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const from = anchor ?? -1;
      const next = Math.min(tracks.length - 1, Math.max(0, from + (e.key === "ArrowDown" ? 1 : -1)));
      selectRow(next, { extend: e.shiftKey, toggle: false });
    } else if (e.key === "Enter" && anchor !== null) {
      void playRow(anchor);
    } else if (e.key === "a" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      useApp.setState({ selection: new Set(tracks.map((t) => t.id)) });
    }
  };

  return (
    <div className="song-list" style={{ "--grid": GRID_TEMPLATE } as CSSProperties}>
      <div className="song-list-header" role="row">
        <div className="song-list-header-cell" />
        {COLUMNS.map((c) => {
          const sorted = c.sort === query.sort;
          return (
            <button
              key={c.id}
              role="columnheader"
              aria-sort={sorted ? (query.descending ? "descending" : "ascending") : "none"}
              className={sorted ? "song-list-header-cell song-list-header-cell--sorted" : "song-list-header-cell"}
              style={{ justifyContent: c.align === "right" ? "flex-end" : "flex-start" }}
              onClick={() => c.sort && sortBy(c.sort)}
            >
              <span className="song-list-header-title">{c.title}</span>
              {sorted && <SortArrow descending={query.descending} />}
            </button>
          );
        })}
      </div>
      <div ref={scrollRef} className="song-list-body" tabIndex={0} onKeyDown={onKeyDown} role="grid" aria-rowcount={tracks.length}>
        <div className="song-list-stripes" />
        <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
          {virtualizer.getVirtualItems().map((row) => {
            const track = tracks[row.index];
            const selected = selection.has(track.id);
            const classes = ["song-row", row.index % 2 ? "song-row--odd" : "", selected ? "song-row--selected" : ""];
            return (
              <div
                key={track.id}
                role="row"
                aria-selected={selected}
                className={classes.join(" ")}
                style={{ transform: `translateY(${row.start}px)` }}
                onMouseDown={(e) => selectRow(row.index, { extend: e.shiftKey, toggle: e.metaKey || e.ctrlKey })}
                onDoubleClick={() => void playRow(row.index)}
              >
                <div className="song-cell song-cell--status">
                  {track.id === nowPlayingId && <NowPlayingIcon playing={playing} />}
                </div>
                {COLUMNS.map((c) => (
                  <div key={c.id} className="song-cell" style={{ textAlign: c.align ?? "left" }}>
                    {c.render(track, selected)}
                  </div>
                ))}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
