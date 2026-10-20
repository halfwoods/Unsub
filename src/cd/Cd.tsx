import { useState } from "react";
import { cdChooseRelease, cdImport, cdStopImport } from "../api/commands";
import type { AccurateRipResult, ImportFormat } from "../api/types";
import { cdIdOf, useApp } from "../state/store";
import { loadImportPrefs, saveImportPrefs, type ImportPrefs } from "./importPrefs";
import "./Cd.css";

const ENCODERS: { label: string; codec: ImportFormat["codec"]; details: string }[] = [
  {
    label: "FLAC Encoder",
    codec: "flac",
    details: "Lossless and open: every bit of the CD, in about half the space. Plays almost everywhere except older Apple devices.",
  },
  {
    label: "Apple Lossless Encoder",
    codec: "alac",
    details: "Lossless, in the format iPods and Apple devices play natively.",
  },
  {
    label: "AAC Encoder",
    codec: "aac",
    details: "Much smaller files for portable players; some detail you may not hear is discarded.",
  },
];

const AAC_SETTINGS = [
  { label: "Good Quality (128 kbps)", kbps: 128 },
  { label: "High Quality (256 kbps)", kbps: 256 },
  { label: "Maximum Quality (320 kbps)", kbps: 320 },
];

function ImportSettingsSheet({ onClose }: { onClose: () => void }) {
  const [prefs, setPrefs] = useState<ImportPrefs>(loadImportPrefs);
  const encoder = ENCODERS.find((e) => e.codec === prefs.format.codec) ?? ENCODERS[0];
  const kbps = prefs.format.codec === "aac" ? prefs.format.kbps : 256;
  const setCodec = (codec: ImportFormat["codec"]) =>
    setPrefs({ ...prefs, format: codec === "aac" ? { codec, kbps } : { codec } });
  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="sheet import-sheet" role="dialog" aria-label="Import Settings">
        <div className="prefs-grid">
          <span className="prefs-label">Import Using:</span>
          <select className="smart-popup" value={encoder.codec} onChange={(e) => setCodec(e.target.value as ImportFormat["codec"])}>
            {ENCODERS.map((e) => (
              <option key={e.codec} value={e.codec}>
                {e.label}
              </option>
            ))}
          </select>
          {prefs.format.codec === "aac" && (
            <>
              <span className="prefs-label">Setting:</span>
              <select
                className="smart-popup"
                value={kbps}
                onChange={(e) => setPrefs({ ...prefs, format: { codec: "aac", kbps: Number(e.target.value) } })}
              >
                {AAC_SETTINGS.map((s) => (
                  <option key={s.kbps} value={s.kbps}>
                    {s.label}
                  </option>
                ))}
              </select>
            </>
          )}
          <span className="prefs-label">Details:</span>
          <p className="import-details">{encoder.details}</p>
        </div>
        <label className="prefs-check">
          <input type="checkbox" checked={prefs.verify} onChange={(e) => setPrefs({ ...prefs, verify: e.target.checked })} />
          Verify songs with AccurateRip
        </label>
        <p className="prefs-note">
          Compares each song with other people's rips of the same CD, to confirm it was read without errors.
        </p>
        <div className="sheet-buttons">
          <button className="push-button" onClick={onClose}>
            Cancel
          </button>
          <button
            className="push-button push-button--default"
            onClick={() => {
              saveImportPrefs(prefs);
              onClose();
            }}
          >
            OK
          </button>
        </div>
      </div>
    </div>
  );
}

/** iTunes 10's CD controls at the bottom right: which album it is, Import Settings…, Import CD. */
export function CdActions({ discId }: { discId: string }) {
  const cd = useApp((s) => s.cds.find((c) => c.id === discId));
  const importing = useApp((s) => s.cdRip !== null);
  const showNotice = useApp((s) => s.showNotice);
  const [settingsOpen, setSettingsOpen] = useState(false);
  if (!cd) return null;

  const start = () => {
    const { format, verify } = loadImportPrefs();
    void cdImport(cd.id, format, verify).catch((e: unknown) => showNotice(String(e)));
  };

  return (
    <div className="bottom-right">
      {cd.lookup.state === "pending" && <span className="cd-lookup">Looking up CD…</span>}
      {cd.lookup.state === "notFound" && <span className="cd-lookup">MusicBrainz doesn't know this CD</span>}
      {cd.lookup.state === "failed" && <span className="cd-lookup" title={cd.lookup.message}>Couldn't look up this CD</span>}
      {cd.releases.length > 1 && (
        // Reissues and regional editions can share one disc; iTunes asked which one you have.
        <select
          className="smart-popup cd-release"
          aria-label="Edition"
          value={cd.chosen}
          onChange={(e) => void cdChooseRelease(cd.id, Number(e.target.value))}
        >
          {cd.releases.map((r, i) => (
            <option key={i} value={i}>
              {[r.title, r.year, r.discCount > 1 ? `disc ${r.discNumber} of ${r.discCount}` : null].filter(Boolean).join(" · ")}
            </option>
          ))}
        </select>
      )}
      <button className="push-button bottom-push" onClick={() => setSettingsOpen(true)}>
        Import Settings…
      </button>
      {cd.importing ? (
        <button className="push-button bottom-push" onClick={() => void cdStopImport()}>
          Stop Importing
        </button>
      ) : (
        <button className="push-button bottom-push" disabled={importing} onClick={start}>
          Import CD
        </button>
      )}
      {settingsOpen && <ImportSettingsSheet onClose={() => setSettingsOpen(false)} />}
    </div>
  );
}

function describeVerdict(verdict: AccurateRipResult) {
  switch (verdict.result) {
    case "matched":
      return `Imported and verified: it matches ${verdict.confidence} other ${verdict.confidence === 1 ? "rip" : "rips"} on AccurateRip.`;
    case "notInDatabase":
      return "Imported. AccurateRip has no rips of this CD to compare with yet.";
    case "mismatch":
      return "Imported, but it doesn't match AccurateRip. That's normal for drives with a read offset; otherwise a scratch may have caused read errors.";
    case "skipped":
      return "Imported (not verified).";
  }
}

/** The status column during and after an import: orange while reading, green once done. */
export function CdImportStatus({ number }: { number: number }) {
  const importing = useApp((s) => s.cdRip !== null && s.cdRip.discId === cdIdOf(s.source) && s.cdRip.current === number);
  const verdict = useApp((s) => s.cds.find((cd) => cd.id === cdIdOf(s.source))?.imported.find((t) => t.number === number)?.accuraterip);
  if (importing) {
    return (
      <svg className="cd-status cd-status--importing" width={12} height={12} viewBox="0 0 12 12" aria-label="Importing">
        <circle cx="6" cy="6" r="6" />
        <path d="M3.2 6a2.8 2.8 0 0 1 4.9-1.9M8.8 6a2.8 2.8 0 0 1-4.9 1.9M8.4 2.6v1.8H6.6M3.6 9.4V7.6h1.8" />
      </svg>
    );
  }
  if (!verdict) return null;
  const mismatch = verdict.result === "mismatch";
  return (
    <svg className={mismatch ? "cd-status cd-status--mismatch" : "cd-status cd-status--done"} width={12} height={12} viewBox="0 0 12 12" aria-label={describeVerdict(verdict)}>
      <title>{describeVerdict(verdict)}</title>
      <circle cx="6" cy="6" r="6" />
      <path d={mismatch ? "M6 3v3.6M6 8.4v.6" : "M3.3 6.2l1.8 1.8 3.6-3.8"} />
    </svg>
  );
}
