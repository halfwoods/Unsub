import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type MouseEvent, type RefObject } from "react";
import { artworkUrl, libraryTracks } from "../api/commands";
import { useApp, type GridGroup } from "../state/store";
import { pluralize } from "../util/format";
import { shownTracksOf } from "./albumKeys";
import { readViewScroll, saveViewScroll, scopeKey } from "./browseScope";
import { Artwork } from "./Artwork";
import { showAlbumMenu, showMenu } from "./songMenu";
import "./AlbumGrid.css";

const LABEL_HEIGHT = 44;
const GAP = 22;
const PADDING = 20;
const GROUPS: { id: GridGroup; label: string }[] = [
  { id: "albums", label: "Albums" }, { id: "artists", label: "Artists" },
  { id: "genres", label: "Genres" }, { id: "composers", label: "Composers" },
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

function ArtistArt({ ids, name, size }: { ids: number[]; name: string; size: number }) {
  const [failed, setFailed] = useState<Set<number>>(new Set());
  const covers = ids.filter((id) => !failed.has(id)).slice(0, 4);
  const initials = name.split(/\s+/).slice(0, 2).map((word) => word[0]).join("").toUpperCase() || "?";
  return <div className="album-tile-art artist-mosaic" style={{ width: size, height: size }} data-covers={covers.length}>
    {covers.length === 0 ? <span className="artist-initials">{initials}</span> : covers.map((id) => <img
      key={id} src={artworkUrl(id, { type: "library" }, size)} alt="" draggable={false} loading="lazy"
      onError={() => setFailed((before) => new Set([...before, id]))}
    />)}
  </div>;
}
interface Tile { key: string; title: string; subtitle: string; artwork: number | null; covers?: number[] }

export function AlbumGrid() {
  const albums = useApp((s) => s.albums);
  const artists = useApp((s) => s.artists);
  const genres = useApp((s) => s.genres);
  const composers = useApp((s) => s.composers);
  const group = useApp((s) => s.gridGroup);
  const source = useApp((s) => s.source);
  const query = useApp((s) => s.query);
  const loading = useApp((s) => s.loading);
  const loadError = useApp((s) => s.loadError);
  const size = useApp((s) => s.viewPrefs.gridSize);
  const { setGridGroup, setGridSize, openBrowse, playTrackIds, showNotice } = useApp.getState();
  const [selected, setSelected] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const width = useWidth(scrollRef);
  const viewKey = scopeKey(source, "grid", group, query);
  const items: Tile[] = useMemo(() => ({
    albums: () => albums.map((a) => ({ key: a.key, title: a.album || "Unknown Album", subtitle: `${a.year ? `${a.year} · ` : ""}${a.albumArtist || "Unknown Artist"}`, artwork: a.artworkTrackId })),
    artists: () => artists.map((a) => ({ key: a.key, title: a.name || "Unknown Artist", subtitle: `${pluralize(a.albumCount, "album", "albums")} · ${pluralize(a.trackCount, "song", "songs")}`, artwork: null, covers: a.artworkTrackIds })),
    genres: () => genres.map((g) => ({ key: g.name, title: g.name || "Unknown Genre", subtitle: `${pluralize(g.albumCount, "album", "albums")} · ${pluralize(g.trackCount, "song", "songs")}`, artwork: null })),
    composers: () => composers.map((c) => ({ key: c.name, title: c.name, subtitle: pluralize(c.trackCount, "song", "songs"), artwork: null })),
  })[group](), [albums, artists, genres, composers, group]);
  const columns = Math.max(1, Math.floor((width - PADDING * 2 + GAP) / (size + GAP)));
  const virtualizer = useVirtualizer({ count: Math.ceil(items.length / columns), getScrollElement: () => scrollRef.current, estimateSize: () => size + LABEL_HEIGHT + GAP, overscan: 3 });
  useEffect(() => virtualizer.measure(), [size, columns, virtualizer]);
  useLayoutEffect(() => {
    if (!loading && scrollRef.current) scrollRef.current.scrollTop = readViewScroll(viewKey);
    setSelected(null);
  }, [viewKey, loading]);

  const open = (item: Tile) => {
    if (loading) return;
    if (group === "albums") openBrowse(item.title, { albumGroup: item.key }, true);
    else if (group === "artists") openBrowse(item.title, { artistGroup: item.key, albumGroup: null });
    else if (group === "genres") openBrowse(item.title, { genre: item.key, artistGroup: null, albumGroup: null });
    else openBrowse(item.title, { composer: item.key, artistGroup: null, albumGroup: null });
  };
  const play = async (item: Tile, shuffled = false) => {
    if (loading) return;
    try {
      const filters = group === "albums" ? { albumGroup: item.key } : group === "artists" ? { artistGroup: item.key, albumGroup: null } : group === "genres" ? { genre: item.key, artistGroup: null, albumGroup: null } : { composer: item.key, artistGroup: null, albumGroup: null };
      const tracks = await libraryTracks({ ...query, ...filters, sort: group === "artists" ? "year" : "album", descending: false });
      if (useApp.getState().shuffle !== shuffled) useApp.getState().toggleShuffle();
      await playTrackIds(tracks.filter((t) => t.enabled && t.kind !== "radio").map((t) => t.id), shuffled);
    } catch (error) { showNotice(`Couldn't play this selection: ${String(error)}`); }
  };
  const onContextMenu = (e: MouseEvent, item: Tile) => {
    e.preventDefault(); setSelected(item.key);
    if (group === "albums") {
      const album = albums.find((a) => a.key === item.key);
      if (album) void showAlbumMenu(shownTracksOf(useApp.getState().tracks, album));
    } else void showMenu([
      { text: "Open Albums", action: () => open(item) },
      { text: "Play", action: () => void play(item) },
      { text: "Shuffle", action: () => void play(item, true) },
    ]);
  };
  const selectedItem = items.find((item) => item.key === selected);
  return <div className="album-grid">
    <div className="album-grid-bar">
      <div className="album-grid-groups" role="tablist" aria-label="Browse music by">
        {GROUPS.map((g) => <button key={g.id} role="tab" className="pill-tab" aria-selected={group === g.id} onClick={() => setGridGroup(g.id)}>{g.label}</button>)}
      </div>
      <input className="album-grid-size" type="range" min={100} max={260} value={size} aria-label="Artwork size" onChange={(e) => setGridSize(Number(e.target.value))} />
    </div>
    <div className="grid-selection-bar">
      <span title={selectedItem?.title}>{selectedItem?.title ?? (group === "artists" ? "Select an artist to browse or play" : "Select an item to browse or play")}</span>
      {query.artistGroup != null && group === "albums" && <span className="grid-chronology">Albums by year</span>}
      <button className="push-button" disabled={!selectedItem || loading} onClick={() => selectedItem && open(selectedItem)}>{group === "albums" ? "Show Songs" : "Open Albums"}</button>
      <button className="push-button" disabled={!selectedItem || loading} onClick={() => selectedItem && void play(selectedItem)}>Play</button>
      <button className="push-button" disabled={!selectedItem || loading} onClick={() => selectedItem && void play(selectedItem, true)}>Shuffle</button>
    </div>
    <div ref={scrollRef} className="album-grid-scroll" aria-label={`${group} grid`} aria-busy={loading}
      onScroll={(e) => { if (!loading) saveViewScroll(viewKey, e.currentTarget.scrollTop); }}>
      <div style={{ height: virtualizer.getTotalSize() + PADDING, position: "relative" }}>
        {virtualizer.getVirtualItems().map((row) => <div key={row.index} className="album-grid-row" style={{ transform: `translateY(${row.start + PADDING}px)`, gap: GAP, paddingLeft: PADDING }}>
          {items.slice(row.index * columns, row.index * columns + columns).map((item, col) => <button key={item.key}
            data-tile-index={row.index * columns + col} className={selected === item.key ? "album-tile album-tile--selected" : "album-tile"}
            aria-label={`${item.title}, ${item.subtitle}`} style={{ width: Math.min(size, Math.max(100, width - PADDING * 2)) }} title={`${item.title}\n${item.subtitle}`}
            onClick={() => setSelected(item.key)} onContextMenu={(e) => onContextMenu(e, item)} onDoubleClick={() => open(item)}
            onKeyDown={(e) => {
              if (e.key === "Enter") { e.preventDefault(); open(item); return; }
              const delta = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : e.key === "ArrowDown" ? columns : e.key === "ArrowUp" ? -columns : 0;
              if (!delta) return;
              e.preventDefault();
              const next = Math.max(0, Math.min(items.length - 1, row.index * columns + col + delta));
              setSelected(items[next].key); virtualizer.scrollToIndex(Math.floor(next / columns), { align: "auto" });
              requestAnimationFrame(() => scrollRef.current?.querySelector<HTMLElement>(`[data-tile-index="${next}"]`)?.focus());
            }}>
            {item.artwork !== null ? <Artwork key={item.artwork} trackId={item.artwork} size={size} className="album-tile-art" /> : <ArtistArt key={`${item.key}-${(item.covers ?? []).join()}`} ids={item.covers ?? []} name={item.title} size={size} />}
            <span className="album-tile-title">{item.title}</span><span className="album-tile-subtitle">{item.subtitle}</span>
          </button>)}
        </div>)}
      </div>
      {items.length === 0 && <p className="grid-empty" role="status">{loading ? "Loading…" : loadError ? "Couldn't load this view." : "No items match this view."}</p>}
    </div>
  </div>;
}
