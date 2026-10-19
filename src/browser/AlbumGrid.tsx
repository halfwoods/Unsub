import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useRef, useState, type RefObject } from "react";
import { useApp, type GridGroup } from "../state/store";
import { pluralize } from "../util/format";
import { Artwork } from "./Artwork";
import "./AlbumGrid.css";

const LABEL_HEIGHT = 40;
const GAP = 22;
const PADDING = 20;

const GROUPS: { id: GridGroup; label: string }[] = [
  { id: "albums", label: "Albums" },
  { id: "artists", label: "Artists" },
  { id: "genres", label: "Genres" },
  { id: "composers", label: "Composers" },
];

function useWidth(ref: RefObject<HTMLElement | null>) {
  const [width, setWidth] = useState(0);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const observer = new ResizeObserver(([entry]) => setWidth(entry.contentRect.width));
    observer.observe(el);
    return () => observer.disconnect();
  }, [ref]);
  return width;
}

export function AlbumGrid() {
  const albums = useApp((s) => s.albums);
  const artists = useApp((s) => s.artists);
  const genres = useApp((s) => s.genres);
  const composers = useApp((s) => s.composers);
  const group = useApp((s) => s.gridGroup);
  const setGridGroup = useApp((s) => s.setGridGroup);
  const setSearch = useApp((s) => s.setSearch);
  const playAlbum = useApp((s) => s.playAlbum);
  const [size, setSize] = useState(160);
  const [selected, setSelected] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const width = useWidth(scrollRef);

  const items: { key: string; title: string; subtitle: string; artwork: number | null }[] = {
    albums: () => albums.map((a) => ({ key: a.key, title: a.album || "Unknown Album", subtitle: a.albumArtist, artwork: a.artworkTrackId })),
    artists: () => artists.map((a) => ({ key: a.key, title: a.name || "Unknown Artist", subtitle: pluralize(a.albumCount, "album", "albums"), artwork: null })),
    genres: () => genres.map((g) => ({ key: g.name, title: g.name, subtitle: pluralize(g.albumCount, "album", "albums"), artwork: null })),
    composers: () => composers.map((c) => ({ key: c.name, title: c.name, subtitle: pluralize(c.trackCount, "song", "songs"), artwork: null })),
  }[group]();

  const columns = Math.max(1, Math.floor((width - PADDING * 2 + GAP) / (size + GAP)));
  const rowCount = Math.ceil(items.length / columns);
  const virtualizer = useVirtualizer({
    count: rowCount,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => size + LABEL_HEIGHT + GAP,
    overscan: 3,
  });
  useEffect(() => virtualizer.measure(), [size, columns, virtualizer]);

  // iTunes: double-clicking an artist, genre or composer narrows the grid to its albums.
  const open = (key: string, title: string) => {
    if (group === "albums") void playAlbum(key);
    else {
      setGridGroup("albums");
      setSearch(title);
    }
  };

  return (
    <div className="album-grid">
      <div className="album-grid-bar">
        <div className="album-grid-groups" role="tablist">
          {GROUPS.map((g) => (
            <button
              key={g.id}
              role="tab"
              className="pill-tab"
              aria-selected={group === g.id}
              onClick={() => setGridGroup(g.id)}
            >
              {g.label}
            </button>
          ))}
        </div>
        <input
          className="album-grid-size"
          type="range"
          min={100}
          max={260}
          value={size}
          aria-label="Artwork size"
          onChange={(e) => setSize(Number(e.target.value))}
        />
      </div>
      <div ref={scrollRef} className="album-grid-scroll">
        <div style={{ height: virtualizer.getTotalSize() + PADDING, position: "relative" }}>
          {virtualizer.getVirtualItems().map((row) => (
            <div
              key={row.index}
              className="album-grid-row"
              style={{ transform: `translateY(${row.start + PADDING}px)`, gap: GAP, paddingLeft: PADDING }}
            >
              {items.slice(row.index * columns, row.index * columns + columns).map((item) => (
                <button
                  key={item.key}
                  className={selected === item.key ? "album-tile album-tile--selected" : "album-tile"}
                  style={{ width: size }}
                  onClick={() => setSelected(item.key)}
                  onDoubleClick={() => open(item.key, item.title)}
                >
                  {item.artwork !== null ? (
                    <Artwork trackId={item.artwork} size={size} className="album-tile-art" />
                  ) : (
                    <div className="album-tile-art album-tile-art--artist" style={{ width: size, height: size }}>
                      {item.title.slice(0, 1).toUpperCase()}
                    </div>
                  )}
                  <span className="album-tile-title">{item.title}</span>
                  <span className="album-tile-subtitle">{item.subtitle}</span>
                </button>
              ))}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
