import { useEffect, useRef, useState, type ReactNode } from "react";
import { podcastsAppShows, podcastsDirectoryCategories, podcastsDirectorySearch, podcastsDirectoryTop, podcastsSubscribe } from "../api/commands";
import type { PodcastCategory, PodcastFeed, PodcastListing, PodcastListings } from "../api/types";
import { useApp } from "../state/store";
import { pluralize } from "../util/format";

/** Searching follows the last key by a moment, so a typed name is one request and not one per letter. Return searches at once. */
const SEARCH_DELAY_MS = 400;

type Results = { status: "idle" } | { status: "loading" } | { status: "done"; shows: PodcastListing[] } | { status: "failed"; message: string };

type Source = PodcastListings["source"];

/** Runs `load` for each new `request` (none for null) and keeps only the answer to the latest one, saying which directory gave it. */
function useListings<K>(request: K | null, load: (request: K) => Promise<PodcastListings>, onSource: (source: Source) => void): Results {
  const [results, setResults] = useState<Results>({ status: "idle" });
  useEffect(() => {
    if (request === null) {
      setResults({ status: "idle" });
      return;
    }
    let current = true;
    setResults({ status: "loading" });
    load(request).then(
      (listings) => {
        if (!current) return;
        setResults({ status: "done", shows: listings.shows });
        onSource(listings.source);
      },
      (e: unknown) => current && setResults({ status: "failed", message: String(e) }),
    );
    return () => {
      current = false;
    };
  }, [request, load, onSource]);
  return results;
}

/** A directory and a subscription can spell one address differently: scheme, case, a closing slash. */
const feedKey = (url: string) => url.trim().replace(/^[a-z][a-z0-9+.-]*:\/\//i, "").replace(/\/+$/, "").toLowerCase();

/** Which shows are already followed: the subscriptions, plus those added while the sheet has been open. */
function useSubscribed(feeds: PodcastFeed[]) {
  const [added, setAdded] = useState<ReadonlySet<string>>(new Set());
  // A feed that redirected is stored where it ended up, but directories list the address it was asked for.
  const known = new Set(feeds.flatMap((f) => [f.url, ...f.aliases]).map(feedKey));
  return {
    isSubscribed: (url: string) => known.has(feedKey(url)) || added.has(feedKey(url)),
    markSubscribed: (url: string) => setAdded((a) => new Set(a).add(feedKey(url))),
  };
}

function ShowRow({ show, lead, trail }: { show: PodcastListing; lead?: ReactNode; trail?: ReactNode }) {
  const details = [show.author, show.genre, show.episodeCount !== null && pluralize(show.episodeCount, "episode", "episodes")].filter(Boolean).join(" · ");
  return (
    <li className="find-row">
      {lead}
      {show.artworkUrl ? <img className="find-art" src={show.artworkUrl} alt="" loading="lazy" /> : <span className="find-art" />}
      <span className="find-text">
        <strong title={show.title}>{show.title}</strong>
        <small title={details}>{details}</small>
      </span>
      {trail}
    </li>
  );
}

function ShowList({ results, idleText, row }: { results: Results; idleText: string; row: (show: PodcastListing) => ReactNode }) {
  const note =
    results.status === "idle"
      ? idleText
      : results.status === "loading"
        ? "Loading…"
        : results.status === "failed"
          ? results.message
          : results.shows.length === 0
            ? "No shows found."
            : null;
  return (
    <div className="find-list">
      {note !== null ? (
        <p className={results.status === "failed" ? "find-note find-note--error" : "find-note"}>{note}</p>
      ) : (
        <ul aria-label="Shows">{results.status === "done" && results.shows.map(row)}</ul>
      )}
    </div>
  );
}

const topShows = (category: number | "all") => podcastsDirectoryTop(category === "all" ? null : category);

interface TabProps {
  onSource: (source: Source) => void;
  row: (show: PodcastListing) => ReactNode;
}

function TopShows({ categories, onSource, row }: TabProps & { categories: PodcastCategory[] }) {
  const [category, setCategory] = useState<number | "all">("all");
  const results = useListings(category, topShows, onSource);
  return (
    <>
      <div className="find-controls">
        <label htmlFor="find-category">Category:</label>
        <select id="find-category" className="smart-popup" autoFocus value={category} onChange={(e) => setCategory(e.target.value === "all" ? "all" : Number(e.target.value))}>
          <option value="all">All Categories</option>
          {categories.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name}
            </option>
          ))}
        </select>
      </div>
      <ShowList results={results} idleText="" row={row} />
    </>
  );
}

/** An object, so that asking for the same name again is a new request: Return retries a search that failed. */
interface Search {
  term: string;
}

const searchFor = ({ term }: Search) => podcastsDirectorySearch(term);

function SearchShows({ active, onSource, row }: TabProps & { active: boolean }) {
  const [typed, setTyped] = useState("");
  const [search, setSearch] = useState<Search | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const results = useListings(search, searchFor, onSource);
  const searchNow = () => setSearch(typed.trim() ? { term: typed.trim() } : null);

  // Return has usually searched the name already by the time the delay is up.
  useEffect(() => {
    const term = typed.trim();
    const timer = setTimeout(() => setSearch((current) => (current?.term === term ? current : term ? { term } : null)), SEARCH_DELAY_MS);
    return () => clearTimeout(timer);
  }, [typed]);
  useEffect(() => {
    if (active) input.current?.focus();
  }, [active]);

  return (
    <>
      <div className="find-controls">
        <input
          ref={input}
          className="smart-input find-search"
          aria-label="Search for a podcast"
          placeholder="Name of a show"
          spellCheck={false}
          value={typed}
          onChange={(e) => setTyped(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && searchNow()}
        />
      </div>
      <ShowList results={results} idleText="Type the name of a show." row={row} />
    </>
  );
}

/** Podcasts › Find Podcasts…: the most popular shows by category, or a search by name, each with a Subscribe button. */
export function FindPodcastsSheet({ feeds, onClose }: { feeds: PodcastFeed[]; onClose: () => void }) {
  const showNotice = useApp((s) => s.showNotice);
  const { isSubscribed, markSubscribed } = useSubscribed(feeds);
  const [tab, setTab] = useState<"top" | "search">("top");
  const [pending, setPending] = useState<ReadonlySet<string>>(new Set());
  const [categories, setCategories] = useState<PodcastCategory[]>([]);
  // The latest answer of each tab says where its shows came from: a request the first directory failed is answered by the other.
  const [topSource, setTopSource] = useState<Source | null>(null);
  const [searchSource, setSearchSource] = useState<Source | null>(null);
  const source = tab === "top" ? topSource : searchSource;
  useEffect(() => {
    void podcastsDirectoryCategories().then(setCategories);
  }, []);

  const subscribe = async (show: PodcastListing) => {
    setPending((p) => new Set(p).add(show.feedUrl));
    try {
      const feed = await podcastsSubscribe(show.feedUrl);
      markSubscribed(show.feedUrl);
      markSubscribed(feed.url);
      showNotice(`Subscribed to “${show.title}”`);
    } catch (e) {
      showNotice(String(e));
    } finally {
      setPending((p) => {
        const next = new Set(p);
        next.delete(show.feedUrl);
        return next;
      });
    }
  };

  const row = (show: PodcastListing) => (
    <ShowRow
      key={show.feedUrl}
      show={show}
      trail={
        isSubscribed(show.feedUrl) ? (
          <span className="find-subscribed">Subscribed</span>
        ) : (
          <button className="push-button" aria-label={`Subscribe to ${show.title}`} disabled={pending.has(show.feedUrl)} onClick={() => void subscribe(show)}>
            {pending.has(show.feedUrl) ? "Subscribing…" : "Subscribe"}
          </button>
        )
      }
    />
  );

  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="sheet find-podcasts" role="dialog" aria-label="Find Podcasts">
        <div className="pill-tabs find-tabs" role="tablist">
          <button role="tab" aria-selected={tab === "top"} className="pill-tab" onClick={() => setTab("top")}>
            Top Shows
          </button>
          <button role="tab" aria-selected={tab === "search"} className="pill-tab" onClick={() => setTab("search")}>
            Search
          </button>
        </div>
        <div hidden={tab !== "top"}>
          <TopShows categories={categories} onSource={setTopSource} row={row} />
        </div>
        <div hidden={tab !== "search"}>
          <SearchShows active={tab === "search"} onSource={setSearchSource} row={row} />
        </div>
        {source && (
          <p className="find-source">
            {source === "podcastIndex" ? "Searches use Podcast Index, the open podcast directory." : "Searches use Apple's public podcast directory."}
          </p>
        )}
        <div className="sheet-buttons">
          <button className="push-button push-button--default" onClick={onClose}>
            Done
          </button>
        </div>
      </div>
    </div>
  );
}

/** A show that could not be subscribed to, and why. */
interface Failure {
  feedUrl: string;
  title: string;
  message: string;
}

/** Reading the Podcasts app's library is what makes macOS ask for permission, so it starts here, when the user chose to import. */
export function ImportFromPodcastsAppSheet({ feeds, onClose }: { feeds: PodcastFeed[]; onClose: () => void }) {
  const showNotice = useApp((s) => s.showNotice);
  const { isSubscribed, markSubscribed } = useSubscribed(feeds);
  const [shows, setShows] = useState<PodcastListing[] | null>(null);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);
  const [failures, setFailures] = useState<Failure[]>([]);
  const busy = progress !== null;

  useEffect(() => {
    let current = true;
    podcastsAppShows().then(
      (list) => {
        if (!current) return;
        setShows(list);
        setSelected(new Set(list.filter((s) => !isSubscribed(s.feedUrl)).map((s) => s.feedUrl)));
      },
      (e: unknown) => current && setError(String(e)),
    );
    return () => {
      current = false;
    };
    // The ticks start from the subscriptions as they were when the list arrived; later changes are the user's.
  }, []);

  const eligible = (shows ?? []).filter((s) => !isSubscribed(s.feedUrl));
  const toggle = (url: string, on: boolean) =>
    setSelected((current) => {
      const next = new Set(current);
      if (on) next.add(url);
      else next.delete(url);
      return next;
    });

  const start = async () => {
    const chosen = eligible.filter((s) => selected.has(s.feedUrl));
    const failed: Failure[] = [];
    setFailures([]);
    for (const [index, show] of chosen.entries()) {
      setProgress({ done: index, total: chosen.length });
      try {
        const feed = await podcastsSubscribe(show.feedUrl);
        markSubscribed(show.feedUrl);
        markSubscribed(feed.url);
        toggle(show.feedUrl, false);
      } catch (e) {
        failed.push({ feedUrl: show.feedUrl, title: show.title, message: String(e) });
      }
    }
    setProgress(null);
    setFailures(failed);
    const added = chosen.length - failed.length;
    showNotice(failed.length === 0 ? `Subscribed to ${pluralize(added, "podcast", "podcasts")}` : `Subscribed to ${added.toLocaleString()} of ${pluralize(chosen.length, "podcast", "podcasts")}`);
    if (failed.length === 0) onClose();
  };

  const allSelected = eligible.length > 0 && eligible.every((s) => selected.has(s.feedUrl));
  const status = progress
    ? `Subscribing to ${(progress.done + 1).toLocaleString()} of ${progress.total.toLocaleString()}…`
    : `${selected.size.toLocaleString()} of ${eligible.length.toLocaleString()} selected`;

  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && !busy && onClose()}>
      <div className="sheet find-podcasts" role="dialog" aria-label="Import from Podcasts app">
        <h2 className="podcast-sheet-title">Import from Podcasts app</h2>
        <p className="find-intro">The shows you follow in the Podcasts app. Unsub subscribes to the ones that are ticked.</p>
        {error && <p className="smart-error">{error}</p>}
        {!error && shows === null && <p className="find-note">Reading the Podcasts app…</p>}
        {shows && shows.length === 0 && <p className="find-note">You aren't following any shows in the Podcasts app.</p>}
        {shows && shows.length > 0 && (
          <>
            <label className="prefs-check find-select-all">
              <input
                type="checkbox"
                checked={allSelected}
                disabled={busy || eligible.length === 0}
                onChange={(e) => setSelected(new Set(e.target.checked ? eligible.map((s) => s.feedUrl) : []))}
              />
              Select All
            </label>
            <div className="find-list">
              <ul aria-label="Followed shows">
                {shows.map((show) => {
                  const subscribed = isSubscribed(show.feedUrl);
                  return (
                    <ShowRow
                      key={show.feedUrl}
                      show={show}
                      lead={
                        <input
                          type="checkbox"
                          aria-label={`Subscribe to ${show.title}`}
                          checked={!subscribed && selected.has(show.feedUrl)}
                          disabled={subscribed || busy}
                          onChange={(e) => toggle(show.feedUrl, e.target.checked)}
                        />
                      }
                      trail={subscribed && <span className="find-subscribed">Subscribed</span>}
                    />
                  );
                })}
              </ul>
            </div>
            <p className="find-status">{status}</p>
          </>
        )}
        {failures.length > 0 && (
          <>
            <p className="smart-error">
              {pluralize(failures.length, "show couldn't be subscribed to", "shows couldn't be subscribed to")}. They're still ticked, so Import tries them again.
            </p>
            <ul className="find-failures" aria-label="Failures">
              {failures.map((f) => (
                <li key={f.feedUrl}>
                  <strong>{f.title}</strong>: {f.message}
                </li>
              ))}
            </ul>
          </>
        )}
        <div className="sheet-buttons">
          <button className="push-button" disabled={busy} onClick={onClose}>
            {failures.length > 0 ? "Close" : "Cancel"}
          </button>
          <button className="push-button push-button--default" disabled={busy || selected.size === 0} onClick={() => void start()}>
            {busy ? "Importing…" : "Import"}
          </button>
        </div>
      </div>
    </div>
  );
}
