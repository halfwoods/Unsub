import { useEffect, useState } from "react";
import { onPurchasesChanged, purchasesAccept, purchasesIgnore, purchasesScan, purchasesSetWatching, purchasesStatus } from "../api/commands";
import type { PurchaseStore, PurchasesStatus } from "../api/types";
import { useApp } from "../state/store";
import { formatBytes, pluralize } from "../util/format";
import "./PurchasesViews.css";

const STORE_NAMES: Record<PurchaseStore, string> = {
  bandcamp: "Bandcamp",
  qobuz: "Qobuz",
  sevenDigital: "7digital",
  hdtracks: "HDtracks",
  unknown: "Download",
};

function usePurchases() {
  const [status, setStatus] = useState<PurchasesStatus | null>(null);
  useEffect(() => {
    const refresh = () => void purchasesStatus().then(setStatus);
    refresh();
    const subscription = onPurchasesChanged(refresh);
    return () => void subscription.then((unlisten) => unlisten());
  }, []);
  return status;
}

const formatDate = (unix: number) => new Date(unix * 1000).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });

/** New downloads from stores that sell files you keep, waiting to be added to the library. */
export function InboxView() {
  const status = usePurchases();
  const [busy, setBusy] = useState<ReadonlySet<number>>(new Set());
  const [error, setError] = useState<string | null>(null);
  if (!status) return <div className="purchases" />;

  if (!status.watching) {
    return (
      <div className="placard">
        <div className="placard-card">
          <h1>Purchases Inbox</h1>
          <p>
            Buy albums from Bandcamp, Qobuz, 7digital or HDtracks and Unsub notices the downloads, then adds them to your
            library with one click. Your downloads stay where they are.
          </p>
          <div className="ownership-intro-actions">
            <button className="push-button" onClick={() => void purchasesSetWatching(true)}>
              Watch Downloads Folder
            </button>
          </div>
          <p className="purchases-note">macOS will ask before Unsub can look in your Downloads folder.</p>
        </div>
      </div>
    );
  }

  const accept = async (id: number) => {
    setBusy((b) => new Set(b).add(id));
    setError(null);
    try {
      await purchasesAccept(id);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy((b) => {
        const next = new Set(b);
        next.delete(id);
        return next;
      });
    }
  };

  return (
    <div className="purchases">
      {error && <div className="ownership-error-bar">{error}</div>}
      {status.candidates.length === 0 ? (
        <div className="purchases-empty">Nothing new in {status.downloads}. Purchases you download will appear here.</div>
      ) : (
        <ul className="purchases-list">
          {status.candidates.map((c) => (
            <li key={c.id} className="purchase-row">
              <span className={`purchase-store purchase-store--${c.store}`}>{STORE_NAMES[c.store]}</span>
              <span className="purchase-main">
                <strong>{c.album || "Untitled"}</strong>
                <span>{c.artist}</span>
              </span>
              <span className="purchase-meta">
                {pluralize(c.trackCount, "song", "songs")} · {c.formats.join(", ")} · {formatBytes(c.bytes)}
              </span>
              <button className="push-button" disabled={busy.has(c.id)} onClick={() => void accept(c.id)}>
                {busy.has(c.id) ? "Adding…" : "Add to Library"}
              </button>
              <button className="push-button" disabled={busy.has(c.id)} onClick={() => void purchasesIgnore(c.id)}>
                Ignore
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="party-bar">
        <span className="purchases-watching">Watching {status.downloads}</span>
        <button className="push-button" onClick={() => void purchasesScan()}>
          Check Now
        </button>
        <button className="push-button" onClick={() => void purchasesSetWatching(false)}>
          Stop Watching
        </button>
      </div>
    </div>
  );
}

/** Everything bought and added through the Inbox, newest first. */
export function PurchasedView() {
  const status = usePurchases();
  const setSource = useApp((s) => s.setSource);
  const setSearch = useApp((s) => s.setSearch);
  if (!status) return <div className="purchases" />;
  if (status.purchases.length === 0) {
    return (
      <div className="placard">
        <div className="placard-card">
          <h1>Purchased</h1>
          <p>Albums you buy and add through the Purchases Inbox are listed here, with where and when you bought them.</p>
        </div>
      </div>
    );
  }
  return (
    <div className="purchases">
      <ul className="purchases-list">
        {[...status.purchases]
          .sort((a, b) => b.importedAt - a.importedAt)
          .map((p) => (
            <li
              key={p.id}
              className="purchase-row"
              title="Double-click to show it in Music"
              onDoubleClick={() => {
                setSource("music");
                setSearch(p.album);
              }}
            >
              <span className={`purchase-store purchase-store--${p.store}`}>{STORE_NAMES[p.store]}</span>
              <span className="purchase-main">
                <strong>{p.album}</strong>
                <span>{p.artist}</span>
              </span>
              <span className="purchase-meta">Added {formatDate(p.importedAt)}</span>
            </li>
          ))}
      </ul>
    </div>
  );
}
