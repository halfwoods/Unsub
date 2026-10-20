import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useRef, useState, type CSSProperties, type KeyboardEvent } from "react";
import {
  onLibraryChanged,
  radioAdd,
  radioGenres,
  radioMyStations,
  radioPlay,
  radioPlayDirectory,
  radioRecent,
  radioRemove,
  radioSearch,
  radioStations,
} from "../api/commands";
import type { DirectoryStation, RadioGenre, RadioStation } from "../api/types";
import { STATUS_COLUMN_WIDTH } from "../browser/columns";
import { NowPlayingIcon } from "../browser/SongList";
import { useApp } from "../state/store";
import { pluralize } from "../util/format";
import "../browser/SongList.css";
import "./RadioView.css";

/** Stations a genre shows at a time, and each "More…" adds. */
const PAGE = 50;
const RECENT = 10;
const SEARCH_LIMIT = 100;
/** Searches go to a community-run server, so they wait for a pause in typing. */
const SEARCH_DELAY_MS = 300;
const COMMENT_TAGS = 5;
const GRID = `${STATUS_COLUMN_WIDTH}px minmax(120px, 260fr) 76px minmax(120px, 300fr) 26px`;

interface Row {
  /** The library's id, once the station is kept. */
  id: number | null;
  /** The directory's id, for stations that came from it. */
  uuid: string | null;
  name: string;
  bitRate: string;
  comment: string;
  play: () => void;
  action: { label: string; symbol: string; run: () => void };
}

type Entry =
  | { type: "group"; key: string; label: string; open: boolean }
  | { type: "station"; key: string; row: Row; indent: boolean }
  | { type: "more"; key: string; genre: string }
  | { type: "message"; key: string; text: string };

interface GenreList {
  stations: DirectoryStation[];
  more: boolean;
  loading: boolean;
  error: string | null;
}

const bitRate = (kbps: number | null) => (kbps ? `${kbps} kbps` : "");

/** iTunes 10's Radio: your stations, what you heard lately, then the directory by genre. Text only; station icons and homepages are never loaded. */
export function RadioView() {
  const search = useApp((s) => s.query.search);
  const rowHeight = useApp((s) => s.rowHeight);
  const showNotice = useApp((s) => s.showNotice);
  const liveId = useApp((s) => (s.playback.live ? s.playback.trackId : null));
  const liveUuid = useApp((s) => (s.playback.live ? (s.liveStation?.radioBrowserUuid ?? null) : null));
  const [mine, setMine] = useState<RadioStation[]>([]);
  const [recent, setRecent] = useState<RadioStation[]>([]);
  const [genres, setGenres] = useState<RadioGenre[] | null>(null);
  const [genresError, setGenresError] = useState<string | null>(null);
  const [genreLists, setGenreLists] = useState<Record<string, GenreList>>({});
  const [results, setResults] = useState<DirectoryStation[] | null>(null);
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set(["mine", "recent"]));
  const [selected, setSelected] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const load = () =>
      void Promise.all([radioMyStations(), radioRecent(RECENT)]).then(
        ([saved, played]) => {
          setMine(saved);
          setRecent(played);
        },
        (e: unknown) => showNotice(String(e)),
      );
    load();
    const unlisten = onLibraryChanged(load);
    return () => void unlisten.then((stop) => stop());
  }, [showNotice]);

  useEffect(() => {
    void radioGenres().then(setGenres, (e: unknown) => setGenresError(String(e)));
  }, []);

  useEffect(() => {
    if (search === null) return;
    setResults(null);
    let stale = false;
    const timer = setTimeout(() => {
      radioSearch(search, SEARCH_LIMIT).then(
        (found) => !stale && setResults(found),
        (e: unknown) => {
          if (stale) return;
          setResults([]);
          showNotice(String(e));
        },
      );
    }, SEARCH_DELAY_MS);
    return () => {
      stale = true;
      clearTimeout(timer);
    };
  }, [search, showNotice]);

  const loadGenre = (name: string) => {
    const list = genreLists[name];
    if (list?.loading) return;
    const known = list?.stations ?? [];
    setGenreLists((lists) => ({ ...lists, [name]: { stations: known, more: false, loading: true, error: null } }));
    radioStations(name, known.length, PAGE).then(
      (page) => {
        const seen = new Set(known.map((s) => s.uuid));
        const stations = [...known, ...page.filter((s) => !seen.has(s.uuid))];
        setGenreLists((lists) => ({ ...lists, [name]: { stations, more: page.length === PAGE, loading: false, error: null } }));
      },
      (e: unknown) => setGenreLists((lists) => ({ ...lists, [name]: { stations: known, more: false, loading: false, error: String(e) } })),
    );
  };

  const toggle = (key: string) => {
    const opening = !expanded.has(key);
    setExpanded((current) => {
      const next = new Set(current);
      if (opening) next.add(key);
      else next.delete(key);
      return next;
    });
    const genre = key.startsWith("genre:") ? key.slice("genre:".length) : null;
    if (opening && genre !== null && (!genreLists[genre] || genreLists[genre].error)) loadGenre(genre);
  };

  const fail = (error: unknown) => showNotice(String(error));

  const savedRow = (station: RadioStation): Row => ({
    id: station.id,
    uuid: station.radioBrowserUuid,
    name: station.name,
    bitRate: bitRate(station.bitrateKbps),
    comment: [station.genre, station.codec].filter(Boolean).join(" · "),
    play: () => void radioPlay(station.id).catch(fail),
    action: { label: "Remove from My Stations", symbol: "−", run: () => void radioRemove(station.id).catch(fail) },
  });

  const directoryRow = (station: DirectoryStation): Row => ({
    id: null,
    uuid: station.uuid,
    name: station.name,
    bitRate: bitRate(station.bitrateKbps),
    comment: [station.tags.slice(0, COMMENT_TAGS).join(", "), station.country].filter(Boolean).join(" · "),
    play: () => void radioPlayDirectory(station).catch(fail),
    action: {
      label: "Add to My Stations",
      symbol: "+",
      run: () => void radioAdd(station).then(() => showNotice(`Added “${station.name}” to My Stations`), fail),
    },
  });

  const entries: Entry[] = [];
  const stations = (group: string, rows: Row[], indent: boolean) =>
    rows.forEach((row) => entries.push({ type: "station", key: `${group}:${row.id ?? row.uuid}`, row, indent }));
  if (search !== null) {
    if (results === null) entries.push({ type: "message", key: "searching", text: "Searching…" });
    else if (results.length === 0) entries.push({ type: "message", key: "none", text: "No streams found." });
    else stations("search", results.map(directoryRow), false);
  } else {
    entries.push({ type: "group", key: "mine", label: "My Stations", open: expanded.has("mine") });
    if (expanded.has("mine")) stations("mine", mine.map(savedRow), true);
    entries.push({ type: "group", key: "recent", label: "Recently Played", open: expanded.has("recent") });
    if (expanded.has("recent")) stations("recent", recent.map(savedRow), true);
    if (genres === null) {
      const text = genresError ? `Couldn’t load the station directory: ${genresError}` : "Loading the station directory…";
      entries.push({ type: "message", key: "genres", text });
    }
    for (const genre of genres ?? []) {
      const key = `genre:${genre.name}`;
      const open = expanded.has(key);
      entries.push({ type: "group", key, label: `${genre.name} (${pluralize(genre.count, "stream", "streams")})`, open });
      if (!open) continue;
      const list = genreLists[genre.name];
      if (list) stations(key, list.stations.map(directoryRow), true);
      if (list?.error) entries.push({ type: "message", key: `${key}:error`, text: `Couldn’t load streams: ${list.error}` });
      else if (!list || list.loading) entries.push({ type: "message", key: `${key}:loading`, text: "Loading…" });
      else if (list.more) entries.push({ type: "more", key: `${key}:more`, genre: genre.name });
      else if (list.stations.length === 0) entries.push({ type: "message", key: `${key}:none`, text: "No streams found." });
    }
  }

  const virtualizer = useVirtualizer({
    count: entries.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => rowHeight,
    overscan: 24,
  });
  useEffect(() => virtualizer.measure(), [rowHeight, virtualizer]);

  const activate = (entry: Entry) => {
    if (entry.type === "group") toggle(entry.key);
    else if (entry.type === "station") entry.row.play();
    else if (entry.type === "more") loadGenre(entry.genre);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const rows = entries.filter((entry) => entry.type !== "message");
    const at = rows.findIndex((entry) => entry.key === selected);
    const current = rows[at];
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const next = rows[Math.min(rows.length - 1, Math.max(0, at + (e.key === "ArrowDown" ? 1 : -1)))];
      if (!next) return;
      setSelected(next.key);
      virtualizer.scrollToIndex(entries.indexOf(next), { align: "auto" });
    } else if (e.key === "Enter" && current) {
      activate(current);
    } else if ((e.key === "Backspace" || e.key === "Delete") && current?.type === "station" && current.row.id !== null) {
      e.preventDefault();
      current.row.action.run();
    }
  };

  const isLive = (row: Row) => (liveId !== null && row.id === liveId) || (liveUuid !== null && row.uuid === liveUuid);

  return (
    <div className="song-list" style={{ "--grid": GRID } as CSSProperties}>
      <div className="song-list-header" role="row">
        <div className="song-list-header-cell" />
        <div role="columnheader" className="song-list-header-cell">
          <span className="song-list-header-title">Stream</span>
        </div>
        <div role="columnheader" className="song-list-header-cell" style={{ justifyContent: "flex-end" }}>
          <span className="song-list-header-title">Bit Rate</span>
        </div>
        <div role="columnheader" className="song-list-header-cell">
          <span className="song-list-header-title">Comment</span>
        </div>
        <div className="song-list-header-cell" />
      </div>
      <div ref={scrollRef} className="song-list-body" tabIndex={0} onKeyDown={onKeyDown} role="grid" aria-rowcount={entries.length}>
        <div className="song-list-stripes" />
        <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
          {virtualizer.getVirtualItems().map((item) => {
            const entry = entries[item.index];
            const classes = ["song-row", item.index % 2 ? "song-row--odd" : "", selected === entry.key ? "song-row--selected" : ""];
            const style = { transform: `translateY(${item.start}px)` };
            switch (entry.type) {
              case "message":
                return (
                  <div key={entry.key} role="row" style={style} className={classes.join(" ")}>
                    <div className="song-cell" />
                    <div className="song-cell radio-span radio-message">{entry.text}</div>
                  </div>
                );
              case "group":
                return (
                  <div
                    key={entry.key}
                    role="row"
                    style={style}
                    className={[...classes, "radio-group"].join(" ")}
                    aria-expanded={entry.open}
                    onMouseDown={() => setSelected(entry.key)}
                    onDoubleClick={() => toggle(entry.key)}
                  >
                    <div className="song-cell song-cell--status" onClick={() => toggle(entry.key)}>
                      <span className={entry.open ? "radio-disclosure radio-disclosure--open" : "radio-disclosure"} />
                    </div>
                    <div className="song-cell radio-span">{entry.label}</div>
                  </div>
                );
              case "more":
                return (
                  <div key={entry.key} role="row" style={style} className={classes.join(" ")} onMouseDown={() => setSelected(entry.key)} onClick={() => loadGenre(entry.genre)}>
                    <div className="song-cell" />
                    <div className="song-cell radio-span radio-indent radio-more">More…</div>
                  </div>
                );
              case "station": {
                const { row } = entry;
                return (
                  <div key={entry.key} role="row" style={style} className={classes.join(" ")} aria-selected={selected === entry.key} onMouseDown={() => setSelected(entry.key)} onDoubleClick={row.play}>
                    <div className="song-cell song-cell--status">{isLive(row) && <NowPlayingIcon playing />}</div>
                    <div className={entry.indent ? "song-cell radio-indent" : "song-cell"}>{row.name}</div>
                    <div className="song-cell" style={{ textAlign: "right" }}>
                      {row.bitRate}
                    </div>
                    <div className="song-cell">{row.comment}</div>
                    <div className="song-cell song-cell--status">
                      <button
                        className="radio-action"
                        aria-label={row.action.label}
                        title={row.action.label}
                        onMouseDown={(e) => e.stopPropagation()}
                        onDoubleClick={(e) => e.stopPropagation()}
                        onClick={row.action.run}
                      >
                        {row.action.symbol}
                      </button>
                    </div>
                  </div>
                );
              }
            }
          })}
        </div>
      </div>
    </div>
  );
}
