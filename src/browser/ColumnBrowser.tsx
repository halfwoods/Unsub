import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useRef } from "react";
import { cdIdOf, isLibrarySource, isPlaylistSource, useApp } from "../state/store";
import { hasBrowseFilters } from "./browseScope";
import "./ColumnBrowser.css";

interface Choice { key: string; name: string; count: number }
function Pane({ title, choices, value, onChange }: { title: string; choices: Choice[]; value: string | null; onChange: (value: string | null) => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const rows = [{ key: null, name: `All ${title} (${choices.length.toLocaleString()})`, count: null }, ...choices];
  const selected = rows.findIndex((row) => row.key === value);
  const virtualizer = useVirtualizer({ count: rows.length, getScrollElement: () => ref.current, estimateSize: () => 20, overscan: 4 });
  useEffect(() => { if (selected >= 0) virtualizer.scrollToIndex(selected, { align: "auto" }); }, [selected, virtualizer]);
  return <section className="column-browser-pane" aria-label={title}>
    <div className="column-browser-heading">{title}</div>
    <div ref={ref} className="column-browser-list" role="listbox" aria-label={title} tabIndex={0}
      aria-activedescendant={selected >= 0 ? `browser-${title}-${selected}` : undefined}
      onKeyDown={(e) => {
        let next = selected;
        if (e.key === "ArrowDown") next = Math.min(rows.length - 1, selected + 1);
        else if (e.key === "ArrowUp") next = Math.max(0, selected - 1);
        else if (e.key === "Home") next = 0;
        else if (e.key === "End") next = rows.length - 1;
        else if (e.key === "Escape") next = 0;
        else return;
        e.preventDefault(); e.stopPropagation(); onChange(rows[next].key);
      }}>
      <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
        {virtualizer.getVirtualItems().map((row) => <div key={row.index} id={`browser-${title}-${row.index}`}
          role="option" aria-selected={row.index === selected} className="column-browser-choice"
          style={{ transform: `translateY(${row.start}px)` }} title={rows[row.index].name}
          onMouseDown={(e) => { e.preventDefault(); ref.current?.focus(); }} onClick={() => onChange(rows[row.index].key)}>
          <span>{rows[row.index].name}</span><span className="column-browser-count">{rows[row.index].count?.toLocaleString()}</span>
        </div>)}
      </div>
    </div>
  </section>;
}

export function ColumnBrowser() {
  const source = useApp((s) => s.source);
  const shown = useApp((s) => s.viewPrefs.browser[s.source] ?? false);
  const facets = useApp((s) => s.browserFacets);
  const query = useApp((s) => s.query);
  const loading = useApp((s) => s.loading);
  const { setBrowserVisible, setBrowserFilter, resetBrowse } = useApp.getState();
  const local = isLibrarySource(source) || isPlaylistSource(source);
  if (cdIdOf(source) !== null) return null;
  return <>
    <div className="song-view-bar">
      {local && <button className="browser-text-button" aria-expanded={shown} onClick={() => setBrowserVisible(!shown)}>{shown ? "Hide" : "Show"} Column Browser</button>}
      {local && hasBrowseFilters(query) && <button className="browser-text-button" onClick={resetBrowse}>Clear Filters</button>}
      <button className="browser-text-button view-options-button" onClick={() => useApp.setState({ viewOptionsOpen: true })}>View Options…</button>
    </div>
    {local && shown && <div className="column-browser" aria-label="Column Browser" aria-busy={loading}>
      <Pane title="Genres" choices={(facets?.genres ?? []).map((g) => ({ key: g.name, name: g.name || "Unknown Genre", count: g.trackCount }))}
        value={query.genre ?? null} onChange={(v) => setBrowserFilter("genre", v)} />
      <Pane title="Artists" choices={(facets?.artists ?? []).map((a) => ({ key: a.key, name: a.name || "Unknown Artist", count: a.trackCount }))}
        value={query.artistGroup ?? null} onChange={(v) => setBrowserFilter("artistGroup", v)} />
      <Pane title="Albums" choices={(facets?.albums ?? []).map((a) => ({ key: a.key, name: `${a.album || "Unknown Album"} — ${a.albumArtist || "Unknown Artist"}`, count: a.trackCount }))}
        value={query.albumGroup ?? null} onChange={(v) => setBrowserFilter("albumGroup", v)} />
    </div>}
  </>;
}
