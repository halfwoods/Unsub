import { useVirtualizer } from "@tanstack/react-virtual";
import { useMemo, useRef, type CSSProperties, type MouseEvent } from "react";
import type { Track } from "../api/types";
import { playlistIdOf, useApp } from "../state/store";
import { isContextClick } from "../util/platform";
import { trackAlbumKey } from "./albumKeys";
import { Artwork } from "./Artwork";
import { COLUMNS, gridTemplate } from "./columns";
import { showAlbumMenu, showSongMenu } from "./songMenu";
import { NowPlayingIcon } from "./SongList";
import "./AlbumList.css";
import "./SongList.css";

const ART = 140;
const ALBUM_COLUMN = ART + 30;

// The album column replaces Album; the rest are the song list's own columns.
const TRACK_COLUMNS = COLUMNS.filter((c) => c.id !== "album");
const GRID_TEMPLATE = gridTemplate(TRACK_COLUMNS);

interface Group {
  start: number;
  tracks: Track[];
}

/** Albums are runs of consecutive tracks; the list is sorted by album while this view is on. */
function groupTracks(tracks: Track[]): Group[] {
  const groups: Group[] = [];
  tracks.forEach((track, index) => {
    const last = groups[groups.length - 1];
    if (last && trackAlbumKey(last.tracks[0]) === trackAlbumKey(track)) last.tracks.push(track);
    else groups.push({ start: index, tracks: [track] });
  });
  return groups;
}

export function AlbumList() {
  const tracks = useApp((s) => s.tracks);
  const selection = useApp((s) => s.selection);
  const nowPlayingId = useApp((s) => (s.playingSource.type === "library" ? s.playback.trackId : null));
  const playing = useApp((s) => s.playback.state === "playing");
  const selectRow = useApp((s) => s.selectRow);
  const playRow = useApp((s) => s.playRow);
  const isRegularPlaylist = useApp((s) => s.playlists.some((p) => p.id === playlistIdOf(s.source) && p.kind === "regular"));
  const groups = useMemo(() => groupTracks(tracks), [tracks]);
  const scrollRef = useRef<HTMLDivElement>(null);
  const rowHeight = useApp((s) => s.rowHeight);
  // Each album block is tall enough for its cover plus the album/artist caption.
  const minRows = Math.ceil((ART + 42) / rowHeight);

  const virtualizer = useVirtualizer({
    count: groups.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => Math.max(groups[i].tracks.length, minRows) * rowHeight,
    overscan: 4,
  });

  const onSongMenu = (e: MouseEvent, track: Track, index: number) => {
    e.preventDefault();
    if (!selection.has(track.id)) selectRow(index, { extend: false, toggle: false });
    void showSongMenu(track, isRegularPlaylist);
  };

  // An album already selected whole keeps the selection, which may take in other albums; any other is selected in its place.
  const onAlbumMenu = (e: MouseEvent, group: Group) => {
    e.preventDefault();
    if (group.tracks.every((t) => selection.has(t.id))) {
      void showAlbumMenu(tracks.filter((t) => selection.has(t.id)));
      return;
    }
    useApp.setState({ selection: new Set(group.tracks.map((t) => t.id)), selectionAnchor: group.start });
    void showAlbumMenu(group.tracks);
  };

  return (
    <div className="song-list album-list" style={{ "--grid": GRID_TEMPLATE } as CSSProperties}>
      <div className="song-list-header album-list-header" role="row">
        <div className="song-list-header-cell" style={{ width: ALBUM_COLUMN }}>
          Album
        </div>
        <div className="album-list-header-tracks">
          <div className="song-list-header-cell" />
          {TRACK_COLUMNS.map((c) => (
            <div key={c.id} className="song-list-header-cell" style={{ justifyContent: c.align === "right" ? "flex-end" : "flex-start" }}>
              {c.title}
            </div>
          ))}
        </div>
      </div>
      <div ref={scrollRef} className="song-list-body" tabIndex={0} role="grid">
        <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
          {virtualizer.getVirtualItems().map((item) => {
            const group = groups[item.index];
            const first = group.tracks[0];
            const rows = Math.max(group.tracks.length, minRows);
            return (
              <div key={item.key} className="album-block" style={{ transform: `translateY(${item.start}px)`, height: rows * rowHeight }}>
                <div className="album-block-info" style={{ width: ALBUM_COLUMN }} onContextMenu={(e) => onAlbumMenu(e, group)}>
                  <Artwork trackId={first.id} size={ART} className="album-block-art" />
                  <div className="album-block-title">{first.album || "Unknown Album"}</div>
                  <div className="album-block-artist">{first.compilation ? "Various Artists" : first.albumArtist || first.artist}</div>
                </div>
                <div className="album-block-tracks">
                  {Array.from({ length: rows }, (_, row) => {
                    const track = group.tracks[row];
                    const index = group.start + row;
                    if (!track) return <div key={`pad-${row}`} className={row % 2 ? "song-row-static song-row--odd" : "song-row-static"} />;
                    const selected = selection.has(track.id);
                    const classes = ["song-row-static", row % 2 ? "song-row--odd" : "", selected ? "song-row--selected" : ""];
                    return (
                      <div
                        key={track.id}
                        role="row"
                        aria-selected={selected}
                        className={classes.join(" ")}
                        // A context click is settled in onContextMenu, so it can keep a multi-selection.
                        onMouseDown={(e) => !isContextClick(e) && selectRow(index, { extend: e.shiftKey, toggle: e.metaKey || e.ctrlKey })}
                        onContextMenu={(e) => onSongMenu(e, track, index)}
                        onDoubleClick={() => void playRow(index)}
                      >
                        <div className="song-cell song-cell--status">{track.id === nowPlayingId && <NowPlayingIcon playing={playing} />}</div>
                        {TRACK_COLUMNS.map((c) => (
                          <div key={c.id} className="song-cell" style={{ textAlign: c.align ?? "left" }}>
                            {c.render(track, selected, index)}
                          </div>
                        ))}
                      </div>
                    );
                  })}
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
