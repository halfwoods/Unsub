import { message } from "@tauri-apps/plugin-dialog";
import { useEffect, useState, type MouseEvent } from "react";
import {
  onPodcastProgress,
  onPodcastsChanged,
  podcastsEpisodes,
  podcastsFeeds,
  podcastsRefresh,
  podcastsSetPlayed,
  podcastsSetSettings,
  podcastsSubscribe,
  podcastsUnsubscribe,
} from "../api/commands";
import type { FeedSettings, KeepRule, PodcastEpisode, PodcastFeed } from "../api/types";
import { SEPARATOR, showMenu } from "../browser/songMenu";
import { useApp } from "../state/store";
import { formatClock } from "../util/format";
import { IS_MAC, isContextClick } from "../util/platform";
import { FindPodcastsSheet, ImportFromPodcastsAppSheet } from "./FindPodcasts";
import "./PodcastsView.css";

/** A batch of subscriptions announces a change per show; one reload after the last is enough. */
const RELOAD_DELAY_MS = 250;

/** Feeds put HTML in descriptions; the list shows plain text only, so a feed can't inject markup. */
function plainText(html: string) {
  return new DOMParser().parseFromString(html, "text/html").body.textContent?.replace(/\s+/g, " ").trim() ?? "";
}

const formatDate = (unix: number | null) => (unix ? new Date(unix * 1000).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" }) : "");

function SubscribeSheet({ onClose }: { onClose: () => void }) {
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const subscribe = async () => {
    setBusy(true);
    setError(null);
    try {
      await podcastsSubscribe(url);
      onClose();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };
  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="sheet podcast-sheet" role="dialog" aria-label="Subscribe to Podcast">
        <label className="podcast-sheet-label" htmlFor="podcast-url">
          URL:
        </label>
        <input
          id="podcast-url"
          className="smart-input podcast-url"
          autoFocus
          placeholder="https://example.com/feed.xml"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && url.trim() && void subscribe()}
        />
        {error && <p className="smart-error">{error}</p>}
        <div className="sheet-buttons">
          <button className="push-button" onClick={onClose}>
            Cancel
          </button>
          <button className="push-button push-button--default" disabled={!url.trim() || busy} onClick={() => void subscribe()}>
            {busy ? "Subscribing…" : "OK"}
          </button>
        </div>
      </div>
    </div>
  );
}

const KEEP_DOWNLOADS = "Keep Downloaded Episodes";
const DELETE_DOWNLOADS = "Delete Downloads";

/** Asks what becomes of the downloaded files before unsubscribing, since they are the listener's to keep. */
async function unsubscribe(feed: PodcastFeed) {
  try {
    const choice = await message(`Unsubscribe from “${feed.title}”? You can keep the episodes you've downloaded, or delete them.`, {
      title: "Unsub",
      kind: "warning",
      buttons: { yes: KEEP_DOWNLOADS, no: DELETE_DOWNLOADS, cancel: "Cancel" },
    });
    if (choice === KEEP_DOWNLOADS || choice === DELETE_DOWNLOADS) await podcastsUnsubscribe(feed.id, choice === DELETE_DOWNLOADS);
  } catch (e) {
    useApp.getState().showNotice(String(e));
  }
}

const KEEP_OPTIONS: { label: string; value: KeepRule }[] = [
  { label: "All episodes", value: "all" },
  { label: "All unplayed episodes", value: "unplayed" },
  { label: "Most recent episode", value: { lastN: 1 } },
  { label: "Last 5 episodes", value: { lastN: 5 } },
  { label: "Last 10 episodes", value: { lastN: 10 } },
];

function SettingsSheet({ feed, onClose }: { feed: PodcastFeed; onClose: () => void }) {
  const [settings, setSettings] = useState<FeedSettings>(feed.settings);
  const keepIndex = Math.max(0, KEEP_OPTIONS.findIndex((o) => JSON.stringify(o.value) === JSON.stringify(settings.keep)));
  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="sheet podcast-sheet" role="dialog" aria-label="Podcast Settings">
        <h2 className="podcast-sheet-title">{feed.title}</h2>
        <div className="prefs-grid">
          <span className="prefs-label">When new episodes are available:</span>
          <select
            className="smart-popup"
            value={settings.autoDownloadLatest}
            onChange={(e) => setSettings({ ...settings, autoDownloadLatest: Number(e.target.value) })}
          >
            <option value={0}>Do nothing</option>
            <option value={1}>Download the most recent one</option>
            <option value={3}>Download the latest 3</option>
            <option value={10}>Download the latest 10</option>
          </select>
          <span className="prefs-label">Episodes to keep:</span>
          <select className="smart-popup" value={keepIndex} onChange={(e) => setSettings({ ...settings, keep: KEEP_OPTIONS[Number(e.target.value)].value })}>
            {KEEP_OPTIONS.map((o, i) => (
              <option key={o.label} value={i}>
                {o.label}
              </option>
            ))}
          </select>
        </div>
        <div className="sheet-buttons">
          <button className="push-button" onClick={onClose}>
            Cancel
          </button>
          <button className="push-button push-button--default" onClick={() => void podcastsSetSettings(feed.id, settings).then(onClose)}>
            OK
          </button>
        </div>
      </div>
    </div>
  );
}

/** What stands in for an episode that isn't downloaded: GET, then "Downloading…" until the size is known, then a bar with its percentage. */
function EpisodeDownload({ download, onGet }: { download: { fraction: number | null } | undefined; onGet: () => void }) {
  if (!download) {
    return (
      <button className="podcast-get" onClick={(e) => !isContextClick(e) && onGet()}>
        GET
      </button>
    );
  }
  if (download.fraction === null) return <span className="podcast-status">Downloading…</span>;
  const percent = Math.round(download.fraction * 100);
  return (
    <>
      <span className="podcast-progress">
        <span style={{ width: `${percent}%` }} />
      </span>
      <span className="podcast-percent">{percent}%</span>
    </>
  );
}

/** iTunes 10's Podcasts list: shows with disclosure triangles, episodes underneath, GET buttons. */
export function PodcastsView() {
  const [feeds, setFeeds] = useState<PodcastFeed[] | null>(null);
  const [episodes, setEpisodes] = useState<Record<number, PodcastEpisode[]>>({});
  const [open, setOpen] = useState<ReadonlySet<number>>(new Set());
  const [selectedFeed, setSelectedFeed] = useState<number | null>(null);
  const [sheet, setSheet] = useState<"find" | "import" | "subscribe" | "settings" | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const playTrackIds = useApp((s) => s.playTrackIds);
  const showNotice = useApp((s) => s.showNotice);
  const downloads = useApp((s) => s.podcastDownloads);
  const downloadPodcasts = useApp((s) => s.downloadPodcasts);
  const playPodcastWhenDownloaded = useApp((s) => s.playPodcastWhenDownloaded);

  useEffect(() => {
    const load = () =>
      void podcastsFeeds().then(async (list) => {
        setFeeds(list);
        const loaded = await Promise.all(list.map(async (f) => [f.id, await podcastsEpisodes(f.id)] as const));
        setEpisodes(Object.fromEntries(loaded));
      });
    load();
    let reloadTimer: ReturnType<typeof setTimeout> | undefined;
    const subscriptions = [
      onPodcastsChanged(() => {
        clearTimeout(reloadTimer);
        reloadTimer = setTimeout(load, RELOAD_DELAY_MS);
      }),
      // The row turns into a playable one at once, instead of falling back to GET until the reload above catches up.
      onPodcastProgress((p) => {
        if (p.state !== "done") return;
        const landed = (e: PodcastEpisode) => (e.id === p.episodeId ? { ...e, trackId: p.trackId } : e);
        setEpisodes((all) => Object.fromEntries(Object.entries(all).map(([feedId, list]) => [feedId, list.map(landed)])));
      }),
    ];
    return () => {
      clearTimeout(reloadTimer);
      subscriptions.forEach((s) => void s.then((unlisten) => unlisten()));
    };
  }, []);

  if (!feeds) return <div className="podcasts" />;
  const feed = feeds.find((f) => f.id === selectedFeed) ?? null;
  const toggle = (id: number) =>
    setOpen((o) => {
      const next = new Set(o);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  const report = (error: unknown) => showNotice(String(error));
  const play = (episode: PodcastEpisode) =>
    episode.trackId !== null ? void playTrackIds([episode.trackId]).catch(report) : playPodcastWhenDownloaded(episode);

  const onFeedMenu = (e: MouseEvent, f: PodcastFeed) => {
    e.preventDefault();
    setSelectedFeed(f.id);
    const unplayed = (episodes[f.id] ?? []).filter((episode) => !episode.played);
    void showMenu([
      { text: "Update Podcast", action: () => void podcastsRefresh(f.id).catch(report) },
      { text: "Podcast Settings…", action: () => setSheet("settings") },
      { text: "Mark All as Played", enabled: unplayed.length > 0, action: () => void Promise.all(unplayed.map((episode) => podcastsSetPlayed(episode.id, true))).catch(report) },
      SEPARATOR,
      { text: "Unsubscribe…", action: () => void unsubscribe(f) },
    ]);
  };

  const onEpisodeMenu = (e: MouseEvent, episode: PodcastEpisode) => {
    e.preventDefault();
    const downloaded = episode.trackId !== null;
    void showMenu([
      { text: downloaded ? "Play" : "Download and Play", action: () => play(episode) },
      ...(downloaded ? [] : [{ text: "Download", enabled: !(episode.id in downloads), action: () => downloadPodcasts([episode]) }]),
      SEPARATOR,
      { text: episode.played ? "Mark as Unplayed" : "Mark as Played", action: () => void podcastsSetPlayed(episode.id, !episode.played).catch(report) },
    ]);
  };

  return (
    <div className="podcasts">
      <div className="song-list-header podcasts-header">
        <div className="song-list-header-cell">Podcast</div>
        <div className="song-list-header-cell">Time</div>
        <div className="song-list-header-cell">Release Date</div>
        <div className="song-list-header-cell">Description</div>
      </div>
      <div className="podcasts-body song-list-body" tabIndex={0}>
        {feeds.length === 0 && (
          <div className="podcasts-empty">
            <h1>Podcasts</h1>
            <p>Find a podcast by name, or subscribe to any podcast with its feed address. New episodes download automatically if you want, and you keep the files.</p>
            <div className="podcasts-empty-buttons">
              <button className="push-button push-button--default" onClick={() => setSheet("find")}>
                Find Podcasts…
              </button>
              <button className="push-button" onClick={() => setSheet("subscribe")}>
                Subscribe to Podcast…
              </button>
            </div>
          </div>
        )}
        {feeds.map((f) => {
          const isOpen = open.has(f.id);
          const list = episodes[f.id] ?? [];
          const missing = list.filter((e) => e.trackId === null && !(e.id in downloads));
          return (
            <div key={f.id}>
              <div className={selectedFeed === f.id ? "podcast-row podcast-row--feed song-row--selected" : "podcast-row podcast-row--feed"} onMouseDown={() => setSelectedFeed(f.id)} onContextMenu={(e) => onFeedMenu(e, f)}>
                <span className="podcast-title">
                  <button
                    className={isOpen ? "sidebar-disclosure sidebar-disclosure--open podcast-disclosure" : "sidebar-disclosure podcast-disclosure"}
                    aria-label={isOpen ? `Collapse ${f.title}` : `Expand ${f.title}`}
                    // Ctrl-click on a Mac is a context click that WebKit reports as a click too, which must not act on the row.
                    onClick={(e) => !isContextClick(e) && toggle(f.id)}
                  />
                  {f.artworkUrl && <img className="podcast-art" src={f.artworkUrl} alt="" />}
                  <strong>{f.title}</strong>
                  {f.unplayedCount > 0 && <span className="podcast-count">{f.unplayedCount}</span>}
                </span>
                <span />
                <span>{formatDate(list[0]?.published ?? null)}</span>
                <span className="podcast-description">
                  {plainText(f.description)}
                  {missing.length > 0 && (
                    <button className="podcast-get" onClick={(e) => !isContextClick(e) && downloadPodcasts(missing)}>
                      GET ALL
                    </button>
                  )}
                </span>
              </div>
              {isOpen &&
                list.map((e, i) => (
                  <div key={e.id} className={i % 2 ? "podcast-row song-row--odd" : "podcast-row"} onDoubleClick={() => play(e)} onContextMenu={(event) => onEpisodeMenu(event, e)}>
                    <span className="podcast-title podcast-title--episode">
                      <span className={e.played ? "podcast-dot" : "podcast-dot podcast-dot--unplayed"} />
                      {e.title}
                    </span>
                    <span>{e.durationMs ? formatClock(e.durationMs) : ""}</span>
                    <span>{formatDate(e.published)}</span>
                    <span className="podcast-description">
                      {plainText(e.description)}
                      {e.trackId === null && <EpisodeDownload download={downloads[e.id]} onGet={() => downloadPodcasts([e])} />}
                      {e.trackId !== null && !e.played && (
                        <button className="podcast-get" onClick={(event) => !isContextClick(event) && void podcastsSetPlayed(e.id, true)} title="Mark as played">
                          ✓
                        </button>
                      )}
                    </span>
                  </div>
                ))}
            </div>
          );
        })}
      </div>
      <div className="party-bar podcasts-bar">
        <div className="podcasts-bar-start">
          <button className="push-button" onClick={() => setSheet("find")}>
            Find Podcasts…
          </button>
          {IS_MAC && (
            <button className="push-button" onClick={() => setSheet("import")}>
              Import from Podcasts app…
            </button>
          )}
          <button className="push-button" onClick={() => setSheet("subscribe")}>
            Subscribe to Podcast…
          </button>
        </div>
        <button
          className="push-button"
          disabled={refreshing}
          onClick={() => {
            setRefreshing(true);
            void podcastsRefresh(null)
              .catch((e: unknown) => showNotice(String(e)))
              .finally(() => setRefreshing(false));
          }}
        >
          {refreshing ? "Refreshing…" : "Refresh"}
        </button>
        <button className="push-button" disabled={!feed} onClick={() => setSheet("settings")}>
          Settings…
        </button>
        <button className="push-button" disabled={!feed} onClick={() => feed && void unsubscribe(feed)}>
          Unsubscribe…
        </button>
      </div>
      {sheet === "find" && <FindPodcastsSheet feeds={feeds} onClose={() => setSheet(null)} />}
      {sheet === "import" && <ImportFromPodcastsAppSheet feeds={feeds} onClose={() => setSheet(null)} />}
      {sheet === "subscribe" && <SubscribeSheet onClose={() => setSheet(null)} />}
      {sheet === "settings" && feed && <SettingsSheet feed={feed} onClose={() => setSheet(null)} />}
    </div>
  );
}
