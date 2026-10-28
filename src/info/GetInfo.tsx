import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useState } from "react";
import { libraryEditTracks, libraryLyrics, libraryTrack } from "../api/commands";
import type { ArtworkChange, LyricsSource, MediaKind, Track, TrackEdits } from "../api/types";
import { Artwork } from "../browser/Artwork";
import { useApp } from "../state/store";
import { dbOnlyNotice, formatAudioKind, formatBitRate, formatBytes, formatClock, formatMetadataDate, formatSampleRate } from "../util/format";
import "./GetInfo.css";

type Tab = "summary" | "info" | "sorting" | "options" | "artwork" | "lyrics";

type TextField = "title" | "artist" | "albumArtist" | "album" | "genre" | "composer" | "sortTitle" | "sortArtist" | "sortAlbum" | "sortAlbumArtist";
type NumberField = "year" | "trackNumber" | "trackCount" | "discNumber" | "discCount";
type Field = TextField | NumberField | "compilation" | "kind" | "rating" | "rememberPosition";

const NUMBER_FIELDS: readonly NumberField[] = ["year", "trackNumber", "trackCount", "discNumber", "discCount"];

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
    else if ((NUMBER_FIELDS as readonly string[]).includes(field)) edits[field as NumberField] = value === "" ? null : Number(value);
    else edits[field as TextField] = value;
  }
  return edits;
}

const KINDS: { value: MediaKind; label: string }[] = [
  { value: "music", label: "Music" },
  { value: "podcast", label: "Podcast" },
  { value: "audiobook", label: "Audiobook" },
];

const formatDate = (unix: number | null) => formatMetadataDate(unix) || "—";

function Summary({ track }: { track: Track }) {
  const facts: [string, string][] = [
    ["Kind", formatAudioKind(track.codec) || "—"],
    ["Duration", formatClock(track.durationMs)],
    ["Size", formatBytes(track.fileSize)],
    ["Bit Rate", formatBitRate(track.bitrateKbps) || "—"],
    ["Sample Rate", formatSampleRate(track.sampleRate) || "—"],
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
  const listTracks = useApp((s) => s.tracks);
  const showNotice = useApp((s) => s.showNotice);
  const openLibrarySheet = useApp((s) => s.openLibrarySheet);
  const [tab, setTab] = useState<Tab>(single ? "summary" : "info");
  const initial = useMemo(() => {
    const fields: Field[] = ["title", "artist", "albumArtist", "album", "genre", "composer", "sortTitle", "sortArtist", "sortAlbum", "sortAlbumArtist", ...NUMBER_FIELDS, "compilation", "kind", "rating", "rememberPosition"];
    return Object.fromEntries(fields.map((f) => [f, displayed(tracks, f)])) as Record<Field, string>;
  }, [tracks]);
  const [values, setValues] = useState(initial);
  const [applied, setApplied] = useState<ReadonlySet<Field>>(new Set());
  const [artwork, setArtwork] = useState<ArtworkChange | null>(null);
  // Read when the tab is first shown, since a sidecar or big tag isn't worth loading for every Get Info.
  const [lyrics, setLyrics] = useState<{ text: string; source: LyricsSource | null; edited: boolean } | null>(null);
  const [saving, setSaving] = useState(false);

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
    if (applied.size === 0 && !artwork && !lyrics?.edited) return true;
    setSaving(true);
    try {
      const edits = editsFrom(values, applied);
      if (lyrics?.edited) edits.lyrics = lyrics.text;
      const report = await libraryEditTracks(
        tracks.map((t) => t.id),
        edits,
        artwork,
      );
      const notice = dbOnlyNotice(report);
      if (notice) showNotice(notice);
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
        <input
          id={`info-${name}`}
          className="smart-input"
          inputMode={(NUMBER_FIELDS as readonly string[]).includes(name) ? "numeric" : undefined}
          value={values[name]}
          placeholder={!single && displayed(tracks, name) === "" ? "Mixed" : undefined}
          onChange={(e) => set(name, (NUMBER_FIELDS as readonly string[]).includes(name) ? e.target.value.replace(/\D/g, "") : e.target.value)}
        />
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
    { id: "artwork", label: "Artwork" },
    ...(single ? [{ id: "lyrics" as const, label: "Lyrics" }] : []),
  ];

  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
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
            <button key={t.id} role="tab" aria-selected={tab === t.id} className="pill-tab" onClick={() => setTab(t.id)}>
              {t.label}
            </button>
          ))}
        </div>
        <div className="info-body">
          {tab === "summary" && single && <Summary track={single} />}
          {tab === "info" && (
            <div className="info-grid">
              {field("title", "Name")}
              {field("artist", "Artist")}
              {field("albumArtist", "Album Artist")}
              {field("album", "Album")}
              {field("composer", "Composer")}
              {field("genre", "Genre")}
              {field("year", "Year", false)}
              {numberPair("trackNumber", "trackCount", "Track Number")}
              {numberPair("discNumber", "discCount", "Disc Number")}
              <span />
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
              <button className="push-button" disabled={saving || position <= 0} onClick={() => void step(-1)}>
                Previous
              </button>
              <button className="push-button" disabled={saving || position < 0 || position >= listTracks.length - 1} onClick={() => void step(1)}>
                Next
              </button>
            </>
          )}
          <span className="info-spacer" />
          <button className="push-button" onClick={onClose}>
            Cancel
          </button>
          <button className="push-button push-button--default" disabled={saving} onClick={() => void save().then((ok) => ok && onClose())}>
            OK
          </button>
        </div>
      </div>
    </div>
  );
}
