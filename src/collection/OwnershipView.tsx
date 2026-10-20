import { openUrl } from "@tauri-apps/plugin-opener";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  collectionDownloadFromIcloud,
  collectionImportOwned,
  collectionLoadMusicApp,
  collectionLocateFolder,
  collectionLoadXml,
  collectionReport,
  onCollectionChanged,
  onLibraryChanged,
  purchasesLinks,
} from "../api/commands";
import type { AlbumGroup, OwnershipReport, OwnStatus, ReportTrack, StoreLink } from "../api/types";
import { formatClock, pluralize } from "../util/format";
import { IS_MAC } from "../util/platform";
import "./OwnershipView.css";

type Filter = OwnStatus | "all";

const FILTERS: { id: Filter; label: string }[] = [
  { id: "buy", label: "To Buy" },
  { id: "import", label: "To Import" },
  { id: "locate", label: "Missing" },
  { id: "redownload", label: "Re-download" },
  { id: "inLibrary", label: "In Library" },
  { id: "all", label: "All" },
];

const STATUS_LABEL: Record<OwnStatus, string> = {
  inLibrary: "In Library",
  import: "Owned file",
  locate: "File missing",
  redownload: "Owned in iCloud",
  buy: "Streaming only",
  unknown: "Unknown",
};

/** Stores that sell DRM-free downloads you keep. */
const STORES = [
  { name: "Bandcamp", url: (q: string) => `https://bandcamp.com/search?q=${q}&item_type=a` },
  { name: "Qobuz", url: (q: string) => `https://www.qobuz.com/us-en/search?q=${q}` },
  { name: "7digital", url: (q: string) => `https://us.7digital.com/search?q=${q}` },
  { name: "iTunes", url: (q: string) => `https://music.apple.com/search?term=${q}` },
];

const HEADER_HEIGHT = 30;

type Row = { type: "album"; album: AlbumGroup; count: number } | { type: "track"; track: ReportTrack; album: AlbumGroup };

function Reading() {
  return (
    <div className="placard">
      <div className="placard-card ownership-intro">
        <h1>Reading Your Music Library…</h1>
        <div className="ownership-busy" />
        <p>Big libraries take a little while. Nothing in your Music library is changed.</p>
      </div>
    </div>
  );
}

/** Missing files are yours: say where they were and let the user point us at them. */
function MissingFiles({ report }: { report: OwnershipReport }) {
  if (report.totals.locate === 0) return null;
  const home = report.missingRoots.find((r) => r.root.startsWith("/Users/"))?.root.split("/").slice(0, 3).join("/");
  const label = (root: string) =>
    root.startsWith("/Volumes/") ? `the drive “${root.slice("/Volumes/".length)}”` : home && root.startsWith(home) ? `~${root.slice(home.length)}` : root;
  return (
    <div className="ownership-missing">
      <p>
        <strong>{pluralize(report.totals.locate, "song you own has", "songs you own have")} no file on this Mac.</strong> Your
        Music library remembers where they were:
      </p>
      <ul>
        {report.missingRoots.slice(0, 4).map((r) => (
          <li key={r.root}>
            {r.count.toLocaleString()} in {label(r.root)}
            {r.available ? " (available now: Import picks them up)" : r.root.startsWith("/Volumes/") ? " (connect it, then Refresh)" : ""}
          </li>
        ))}
      </ul>
      <button className="push-button" onClick={() => void collectionLocateFolder()}>
        Locate Folder…
      </button>
    </div>
  );
}

/** Purchases and uploads are yours in iCloud; Music can fetch them so they can be imported. */
function Redownload({ report }: { report: OwnershipReport }) {
  const [status, setStatus] = useState<string | null>(null);
  if (report.totals.redownload === 0 || !IS_MAC) return null;
  const start = async () => {
    setStatus("Asking Music to download…");
    try {
      const { requested, accepted } = await collectionDownloadFromIcloud();
      setStatus(
        accepted > 0
          ? `Music is downloading ${pluralize(accepted, "song", "songs")}${accepted < requested ? ` (${(requested - accepted).toLocaleString()} weren't available in iCloud)` : ""}. When it's done, click Refresh, then Import.`
          : "Music couldn't find these songs in iCloud. If Sync Library is off, turn it on in Music › Settings › General.",
      );
    } catch (e) {
      setStatus(String(e));
    }
  };
  return (
    <div className="ownership-missing">
      <p>
        <strong>{pluralize(report.totals.redownload, "song is", "songs are")} yours in iCloud</strong> (purchases and uploaded or
        matched songs) but not on this Mac.
      </p>
      <button className="push-button" onClick={() => void start()}>
        Download from iCloud
      </button>
      {status && <p className="ownership-status-note">{status}</p>}
    </div>
  );
}

function Intro({ error, onLoadXml, onLoadMusic }: { error: string | null; onLoadXml: () => void; onLoadMusic: () => void }) {
  return (
    <div className="placard">
      <div className="placard-card ownership-intro">
        <h1>Own Your Streaming</h1>
        <p>
          Compare your Apple Music library with this one. Unsub sorts every song into what's already here, files
          you own that it can import, purchases you can re-download, and what you'd need to buy.
        </p>
        <div className="ownership-intro-actions">
          {IS_MAC && (
            <button className="push-button" onClick={onLoadMusic}>
              Compare with Music Library
            </button>
          )}
          <button className="push-button" onClick={onLoadXml}>
            Choose Library XML…
          </button>
        </div>
        <p className="ownership-note">
          {IS_MAC
            ? "macOS will ask before Unsub can read your Music library. Nothing in it is changed."
            : "In iTunes, choose File › Library › Export Library…, then pick the file here."}
        </p>
        {error && <p className="ownership-error">{error}</p>}
      </div>
    </div>
  );
}

function Progress({ report }: { report: OwnershipReport }) {
  const { songs, inLibrary, import: toImport, redownload, buy, unknown } = report.totals;
  const { locate } = report.totals;
  const owned = inLibrary + toImport + locate + redownload;
  const parts = [
    { key: "in-library", value: inLibrary, label: "In Unsub" },
    { key: "owned", value: toImport + locate + redownload, label: "Owned elsewhere" },
    { key: "buy", value: buy, label: "Streaming only" },
    { key: "unknown", value: unknown, label: "Unknown" },
  ];
  return (
    <div className="ownership-progress">
      <div className="ownership-progress-title">
        <strong>{owned.toLocaleString()}</strong> of {pluralize(songs, "song", "songs")} owned
      </div>
      <div className="ownership-bar">
        {parts
          .filter((p) => p.value > 0)
          .map((p) => (
            <div key={p.key} className={`ownership-bar-part ownership-${p.key}`} style={{ flexGrow: p.value }} />
          ))}
      </div>
      <div className="ownership-legend">
        {parts.map((p) => (
          <span key={p.key}>
            <span className={`ownership-swatch ownership-${p.key}`} />
            {p.label} {p.value.toLocaleString()}
          </span>
        ))}
      </div>
    </div>
  );
}

export function OwnershipView() {
  const [report, setReport] = useState<OwnershipReport | null | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState<Filter>("buy");
  const [reading, setReading] = useState(false);
  // Exact store links from MusicBrainz, fetched per album on request (it allows one lookup a second).
  const [links, setLinks] = useState<Record<string, StoreLink[] | "loading">>({});
  const scrollRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const refresh = () => void collectionReport().then(setReport, (e) => setError(String(e)));
    refresh();
    const subscriptions = [
      onCollectionChanged((message) => {
        setReading(false);
        setError(message);
        refresh();
      }),
      // Imports and purchases arriving change what you own, so re-check.
      onLibraryChanged(refresh),
    ];
    return () => subscriptions.forEach((s) => void s.then((unlisten) => unlisten()));
  }, []);

  const rows = useMemo<Row[]>(() => {
    if (!report) return [];
    return report.albums.flatMap((album) => {
      const tracks = filter === "all" ? album.tracks : album.tracks.filter((t) => t.status === filter);
      if (tracks.length === 0) return [];
      return [
        { type: "album" as const, album, count: tracks.length },
        ...tracks.map((track) => ({ type: "track" as const, track, album })),
      ];
    });
  }, [report, filter]);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => (rows[i].type === "album" ? HEADER_HEIGHT : 18),
    overscan: 20,
  });

  const loadXml = () => {
    setError(null);
    void collectionLoadXml();
  };
  const loadMusic = () => {
    setError(null);
    setReading(true);
    void collectionLoadMusicApp();
  };

  if (reading) return <Reading />;
  if (report === undefined) return <div className="ownership" />;
  if (report === null) return <Intro error={error} onLoadXml={loadXml} onLoadMusic={loadMusic} />;

  const counts: Record<Filter, number> = {
    buy: report.totals.buy,
    import: report.totals.import,
    locate: report.totals.locate,
    redownload: report.totals.redownload,
    inLibrary: report.totals.inLibrary,
    unknown: report.totals.unknown,
    all: report.totals.songs,
  };

  return (
    <div className="ownership">
      <header className="ownership-header">
        <Progress report={report} />
        <div className="ownership-actions">
          {report.totals.import > 0 && (
            <button className="push-button" onClick={() => void collectionImportOwned()}>
              Import {pluralize(report.totals.import, "Owned File", "Owned Files")}
            </button>
          )}
          <button
            className="push-button"
            title={`Read ${new Date(report.loadedAt * 1000).toLocaleString()} from ${report.source}`}
            onClick={() => (report.source === "Music" ? loadMusic() : loadXml())}
          >
            Refresh
          </button>
        </div>
      </header>
      {error && <div className="ownership-error-bar">{error}</div>}
      {filter === "locate" && <MissingFiles report={report} />}
      {filter === "redownload" && <Redownload report={report} />}
      {filter === "buy" && report.totals.buy === 0 && (
        <div className="ownership-hint">
          No Apple Music catalog songs found. Songs you add from Apple Music only reach this list when Music › Settings ›
          General › Sync Library is on.
        </div>
      )}
      <div className="pill-tabs" role="tablist">
        {FILTERS.map((f) => (
          <button key={f.id} role="tab" aria-selected={filter === f.id} className="pill-tab" onClick={() => setFilter(f.id)}>
            {f.label} ({counts[f.id].toLocaleString()})
          </button>
        ))}
      </div>
      <div ref={scrollRef} className="ownership-list">
        <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
          {virtualizer.getVirtualItems().map((item) => {
            const row = rows[item.index];
            const style = { transform: `translateY(${item.start}px)`, height: item.size };
            if (row.type === "album") {
              const query = encodeURIComponent(`${row.album.albumArtist} ${row.album.album}`.trim());
              return (
                <div key={`a-${item.index}`} className="ownership-album" style={style}>
                  <span className="ownership-album-title">{row.album.album || "Unknown Album"}</span>
                  <span className="ownership-album-artist">{row.album.albumArtist}</span>
                  <span className="ownership-album-count">{pluralize(row.count, "song", "songs")}</span>
                  {row.album.tracks.some((t) => t.status === "buy") && (
                    <span className="ownership-stores">
                      {(() => {
                        const key = `${row.album.albumArtist}\u0000${row.album.album}`;
                        const found = links[key];
                        if (Array.isArray(found) && found.length > 0) {
                          return found.map((link) => (
                            <button key={link.url} className="ownership-store ownership-store--exact" onClick={() => void openUrl(link.url)}>
                              {link.store}
                            </button>
                          ));
                        }
                        return (
                          <>
                            {STORES.map((store) => (
                              <button key={store.name} className="ownership-store" onClick={() => void openUrl(store.url(query))}>
                                {store.name}
                              </button>
                            ))}
                            {found === undefined && (
                              <button
                                className="ownership-store"
                                title="Look up exact store pages on MusicBrainz"
                                onClick={() => {
                                  setLinks((l) => ({ ...l, [key]: "loading" }));
                                  void purchasesLinks(row.album.albumArtist, row.album.album).then(
                                    (result) => setLinks((l) => ({ ...l, [key]: result })),
                                    () => setLinks((l) => ({ ...l, [key]: [] })),
                                  );
                                }}
                              >
                                Find Stores
                              </button>
                            )}
                            {found === "loading" && <span className="ownership-finding">Finding…</span>}
                          </>
                        );
                      })()}
                    </span>
                  )}
                </div>
              );
            }
            const { track } = row;
            return (
              <div key={`t-${item.index}`} className={`ownership-track ${item.index % 2 ? "ownership-track--odd" : ""}`} style={style}>
                <span className="ownership-track-number">{track.trackNumber ?? ""}</span>
                <span className="ownership-track-title">{track.title}</span>
                <span className="ownership-track-artist">{track.artist === row.album.albumArtist ? "" : track.artist}</span>
                <span className="ownership-track-time">{track.durationMs ? formatClock(track.durationMs) : ""}</span>
                <span className={`ownership-status ownership-status--${track.status}`}>{STATUS_LABEL[track.status]}</span>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
