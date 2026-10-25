import { useEffect, useRef, useState } from "react";
import { useApp, playlistIdOf } from "../state/store";
import { useLibraryViews } from "../preferences/libraryViewStore";
import { BUILT_IN_PRESETS, columnWidth, isLayoutSource, layoutFor, type ColumnId } from "../preferences/libraryViews";
import { columnLabel } from "./columns";
import "./ViewOptions.css";

export function ViewOptions() {
  const open = useLibraryViews(s => s.optionsOpen), prefs = useLibraryViews(s => s.prefs);
  const close = useLibraryViews(s => s.setOptionsOpen);
  const source = useApp(s => s.source), playlists = useApp(s => s.playlists);
  const actions = useLibraryViews.getState(), dialog = useRef<HTMLDivElement>(null), searchRef = useRef<HTMLInputElement>(null);
  const [search, setSearch] = useState(""), [presetName, setPresetName] = useState(""), [selectedPreset, setSelectedPreset] = useState("");
  const [copying, setCopying] = useState(false), [targets, setTargets] = useState<Set<number>>(new Set()), [message, setMessage] = useState("");
  const playlist = playlistIdOf(source), regular = playlists.some(p => p.id === playlist && p.kind === "regular");
  const layout = layoutFor(prefs, source, regular), editable = isLayoutSource(source);
  useEffect(() => {
    if (!open) return;
    const previous = document.activeElement as HTMLElement | null;
    (searchRef.current ?? dialog.current?.querySelector<HTMLButtonElement>("button"))?.focus();
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(false); }
      if (event.key !== "Tab") return;
      const controls = Array.from(dialog.current?.querySelectorAll<HTMLElement>('button:not(:disabled),input:not(:disabled),select:not(:disabled),[tabindex="0"]') ?? []).filter(el => el.getClientRects().length > 0);
      const first = controls[0], last = controls[controls.length - 1];
      if (!dialog.current?.contains(document.activeElement)) { event.preventDefault(); (event.shiftKey ? last : first)?.focus(); }
      else if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    };
    document.addEventListener("keydown", onKey, true);
    return () => { document.removeEventListener("keydown", onKey, true); previous?.focus(); };
  }, [open, close]);
  useEffect(() => { setSearch(""); setCopying(false); setTargets(new Set()); setMessage(""); setSelectedPreset(""); }, [source, open]);
  if (!open) return null;
  const update = (changes: Parameters<typeof actions.updateLayout>[1]) => actions.updateLayout(source, changes, regular);
  const move = (id: ColumnId, direction: number) => {
    const order = [...layout.order], index = order.indexOf(id), to = index + direction;
    if (to < 0 || to >= order.length) return;
    [order[index], order[to]] = [order[to], order[index]]; update({ order });
  };
  const eligible = playlists.filter(p => p.kind !== "folder" && p.id !== playlist);
  const presets = [...BUILT_IN_PRESETS, ...prefs.customPresets];
  const shown = layout.order.filter(id => columnLabel(id).toLowerCase().includes(search.trim().toLowerCase()));
  return <div className="sheet-backdrop" onMouseDown={event => { if (event.target === event.currentTarget) close(false); }}>
    <div ref={dialog} className="sheet view-options" role="dialog" aria-modal="true" aria-labelledby="view-options-title">
      <h2 id="view-options-title">Song List View Options</h2>
      {!editable ? <p>This source keeps its own song columns. Select Music or a playlist to customize a library view.</p> : <>
        <p className="view-options-scope">{playlist === null ? "Library default — inherited by playlists without their own layout." : prefs.playlists[String(playlist)] ? "This playlist has its own layout." : "This playlist inherits the library columns. Changes create its own layout."}</p>
        <label className="view-options-search">Find a column<input ref={searchRef} type="search" value={search} onChange={event => setSearch(event.target.value)} placeholder="Search columns" /></label>
        <div className="view-options-columns" aria-label="Song columns">
          {shown.map(id => <div className="view-options-column" key={id}>
            <label><input type="checkbox" checked={layout.visible.includes(id)} disabled={id === "name"}
              onChange={event => update({ visible: event.target.checked ? [...layout.visible, id] : layout.visible.filter(key => key !== id) })} />{columnLabel(id)}</label>
            <button className="push-button" aria-label={`Move ${columnLabel(id)} left`} disabled={layout.order.indexOf(id) === 0} onClick={() => move(id, -1)}>←</button>
            <button className="push-button" aria-label={`Move ${columnLabel(id)} right`} disabled={layout.order.indexOf(id) === layout.order.length - 1} onClick={() => move(id, 1)}>→</button>
            <input key={`${id}-${layout.widths[id]}`} type="number" min={id === "enabled" ? 22 : 40} max={800} defaultValue={columnWidth(id, layout.widths[id])}
              aria-label={`${columnLabel(id)} width in pixels`} onBlur={event => {
                const value = Number(event.target.value);
                if (event.target.value && Number.isFinite(value)) update({ widths: { ...layout.widths, [id]: columnWidth(id, value) } });
                else event.target.value = String(columnWidth(id, layout.widths[id]));
              }} onKeyDown={event => { if (event.key === "Enter") event.currentTarget.blur(); }} />
          </div>)}
          {shown.length === 0 && <p>No matching columns.</p>}
        </div>
        <p className="view-options-hint">Drag headings to reorder. Drag a divider to resize or double-click to fit. Name always stays visible.</p>
        <div className="view-options-presets"><label>Layout<select value={selectedPreset} onChange={event => {
          setSelectedPreset(event.target.value); const item = event.target.value === "" ? undefined : presets[Number(event.target.value)]; if (item) update(item.layout);
        }}><option value="">Choose a preset…</option>{presets.map((item, index) => <option key={`${index}-${item.name}`} value={index}>{item.name}</option>)}</select></label>
          {Number(selectedPreset) >= BUILT_IN_PRESETS.length && selectedPreset !== "" && <button className="push-button" onClick={() => {
            const item = presets[Number(selectedPreset)]; if (item) actions.removePreset(item.name); setSelectedPreset(""); dialog.current?.querySelector<HTMLSelectElement>("select")?.focus();
          }}>Delete Saved Layout</button>}
        </div>
        <div className="view-options-save"><input aria-label="New layout name" placeholder="Name this layout" maxLength={80} value={presetName} onChange={event => setPresetName(event.target.value)} />
          <button className="push-button" disabled={!presetName.trim()} onClick={() => { actions.savePreset(presetName, layout); setMessage(`Saved “${presetName.trim()}”.`); setPresetName(""); }}>Save Layout</button></div>
        <div className="view-options-actions">
          <button className="push-button" onClick={() => { actions.restore(source); setSelectedPreset(""); }}>Restore Defaults</button>
          {playlist !== null && <button className="push-button" onClick={() => { actions.makeLibraryDefault(layout); setMessage("Library default updated."); }}>Use as Library Default</button>}
          <button className="push-button" data-copy-layout-toggle disabled={eligible.length === 0} onClick={() => setCopying(!copying)} aria-expanded={copying}>Apply to Other Playlists…</button>
        </div>
        {copying && <fieldset className="view-options-copy"><legend>Apply current layout to selected playlists</legend><div>{eligible.map(item => <label key={item.id}>
          <input type="checkbox" checked={targets.has(item.id)} onChange={event => setTargets(current => { const next = new Set(current); if (event.target.checked) next.add(item.id); else next.delete(item.id); return next; })} />
          <span title={item.name}>{item.name}</span></label>)}</div>
          <button className="push-button" disabled={targets.size === 0} onClick={() => { actions.copyToPlaylists(layout, [...targets]); setMessage(`Applied to ${targets.size} playlists.`); setCopying(false); setTargets(new Set()); dialog.current?.querySelector<HTMLButtonElement>("[data-copy-layout-toggle]")?.focus(); }}>Apply Layout</button>
        </fieldset>}
        <p className="view-options-status" role="status">{message}</p>
      </>}
      <div className="sheet-buttons"><button className="push-button push-button--default" onClick={() => close(false)}>Done</button></div>
    </div>
  </div>;
}
