import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type DragEvent, type KeyboardEvent, type MouseEvent } from "react";
import { useLibraryViews } from "../preferences/libraryViewStore";
import { isLayoutSource, layoutFor, type ColumnId } from "../preferences/libraryViews";
import { libraryAddToPlaylist, libraryMoveInPlaylist } from "../api/commands";
import { DEVICE_TRACKS_MIME, TRACKS_MIME, type DeviceDrag } from "../sidebar/PlaylistRows";
import { CdImportStatus } from "../cd/Cd";
import {
  cdIdOf,
  parseDeviceSource,
  playlistIdOf,
  playSourceOf,
  sameSource,
  sharedIdOf,
  useApp,
} from "../state/store";
import { isContextClick } from "../util/platform";
import { CD_COLUMNS, COLUMNS, gridTemplate, layoutColumns, POSITION_COLUMN, SHARED_COLUMNS, STATUS_COLUMN_WIDTH } from "./columns";
import { ColumnHeadings } from "./ColumnHeadings";
import { formatColumnText } from "./columnFormat";
import { hasBrowseFilters } from "./browseScope";
import { showCdSongMenu, showDeviceSongMenu, showMenu, showSongMenu } from "./songMenu";
import "./SongList.css";

/** Row positions being dragged within one playlist, so drops reorder instead of adding copies. */
const ROWS_MIME = "application/x-unsub-rows";

export function NowPlayingIcon({ playing }: { playing: boolean }) {
  return (
    <svg className={playing ? "now-playing now-playing--on" : "now-playing"} width={12} height={10} viewBox="0 0 12 10" aria-label={playing ? "Playing" : "Paused"}>
      <path d="M0 3h2.3L5.5 0.3v9.4L2.3 7H0z" />
      <path className="now-playing-wave" d="M7.3 3.2q1.2 1.8 0 3.6M9.2 1.6q2.4 3.4 0 6.8" />
    </svg>
  );
}

export function SongList() {
  const source = useApp((s) => s.source);
  const prefs = useLibraryViews((s) => s.prefs);
  const [draftWidths, setDraftWidths] = useState<Partial<Record<ColumnId, number>>>({});
  const tracks = useApp((s) => s.tracks);
  const query = useApp((s) => s.query);
  const selection = useApp((s) => s.selection);
  const anchor = useApp((s) => s.selectionAnchor);
  const songReveal = useApp((s) => s.songReveal);
  // Library and iPod track ids can collide, so the speaker only shows in the list playback came from.
  const nowPlayingId = useApp((s) =>
    sameSource(s.playingSource, playSourceOf(s.source)) ? s.playback.trackId : null,
  );
  const playing = useApp((s) => s.playback.state === "playing");
  const playlistId = useApp((s) => playlistIdOf(s.source));
  const isRegularPlaylist = useApp((s) => s.playlists.some((p) => p.id === playlistIdOf(s.source) && p.kind === "regular"));
  const layout = useMemo(() => layoutFor(prefs, source, isRegularPlaylist), [prefs, source, isRegularPlaylist]);
  const editable = isLayoutSource(source);
  useEffect(() => setDraftWidths({}), [source]);
  const { sortBy, selectRow, playRow, deleteSelection, refresh, refreshPlaylists } = useApp.getState();
  const [dropIndex, setDropIndex] = useState<number | null>(null);
  const rowHeight = useApp((s) => s.rowHeight);

  const cdId = useApp((s) => cdIdOf(s.source));
  const isCd = cdId !== null;
  const isShared = useApp((s) => sharedIdOf(s.source) !== null);
  // An iPod's songs can't be edited, but they can be copied into the library.
  const deviceId = useApp((s) => parseDeviceSource(s.source)?.deviceId ?? null);
  const columns = useMemo(() => {
    // Device IDs can collide with library IDs: ratings and checkboxes must never edit local songs.
    const deviceColumns = COLUMNS.filter((c) => c.id !== "enabled").map((c) => c.id === "rating" ? { ...c, render: (t: typeof tracks[number]) => formatColumnText("rating", t) } : c);
    const base = isCd ? CD_COLUMNS : isShared ? SHARED_COLUMNS : deviceId !== null ? deviceColumns : layoutColumns(layout);
    return (isRegularPlaylist ? [POSITION_COLUMN, ...base] : base).map(c => ({ ...c, flex: false, width: draftWidths[c.id as ColumnId] ?? c.width }));
  }, [isCd, isShared, deviceId, layout, isRegularPlaylist, draftWidths]);
  const tableWidth = STATUS_COLUMN_WIDTH + columns.reduce((sum, c) => sum + c.width, 0);
  // Reordering only makes sense while the playlist is shown in its own order.
  const reorderable = isRegularPlaylist && query.sort === "playlistOrder" && !query.descending && query.search === null && !hasBrowseFilters(query) && query.ids == null;
  useEffect(() => setDropIndex(null), [source, query]);

  const scrollRef = useRef<HTMLDivElement>(null);
  const headerRef = useRef<HTMLDivElement>(null);
  const [headerHeight, setHeaderHeight] = useState(20);
  useLayoutEffect(() => {
    const element = headerRef.current; if (!element) return;
    const measure = () => setHeaderHeight(element.offsetHeight);
    measure(); const observer = new ResizeObserver(measure); observer.observe(element);
    return () => observer.disconnect();
  }, []);
  const virtualizer = useVirtualizer({
    count: tracks.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => rowHeight,
    overscan: 24,
    scrollMargin: headerHeight,
    scrollPaddingStart: headerHeight,
  });

  const lastScrolledAnchor = useRef<number | null>(null);
  useLayoutEffect(() => {
    const anchorChanged = lastScrolledAnchor.current !== anchor;
    lastScrolledAnchor.current = anchor;
    const element = scrollRef.current;
    if (!element || !songReveal || songReveal.source !== source || songReveal.query !== query || songReveal.trackId !== nowPlayingId
      || anchor === null || tracks[anchor]?.id !== songReveal.trackId) {
      if (anchorChanged && anchor !== null) virtualizer.scrollToIndex(anchor, { align: "auto" });
      return;
    }
    // A sheet may have opened between the command and this layout effect.
    if (document.querySelector('.sheet-backdrop, [aria-modal="true"]')) {
      if (useApp.getState().songReveal === songReveal) useApp.setState({ songReveal: null });
      return;
    }
    virtualizer.scrollToIndex(anchor, { align: "center" });
    const name = columns.findIndex(column => column.id === "name");
    if (name >= 0) {
      const left = STATUS_COLUMN_WIDTH + columns.slice(0, name).reduce((sum, column) => sum + column.width, 0), right = left + columns[name].width;
      if (left < element.scrollLeft || columns[name].width >= element.clientWidth) element.scrollLeft = left;
      else if (right > element.scrollLeft + element.clientWidth) element.scrollLeft = right - element.clientWidth;
    }
    element.focus({ preventScroll: true });
    if (useApp.getState().songReveal === songReveal) useApp.setState({ songReveal: null });
  }, [songReveal, source, query, nowPlayingId, anchor, tracks, columns, virtualizer]);

  useEffect(() => virtualizer.measure(), [rowHeight, virtualizer]);

  const selectedPositions = () => tracks.flatMap((t, i) => (selection.has(t.id) ? [i] : []));

  const afterPlaylistEdit = async () => {
    await Promise.all([refresh(), refreshPlaylists()]);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.target instanceof Element && e.target.closest(".song-list-header")) return;
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
    e.preventDefault();
    if (!selection.has(tracks[index].id)) selectRow(index, { extend: false, toggle: false });
    const play = () => void playRow(index);
    if (deviceId !== null) void showDeviceSongMenu(deviceId);
    else if (cdId !== null) void showCdSongMenu(cdId, play);
    else if (isShared) void showMenu([{ text: "Play", action: play }]);
    // A CD's, iPod's or shared library's songs are read-only; the rest are the library's.
    else void showSongMenu(tracks[index], isRegularPlaylist);
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
    if (!reorderable || playlistId === null || to === null) return;
    const rows = e.dataTransfer.getData(ROWS_MIME);
    const ids = e.dataTransfer.getData(TRACKS_MIME);
    if (rows) void libraryMoveInPlaylist(playlistId, JSON.parse(rows) as number[], to).then(afterPlaylistEdit);
    else if (ids) void libraryAddToPlaylist(playlistId, JSON.parse(ids) as number[], to).then(afterPlaylistEdit);
  };

  return (
    <div className="song-list song-list--custom" style={{ "--grid": gridTemplate(columns), "--song-table-width": `${tableWidth}px` } as CSSProperties}>
      <div
        ref={scrollRef}
        className="song-list-body"
        tabIndex={0}
        onKeyDown={onKeyDown}
        onDragLeave={(e) => e.currentTarget === e.target && setDropIndex(null)}
        onDrop={onDrop}
        role="grid"
        aria-label="Songs"
        aria-colcount={columns.length + 1}
        aria-rowcount={tracks.length + 1}
      >
        <div ref={headerRef} className="song-list-header" role="row" aria-rowindex={1}>
          <ColumnHeadings columns={columns} layout={layout} source={source} regular={isRegularPlaylist} editable={editable}
            sort={query.sort} descending={query.descending} tracks={tracks} sortBy={sortBy}
            preview={(id, width) => setDraftWidths(current => { const next = { ...current }; if (width === null) delete next[id]; else next[id] = width; return next; })} />
        </div>
        <div className="song-list-stripes" style={{ top: headerHeight }} />
        <div className="song-list-rows" style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
          {virtualizer.getVirtualItems().map((row) => {
            const track = tracks[row.index];
            const selected = selection.has(track.id);
            const classes = ["song-row", row.index % 2 ? "song-row--odd" : "", selected ? "song-row--selected" : ""];
            return (
              <div
                // Playlists may repeat a song, so rows are keyed by position as well as id.
                key={`${row.index}-${track.id}`}
                role="row"
                aria-rowindex={row.index + 2}
                aria-selected={selected}
                className={classes.join(" ")}
                style={{ transform: `translateY(${row.start - headerHeight}px)` }}
                draggable
                onDragStart={(e) => onDragStart(e, row.index)}
                onDragOver={(e) => onDragOver(e, row.index)}
                // A context click is settled in onContextMenu, so it can keep a multi-selection.
                onMouseDown={(e) => !isContextClick(e) && selectRow(row.index, { extend: e.shiftKey, toggle: e.metaKey || e.ctrlKey })}
                onContextMenu={(e) => onContextMenu(e, row.index)}
                onDoubleClick={() => void playRow(row.index)}
              >
                <div role="gridcell" className="song-cell song-cell--status">
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
                  <div key={c.id} role="gridcell" title={formatColumnText(c.id, track) || undefined} className="song-cell" style={{ textAlign: c.align ?? "left" }}>
                    {c.render(track, selected, row.index)}
                  </div>
                ))}
              </div>
            );
          })}
          {dropIndex !== null && <div className="song-drop-line" style={{ top: dropIndex * rowHeight - 1 }} />}
        </div>
        {tracks.length === 0 && <div className="song-list-empty">No songs match this view.</div>}
      </div>
    </div>
  );
}
