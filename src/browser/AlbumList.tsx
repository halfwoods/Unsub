import { useVirtualizer } from "@tanstack/react-virtual";
import { useMemo, useRef, type CSSProperties } from "react";
import type { Track } from "../api/types";
import { useApp } from "../state/store";
import { trackAlbumKey } from "./albumKeys";
import { Artwork } from "./Artwork";
import { COLUMNS, gridTemplate } from "./columns";
import { NowPlayingIcon } from "./SongList";
import "./AlbumList.css";
import "./SongList.css";

const ROW = 18;
const ART = 140;
/** Each album block is tall enough for its cover plus the album/artist caption. */
const MIN_ROWS = Math.ceil((ART + 42) / ROW);
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
  const groups = useMemo(() => groupTracks(tracks), [tracks]);
  const scrollRef = useRef<HTMLDivElement>(null);

  const virtualizer = useVirtualizer({
    count: groups.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => Math.max(groups[i].tracks.length, MIN_ROWS) * ROW,
    overscan: 4,
  });

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
            const rows = Math.max(group.tracks.length, MIN_ROWS);
            return (
              <div key={item.key} className="album-block" style={{ transform: `translateY(${item.start}px)`, height: rows * ROW }}>
                <div className="album-block-info" style={{ width: ALBUM_COLUMN }}>
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
                        onMouseDown={(e) => selectRow(index, { extend: e.shiftKey, toggle: e.metaKey || e.ctrlKey })}
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
