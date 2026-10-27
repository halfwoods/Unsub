import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent, type MouseEvent } from "react";
import type { Track } from "../api/types";
import { playlistIdOf, useApp } from "../state/store";
import { useLibraryViews } from "../preferences/libraryViewStore";
import { layoutFor, type ColumnId } from "../preferences/libraryViews";
import { isContextClick } from "../util/platform";
import { albumBlocks, albumRowWindow, type AlbumBlock } from "./albumKeys";
import { Artwork } from "./Artwork";
import { gridTemplate, layoutColumns, POSITION_COLUMN, STATUS_COLUMN_WIDTH } from "./columns";
import { ColumnHeadings } from "./ColumnHeadings";
import { showAlbumMenu, showSongMenu } from "./songMenu";
import { NowPlayingIcon } from "./SongList";
import "./AlbumList.css";
import "./SongList.css";

const ART = 140;
const ALBUM_COLUMN = ART + 30;

export function AlbumList() {
  const source = useApp((s) => s.source);
  const tracks = useApp((s) => s.tracks);
  const query = useApp((s) => s.query);
  const selection = useApp((s) => s.selection);
  const anchor = useApp((s) => s.selectionAnchor);
  const nowPlayingId = useApp((s) => (s.playingSource.type === "library" ? s.playback.trackId : null));
  const playing = useApp((s) => s.playback.state === "playing");
  const { selectRow, playRow, sortBy, deleteSelection } = useApp.getState();
  const isRegularPlaylist = useApp((s) => s.playlists.some((p) => p.id === playlistIdOf(s.source) && p.kind === "regular"));
  const prefs = useLibraryViews((s) => s.prefs);
  const layout = useMemo(() => layoutFor(prefs, source, isRegularPlaylist), [prefs, source, isRegularPlaylist]);
  const [draftWidths, setDraftWidths] = useState<Partial<Record<ColumnId, number>>>({});
  useEffect(() => setDraftWidths({}), [source]);
  const columns = useMemo(() => {
    const base = layoutColumns(layout).filter(c => c.id !== "album");
    return (isRegularPlaylist ? [POSITION_COLUMN, ...base] : base).map(c => ({ ...c, width: draftWidths[c.id as ColumnId] ?? c.width }));
  }, [layout, isRegularPlaylist, draftWidths]);
  const tableWidth = ALBUM_COLUMN + STATUS_COLUMN_WIDTH + columns.reduce((sum, column) => sum + column.width, 0);
  const groups = useMemo(() => albumBlocks(tracks), [tracks]);
  const scrollRef = useRef<HTMLDivElement>(null);
  const headerRef = useRef<HTMLDivElement>(null);
  const [headerHeight, setHeaderHeight] = useState(20);
  const rowHeight = useApp((s) => s.rowHeight);
  const minRows = Math.ceil((ART + 42) / rowHeight);
  useLayoutEffect(() => {
    const element = headerRef.current; if (!element) return;
    const measure = () => setHeaderHeight(element.offsetHeight);
    measure(); const observer = new ResizeObserver(measure); observer.observe(element);
    return () => observer.disconnect();
  }, []);
  const virtualizer = useVirtualizer({
    count: groups.length,
    getScrollElement: () => scrollRef.current,
    getItemKey: (index) => groups[index].key,
    estimateSize: (index) => Math.max(groups[index].tracks.length, minRows) * rowHeight,
    scrollMargin: headerHeight,
    scrollPaddingStart: headerHeight,
    overscan: 4,
  });
  // Estimate callbacks are not part of the virtualizer's measurement-cache key.
  useLayoutEffect(() => virtualizer.measure(), [groups, minRows, rowHeight, virtualizer]);
  useLayoutEffect(() => {
    const element = scrollRef.current;
    if (!element || anchor === null) return;
    const groupIndex = groups.findIndex(group => anchor >= group.start && anchor < group.start + group.tracks.length);
    if (groupIndex < 0) return;
    const top = headerHeight + groups.slice(0, groupIndex).reduce((sum, group) => sum + Math.max(group.tracks.length, minRows) * rowHeight, 0)
      + (anchor - groups[groupIndex].start) * rowHeight;
    if (top < element.scrollTop + headerHeight) element.scrollTop = top - headerHeight;
    else if (top + rowHeight > element.scrollTop + element.clientHeight) element.scrollTop = top + rowHeight - element.clientHeight;
  }, [anchor, groups, headerHeight, minRows, rowHeight]);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.target instanceof Element && event.target.closest(".song-list-header")) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      selectRow(Math.min(tracks.length - 1, Math.max(0, (anchor ?? -1) + (event.key === "ArrowDown" ? 1 : -1))), { extend: event.shiftKey, toggle: false });
    } else if (event.key === "Enter" && anchor !== null) { event.preventDefault(); void playRow(anchor); }
    else if (event.key === "a" && (event.metaKey || event.ctrlKey)) { event.preventDefault(); useApp.setState({ selection: new Set(tracks.map(t => t.id)) }); }
    else if (event.key === "Delete" || event.key === "Backspace") { event.preventDefault(); void deleteSelection(); }
  };
  const onSongMenu = (event: MouseEvent, track: Track, index: number) => {
    event.preventDefault();
    if (!selection.has(track.id)) selectRow(index, { extend: false, toggle: false });
    void showSongMenu(track, isRegularPlaylist);
  };
  const onAlbumMenu = (event: MouseEvent, group: AlbumBlock) => {
    event.preventDefault();
    if (group.tracks.every(t => selection.has(t.id))) { void showAlbumMenu(tracks.filter(t => selection.has(t.id))); return; }
    useApp.setState({ selection: new Set(group.tracks.map(t => t.id)), selectionAnchor: group.start });
    void showAlbumMenu(group.tracks);
  };

  return (
    <div className="song-list song-list--custom album-list" style={{ "--grid": gridTemplate(columns), "--song-table-width": `${tableWidth}px` } as CSSProperties}>
      <div ref={scrollRef} className="song-list-body" tabIndex={0} role="grid" aria-label="Songs by album" onKeyDown={onKeyDown}>
        <div ref={headerRef} className="song-list-header album-list-header" role="row">
          <div className="song-list-header-cell album-list-album-heading" style={{ width: ALBUM_COLUMN }} role="columnheader">
            <button className="song-heading-sort" onClick={() => sortBy("album")} aria-label="Album, sort">Album</button>
          </div>
          <div className="album-list-header-tracks">
            <ColumnHeadings columns={columns} layout={layout} source={source} regular={isRegularPlaylist} editable
              sort={query.sort} descending={query.descending} tracks={tracks} sortBy={sortBy} excluded={["album"]}
              preview={(id, width) => setDraftWidths(current => { const next = { ...current }; if (width === null) delete next[id]; else next[id] = width; return next; })} />
          </div>
        </div>
        {tracks.length === 0 && <div className="song-list-empty" role="status">No songs match this view. Clear the search or filters to see more.</div>}
        <div className="song-list-rows" style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
          {virtualizer.getVirtualItems().map(item => {
            const group = groups[item.index], first = group.tracks[0], rows = Math.max(group.tracks.length, minRows);
            const window = albumRowWindow(rows, item.start, virtualizer.scrollOffset ?? 0, virtualizer.scrollRect?.height ?? 600, rowHeight);
            return <div key={item.key} className="album-block" style={{ transform: `translateY(${item.start - headerHeight}px)`, height: rows * rowHeight }}>
              <div className="album-block-info" style={{ width: ALBUM_COLUMN }} onContextMenu={event => onAlbumMenu(event, group)}>
                <Artwork trackId={first.id} size={ART} className="album-block-art" />
                <div className="album-block-title" title={first.album}>{first.album || "Unknown Album"}</div>
                <div className="album-block-artist" title={first.compilation ? "Various Artists" : first.albumArtist || first.artist}>{first.compilation ? "Various Artists" : first.albumArtist || first.artist}</div>
              </div>
              <div className="album-block-tracks">
                <div style={{ height: window.start * rowHeight }} aria-hidden="true" />
                {Array.from({ length: window.end - window.start }, (_, visibleRow) => {
                  const row = window.start + visibleRow;
                  const track = group.tracks[row], index = group.start + row;
                  if (!track) return <div key={`pad-${row}`} className={row % 2 ? "song-row-static song-row--odd" : "song-row-static"} />;
                  const selected = selection.has(track.id);
                  return <div key={`${index}-${track.id}`} role="row" aria-selected={selected}
                    className={["song-row-static", row % 2 ? "song-row--odd" : "", selected ? "song-row--selected" : ""].join(" ")}
                    onMouseDown={event => !isContextClick(event) && selectRow(index, { extend: event.shiftKey, toggle: event.metaKey || event.ctrlKey })}
                    onContextMenu={event => onSongMenu(event, track, index)} onDoubleClick={() => void playRow(index)}>
                    <div className="song-cell song-cell--status" role="gridcell">{track.id === nowPlayingId && <NowPlayingIcon playing={playing} />}</div>
                    {columns.map(column => <div key={column.id} role="gridcell" className="song-cell" style={{ textAlign: column.align ?? "left" }}>{column.render(track, selected, index)}</div>)}
                  </div>;
                })}
                <div style={{ height: (rows - window.end) * rowHeight }} aria-hidden="true" />
              </div>
            </div>;
          })}
        </div>
      </div>
    </div>
  );
}
