import { useEffect, useId, useMemo, useRef, useState, type ReactNode } from "react";
import { libraryCleanUp, libraryCleanUpPreview } from "../api/commands";
import type { Capitalization, CleanUpChange, CleanUpField, CleanUpPreview, CleanUpRules } from "../api/types";
import { useApp } from "../state/store";
import { pluralize } from "../util/format";
import "./CleanUpSheet.css";

/** Typing is followed by one preview, not one per key: the rules run over every song in the list. */
const PREVIEW_DELAY_MS = 150;

const FIELDS: { value: CleanUpField; label: string }[] = [
  { value: "title", label: "Name" },
  { value: "artist", label: "Artist" },
  { value: "album", label: "Album" },
  { value: "albumArtist", label: "Album Artist" },
  { value: "composer", label: "Composer" },
  { value: "genre", label: "Genre" },
];

const CAPITALIZATIONS: { value: Capitalization; label: string }[] = [
  { value: "title", label: "Title Case" },
  { value: "sentence", label: "Sentence case" },
  { value: "lower", label: "lowercase" },
  { value: "upper", label: "UPPERCASE" },
];

/** Every box and entry on the sheet, including those of operations that are switched off. */
interface Draft {
  field: CleanUpField;
  tidy: boolean;
  removeJunk: boolean;
  removeTrackNumbers: boolean;
  removeArtist: boolean;
  removeCharacters: boolean;
  count: string;
  from: "start" | "end";
  replace: boolean;
  find: string;
  with: string;
  matchCase: boolean;
  capitalize: boolean;
  style: Capitalization;
}

const INITIAL: Draft = {
  field: "title",
  tidy: false,
  removeJunk: false,
  removeTrackNumbers: false,
  removeArtist: false,
  removeCharacters: false,
  count: "1",
  from: "start",
  replace: false,
  find: "",
  with: "",
  matchCase: false,
  capitalize: false,
  style: "title",
};

function rulesFrom(draft: Draft): CleanUpRules {
  const count = Number(draft.count);
  // Track numbers and artists are only in names, so the other fields leave those boxes unused.
  const nameOnly = draft.field === "title";
  return {
    field: draft.field,
    tidy: draft.tidy,
    removeJunk: draft.removeJunk,
    removeTrackNumbers: nameOnly && draft.removeTrackNumbers,
    removeArtist: nameOnly && draft.removeArtist,
    removeCharacters: draft.removeCharacters && count > 0 ? { count, from: draft.from } : null,
    replace: draft.replace ? { find: draft.find, with: draft.with, matchCase: draft.matchCase } : null,
    capitalize: draft.capitalize ? draft.style : null,
  };
}

function Operation({ label, on, disabled, hint, onChange, children }: { label: string; on: boolean; disabled?: boolean; hint?: string; onChange: (on: boolean) => void; children?: ReactNode }) {
  const id = useId();
  return (
    <div className={disabled ? "cleanup-row cleanup-row--off" : "cleanup-row"} title={disabled ? hint : undefined}>
      <input id={id} type="checkbox" checked={on && !disabled} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <label htmlFor={id}>{label}</label>
      {children}
    </div>
  );
}

/** What a change also fills in, which the song had no value for. */
const fills = (change: CleanUpChange) =>
  [
    change.artist !== null && `Artist: ${change.artist}`,
    change.discNumber !== null && `Disc ${change.discNumber}`,
    change.trackNumber !== null && `Track ${change.trackNumber}`,
  ]
    .filter(Boolean)
    .join(", ");

/** Advanced › Clean Up Song Info: tidies one field of the songs, showing each before and after first. */
export function CleanUpSheet({ trackIds }: { trackIds: number[] }) {
  const close = useApp((s) => s.openLibrarySheet);
  const showNotice = useApp((s) => s.showNotice);
  const [draft, setDraft] = useState(INITIAL);
  const [previewed, setPreviewed] = useState<{ rules: CleanUpRules; result: CleanUpPreview } | null>(null);
  const rules = useMemo(() => rulesFrom(draft), [draft]);
  const update = (change: Partial<Draft>) => setDraft((d) => ({ ...d, ...change }));
  const nameOnly = draft.field === "title";
  const preview = previewed?.result;
  // Apply waits for the list of what the rules in front of the user now would do.
  const shown = previewed?.rules === rules;

  const latest = useRef(rules);
  const asked = useRef<CleanUpRules | null>(null);
  const busy = useRef(false);
  const open = useRef(true);
  useEffect(() => {
    open.current = true;
    return () => {
      open.current = false;
    };
  }, []);

  useEffect(() => {
    latest.current = rules;
    const timer = setTimeout(function ask() {
      // One request at a time, since each runs over the whole list on the connection the song list uses too.
      // Whichever finishes asks again if the rules changed meanwhile, and only for the latest of them.
      if (busy.current || asked.current === latest.current) return;
      busy.current = true;
      const sent = latest.current;
      asked.current = sent;
      libraryCleanUpPreview(trackIds, sent)
        .then((result) => open.current && setPreviewed({ rules: sent, result }))
        .catch((e: unknown) => showNotice(String(e)))
        .finally(() => {
          busy.current = false;
          if (open.current) ask();
        });
    }, PREVIEW_DELAY_MS);
    return () => clearTimeout(timer);
  }, [trackIds, rules, showNotice]);

  // The work goes on in the background, with its progress on the LCD, so the sheet is done at once.
  const apply = () => {
    close(null);
    void libraryCleanUp(trackIds, rules).catch((e: unknown) => showNotice(String(e)));
  };

  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && close(null)}>
      <div className="sheet cleanup-sheet" role="dialog" aria-label="Clean Up Song Info">
        <p className="cleanup-title">Clean Up {pluralize(trackIds.length, "Song", "Songs")}</p>
        <div className="cleanup-field">
          <label htmlFor="cleanup-field">Change:</label>
          <select id="cleanup-field" className="smart-popup" value={draft.field} onChange={(e) => update({ field: e.target.value as CleanUpField })}>
            {FIELDS.map((f) => (
              <option key={f.value} value={f.value}>
                {f.label}
              </option>
            ))}
          </select>
        </div>
        <div className="cleanup-rows">
          <Operation label="Tidy up spaces and underscores" on={draft.tidy} onChange={(tidy) => update({ tidy })} />
          <Operation label="Remove tags like (HD) and web addresses" on={draft.removeJunk} onChange={(removeJunk) => update({ removeJunk })} />
          <Operation
            label="Remove track numbers from the start"
            on={draft.removeTrackNumbers}
            disabled={!nameOnly}
            hint="Only for Name"
            onChange={(removeTrackNumbers) => update({ removeTrackNumbers })}
          />
          <Operation label="Remove the artist from the start" on={draft.removeArtist} disabled={!nameOnly} hint="Only for Name" onChange={(removeArtist) => update({ removeArtist })} />
          <Operation label="Remove" on={draft.removeCharacters} onChange={(removeCharacters) => update({ removeCharacters })}>
            <input
              className="smart-input smart-input--number"
              aria-label="Number of characters"
              inputMode="numeric"
              value={draft.count}
              onChange={(e) => update({ count: e.target.value.replace(/\D/g, ""), removeCharacters: true })}
            />
            characters from the
            <select className="smart-popup" aria-label="Which end" value={draft.from} onChange={(e) => update({ from: e.target.value as Draft["from"], removeCharacters: true })}>
              <option value="start">start</option>
              <option value="end">end</option>
            </select>
          </Operation>
          <Operation label="Replace" on={draft.replace} onChange={(replace) => update({ replace })}>
            <input className="smart-input cleanup-text" aria-label="Text to find" value={draft.find} onChange={(e) => update({ find: e.target.value, replace: true })} />
            with
            <input className="smart-input cleanup-text" aria-label="Replacement" value={draft.with} onChange={(e) => update({ with: e.target.value, replace: true })} />
            <label className="prefs-check cleanup-match">
              <input type="checkbox" checked={draft.matchCase} onChange={(e) => update({ matchCase: e.target.checked })} />
              Match case
            </label>
          </Operation>
          <Operation label="Change capitalization to" on={draft.capitalize} onChange={(capitalize) => update({ capitalize })}>
            <select className="smart-popup" aria-label="Capitalization" value={draft.style} onChange={(e) => update({ style: e.target.value as Capitalization, capitalize: true })}>
              {CAPITALIZATIONS.map((c) => (
                <option key={c.value} value={c.value}>
                  {c.label}
                </option>
              ))}
            </select>
          </Operation>
        </div>
        <p className="cleanup-summary">
          {preview ? `${preview.total.toLocaleString()} of ${trackIds.length.toLocaleString()} songs will change` : "Checking…"}
          {preview && preview.total > preview.changes.length && ` (showing the first ${preview.changes.length.toLocaleString()})`}
        </p>
        {preview && preview.inCloud > 0 && (
          <p className="cleanup-note">
            {preview.inCloud === 1 ? "One of these songs is" : `${preview.inCloud.toLocaleString()} of these songs are`} only in iCloud. Unsub keeps
            the new info, and a later Import Music Library won't replace it.
          </p>
        )}
        <ul className="cleanup-list" aria-label="Changes">
          {preview?.changes.map((change) => (
            <li key={change.id}>
              <span className="cleanup-before" title={change.before}>
                {change.before}
              </span>
              <span className="cleanup-arrow" role="img" aria-label="becomes">
                →
              </span>
              <span className="cleanup-after" title={change.after}>
                {change.after}
                {fills(change) && <small>{fills(change)}</small>}
              </span>
            </li>
          ))}
        </ul>
        <div className="sheet-buttons">
          <button className="push-button" onClick={() => close(null)}>
            Cancel
          </button>
          <button className="push-button push-button--default" disabled={!shown || !preview || preview.total === 0} onClick={apply}>
            Apply
          </button>
        </div>
      </div>
    </div>
  );
}
