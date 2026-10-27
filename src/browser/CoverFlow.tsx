import { useEffect, useMemo, useRef, useState, type MouseEvent, type PointerEvent, type WheelEvent } from "react";
import type { AlbumSummary } from "../api/types";
import { useApp } from "../state/store";
import { isContextClick } from "../util/platform";
import { shownTracksOf, summaryAlbumKey, trackAlbumKey } from "./albumKeys";
import { Artwork } from "./Artwork";
import { showAlbumMenu } from "./songMenu";
import { SongList } from "./SongList";
import "./CoverFlow.css";

/** How many covers to draw on each side; the rest are off-stage anyway. */
const VISIBLE = 9;


function placement(offset: number, cover: number) {
  if (offset === 0) return `translateX(-50%) translateZ(${cover * 0.35}px)`;
  const side = Math.sign(offset);
  // A clear gap between the facing cover and each side stack, then tight spacing, like iTunes.
  const x = side * (cover * 0.78 + (Math.abs(offset) - 1) * cover * 0.17);
  return `translateX(calc(-50% + ${x}px)) rotateY(${-side * 70}deg)`;
}

function Stage({ albums, center, onCenter }: { albums: AlbumSummary[]; center: number; onCenter: (i: number) => void }) {
  const stageRef = useRef<HTMLDivElement>(null);
  const [height, setHeight] = useState(300);
  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const observer = new ResizeObserver(([entry]) => setHeight(entry.contentRect.height));
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const cover = Math.max(80, Math.round(height * 0.52));
  const current = albums[center];
  const clamp = (i: number) => Math.min(Math.max(0, albums.length - 1), Math.max(0, i));

  const onWheel = (e: WheelEvent) => {
    const delta = Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
    if (Math.abs(delta) > 4) onCenter(clamp(center + Math.sign(delta)));
  };

  const sliderRef = useRef<HTMLDivElement>(null);
  const seek = (clientX: number) => {
    const rect = sliderRef.current?.getBoundingClientRect();
    if (!rect || albums.length < 2) return;
    onCenter(clamp(Math.round(((clientX - rect.left) / rect.width) * (albums.length - 1))));
  };
  // The album under the cursor takes the centre, which is what selects it; the one already there keeps the song selection.
  const onAlbumMenu = (e: MouseEvent, i: number) => {
    e.preventDefault();
    if (i !== center) onCenter(i);
    void showAlbumMenu(shownTracksOf(useApp.getState().tracks, albums[i]));
  };

  const onSliderDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    seek(e.clientX);
  };
  const onSliderMove = (e: PointerEvent<HTMLDivElement>) => {
    if (e.currentTarget.hasPointerCapture(e.pointerId)) seek(e.clientX);
  };

  const thumbWidth = Math.max(24, 100 / Math.max(1, albums.length));
  const thumbLeft = albums.length > 1 ? (center / (albums.length - 1)) * (100 - thumbWidth) : 0;

  return (
    <div
      ref={stageRef}
      className="coverflow-stage"
      tabIndex={0}
      aria-label="Cover Flow albums"
      onWheel={onWheel}
      onKeyDown={(e) => {
        if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)) {
          e.preventDefault();
          onCenter(clamp(e.key === "Home" ? 0 : e.key === "End" ? albums.length - 1 : center + (e.key === "ArrowLeft" ? -1 : 1)));
        }
        if (e.key === "Enter" && e.target === e.currentTarget && current) void useApp.getState().playAlbum(current.key);
      }}
    >
      <div className="coverflow-floor" style={{ height: cover * 1.1, top: height * 0.1 }}>
        {albums.slice(Math.max(0, center - VISIBLE), center + VISIBLE + 1).map((album, localIndex) => {
          const i = Math.max(0, center - VISIBLE) + localIndex;
          const offset = i - center;
          return (
            <button
              key={album.key}
              className="coverflow-cover"
              tabIndex={i === center ? 0 : -1}
              aria-current={i === center ? "true" : undefined}
              onDoubleClick={() => void useApp.getState().playAlbum(album.key)}
              onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); e.stopPropagation(); void useApp.getState().playAlbum(album.key); } }}
              style={{ width: cover, height: cover, transform: placement(offset, cover), zIndex: 100 - Math.abs(offset) }}
              // Ctrl-click on a Mac is a context click that WebKit reports as a click too, which would reset the song selection.
              onClick={(e) => !isContextClick(e) && onCenter(i)}
              onContextMenu={(e) => onAlbumMenu(e, i)}
              aria-label={`${album.album} by ${album.albumArtist}`}
            >
              <Artwork trackId={album.artworkTrackId} size={cover} />
            </button>
          );
        })}
      </div>
      {current && (
        <div className="coverflow-caption" style={{ top: height * 0.1 + cover + 14 }} onContextMenu={(e) => onAlbumMenu(e, center)}>
          <div className="coverflow-caption-title">{current.album || "Unknown Album"}</div>
          <div className="coverflow-caption-artist">{current.albumArtist}</div>
        </div>
      )}
      {!current && <div className="coverflow-empty">No albums in this view</div>}
      <div ref={sliderRef} className="coverflow-slider" role="slider" aria-label="Album" aria-valuemin={1} aria-valuemax={Math.max(1, albums.length)} aria-valuenow={center + 1} aria-valuetext={current?.album || "No albums"} tabIndex={albums.length ? 0 : -1} onPointerDown={onSliderDown} onPointerMove={onSliderMove}
        onPointerUp={(e) => { if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId); }}>
        <div className="coverflow-slider-thumb" style={{ left: `${thumbLeft}%`, width: `${thumbWidth}%` }} />
      </div>
    </div>
  );
}

/** iTunes 10 Cover Flow: the album stage above, the full song list below, kept in step. */
export function CoverFlow() {
  const albums = useApp((s) => s.albums);
  const tracks = useApp((s) => s.tracks);
  const selection = useApp((s) => s.selection);
  const selectRow = useApp((s) => s.selectRow);
  const anchor = useApp((s) => s.selectionAnchor);
  const [centerKey, setCenterKey] = useState<string | null>(null);

  const albumIndex = useMemo(() => new Map(albums.map((a, i) => [summaryAlbumKey(a), i])), [albums]);

  const center = centerKey === null ? 0 : (albumIndex.get(centerKey) ?? 0);

  // Selecting a song elsewhere slides the stage to its album.
  useEffect(() => {
    const active = anchor === null ? undefined : tracks[anchor];
    const first = active && selection.has(active.id) ? active : tracks.find((t) => selection.has(t.id));
    const index = first ? albumIndex.get(trackAlbumKey(first)) : undefined;
    if (index !== undefined) setCenterKey(albums[index].key);
  }, [selection, tracks, anchor, albumIndex, albums]);

  const onCenter = (i: number) => {
    const album = albums[i];
    if (!album) return;
    const key = summaryAlbumKey(album);
    setCenterKey(key);
    const active = anchor === null ? undefined : tracks[anchor];
    if (active && selection.has(active.id) && trackAlbumKey(active) === key) return;
    const row = tracks.findIndex((t) => trackAlbumKey(t) === key);
    if (row >= 0) selectRow(row, { extend: false, toggle: false });
  };

  return (
    <div className="coverflow">
      <Stage albums={albums} center={Math.min(center, Math.max(0, albums.length - 1))} onCenter={onCenter} />
      <SongList />
    </div>
  );
}
