import { useEffect, useMemo, useRef, useState } from "react";
import { allowedColumn, BUILTIN_LAYOUTS, COLUMN_SPECS, layoutForSource, type ColumnId } from "../preferences/libraryViews";
import { playlistIdOf, useApp } from "../state/store";
import "./ViewOptions.css";

export function ViewOptions() {
  const source = useApp((s) => s.source);
  const prefs = useApp((s) => s.viewPrefs);
  const playlists = useApp((s) => s.playlists);
  const query = useApp((s) => s.query);
  const { setColumnLayout, restoreColumnLayout, saveColumnPreset, deleteColumnPreset, applyLayoutToPlaylists } = useApp.getState();
  const regular = playlists.some((p) => p.id === playlistIdOf(source) && p.kind === "regular");
  const layout = useMemo(() => layoutForSource(prefs, source, regular), [prefs, source, regular]);
  const [search, setSearch] = useState("");
  const [presetName, setPresetName] = useState("");
  const [selectedPreset, setSelectedPreset] = useState("");
  const [applying, setApplying] = useState(false);
  const [targets, setTargets] = useState<Set<number>>(new Set());
  const [message, setMessage] = useState("");
  const sheet = useRef<HTMLDivElement>(null);
  const close = () => useApp.setState({ viewOptionsOpen: false });
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    sheet.current?.querySelector<HTMLInputElement>("input[type=search]")?.focus();
    return () => { if (previous?.isConnected) previous.focus(); };
  }, []);
  const patchVisible = (id: ColumnId, checked: boolean) => setColumnLayout({ ...layout, visible: checked ? [...layout.visible, id] : layout.visible.filter((c) => c !== id) });
  const move = (id: ColumnId, direction: number) => {
    const order = [...layout.order];
    const at = order.indexOf(id);
    const to = at + direction;
    if (to < 0 || to >= order.length) return;
    [order[at], order[to]] = [order[to], order[at]];
    setColumnLayout({ ...layout, order });
  };
  const columns = layout.order.map((id) => COLUMN_SPECS.find((c) => c.id === id)!).filter((c) => allowedColumn(c.id, source) && c.title.toLowerCase().includes(search.toLowerCase()));
  return <div className="sheet-backdrop">
    <div ref={sheet} className="sheet view-options-sheet" role="dialog" aria-modal="true" aria-labelledby="view-options-title"
      onKeyDown={(e) => {
        if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(); }
        if (e.key !== "Tab") return;
        const controls = Array.from(sheet.current!.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled), select:not(:disabled), [tabindex='0']"));
        const first = controls[0]; const last = controls[controls.length - 1];
        if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last?.focus(); }
        else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first?.focus(); }
      }}>
      <h2 id="view-options-title">View Options</h2>
      <p>{source.startsWith("playlist:") ? `Columns for ${playlists.find((p) => p.id === playlistIdOf(source))?.name ?? "this playlist"}${prefs.overrides[source] ? "" : " (using library default)"}.` : /^(shared|device):/.test(source) ? "Columns for this source. Songs remain read-only." : "Default song columns for the library and playlists without an override."}</p>
      <div className="view-options-presets">
        <label>Layout <select className="smart-popup" aria-label="Column layout preset" value={selectedPreset} onChange={(e) => {
          const value = e.target.value; setSelectedPreset(value);
          const selected = value.startsWith("builtin:") ? BUILTIN_LAYOUTS[value.slice(8)] : prefs.presets[value.slice(7)];
          if (selected) setColumnLayout(selected);
        }}>
          <option value="">Current layout</option>
          <optgroup label="Included">{Object.keys(BUILTIN_LAYOUTS).map((name) => <option key={name} value={`builtin:${name}`}>{name}</option>)}</optgroup>
          {Object.keys(prefs.presets).length > 0 && <optgroup label="Saved layouts">{Object.keys(prefs.presets).map((name) => <option key={name} value={`custom:${name}`}>{name}</option>)}</optgroup>}
        </select></label>
        {selectedPreset.startsWith("custom:") && <button className="push-button" onClick={() => { deleteColumnPreset(selectedPreset.slice(7)); setSelectedPreset(""); }}>Delete Layout</button>}
      </div>
      <input type="search" className="view-options-search" aria-label="Search columns" placeholder="Search columns" value={search} onChange={(e) => setSearch(e.target.value)} />
      <div className="view-options-columns" aria-label="Song columns">
        {columns.map((column) => <div className="view-options-column" key={column.id}>
          <label><input type="checkbox" checked={layout.visible.includes(column.id)} disabled={column.id === "name"} onChange={(e) => patchVisible(column.id, e.target.checked)} />{column.title}</label>
          <input type="number" min={column.id === "enabled" ? 22 : 28} max={900} step={1} aria-label={`${column.title} width`} value={layout.widths[column.id] ?? column.width}
            onChange={(e) => { const width = e.target.valueAsNumber; if (Number.isFinite(width)) setColumnLayout({ ...layout, widths: { ...layout.widths, [column.id]: width } }); }} />
          <span className="view-width-unit">px</span>
          <button className="view-move-button" aria-label={`Move ${column.title} left`} disabled={layout.order.indexOf(column.id) === 0} onClick={() => move(column.id, -1)}>←</button>
          <button className="view-move-button" aria-label={`Move ${column.title} right`} disabled={layout.order.indexOf(column.id) === layout.order.length - 1} onClick={() => move(column.id, 1)}>→</button>
        </div>)}
        {columns.length === 0 && <p>No matching columns.</p>}
      </div>
      <div className="view-options-save"><input aria-label="New layout name" placeholder="Layout name" maxLength={80} value={presetName} onChange={(e) => setPresetName(e.target.value)} />
        <button className="push-button" disabled={!presetName.trim()} onClick={() => { saveColumnPreset(presetName); setSelectedPreset(`custom:${presetName.trim()}`); setMessage(`Saved “${presetName.trim()}”.`); setPresetName(""); }}>Save Layout</button>
      </div>
      <button className="browser-text-button" aria-expanded={applying} onClick={() => setApplying(!applying)}>Apply Layout to Other Playlists…</button>
      {applying && <div className="view-options-apply">
        <p>Choose playlists to receive these columns, widths and sorting.</p>
        <div className="view-options-playlists">{playlists.filter((p) => p.kind !== "folder" && p.id !== playlistIdOf(source)).map((playlist) => <label key={playlist.id}>
          <input type="checkbox" checked={targets.has(playlist.id)} onChange={(e) => setTargets((before) => { const next = new Set(before); if (e.target.checked) next.add(playlist.id); else next.delete(playlist.id); return next; })} />{playlist.name}
        </label>)}</div>
        {playlists.filter((p) => p.kind !== "folder" && p.id !== playlistIdOf(source)).length === 0 && <p>No other playlists.</p>}
        <button className="push-button" disabled={targets.size === 0} onClick={() => { applyLayoutToPlaylists([...targets]); setMessage(`Applied to ${targets.size} ${targets.size === 1 ? "playlist" : "playlists"}.`); setTargets(new Set()); setApplying(false); }}>Apply to Selected Playlists</button>
      </div>}
      <p className="view-options-message" role="status">{message || "Changes are saved automatically. Name always stays visible."}</p>
      <div className="view-options-footer">
        <button className="push-button" onClick={() => { restoreColumnLayout(); setSelectedPreset(""); }}>Restore Defaults</button>
        <span className="view-options-sort">Sort: {query.sort === "playlistOrder" ? "Playlist Order" : COLUMN_SPECS.find((c) => "sort" in c && c.sort === query.sort)?.title} {query.descending ? "↓" : "↑"}</span>
        <button className="push-button push-button--default" onClick={close}>Done</button>
      </div>
    </div>
  </div>;
}
