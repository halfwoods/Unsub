import { useApp } from "../state/store";
import "./BrowseNavigation.css";

export function BrowseNavigation() {
  const path = useApp((s) => s.browsePath);
  const group = useApp((s) => s.gridGroup);
  const { browseBack, browseTo, playBrowse, showNotice } = useApp.getState();
  const hasSongs = useApp((s) => s.tracks.some((track) => track.enabled && track.kind !== "radio"));
  const first = path[0]?.kind;
  const root = first === "artist" ? "Artists" : first === "genre" ? "Genres" : first === "composer" ? "Composers" : group === "artists" ? "Artists" : "Albums";
  return (
    <div className="browse-navigation">
      <button className="push-button browse-back" onClick={browseBack} aria-label="Go back" title="Go back">‹</button>
      <nav className="browse-breadcrumbs" aria-label="Library navigation">
        <button onClick={() => browseTo(-1)} title={root}>{root}</button>
        {path.map((part, index) => (
          <span className="browse-crumb" key={`${part.kind}:${part.key}`}>
            <span aria-hidden="true">›</span>
            <button onClick={() => index < path.length - 1 && browseTo(index)} aria-current={index === path.length - 1 ? "page" : undefined} title={part.label}>{part.label}</button>
          </span>
        ))}
      </nav>
      <div className="browse-play-actions">
        <button className="push-button" disabled={!hasSongs} onClick={() => void playBrowse(false).catch((error: unknown) => showNotice(String(error)))}>Play</button>
        <button className="push-button" disabled={!hasSongs} onClick={() => void playBrowse(true).catch((error: unknown) => showNotice(String(error)))}>Shuffle</button>
      </div>
    </div>
  );
}
