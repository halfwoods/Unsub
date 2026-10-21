import { useMemo, useState } from "react";
import { libraryCreatePlaylist, libraryRenamePlaylist, librarySetSmartRules } from "../api/commands";
import type { SmartOp, SmartRule, SmartRules, SmartValue } from "../api/types";
import { playlistSource, useApp } from "../state/store";
import {
  DATE_UNITS,
  dateToInput,
  defaultValue,
  FIELDS,
  fieldInfo,
  fromDisplay,
  inputToDate,
  LIMIT_ORDERS,
  LIMIT_UNITS,
  newCondition,
  OP_LABELS,
  OPS,
  toDisplay,
  unitSuffix,
} from "./fields";
import "./SmartPlaylistEditor.css";

type Condition = Extract<SmartRule, { type: "condition" }>;

function NumberInput({ value, onChange, unit }: { value: number; onChange: (n: number) => void; unit: Parameters<typeof toDisplay>[0] }) {
  return (
    <input
      className="smart-input smart-input--number"
      type="number"
      step={unit === "rating" ? 1 : "any"}
      min={unit === "rating" ? 0 : undefined}
      max={unit === "rating" ? 5 : undefined}
      value={toDisplay(unit, value)}
      onChange={(e) => onChange(fromDisplay(unit, Number(e.target.value)))}
    />
  );
}

function ValueEditor({ rule, onChange }: { rule: Condition; onChange: (value: SmartValue) => void }) {
  const field = fieldInfo(rule.field);
  const playlists = useApp((s) => s.playlists);
  const editingId = useApp((s) => s.smartEditor?.id ?? null);
  const value = rule.value;
  const suffix = unitSuffix(field.unit);

  if (field.type === "bool") return null;
  if (field.type === "text" && typeof value === "object" && "text" in value) {
    return <input className="smart-input" value={value.text} onChange={(e) => onChange({ text: e.target.value })} />;
  }
  if (field.type === "playlist" && typeof value === "object" && "playlist" in value) {
    return (
      <select className="smart-popup" value={value.playlist} onChange={(e) => onChange({ playlist: Number(e.target.value) })}>
        {playlists
          .filter((p) => p.id !== editingId && p.kind !== "folder")
          .map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
      </select>
    );
  }
  if (typeof value === "object" && "relative" in value) {
    return (
      <>
        <input
          className="smart-input smart-input--number"
          type="number"
          min={1}
          value={value.relative.amount}
          onChange={(e) => onChange({ relative: { ...value.relative, amount: Math.max(1, Number(e.target.value)) } })}
        />
        <select className="smart-popup" value={value.relative.unit} onChange={(e) => onChange({ relative: { ...value.relative, unit: e.target.value as typeof value.relative.unit } })}>
          {DATE_UNITS.map((u) => (
            <option key={u.id} value={u.id}>
              {u.label}
            </option>
          ))}
        </select>
      </>
    );
  }
  if (typeof value === "object" && "range" in value) {
    const { from, to } = value.range;
    if (field.type === "date") {
      return (
        <>
          <input className="smart-input" type="date" value={dateToInput(from)} onChange={(e) => onChange({ range: { from: inputToDate(e.target.value), to } })} />
          <span className="smart-to">to</span>
          <input className="smart-input" type="date" value={dateToInput(to)} onChange={(e) => onChange({ range: { from, to: inputToDate(e.target.value) } })} />
        </>
      );
    }
    return (
      <>
        <NumberInput unit={field.unit} value={from} onChange={(n) => onChange({ range: { from: n, to } })} />
        <span className="smart-to">to</span>
        <NumberInput unit={field.unit} value={to} onChange={(n) => onChange({ range: { from, to: n } })} />
        {suffix && <span className="smart-suffix">{suffix}</span>}
      </>
    );
  }
  if (typeof value === "object" && "number" in value) {
    if (field.type === "date") {
      return <input className="smart-input" type="date" value={dateToInput(value.number)} onChange={(e) => onChange({ number: inputToDate(e.target.value) })} />;
    }
    return (
      <>
        <NumberInput unit={field.unit} value={value.number} onChange={(n) => onChange({ number: n })} />
        {suffix && <span className="smart-suffix">{suffix}</span>}
      </>
    );
  }
  return null;
}

function RuleList({ rules, onChange, depth }: { rules: SmartRules; onChange: (rules: SmartRules) => void; depth: number }) {
  const playlists = useApp((s) => s.playlists);
  const playlistIds = useMemo(() => playlists.filter((p) => p.kind !== "folder").map((p) => p.id), [playlists]);
  const setRule = (index: number, rule: SmartRule) => onChange({ ...rules, rules: rules.rules.map((r, i) => (i === index ? rule : r)) });
  const insertAfter = (index: number, rule: SmartRule) =>
    onChange({ ...rules, rules: [...rules.rules.slice(0, index + 1), rule, ...rules.rules.slice(index + 1)] });
  const remove = (index: number) => onChange({ ...rules, rules: rules.rules.filter((_, i) => i !== index) });
  const canRemove = rules.rules.length > 1 || depth > 0;

  return (
    <div className={depth > 0 ? "smart-group" : undefined}>
      {(depth > 0 || rules.rules.length > 1) && (
        <div className="smart-row smart-match">
          Match
          <select className="smart-popup" value={rules.matchAll ? "all" : "any"} onChange={(e) => onChange({ ...rules, matchAll: e.target.value === "all" })}>
            <option value="all">all</option>
            <option value="any">any</option>
          </select>
          of the following rules:
        </div>
      )}
      {rules.rules.map((rule, index) => (
        <div key={index}>
          {rule.type === "group" ? (
            <div className="smart-row smart-row--group">
              <RuleList depth={depth + 1} rules={rule} onChange={(group) => setRule(index, { ...group, type: "group" })} />
              <div className="smart-row-buttons">
                <button className="smart-round" aria-label="Remove group" onClick={() => remove(index)}>
                  −
                </button>
              </div>
            </div>
          ) : (
            <div className="smart-row">
              <select
                className="smart-popup"
                value={rule.field}
                onChange={(e) => {
                  const field = fieldInfo(e.target.value as Condition["field"]);
                  const op = OPS[field.type].includes(rule.op) ? rule.op : OPS[field.type][0];
                  setRule(index, { type: "condition", field: field.id, op, value: defaultValue(field, op, playlistIds) });
                }}
              >
                {FIELDS.map((f) => (
                  <option key={f.id} value={f.id}>
                    {f.label}
                  </option>
                ))}
              </select>
              <select
                className="smart-popup"
                value={rule.op}
                onChange={(e) => {
                  const op = e.target.value as SmartOp;
                  setRule(index, { ...rule, op, value: defaultValue(fieldInfo(rule.field), op, playlistIds) });
                }}
              >
                {OPS[fieldInfo(rule.field).type].map((op) => (
                  <option key={op} value={op}>
                    {OP_LABELS[op]}
                  </option>
                ))}
              </select>
              <ValueEditor rule={rule} onChange={(value) => setRule(index, { ...rule, value })} />
              <div className="smart-row-buttons">
                <button className="smart-round" aria-label="Remove rule" disabled={!canRemove} onClick={() => remove(index)}>
                  −
                </button>
                <button className="smart-round" aria-label="Add rule" onClick={() => insertAfter(index, newCondition(playlistIds))}>
                  +
                </button>
                {/* iTunes' option-click "…" adds a nested group of rules. */}
                <button
                  className="smart-round"
                  aria-label="Add nested rules"
                  onClick={() =>
                    insertAfter(index, { type: "group", matchAll: true, rules: [newCondition(playlistIds)], limit: null, onlyChecked: false, live: true })
                  }
                >
                  …
                </button>
              </div>
            </div>
          )}
        </div>
      ))}
    </div>
  );
}

/** iTunes 10's Smart Playlist sheet. */
export function SmartPlaylistEditor() {
  const editor = useApp((s) => s.smartEditor);
  const { openSmartEditor, refreshPlaylists, refresh, setSource, setRenamingPlaylist } = useApp.getState();
  const [rules, setRules] = useState<SmartRules>(() => editor?.rules ?? { matchAll: true, rules: [], limit: null, onlyChecked: false, live: true });
  const [name, setName] = useState(editor?.name ?? "");
  const [error, setError] = useState<string | null>(null);
  if (!editor) return null;

  const limit = rules.limit;
  const save = async () => {
    try {
      if (editor.id === null) {
        const created = await libraryCreatePlaylist(editor.name, "smart", null);
        await librarySetSmartRules(created.id, rules);
        await refreshPlaylists();
        setSource(playlistSource(created.id));
        setRenamingPlaylist(created.id);
      } else {
        await librarySetSmartRules(editor.id, rules);
        const renamed = name.trim();
        if (renamed && renamed !== editor.name) await libraryRenamePlaylist(editor.id, renamed);
        await Promise.all([refreshPlaylists(), refresh()]);
      }
      openSmartEditor(null);
    } catch (e) {
      // The library validates field/operator/value combinations; show its reason rather than closing.
      setError(String(e));
    }
  };

  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && openSmartEditor(null)}>
      <div className="sheet smart-sheet" role="dialog" aria-label="Smart Playlist">
        {/* A new playlist is named in the sidebar once it exists, as in iTunes. */}
        {editor.id !== null && (
          <label className="smart-row">
            Name:
            <input className="smart-input" value={name} onChange={(e) => setName(e.target.value)} />
          </label>
        )}
        <RuleList depth={0} rules={rules} onChange={setRules} />
        <div className="smart-options">
          <label className="smart-row">
            <input
              type="checkbox"
              checked={limit !== null}
              onChange={(e) => setRules({ ...rules, limit: e.target.checked ? { amount: 25, unit: "items", selectedBy: "random" } : null })}
            />
            Limit to
            <input
              className="smart-input smart-input--number"
              type="number"
              min={1}
              disabled={!limit}
              value={limit?.amount ?? 25}
              onChange={(e) => limit && setRules({ ...rules, limit: { ...limit, amount: Math.max(1, Number(e.target.value)) } })}
            />
            <select className="smart-popup" disabled={!limit} value={limit?.unit ?? "items"} onChange={(e) => limit && setRules({ ...rules, limit: { ...limit, unit: e.target.value as typeof limit.unit } })}>
              {LIMIT_UNITS.map((u) => (
                <option key={u.id} value={u.id}>
                  {u.label}
                </option>
              ))}
            </select>
            selected by
            <select
              className="smart-popup"
              disabled={!limit}
              value={limit?.selectedBy ?? "random"}
              onChange={(e) => limit && setRules({ ...rules, limit: { ...limit, selectedBy: e.target.value as typeof limit.selectedBy } })}
            >
              {LIMIT_ORDERS.map((o) => (
                <option key={o.id} value={o.id}>
                  {o.label}
                </option>
              ))}
            </select>
          </label>
          <label className="smart-row">
            <input type="checkbox" checked={rules.onlyChecked} onChange={(e) => setRules({ ...rules, onlyChecked: e.target.checked })} />
            Match only checked items
          </label>
          <label className="smart-row">
            <input type="checkbox" checked={rules.live} onChange={(e) => setRules({ ...rules, live: e.target.checked })} />
            Live updating
          </label>
        </div>
        {error && <div className="smart-error">{error}</div>}
        <div className="sheet-buttons">
          <button className="push-button" onClick={() => openSmartEditor(null)}>
            Cancel
          </button>
          <button className="push-button push-button--default" onClick={() => void save()}>
            OK
          </button>
        </div>
      </div>
    </div>
  );
}
