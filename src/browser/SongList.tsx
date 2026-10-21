import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useMemo, useRef, useState, type CSSProperties, type DragEvent, type KeyboardEvent, type MouseEvent } from "react";
import { libraryAddToPlaylist, libraryMoveInPlaylist } from "../api/commands";
import { DEVICE_TRACKS_MIME, TRACKS_MIME, type DeviceDrag } from "../sidebar/PlaylistRows";
import { CdImportStatus } from "../cd/Cd";
import {
  cdIdOf,
  isLibrarySource,
  isPlaylistSource,
  parseDeviceSource,
  playlistIdOf,
  playSourceOf,
  sameSource,
  sharedIdOf,
  useApp,
} from "../state/store";
import { IS_MAC } from "../util/platform";
import { CD_COLUMNS, COLUMNS, gridTemplate, POSITION_COLUMN, SHARED_COLUMNS } from "./columns";
import { showDeviceSongMenu, showSongMenu } from "./songMenu";
import "./SongList.css";

/** Row positions being dragged within one playlist, so drops reorder instead of adding copies. */
const ROWS_MIME = "application/x-unsub-rows";

function SortArrow({ descending }: { descending: boolean }) {
  return (
    <svg className="sort-arrow" width={7} height={5} viewBox="0 0 7 5" aria-hidden="true">
      <path d={descending ? "M0 0h7L3.5 5z" : "M0 5h7L3.5 0z"} />
    </svg>
  );
}

export function NowPlayingIcon({ playing }: { playing: boolean }) {
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
  const playlistId = useApp((s) => playlistIdOf(s.source));
  const isRegularPlaylist = useApp((s) => s.playlists.some((p) => p.id === playlistIdOf(s.source) && p.kind === "regular"));
  const { sortBy, selectRow, playRow, deleteSelection, refresh, refreshPlaylists } = useApp.getState();
  const [dropIndex, setDropIndex] = useState<number | null>(null);
  const rowHeight = useApp((s) => s.rowHeight);

  const isCd = useApp((s) => cdIdOf(s.source) !== null);
  // Only the library's songs can be edited; a CD's, iPod's or shared library's are read-only.
  const isEditable = useApp((s) => isLibrarySource(s.source) || isPlaylistSource(s.source));
  const isShared = useApp((s) => sharedIdOf(s.source) !== null);
  // An iPod's songs can't be edited, but they can be copied into the library.
  const deviceId = useApp((s) => parseDeviceSource(s.source)?.deviceId ?? null);
  const columns = useMemo(
    () => (isCd ? CD_COLUMNS : isShared ? SHARED_COLUMNS : isRegularPlaylist ? [POSITION_COLUMN, ...COLUMNS] : COLUMNS),
    [isCd, isShared, isRegularPlaylist],
  );
  // Reordering only makes sense while the playlist is shown in its own order.
  const reorderable = isRegularPlaylist && query.sort === "playlistOrder" && !query.descending && query.search === null;

  const scrollRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: tracks.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => rowHeight,
    overscan: 24,
  });

  useEffect(() => {
    if (anchor !== null) virtualizer.scrollToIndex(anchor, { align: "auto" });
  }, [anchor, virtualizer]);

  useEffect(() => virtualizer.measure(), [rowHeight, virtualizer]);

  const selectedPositions = () => tracks.flatMap((t, i) => (selection.has(t.id) ? [i] : []));

  const afterPlaylistEdit = async () => {
    await Promise.all([refresh(), refreshPlaylists()]);
  };

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
    } else if (e.key === "Backspace" || e.key === "Delete") {
      e.preventDefault();
      void deleteSelection();
    }
  };

  const onContextMenu = (e: MouseEvent, index: number) => {
    if (!isEditable && deviceId === null) return;
    e.preventDefault();
    if (!selection.has(tracks[index].id)) selectRow(index, { extend: false, toggle: false });
    void (deviceId !== null ? showDeviceSongMenu(deviceId) : showSongMenu(tracks[index], isRegularPlaylist));
  };

  const onDragStart = (e: DragEvent, index: number) => {
    const dragged = selection.has(tracks[index].id) ? tracks.filter((t) => selection.has(t.id)) : [tracks[index]];
    e.dataTransfer.effectAllowed = "copyMove";
    if (deviceId !== null) {
      const drag: DeviceDrag = { deviceId, ids: dragged.map((t) => t.id) };
      e.dataTransfer.setData(DEVICE_TRACKS_MIME, JSON.stringify(drag));
      return;
    }
    e.dataTransfer.setData(TRACKS_MIME, JSON.stringify(dragged.map((t) => t.id)));
    if (reorderable) {
      e.dataTransfer.setData(ROWS_MIME, JSON.stringify(selection.has(tracks[index].id) ? selectedPositions() : [index]));
    }
  };

  const onDragOver = (e: DragEvent, index: number) => {
    if (!reorderable || !e.dataTransfer.types.includes(TRACKS_MIME)) return;
    e.preventDefault();
    const rect = e.currentTarget.getBoundingClientRect();
    setDropIndex(e.clientY - rect.top > rect.height / 2 ? index + 1 : index);
  };

  const onDrop = (e: DragEvent) => {
    e.preventDefault();
    const to = dropIndex;
    setDropIndex(null);
    if (playlistId === null || to === null) return;
    const rows = e.dataTransfer.getData(ROWS_MIME);
    const ids = e.dataTransfer.getData(TRACKS_MIME);
    if (rows) void libraryMoveInPlaylist(playlistId, JSON.parse(rows) as number[], to).then(afterPlaylistEdit);
    else if (ids) void libraryAddToPlaylist(playlistId, JSON.parse(ids) as number[], to).then(afterPlaylistEdit);
  };

  return (
    <div className="song-list" style={{ "--grid": gridTemplate(columns) } as CSSProperties}>
      <div className="song-list-header" role="row">
        <div className="song-list-header-cell" />
        {columns.map((c) => {
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
      <div
        ref={scrollRef}
        className="song-list-body"
        tabIndex={0}
        onKeyDown={onKeyDown}
        onDragLeave={(e) => e.currentTarget === e.target && setDropIndex(null)}
        onDrop={onDrop}
        role="grid"
        aria-rowcount={tracks.length}
      >
        <div className="song-list-stripes" />
        <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
          {virtualizer.getVirtualItems().map((row) => {
            const track = tracks[row.index];
            const selected = selection.has(track.id);
            const classes = ["song-row", row.index % 2 ? "song-row--odd" : "", selected ? "song-row--selected" : ""];
            return (
              <div
                // Playlists may repeat a song, so rows are keyed by position as well as id.
                key={`${row.index}-${track.id}`}
                role="row"
                aria-selected={selected}
                className={classes.join(" ")}
                style={{ transform: `translateY(${row.start}px)` }}
                draggable
                onDragStart={(e) => onDragStart(e, row.index)}
                onDragOver={(e) => onDragOver(e, row.index)}
                onMouseDown={(e) => {
                  // A list with a menu settles a context click (a right-click, or Ctrl-click on a Mac) in onContextMenu, so it can keep a multi-selection.
                  const contextClick = e.button === 2 || (IS_MAC && e.ctrlKey);
                  if (!((isEditable || deviceId !== null) && contextClick)) selectRow(row.index, { extend: e.shiftKey, toggle: e.metaKey || e.ctrlKey });
                }}
                onContextMenu={(e) => onContextMenu(e, row.index)}
                onDoubleClick={() => void playRow(row.index)}
              >
                <div className="song-cell song-cell--status">
                  {track.id === nowPlayingId ? (
                    <NowPlayingIcon playing={playing} />
                  ) : isCd ? (
                    <CdImportStatus number={track.id} />
                  ) : track.missing ? (
                    // iTunes' exclamation mark: the file has gone; playing it offers to locate it.
                    <svg className="missing-icon" width={12} height={12} viewBox="0 0 12 12" aria-label="File not found">
                      <title>The file can't be found</title>
                      <circle cx="6" cy="6" r="6" />
                      <path d="M6 2.8v4M6 8.7v.4" />
                    </svg>
                  ) : (
                    track.inCloud && (
                      <svg className="cloud-icon" width={13} height={9} viewBox="0 0 13 9" aria-label="In iCloud">
                        <path d="M3.5 8.5h6.3a2.7 2.7 0 0 0 .4-5.4A3.6 3.6 0 0 0 3.4 3a2.8 2.8 0 0 0 .1 5.5z" />
                      </svg>
                    )
                  )}
                </div>
                {columns.map((c) => (
                  <div key={c.id} className="song-cell" style={{ textAlign: c.align ?? "left" }}>
                    {c.render(track, selected, row.index)}
                  </div>
                ))}
              </div>
            );
          })}
          {dropIndex !== null && <div className="song-drop-line" style={{ top: dropIndex * rowHeight - 1 }} />}
        </div>
      </div>
    </div>
  );
}
