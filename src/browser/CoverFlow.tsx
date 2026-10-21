import { useEffect, useMemo, useRef, useState, type PointerEvent, type WheelEvent } from "react";
import type { AlbumSummary } from "../api/types";
import { useApp } from "../state/store";
import { summaryAlbumKey, trackAlbumKey } from "./albumKeys";
import { Artwork } from "./Artwork";
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
  const clamp = (i: number) => Math.min(albums.length - 1, Math.max(0, i));

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
      onWheel={onWheel}
      onKeyDown={(e) => {
        if (e.key === "ArrowLeft") onCenter(clamp(center - 1));
        if (e.key === "ArrowRight") onCenter(clamp(center + 1));
      }}
    >
      <div className="coverflow-floor" style={{ height: cover * 1.1, top: height * 0.1 }}>
        {albums.map((album, i) => {
          const offset = i - center;
          if (Math.abs(offset) > VISIBLE) return null;
          return (
            <button
              key={album.key}
              className="coverflow-cover"
              style={{ width: cover, height: cover, transform: placement(offset, cover), zIndex: 100 - Math.abs(offset) }}
              onClick={() => onCenter(i)}
              aria-label={`${album.album} by ${album.albumArtist}`}
            >
              <Artwork trackId={album.artworkTrackId} size={cover} />
            </button>
          );
        })}
      </div>
      {current && (
        <div className="coverflow-caption" style={{ top: height * 0.1 + cover + 14 }}>
          <div className="coverflow-caption-title">{current.album || "Unknown Album"}</div>
          <div className="coverflow-caption-artist">{current.albumArtist}</div>
        </div>
      )}
      <div ref={sliderRef} className="coverflow-slider" onPointerDown={onSliderDown} onPointerMove={onSliderMove}>
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
  const [center, setCenter] = useState(0);

  const albumIndex = useMemo(() => new Map(albums.map((a, i) => [summaryAlbumKey(a), i])), [albums]);

  // Selecting a song elsewhere slides the stage to its album.
  useEffect(() => {
    const first = tracks.find((t) => selection.has(t.id));
    const index = first ? albumIndex.get(trackAlbumKey(first)) : undefined;
    if (index !== undefined) setCenter(index);
  }, [selection, tracks, albumIndex]);

  const onCenter = (i: number) => {
    setCenter(i);
    const album = albums[i];
    if (!album) return;
    const key = summaryAlbumKey(album);
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
