import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useRef, type KeyboardEvent } from "react";
import { useApp } from "../state/store";
import { useLibraryViews } from "../preferences/libraryViewStore";
import { hasBrowseFilters } from "./browseScope";
import "./ColumnBrowser.css";

interface Choice { key: string; label: string; count: number }

function Facet({ label, choices, selected, choose }: { label: string; choices: Choice[]; selected: string | null; choose: (key: string | null) => void }) {
  const scroll = useRef<HTMLDivElement>(null);
  const labels = useRef(new Map<string, string>());
  for (const choice of choices) labels.current.set(choice.key, choice.label);
  const options = selected !== null && !choices.some(c => c.key === selected)
    ? [{ key: selected, label: `${labels.current.get(selected) ?? `Selected ${label.toLowerCase()}`} (no matches)`, count: 0 }, ...choices] : choices;
  const index = selected === null ? 0 : options.findIndex((c) => c.key === selected) + 1;
  const virtualizer = useVirtualizer({ count: options.length + 1, getScrollElement: () => scroll.current, estimateSize: () => 20, overscan: 5 });
  useEffect(() => virtualizer.scrollToIndex(Math.max(0, index), { align: "auto" }), [index, virtualizer]);
  const chooseIndex = (next: number) => choose(next === 0 ? null : options[next - 1]?.key ?? null);
  const onKeyDown = (event: KeyboardEvent) => {
    let next = index;
    if (event.key === "ArrowDown") next = Math.min(options.length, index + 1);
    else if (event.key === "ArrowUp") next = Math.max(0, index - 1);
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = options.length;
    else if (event.key.length === 1 && !event.metaKey && !event.ctrlKey && event.key !== " ") {
      const found = options.findIndex((c, i) => i >= index && c.label.toLocaleLowerCase().startsWith(event.key.toLocaleLowerCase()));
      const wrapped = options.findIndex((c) => c.label.toLocaleLowerCase().startsWith(event.key.toLocaleLowerCase()));
      next = (found >= 0 ? found : wrapped) + 1;
      if (next === 0) return;
    } else return;
    event.preventDefault(); event.stopPropagation(); chooseIndex(next);
  };
  return (
    <section className="browser-facet" aria-label={`${label} filter`}>
      <h2>{label}</h2>
      <div className="browser-facet-list" ref={scroll} role="listbox" aria-label={label} tabIndex={0}
        aria-activedescendant={virtualizer.getVirtualItems().some(row => row.index === index) ? `browser-${label}-${index}` : undefined} onKeyDown={onKeyDown}>
        <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
          {virtualizer.getVirtualItems().map((row) => {
            const choice = row.index === 0 ? null : options[row.index - 1];
            const title = choice?.label ?? `All (${choices.length.toLocaleString()})`;
            return <div key={choice ? `choice:${choice.key}` : "all"} id={`browser-${label}-${row.index}`} role="option" aria-selected={row.index === index}
              title={title} className="browser-choice" style={{ transform: `translateY(${row.start}px)` }} onClick={() => { scroll.current?.focus({ preventScroll: true }); chooseIndex(row.index); }}>
              <span>{title}</span>{choice && <small aria-label={`${choice.count} songs`}>{choice.count.toLocaleString()}</small>}
            </div>;
          })}
        </div>
      </div>
    </section>
  );
}

export function ColumnBrowser() {
  const visible = useLibraryViews((s) => s.prefs.browsing.browserVisible);
  const facets = useApp((s) => s.browserFacets);
  const query = useApp((s) => s.query);
  const { setBrowserFilter, toggleColumnBrowser, clearBrowser } = useApp.getState();
  return <div className="column-browser-shell">
    <div className="browser-control-bar">
      <button className="push-button" aria-expanded={visible} aria-controls="column-browser" onClick={toggleColumnBrowser}>{visible ? "Hide Column Browser" : "Show Column Browser"}</button>
      {hasBrowseFilters(query) && <button className="push-button" onClick={clearBrowser}>Clear Filters</button>}
      <button className="push-button browser-view-options" onClick={() => useLibraryViews.getState().setOptionsOpen(true)}>View Options…</button>
    </div>
    {visible && <div className="column-browser" id="column-browser" aria-label="Column Browser">
      <Facet label="Genre" selected={query.genre ?? null} choose={(value) => setBrowserFilter("genre", value)} choices={facets.genres.map((g) => ({ key: g.key, label: g.name || "Unknown Genre", count: g.trackCount }))} />
      <Facet label="Artist" selected={query.artistKey ?? null} choose={(value) => setBrowserFilter("artistKey", value)} choices={facets.artists.map((a) => ({ key: a.key, label: a.name || "Unknown Artist", count: a.trackCount }))} />
      <Facet label="Album" selected={query.albumKey ?? null} choose={(value) => setBrowserFilter("albumKey", value)} choices={facets.albums.map((a) => ({ key: a.key, label: `${a.album || "Unknown Album"}${a.albumArtist ? ` — ${a.albumArtist}` : ""}`, count: a.trackCount }))} />
    </div>}
  </div>;
}
