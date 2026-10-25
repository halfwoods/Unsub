import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type MouseEvent, type RefObject } from "react";
import { useApp, type GridGroup } from "../state/store";
import { pluralize } from "../util/format";
import { shownTracksOf } from "./albumKeys";
import { Artwork } from "./Artwork";
import { ArtistArtwork } from "./ArtistArtwork";
import { artistInitials, browseScrollKey, nextGridIndex } from "./browseHelpers";
import { showAlbumMenu } from "./songMenu";
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
  const source = useApp((s) => s.source);
  const query = useApp((s) => s.query);
  const path = useApp((s) => s.browsePath);
  const { openArtist, openAlbum, openGroup, playBrowse, showNotice } = useApp.getState();
  const hasSongs = useApp((s) => s.tracks.some((t) => t.enabled && t.kind !== "radio"));
  const [size, setSize] = useState(160);
  const [selected, setSelected] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const width = useWidth(scrollRef);
  const tileSize = width > 0 ? Math.min(size, Math.max(72, width - PADDING * 2)) : size;

  const items = useMemo<{ key: string; title: string; subtitle: string; artwork: number | null; mosaic?: number[] }[]>(() => ({
    albums: () => albums.map((a) => ({ key: a.key, title: a.album || "Unknown Album", subtitle: [query.artistKey != null && a.year ? String(a.year) : "", a.albumArtist].filter(Boolean).join(" · "), artwork: a.artworkTrackId })),
    artists: () => artists.map((a) => ({ key: a.key, title: a.name || "Unknown Artist", subtitle: `${pluralize(a.albumCount, "album", "albums")} · ${pluralize(a.trackCount, "song", "songs")}`, artwork: null, mosaic: a.artworkTrackIds })),
    genres: () => genres.map((g) => ({ key: g.key, title: g.name, subtitle: pluralize(g.albumCount, "album", "albums"), artwork: null })),
    composers: () => composers.map((c) => ({ key: c.name, title: c.name, subtitle: pluralize(c.trackCount, "song", "songs"), artwork: null })),
  }[group]()), [albums, artists, genres, composers, group, query.artistKey]);
  const scrollKey = browseScrollKey(source, group, query);
  const restore = useRef<{ key: string; offset: number } | null>(null);
  const restoredKey = useRef<string | null>(null);
  useLayoutEffect(() => {
    if (restoredKey.current !== scrollKey) {
      restoredKey.current = scrollKey;
      restore.current = { key: scrollKey, offset: useApp.getState().browseScroll[scrollKey] ?? 0 };
      setSelected(null);
    }
    const target = restore.current;
    const frame = requestAnimationFrame(() => {
      if (target?.key === scrollKey && scrollRef.current) scrollRef.current.scrollTop = target.offset;
    });
    return () => cancelAnimationFrame(frame);
  }, [scrollKey, items, width]);
  const remember = () => useApp.getState().setBrowseScroll(scrollKey, scrollRef.current?.scrollTop ?? 0);
  const userScrolling = () => { restore.current = null; };

  const columns = Math.max(1, Math.floor((width - PADDING * 2 + GAP) / (tileSize + GAP)));
  const rowCount = Math.ceil(items.length / columns);
  const virtualizer = useVirtualizer({
    count: rowCount,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => tileSize + LABEL_HEIGHT + GAP,
    overscan: 3,
  });
  useEffect(() => virtualizer.measure(), [tileSize, columns, virtualizer]);

  // iTunes: double-clicking an artist, genre or composer narrows the grid to its albums.
  const open = (key: string, title: string) => {
    remember();
    if (group === "albums") openAlbum(key, title);
    else if (group === "artists") openArtist(key, title);
    else openGroup(group === "genres" ? "genre" : "composer", key, title);
  };
  const onTileKey = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    if (event.key === "Enter") {
      event.preventDefault(); open(items[index].key, items[index].title);
    } else if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) {
      event.preventDefault(); userScrolling();
      const next = nextGridIndex(index, event.key, columns, items.length);
      setSelected(items[next].key);
      virtualizer.scrollToIndex(Math.floor(next / columns), { align: "auto" });
      requestAnimationFrame(() => requestAnimationFrame(() => scrollRef.current?.querySelector<HTMLButtonElement>(`[data-tile-index="${next}"]`)?.focus({ preventScroll: true })));
    }
  };
  const selectedIndex = items.findIndex((item) => item.key === selected);
  const visibleRows = virtualizer.getVirtualItems();
  const selectionMounted = visibleRows.some((row) => selectedIndex >= row.index * columns && selectedIndex < (row.index + 1) * columns);
  const tabStop = selectionMounted ? selectedIndex : (visibleRows[0]?.index ?? 0) * columns;

  // Only albums have songs to play, tag or delete; an artist, genre or composer is a way to find them.
  const onContextMenu = (e: MouseEvent, key: string) => {
    if (group !== "albums") return;
    e.preventDefault();
    setSelected(key);
    const album = albums.find((a) => a.key === key);
    if (album) void showAlbumMenu(shownTracksOf(useApp.getState().tracks, album));
  };

  return (
    <div className="album-grid">
      <div className="album-grid-bar">
        <div className="album-grid-groups" role="tablist" aria-label="Group library by">
          {GROUPS.map((g) => (
            <button
              key={g.id}
              role="tab"
              className="pill-tab"
              aria-selected={group === g.id}
              tabIndex={group === g.id ? 0 : -1}
              onClick={() => { remember(); setGridGroup(g.id); }}
              onKeyDown={(event) => {
                if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
                event.preventDefault();
                const index = GROUPS.findIndex((item) => item.id === g.id);
                const next = (index + (event.key === "ArrowRight" ? 1 : GROUPS.length - 1)) % GROUPS.length;
                remember(); setGridGroup(GROUPS[next].id);
                event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>("button")[next]?.focus();
              }}
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
      <div className="album-grid-actions">
        <span>{items.length.toLocaleString()} {group}{query.artistKey != null && group === "albums" ? " · Oldest first" : ""}</span>
        <button className="push-button" disabled={selected === null} onClick={() => { const item = items.find((entry) => entry.key === selected); if (item) open(item.key, item.title); }}>Open</button>
        {path.length === 0 && <>
          <button className="push-button" disabled={!hasSongs} onClick={() => void playBrowse(false).catch((error: unknown) => showNotice(String(error)))}>Play</button>
          <button className="push-button" disabled={!hasSongs} onClick={() => void playBrowse(true).catch((error: unknown) => showNotice(String(error)))}>Shuffle</button>
        </>}
      </div>
      <div ref={scrollRef} className="album-grid-scroll" aria-label={`${group} grid`}
        onScroll={() => { if (!restore.current) remember(); }} onWheel={userScrolling} onPointerDown={userScrolling}>
        {items.length === 0 && <div className="album-grid-empty" role="status">No {group} match this view. Clear the search or filters to see more.</div>}
        <div style={{ height: virtualizer.getTotalSize() + PADDING, position: "relative" }}>
          {virtualizer.getVirtualItems().map((row) => (
            <div
              key={row.index}
              className="album-grid-row"
              style={{ transform: `translateY(${row.start + PADDING}px)`, paddingLeft: PADDING }}
            >
              {items.slice(row.index * columns, row.index * columns + columns).map((item, column) => (
                <button
                  key={item.key}
                  className={selected === item.key ? "album-tile album-tile--selected" : "album-tile"}
                  style={{ width: tileSize, marginRight: column < columns - 1 ? GAP : 0, flexShrink: 0 }}
                  data-tile-index={row.index * columns + column}
                  tabIndex={tabStop === row.index * columns + column ? 0 : -1}
                  aria-label={`Open ${item.title}, ${item.subtitle}`}
                  title={`${item.title}\n${item.subtitle}`}
                  onFocus={() => setSelected(item.key)}
                  onKeyDown={(event) => onTileKey(event, row.index * columns + column)}
                  onClick={() => setSelected(item.key)}
                  onContextMenu={(e) => onContextMenu(e, item.key)}
                  onDoubleClick={() => open(item.key, item.title)}
                >
                  {item.mosaic ? (
                    <ArtistArtwork key={`${item.key}:${item.mosaic.join(",")}`} name={item.title} trackIds={item.mosaic} size={tileSize} />
                  ) : item.artwork !== null ? (
                    <Artwork key={item.artwork} trackId={item.artwork} size={tileSize} className="album-tile-art" />
                  ) : (
                    <div className="album-tile-art album-tile-art--artist" style={{ width: tileSize, height: tileSize }}>
                      {artistInitials(item.title)}
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
