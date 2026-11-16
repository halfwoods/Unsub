import { ask, message, open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useState } from "react";
import { libraryEditTracks, libraryLyrics, libraryTrack, trimApply, trimGet, trimPreview, trimSet, type SongTrim } from "../api/commands";
import type { ArtworkChange, LyricsSource, MediaKind, Track, TrackEdits } from "../api/types";
import { Artwork } from "../browser/Artwork";
import { useApp } from "../state/store";
import { dbOnlyNotice, formatBytes, formatClock } from "../util/format";
import { parseTrimTime, trimTime } from "./trimTime";
import "./GetInfo.css";

type Tab = "summary" | "info" | "sorting" | "options" | "artwork" | "lyrics" | "trim";

type TextField = "title" | "artist" | "albumArtist" | "album" | "genre" | "composer" | "grouping" | "comment" | "sortTitle" | "sortArtist" | "sortAlbum" | "sortAlbumArtist";
type NumberField = "bpm" | "year" | "trackNumber" | "trackCount" | "discNumber" | "discCount";
type Field = TextField | NumberField | "compilation" | "kind" | "rating" | "rememberPosition";

const NUMBER_FIELDS: readonly NumberField[] = ["bpm", "year", "trackNumber", "trackCount", "discNumber", "discCount"];

/** What a field shows: the song's own value, or for several songs the value they share. */
function displayed(tracks: Track[], field: Field): string {
  const values = new Set(tracks.map((t) => String(t[field] ?? "")));
  return values.size === 1 ? [...values][0] : "";
}

function editsFrom(values: Record<Field, string>, applied: ReadonlySet<Field>): TrackEdits {
  const edits: TrackEdits = {};
  for (const field of applied) {
    const value = values[field];
    if (field === "compilation") edits.compilation = value === "true";
    else if (field === "rememberPosition") edits.rememberPosition = value === "true";
    else if (field === "kind") edits.kind = value as MediaKind;
    else if (field === "rating") edits.rating = Number(value);
    else if (field === "bpm") {
      const bpm = value === "" ? null : Math.round(Number(value));
      if (bpm !== null && (!Number.isFinite(bpm) || bpm < 1 || bpm > 999)) throw new Error("BPM must be between 1 and 999.");
      edits.bpm = bpm;
    } else if ((NUMBER_FIELDS as readonly string[]).includes(field)) edits[field as NumberField] = value === "" ? null : Number(value);
    else edits[field as TextField] = value;
  }
  return edits;
}

const KINDS: { value: MediaKind; label: string }[] = [
  { value: "music", label: "Music" },
  { value: "podcast", label: "Podcast" },
  { value: "audiobook", label: "Audiobook" },
];

const formatDate = (unix: number | null) => (unix ? new Date(unix * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }) : "—");

function Summary({ track }: { track: Track }) {
  const facts: [string, string][] = [
    ["Kind", `${track.codec} audio file`],
    ["Duration", formatClock(track.durationMs)],
    ["Size", formatBytes(track.fileSize)],
    ["Bit Rate", track.bitrateKbps ? `${track.bitrateKbps} kbps` : "—"],
    ["Sample Rate", track.sampleRate ? `${(track.sampleRate / 1000).toLocaleString()} kHz` : "—"],
    ["Bit Depth", track.bitDepth ? `${track.bitDepth}-bit` : "—"],
    ["Channels", track.channels === 1 ? "Mono" : track.channels === 2 ? "Stereo" : track.channels ? `${track.channels} channels` : "—"],
    ["Date Modified", formatDate(track.dateModified)],
    ["Date Added", formatDate(track.dateAdded)],
    ["Play Count", String(track.playCount)],
    ["Last Played", formatDate(track.lastPlayed)],
  ];
  return (
    <div className="info-summary">
      <dl className="info-facts">
        {facts.map(([label, value]) => (
          <div key={label}>
            <dt>{label}:</dt>
            <dd>{value}</dd>
          </div>
        ))}
      </dl>
      <p className="info-where">
        <strong>Where:</strong> {track.path}
        {track.missing && <span className="info-flag"> (the file can't be found)</span>}
        {track.inCloud && <span className="info-flag"> (in iCloud Drive, not downloaded)</span>}
      </p>
    </div>
  );
}

/** iTunes' Get Info, and Multiple Item Information when several songs are selected. */
export function GetInfo({ trackIds }: { trackIds: number[] }) {
  const listTracks = useApp((s) => s.tracks);
  const openLibrarySheet = useApp((s) => s.openLibrarySheet);
  const [tracks, setTracks] = useState<Track[] | null>(null);

  useEffect(() => {
    const known = trackIds.map((id) => listTracks.find((t) => t.id === id));
    if (known.every((t) => t !== undefined)) return setTracks(known as Track[]);
    void Promise.all(trackIds.map((id) => libraryTrack(id))).then((found) => setTracks(found.filter((t): t is Track => t !== null)));
    // Only `trackIds`: the list can refresh under an open sheet, which keeps the songs it opened with.
  }, [trackIds]);

  if (!tracks || tracks.length === 0) return null;
  return <InfoSheet key={trackIds.join(",")} tracks={tracks} onClose={() => openLibrarySheet(null)} />;
}

function InfoSheet({ tracks, onClose }: { tracks: Track[]; onClose: () => void }) {
  const single = tracks.length === 1 ? tracks[0] : null;
  const isMp3 = single?.codec.toLowerCase() === "mp3" && single.path.toLowerCase().endsWith(".mp3");
  const trimPermanentNote = isMp3
    ? "MP3 audio is copied without re-encoding, preserving its encoded quality. Cut points align to MP3 frames (about 26 ms at 44.1 kHz). A backup is kept."
    : "Lossy formats such as AAC, Vorbis, and Opus are re-encoded, which can reduce quality. A backup is kept.";
  const listTracks = useApp((s) => s.tracks);
  const showNotice = useApp((s) => s.showNotice);
  const openLibrarySheet = useApp((s) => s.openLibrarySheet);
  const [tab, setTab] = useState<Tab>(single ? "summary" : "info");
  const initial = useMemo(() => {
    const fields: Field[] = ["title", "artist", "albumArtist", "album", "genre", "composer", "grouping", "comment", "sortTitle", "sortArtist", "sortAlbum", "sortAlbumArtist", ...NUMBER_FIELDS, "compilation", "kind", "rating", "rememberPosition"];
    return Object.fromEntries(fields.map((f) => [f, displayed(tracks, f)])) as Record<Field, string>;
  }, [tracks]);
  const [values, setValues] = useState(initial);
  const [applied, setApplied] = useState<ReadonlySet<Field>>(new Set());
  const [artwork, setArtwork] = useState<ArtworkChange | null>(null);
  // Read when the tab is first shown, since a sidecar or big tag isn't worth loading for every Get Info.
  const [lyrics, setLyrics] = useState<{ text: string; source: LyricsSource | null; edited: boolean } | null>(null);
  const [saving, setSaving] = useState(false);
  const [trimValues, setTrimValues] = useState<{ start: string; end: string; enabled: boolean } | null>(null);
  const [trimDirty, setTrimDirty] = useState(false);
  const [trimError, setTrimError] = useState<string | null>(null);
  const [trimBusy, setTrimBusy] = useState(false);

  useEffect(() => {
    if (tab !== "trim" || !single || trimValues) return;
    void trimGet(single.id).then((trim) => setTrimValues({ start: trimTime(trim.startMs), end: trimTime(trim.endMs), enabled: trim.enabled }))
      .catch((e: unknown) => setTrimError(String(e)));
  }, [tab, single, trimValues]);

  const readTrim = (enabled = trimValues?.enabled ?? false): SongTrim => {
    if (!single || !trimValues) throw new Error("Song trim settings are still loading.");
    const startMs = parseTrimTime(trimValues.start), endMs = parseTrimTime(trimValues.end);
    if (startMs === null || endMs === null || startMs >= endMs || endMs > single.durationMs) {
      throw new Error("Enter valid times within the song, with the end after the start (for example, 0:12.500).");
    }
    return { startMs, endMs, enabled };
  };

  const saveTrim = async (enabled = trimValues?.enabled ?? false) => {
    if (!single) return;
    const trim = readTrim(enabled);
    await trimSet(single.id, trim);
    setTrimValues({ start: trimTime(trim.startMs), end: trimTime(trim.endMs), enabled });
    setTrimDirty(false);
    setTrimError(null);
    return trim;
  };

  const previewTrim = async (ending: boolean) => {
    if (!single) return;
    if (useApp.getState().castTarget) { setTrimError("Select this computer as the output to preview playback limits."); return; }
    setTrimBusy(true);
    const previous = useApp.getState();
    const previewTracks = [single];
    try {
      const trim = readTrim();
      useApp.setState({ playingTracks: previewTracks, playingSource: { type: "library" }, podcastWish: null });
      await trimPreview(single.id, trim, ending);
      setTrimError(null);
    } catch (e) {
      if (useApp.getState().playingTracks === previewTracks) {
        useApp.setState({ playingTracks: previous.playingTracks, playingSource: previous.playingSource, podcastWish: previous.podcastWish });
      }
      setTrimError(String(e));
    }
    finally { setTrimBusy(false); }
  };

  const makeTrimPermanent = async () => {
    if (!single) return;
    let trim: SongTrim;
    try { trim = readTrim(); } catch (e) { setTrimError(String(e)); return; }
    if (trim.startMs === 0 && trim.endMs >= single.durationMs) { setTrimError("Choose a shorter section first."); return; }
    const approved = await ask(
      `Replace this song with the selected section? Playback of this song will stop. ${trimPermanentNote} Specialized tags and synced lyrics may need updating.`,
      { title: "Make Trim Permanent", kind: "warning", okLabel: "Trim Song", cancelLabel: "Cancel" },
    );
    if (!approved) return;
    setTrimBusy(true);
    try {
      if (applied.size > 0 || artwork || lyrics?.edited) {
        if (!(await save())) return;
      }
      const result = await trimApply(single.id, trim);
      await useApp.getState().refresh();
      await message(`The song was trimmed. The original backup is at:\n${result.backupPath}${result.warning ? `\n\n${result.warning}` : ""}`, { title: "Trim Complete", kind: result.warning ? "warning" : "info" });
      onClose();
    } catch (e) { setTrimError(String(e)); }
    finally { setTrimBusy(false); }
  };

  useEffect(() => {
    if (tab !== "lyrics" || !single || lyrics) return;
    libraryLyrics(single.id)
      .then((found) => setLyrics({ text: found?.text ?? "", source: found?.source ?? null, edited: false }))
      .catch((e: unknown) => showNotice(String(e)));
  }, [tab, single, lyrics, showNotice]);

  const set = (field: Field, value: string) => {
    setValues((v) => ({ ...v, [field]: value }));
    setApplied((a) => new Set(a).add(field));
  };
  const toggleApplied = (field: Field, on: boolean) =>
    setApplied((a) => {
      const next = new Set(a);
      if (on) next.add(field);
      else next.delete(field);
      return next;
    });

  const save = async () => {
    if (applied.size === 0 && !artwork && !lyrics?.edited && !trimDirty) return true;
    setSaving(true);
    try {
      const edits = editsFrom(values, applied);
      if (lyrics?.edited) edits.lyrics = lyrics.text;
      if (applied.size > 0 || artwork || lyrics?.edited) {
        const report = await libraryEditTracks(tracks.map((t) => t.id), edits, artwork);
        const notice = dbOnlyNotice(report);
        if (notice) showNotice(notice);
      }
      if (trimDirty) await saveTrim();
      setSaving(false);
      return true;
    } catch (e) {
      showNotice(String(e));
      setSaving(false);
      return false;
    }
  };

  // iTunes' Previous and Next: save this song's changes, then show its neighbour in the list.
  const position = single ? listTracks.findIndex((t) => t.id === single.id) : -1;
  const step = async (by: number) => {
    const neighbour = listTracks[position + by];
    if (neighbour && (await save())) openLibrarySheet({ type: "info", trackIds: [neighbour.id] });
  };

  const field = (name: Field, label: string, wide = true) => (
    <>
      <label className="info-label" htmlFor={`info-${name}`}>
        {label}
      </label>
      <span className={wide ? "info-field" : "info-field info-field--short"}>
        {!single && (
          <input
            type="checkbox"
            className="info-apply"
            aria-label={`Change ${label} for all`}
            checked={applied.has(name)}
            onChange={(e) => toggleApplied(name, e.target.checked)}
          />
        )}
        {name === "comment" ? <textarea
          id={`info-${name}`}
          className="smart-input info-comments"
          rows={3}
          value={values[name]}
          placeholder={!single && displayed(tracks, name) === "" ? "Mixed" : undefined}
          onChange={(e) => set(name, e.target.value)}
        /> : <input
          id={`info-${name}`}
          className="smart-input"
          inputMode={name === "bpm" ? "decimal" : (NUMBER_FIELDS as readonly string[]).includes(name) ? "numeric" : undefined}
          value={values[name]}
          placeholder={!single && displayed(tracks, name) === "" ? "Mixed" : undefined}
          onChange={(e) => {
            const value = e.target.value;
            if (name === "bpm") {
              if (/^\d*\.?\d*$/.test(value)) set(name, value);
            } else set(name, (NUMBER_FIELDS as readonly string[]).includes(name) ? value.replace(/\D/g, "") : value);
          }}
        />}
      </span>
    </>
  );

  const numberPair = (first: NumberField, second: NumberField, label: string) => (
    <>
      <label className="info-label" htmlFor={`info-${first}`}>
        {label}
      </label>
      <span className="info-field info-field--pair">
        {!single && (
          <input
            type="checkbox"
            className="info-apply"
            aria-label={`Change ${label} for all`}
            checked={applied.has(first) || applied.has(second)}
            onChange={(e) => {
              toggleApplied(first, e.target.checked);
              toggleApplied(second, e.target.checked);
            }}
          />
        )}
        <input id={`info-${first}`} className="smart-input" inputMode="numeric" value={values[first]} onChange={(e) => set(first, e.target.value.replace(/\D/g, ""))} />
        of
        <input aria-label={`${label} total`} className="smart-input" inputMode="numeric" value={values[second]} onChange={(e) => set(second, e.target.value.replace(/\D/g, ""))} />
      </span>
    </>
  );

  const pickArtwork = async () => {
    const path = await open({ title: "Choose Artwork", multiple: false, filters: [{ name: "Images", extensions: ["jpg", "jpeg", "png", "gif", "bmp", "webp"] }] });
    if (typeof path === "string") setArtwork({ type: "file", path });
  };

  const tabs: { id: Tab; label: string }[] = [
    ...(single ? [{ id: "summary" as const, label: "Summary" }] : []),
    { id: "info", label: "Info" },
    { id: "sorting", label: "Sorting" },
    { id: "options", label: "Options" },
    ...(single && single.kind === "music" ? [{ id: "trim" as const, label: "Trim" }] : []),
    { id: "artwork", label: "Artwork" },
    ...(single ? [{ id: "lyrics" as const, label: "Lyrics" }] : []),
  ];

  return (
    <div className="sheet-backdrop" onMouseDown={(e) => !trimBusy && e.target === e.currentTarget && onClose()}>
      <div className="sheet info-sheet" role="dialog" aria-label={single ? `Info for ${single.title}` : "Multiple Item Information"}>
        <header className="info-header">
          {single ? <Artwork trackId={single.id} size={56} /> : <span className="info-many">{tracks.length}</span>}
          <div className="info-heading">
            <strong>{single ? single.title : "Multiple Item Information"}</strong>
            <span>{single ? single.artist : `${tracks.length} songs: only the fields you change are applied to all of them.`}</span>
            {single && <span>{single.album}</span>}
          </div>
        </header>
        <div className="pill-tabs" role="tablist">
          {tabs.map((t) => (
            <button key={t.id} role="tab" disabled={trimBusy} aria-selected={tab === t.id} className="pill-tab" onClick={() => setTab(t.id)}>
              {t.label}
            </button>
          ))}
        </div>
        <div className="info-body">
          {tab === "trim" && single && (
            <div className="trim-panel">
              <p>Choose the section to keep. Times refer to the original song.</p>
              {trimValues ? <>
                <div className="trim-times">
                  <label>Start <input id="trim-start" className="smart-input" disabled={trimBusy} value={trimValues.start} onChange={(e) => { setTrimValues({ ...trimValues, start: e.target.value }); setTrimDirty(true); }} placeholder="0:00.000" /></label>
                  <label>End <input id="trim-end" className="smart-input" disabled={trimBusy} value={trimValues.end} onChange={(e) => { setTrimValues({ ...trimValues, end: e.target.value }); setTrimDirty(true); }} placeholder="3:45.000" /></label>
                </div>
                <label className="trim-check"><input type="checkbox" disabled={trimBusy} checked={trimValues.enabled} onChange={(e) => { setTrimValues({ ...trimValues, enabled: e.target.checked }); setTrimDirty(true); }} /> Use these times when playing this song</label>
                <p>Playback limits apply on this computer the next time the song starts. Preview plays this song on its own without saving the times or changing the switch.</p>
                <div className="trim-actions">
                  <button className="push-button" disabled={trimBusy} onClick={() => void previewTrim(false)}>Preview Start</button>
                  <button className="push-button" disabled={trimBusy} onClick={() => void previewTrim(true)}>Preview End</button>
                  <button className="push-button" disabled={trimBusy} onClick={() => void makeTrimPermanent()}>Make Permanent…</button>
                </div>
                <p>Make Permanent replaces the song file. {trimPermanentNote}</p>
              </> : <p>Loading…</p>}
              {trimError && <p className="trim-error" role="alert">{trimError}</p>}
              {trimBusy && <p>Working…</p>}
            </div>
          )}
          {tab === "summary" && single && <Summary track={single} />}
          {tab === "info" && (
            <div className="info-metadata">
              <div className="info-row info-row--wide">{field("title", "Name")}</div>
              <div className="info-row">{field("artist", "Artist")}{field("year", "Year")}</div>
              <div className="info-row">{field("albumArtist", "Album Artist")}{numberPair("trackNumber", "trackCount", "Track #")}</div>
              <div className="info-row">{field("album", "Album")}{numberPair("discNumber", "discCount", "Disc #")}</div>
              <div className="info-row">{field("grouping", "Grouping")}{field("bpm", "BPM")}</div>
              <div className="info-row info-row--wide">{field("composer", "Composer")}</div>
              <div className="info-row info-row--wide">{field("comment", "Comments")}</div>
              <div className="info-row info-row--genre">
                {field("genre", "Genre")}
                <label className="prefs-check info-check">
                  <input
                    type="checkbox"
                    checked={values.compilation === "true"}
                    ref={(el) => {
                      if (el) el.indeterminate = !single && values.compilation === "" && !applied.has("compilation");
                    }}
                    onChange={(e) => set("compilation", String(e.target.checked))}
                  />
                  Part of a compilation
                </label>
              </div>
            </div>
          )}
          {tab === "sorting" && (
            <div className="info-grid">
              {field("sortTitle", "Sort Name")}
              {field("sortArtist", "Sort Artist")}
              {field("sortAlbumArtist", "Sort Album Artist")}
              {field("sortAlbum", "Sort Album")}
            </div>
          )}
          {tab === "options" && (
            <div className="info-grid">
              <label className="info-label" htmlFor="info-kind">
                Media Kind
              </label>
              <span className="info-field info-field--short">
                <select id="info-kind" className="smart-popup" value={values.kind} onChange={(e) => set("kind", e.target.value)}>
                  {values.kind === "" && <option value="">Mixed</option>}
                  {KINDS.map((k) => (
                    <option key={k.value} value={k.value}>
                      {k.label}
                    </option>
                  ))}
                </select>
              </span>
              <label className="info-label" htmlFor="info-rating">
                Rating
              </label>
              <span className="info-field info-field--short">
                <select id="info-rating" className="smart-popup info-stars" value={values.rating} onChange={(e) => set("rating", e.target.value)}>
                  {values.rating === "" && <option value="">Mixed</option>}
                  {[0, 1, 2, 3, 4, 5].map((stars) => (
                    <option key={stars} value={stars * 20}>
                      {stars === 0 ? "None" : "★".repeat(stars)}
                    </option>
                  ))}
                </select>
              </span>
              <span />
              <span className="info-field">
                {!single && (
                  <input
                    type="checkbox"
                    className="info-apply"
                    aria-label="Change Remember playback position for all"
                    checked={applied.has("rememberPosition")}
                    onChange={(e) => toggleApplied("rememberPosition", e.target.checked)}
                  />
                )}
                <label className="prefs-check info-check">
                  <input
                    type="checkbox"
                    checked={values.rememberPosition === "true"}
                    ref={(el) => {
                      if (el) el.indeterminate = !single && values.rememberPosition === "" && !applied.has("rememberPosition");
                    }}
                    onChange={(e) => set("rememberPosition", String(e.target.checked))}
                  />
                  Remember playback position
                </label>
              </span>
            </div>
          )}
          {tab === "artwork" && (
            <div className="info-artwork">
              {artwork?.type === "file" ? (
                <p className="info-artwork-note">New artwork: {artwork.path.split(/[\\/]/).pop()}</p>
              ) : artwork?.type === "remove" ? (
                <p className="info-artwork-note">The artwork will be removed.</p>
              ) : single ? (
                <Artwork trackId={single.id} size={200} />
              ) : (
                <p className="info-artwork-note">Artwork you add here goes on all {tracks.length} songs.</p>
              )}
              <div className="info-artwork-buttons">
                <button className="push-button" onClick={() => void pickArtwork()}>
                  Add…
                </button>
                <button className="push-button" onClick={() => setArtwork({ type: "remove" })}>
                  Delete
                </button>
              </div>
            </div>
          )}
          {tab === "lyrics" && single && (
            <div className="info-lyrics">
              <textarea
                className="info-lyrics-text"
                aria-label="Lyrics"
                value={lyrics?.text ?? ""}
                disabled={!lyrics}
                onChange={(e) => setLyrics((l) => l && { ...l, text: e.target.value, edited: true })}
              />
              {lyrics?.source === "sidecar" && (
                <p className="info-lyrics-note">Synced lyrics from a .lrc file; edits here change the song's own lyrics.</p>
              )}
            </div>
          )}
        </div>
        <div className="sheet-buttons info-buttons">
          {single && (
            <>
              <button className="push-button" disabled={saving || trimBusy || position <= 0} onClick={() => void step(-1)}>
                Previous
              </button>
              <button className="push-button" disabled={saving || trimBusy || position < 0 || position >= listTracks.length - 1} onClick={() => void step(1)}>
                Next
              </button>
            </>
          )}
          <span className="info-spacer" />
          <button className="push-button" disabled={trimBusy} onClick={onClose}>
            Cancel
          </button>
          <button className="push-button push-button--default" disabled={saving || trimBusy} onClick={() => void save().then((ok) => ok && onClose())}>
            OK
          </button>
        </div>
      </div>
    </div>
  );
}
