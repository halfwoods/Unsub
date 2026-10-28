import { useApp } from "../state/store";
import "./ColumnBrowser.css";

export function BrowseNavigation() {
  const history = useApp((s) => s.browseHistory);
  const label = useApp((s) => s.browseLabel);
  const tracks = useApp((s) => s.tracks);
  const loading = useApp((s) => s.loading);
  const { backBrowse, playTrackIds, toggleShuffle } = useApp.getState();
  if (history.length === 0) return null;
  const play = (shuffled: boolean) => {
    if (useApp.getState().shuffle !== shuffled) toggleShuffle();
    void playTrackIds(tracks.filter((t) => t.enabled && t.kind !== "radio").map((t) => t.id), shuffled).catch((error: unknown) => useApp.getState().showNotice(String(error)));
  };
  return <nav className="browse-navigation" aria-label="Library navigation">
    <button className="push-button" onClick={() => backBrowse()} aria-label="Back to previous library view">‹ Back</button>
    {history.map((step, index) => <button key={index} className="browser-text-button" title={step.label} onClick={() => backBrowse(index)}>{step.label} ›</button>)}
    <strong className="browse-current" aria-current="page" title={label ?? ""}>{label}</strong>
    <div className="browse-actions">
      <button className="push-button" disabled={loading || tracks.length === 0} onClick={() => play(false)}>Play</button>
      <button className="push-button" disabled={loading || tracks.length === 0} onClick={() => play(true)}>Shuffle</button>
    </div>
  </nav>;
}
