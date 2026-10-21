import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useState } from "react";
import { elsewhereCancel, elsewhereInspect, elsewhereRun } from "../api/commands";
import type { ElsewherePreview, ElsewhereService } from "../api/types";
import { useApp } from "../state/store";
import { pluralize } from "../util/format";
import "./ImportSheet.css";

interface Service {
  id: ElsewhereService;
  label: string;
  /** Where to ask for the export, in the order to read them. */
  links: { label: string; url: string }[];
  steps: string;
  /** What to know about this service's export. */
  also?: string;
}

const SERVICES: Service[] = [
  {
    id: "spotify",
    label: "Spotify",
    links: [{ label: "Open Spotify Privacy Settings", url: "https://www.spotify.com/account/privacy/" }],
    steps:
      "Ask Spotify for your data. Choose “Account data” for your playlists and the last year of plays, “Extended streaming history” for every play since you joined, or both. Spotify emails a link when it’s ready: account data takes a few days, and the extended history can take up to 30 days.",
    also: "A folder of playlist CSVs from Exportify works too.",
  },
  {
    id: "youtubeMusic",
    label: "YouTube Music",
    links: [{ label: "Open Google Takeout", url: "https://takeout.google.com/" }],
    steps:
      "In Google Takeout, choose “YouTube and YouTube Music”. Keep playlists, music and history, and set history to JSON. Google emails a link to the ZIP.",
    also: "Google lists some playlist songs only by video ID, so Unsub names the ones your history and saved songs know. From your history, only songs on artists’ “Topic” channels are used, and Google’s English export reads best.",
  },
  {
    id: "listenBrainz",
    label: "ListenBrainz",
    links: [
      { label: "Open ListenBrainz Export", url: "https://listenbrainz.org/settings/export/" },
      { label: "Import Last.fm History", url: "https://listenbrainz.org/settings/import/" },
    ],
    steps:
      "On ListenBrainz, export your data and download the ZIP when it’s ready. Last.fm has no file export of its own: import your Last.fm history into ListenBrainz first, then export it from there.",
    also: "Plays you scrobbled from the Music app can’t be told apart from Music’s own play counts, so if you’ve imported your Music library, those plays may count twice.",
  },
];

const percent = (part: number, whole: number) => `${whole > 0 ? Math.round((part / whole) * 100) : 0}% matched`;

/** What the export holds, as the sheet's two summary lines and the notes under them. */
function Findings({ preview }: { preview: ElsewherePreview }) {
  const unsure = preview.playlistAmbiguous + preview.playsAmbiguous;
  return (
    <>
      {preview.playlists > 0 && (
        <p>
          {pluralize(preview.playlists, "playlist", "playlists")} · {pluralize(preview.playlistSongs, "song", "songs")} ·{" "}
          {percent(preview.playlistMatched, preview.playlistSongs)}
        </p>
      )}
      {preview.plays > 0 && (
        <p>
          {pluralize(preview.plays, "play", "plays")} · {percent(preview.playsMatched, preview.plays)}
        </p>
      )}
      <p className="import-library-note">
        Songs in playlists that you don’t have go on your To Own list, and so do songs you played more than once. Importing again replaces the contents of playlists it made before.
        {unsure > 0 && ` ${pluralize(unsure, "entry fits", "entries fit")} more than one of your songs equally well and will be left out.`}
        {preview.shortPlays > 0 && ` ${pluralize(preview.shortPlays, "play", "plays")} under 30 seconds aren’t counted.`}
        {preview.unnamed > 0 && ` ${pluralize(preview.unnamed, "playlist song", "playlist songs")} only had a video ID and can’t be named.`}
      </p>
    </>
  );
}

/** File › Import from Spotify, YouTube Music or ListenBrainz: playlists and plays from a service's export. */
export function ImportElsewhere() {
  const close = () => useApp.setState({ importingElsewhere: false });
  const showNotice = useApp((s) => s.showNotice);
  const [serviceId, setServiceId] = useState<ElsewhereService>("spotify");
  const [path, setPath] = useState<string | null>(null);
  const [preview, setPreview] = useState<ElsewherePreview | null>(null);
  const [reading, setReading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [addPlays, setAddPlays] = useState(true);
  const service = SERVICES.find((s) => s.id === serviceId) ?? SERVICES[0];

  const pick = (id: ElsewhereService) => {
    if (reading || id === serviceId) return;
    setServiceId(id);
    setPath(null);
    setPreview(null);
    setError(null);
    void elsewhereCancel();
  };

  const choose = async (directory: boolean) => {
    const picked = await open({
      title: `Choose Your ${service.label} Export`,
      directory,
      multiple: false,
      filters: directory ? undefined : [{ name: "Export", extensions: ["zip", "json", "jsonl", "csv"] }],
    });
    if (typeof picked !== "string") return;
    setPath(picked);
    setPreview(null);
    setError(null);
    setReading(true);
    try {
      setPreview(await elsewhereInspect(service.id, picked));
    } catch (e) {
      setError(String(e));
    } finally {
      setReading(false);
    }
  };

  const start = () => {
    close();
    void elsewhereRun(addPlays).catch((e: unknown) => showNotice(String(e)));
  };

  const cancel = () => {
    close();
    void elsewhereCancel();
  };

  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && !reading && cancel()}>
      <div className="sheet import-library" role="dialog" aria-label="Import Playlists and Plays">
        <p className="import-library-title">Bring Your Playlists and Plays</p>
        <div className="pill-tabs import-library-services" role="tablist">
          {SERVICES.map((s) => (
            <button key={s.id} role="tab" aria-selected={s.id === serviceId} className="pill-tab" onClick={() => pick(s.id)}>
              {s.label}
            </button>
          ))}
        </div>
        <p>{service.steps}</p>
        <div className="import-library-folder">
          {service.links.map((link) => (
            <button key={link.url} className="push-button" onClick={() => void openUrl(link.url)}>
              {link.label}
            </button>
          ))}
        </div>
        <p className="import-library-title">Then choose what you downloaded</p>
        <div className="import-library-folder">
          <button className="push-button" disabled={reading} onClick={() => void choose(false)}>
            Choose File…
          </button>
          <button className="push-button" disabled={reading} onClick={() => void choose(true)}>
            Choose Folder…
          </button>
          <span className="import-library-path">{reading ? "Reading…" : (path ?? "None chosen")}</span>
        </div>
        {service.also && <p className="import-library-note">{service.also}</p>}
        {error && <p className="smart-error">{error}</p>}
        {preview && <Findings preview={preview} />}
        {preview && preview.plays > 0 && (
          <label className="import-library-check">
            <input type="checkbox" checked={addPlays} onChange={(e) => setAddPlays(e.target.checked)} />
            Add plays to play counts
          </label>
        )}
        <div className="sheet-buttons">
          <button className="push-button" disabled={reading} onClick={cancel}>
            Cancel
          </button>
          <button className="push-button push-button--default" disabled={!preview || reading} onClick={start}>
            Import
          </button>
        </div>
      </div>
    </div>
  );
}
