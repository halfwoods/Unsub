import { useEffect, useState, type ReactNode } from "react";
import { deviceSetSyncSettings, deviceSync, deviceSyncPreview, deviceSyncSettings } from "../api/commands";
import type { Device, Player, SyncPreview, SyncSettings } from "../api/types";
import { SourceIcon } from "../sidebar/icons";
import { useApp } from "../state/store";
import { formatBytes, pluralize } from "../util/format";
import { IpodArt } from "./IpodArt";
import "./DeviceSummary.css";

interface Segment {
  label: string;
  bytes: number;
  className: string;
}

/** iTunes' capacity categories: everything audio together, then video, then whatever else is on the disk. */
function segmentsOf(device: Device): Segment[] {
  const { audioBytes, podcastBytes, audiobookBytes, videoBytes } = device.usage;
  const audio = audioBytes + podcastBytes + audiobookBytes;
  const used = Math.max(0, device.capacityBytes - device.freeBytes);
  const other = Math.max(0, used - audio - videoBytes);
  return [
    { label: "Audio", bytes: audio, className: "capacity-audio" },
    { label: "Video", bytes: videoBytes, className: "capacity-video" },
    { label: "Other", bytes: other, className: "capacity-other" },
    { label: "Free", bytes: device.freeBytes, className: "capacity-free" },
  ];
}

function CapacityBar({ capacityBytes, segments }: { capacityBytes: number; segments: Segment[] }) {
  const total = Math.max(1, capacityBytes);
  return (
    <div className="capacity">
      <div className="capacity-total">
        <strong>Capacity</strong>
        {formatBytes(capacityBytes)}
      </div>
      <div className="capacity-bar">
        {segments
          .filter((s) => s.bytes > 0)
          .map((s) => (
            <div key={s.label} className={`capacity-segment ${s.className}`} style={{ flexGrow: s.bytes / total }} />
          ))}
      </div>
      <div className="capacity-legend">
        {segments.map((s) => (
          <span key={s.label} className="capacity-legend-item">
            <span className={`capacity-swatch ${s.className}`} />
            <span className="capacity-legend-label">{s.label}</span>
            <span className="capacity-legend-value">{formatBytes(s.bytes)}</span>
          </span>
        ))}
      </div>
    </div>
  );
}

type Tab = "summary" | "music" | "podcasts" | "books";

const TABS: { id: Tab; label: string }[] = [
  { id: "summary", label: "Summary" },
  { id: "music", label: "Music" },
  { id: "podcasts", label: "Podcasts" },
  { id: "books", label: "Books" },
];

const AAC_RATES = [128, 192, 256];

type Update = (patch: Partial<SyncSettings>) => void;

function SummaryTab({ device, settings, update }: { device: Device; settings: SyncSettings; update: Update }) {
  const model = device.model;
  return (
    <>
      <section className="device-box">
        <h2>iPod</h2>
        <div className="device-box-content">
          <div className="device-art">
            <IpodArt family={model?.family ?? "classic"} color={model?.color ?? null} />
          </div>
          <dl className="device-facts">
            <dt>Name:</dt>
            <dd>{device.name}</dd>
            <dt>Model:</dt>
            <dd>{model?.name ?? "iPod"}</dd>
            <dt>Capacity:</dt>
            <dd>{formatBytes(device.capacityBytes)}</dd>
            {device.serialNumber && (
              <>
                <dt>Serial Number:</dt>
                <dd>{device.serialNumber}</dd>
              </>
            )}
            <dt>Songs:</dt>
            <dd>{device.counts.music.toLocaleString()}</dd>
          </dl>
        </div>
      </section>
      <section className="device-box">
        <h2>Options</h2>
        <div className="device-box-content device-options">
          <label className="prefs-check">
            <input type="checkbox" checked={settings.onlyChecked} onChange={(e) => update({ onlyChecked: e.target.checked })} />
            Sync only checked songs
          </label>
          <span className="prefs-check">
            <input
              type="checkbox"
              id="convert"
              checked={settings.convertKbps !== null}
              onChange={(e) => update({ convertKbps: e.target.checked ? 256 : null })}
            />
            <label htmlFor="convert">Convert higher bit rate songs to</label>
            <select
              className="smart-popup"
              aria-label="AAC bit rate"
              disabled={settings.convertKbps === null}
              value={settings.convertKbps ?? 256}
              onChange={(e) => update({ convertKbps: Number(e.target.value) })}
            >
              {AAC_RATES.map((kbps) => (
                <option key={kbps} value={kbps}>
                  {kbps} kbps AAC
                </option>
              ))}
            </select>
          </span>
          <p className="prefs-note">
            Songs the iPod can't play as they are, like FLAC or high-resolution files, become Apple Lossless on the iPod; your
            library keeps the originals. Songs that iTunes put on this iPod stay as they are.
          </p>
        </div>
      </section>
    </>
  );
}

function MusicTab({ settings, update }: { settings: SyncSettings; update: Update }) {
  const playlists = useApp((s) => s.playlists);
  const scope = settings.musicScope;
  const chosen = scope.type === "playlists" ? scope.ids : [];
  const toggle = (id: number, on: boolean) =>
    update({ musicScope: { type: "playlists", ids: on ? [...chosen, id] : chosen.filter((c) => c !== id) } });
  return (
    <section className="device-box">
      <div className="device-box-content device-options">
        <label className="prefs-check">
          <input type="checkbox" checked={settings.music} onChange={(e) => update({ music: e.target.checked })} />
          Sync Music
        </label>
        <fieldset className="device-choices" disabled={!settings.music}>
          <label>
            <input type="radio" checked={scope.type === "all"} onChange={() => update({ musicScope: { type: "all" } })} />
            Entire music library
          </label>
          <label>
            <input
              type="radio"
              checked={scope.type === "playlists"}
              onChange={() => update({ musicScope: { type: "playlists", ids: [] } })}
            />
            Selected playlists
          </label>
        </fieldset>
        {settings.music && scope.type === "playlists" && (
          <ul className="device-playlists" aria-label="Playlists">
            {playlists
              .filter((p) => p.kind !== "folder")
              .map((p) => (
                <li key={p.id}>
                  <label>
                    <input type="checkbox" checked={chosen.includes(p.id)} onChange={(e) => toggle(p.id, e.target.checked)} />
                    <SourceIcon name={p.kind === "smart" ? "smart" : "playlist"} />
                    {p.name}
                  </label>
                </li>
              ))}
          </ul>
        )}
      </div>
    </section>
  );
}

function PodcastsTab({ settings, update }: { settings: SyncSettings; update: Update }) {
  return (
    <section className="device-box">
      <div className="device-box-content device-options">
        <label className="prefs-check">
          <input type="checkbox" checked={settings.podcasts} onChange={(e) => update({ podcasts: e.target.checked })} />
          Sync Podcasts
        </label>
        <span className="device-include">
          Include:
          <select
            className="smart-popup"
            disabled={!settings.podcasts}
            value={settings.podcastsUnplayedOnly ? "unplayed" : "all"}
            onChange={(e) => update({ podcastsUnplayedOnly: e.target.value === "unplayed" })}
          >
            <option value="unplayed">all unplayed episodes</option>
            <option value="all">all episodes</option>
          </select>
        </span>
      </div>
    </section>
  );
}

function BooksTab({ settings, update }: { settings: SyncSettings; update: Update }) {
  return (
    <section className="device-box">
      <div className="device-box-content device-options">
        <label className="prefs-check">
          <input type="checkbox" checked={settings.audiobooks} onChange={(e) => update({ audiobooks: e.target.checked })} />
          Sync Audiobooks
        </label>
      </div>
    </section>
  );
}

function describePreview(preview: SyncPreview, noun: string) {
  if (!preview.fits) {
    return `Not enough space: this sync needs ${formatBytes(preview.bytesNeeded)} and ${formatBytes(preview.freeBytes)} is free.`;
  }
  const changes = [
    preview.add > 0 && `adds ${pluralize(preview.add, "song", "songs")} (${formatBytes(preview.bytesNeeded)})`,
    preview.converted > 0 && `${preview.converted.toLocaleString()} converted for the ${noun}`,
    preview.update > 0 && `updates ${preview.update.toLocaleString()}`,
    preview.remove > 0 && `removes ${preview.remove.toLocaleString()}`,
  ].filter(Boolean);
  const skipped = [
    preview.inCloud > 0 && `${preview.inCloud.toLocaleString()} in iCloud`,
    preview.missing > 0 && `${preview.missing.toLocaleString()} with missing files`,
  ].filter(Boolean);
  const summary = changes.length > 0 ? `Sync ${changes.join(", ")}.` : "This iPod is up to date.";
  return skipped.length > 0 ? `${summary} Skipping ${skipped.join(" and ")}.` : summary;
}

/** The writer is new, so the first sync to each iPod says what will happen before it touches anything. */
function FirstSyncSheet({ device, onCancel, onSync }: { device: Device; onCancel: () => void; onSync: () => void }) {
  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onCancel()}>
      <div className="sheet first-sync-sheet" role="dialog" aria-label="Sync this iPod">
        <h2 className="first-sync-title">Sync “{device.name}” with Unsub?</h2>
        <p>
          Unsub will add your chosen music to this iPod and keep it up to date. Songs that iTunes put on it stay as they
          are; Unsub only ever changes or removes songs it added itself.
        </p>
        <p>
          Before every sync it keeps a copy of the iPod's database beside the original, so a sync that goes wrong can be undone.
          This is a new feature, not yet tested on every iPod model: if your iPod shows no music afterwards, keep that copy and
          report the model.
        </p>
        <div className="sheet-buttons">
          <button className="push-button" onClick={onCancel}>
            Cancel
          </button>
          <button className="push-button push-button--default" onClick={onSync}>
            Sync
          </button>
        </div>
      </div>
    </div>
  );
}

/**
 * A device's sync page: its Summary, Music, Podcasts and Books tabs, and the footer with its
 * capacity, what Sync will do, and the Sync button. iPods and other players share it.
 */
function SyncPage({
  deviceId,
  noun,
  device,
  writable,
  unwritable,
  summary,
  capacity,
  firstSync,
}: {
  deviceId: string;
  /** What to call the device in the footer: "iPod" or "player". */
  noun: string;
  /** Replaced after each sync, which is when the preview needs recomputing. */
  device: unknown;
  writable: boolean;
  unwritable: string;
  summary: (settings: SyncSettings, update: Update) => ReactNode;
  capacity: ReactNode;
  /** A sheet to show before the first sync, if the device needs one. */
  firstSync?: (onCancel: () => void, onSync: () => void) => ReactNode;
}) {
  const [tab, setTab] = useState<Tab>("summary");
  const [settings, setSettings] = useState<SyncSettings | null>(null);
  const [saved, setSaved] = useState(0);
  const [preview, setPreview] = useState<SyncPreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);
  const syncing = useApp((s) => s.deviceSync !== null);
  const showNotice = useApp((s) => s.showNotice);

  useEffect(() => {
    void deviceSyncSettings(deviceId).then(setSettings);
  }, [deviceId]);

  // Recomputed once settings load, after each change is saved (the backend reads the saved
  // copy), and after each sync, which replaces `device`.
  const loaded = settings !== null;
  useEffect(() => {
    if (!writable || !loaded || syncing) return;
    let current = true;
    const timer = setTimeout(() => {
      void deviceSyncPreview(deviceId).then(
        (p) => {
          if (!current) return;
          setPreview(p);
          setPreviewError(null);
        },
        (e: unknown) => {
          if (current) setPreviewError(String(e));
        },
      );
    }, 250);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [deviceId, device, saved, loaded, syncing, writable]);

  const update: Update = (patch) => {
    if (!settings) return;
    const next = { ...settings, ...patch };
    setSettings(next);
    void deviceSetSyncSettings(deviceId, next).then(() => setSaved((n) => n + 1));
  };

  const sync = () => void deviceSync(deviceId).catch((e: unknown) => showNotice(String(e)));
  const status = !writable ? unwritable : (previewError ?? (preview ? describePreview(preview, noun) : "Checking what will sync…"));

  return (
    <div className="device-summary">
      <div className="pill-tabs" role="tablist">
        {TABS.map((t) => (
          <button key={t.id} role="tab" aria-selected={tab === t.id} className="pill-tab" onClick={() => setTab(t.id)}>
            {t.label}
          </button>
        ))}
      </div>
      <div className="device-body">
        {settings &&
          (tab === "summary" ? (
            summary(settings, update)
          ) : tab === "music" ? (
            <MusicTab settings={settings} update={update} />
          ) : tab === "podcasts" ? (
            <PodcastsTab settings={settings} update={update} />
          ) : (
            <BooksTab settings={settings} update={update} />
          ))}
      </div>
      <footer className="device-footer">
        <div className="device-footer-main">
          {capacity}
          <p className={preview && !preview.fits ? "sync-status sync-status--warning" : "sync-status"}>{status}</p>
        </div>
        <button
          className="push-button"
          disabled={!writable || syncing || !preview?.fits}
          onClick={() => (firstSync && !settings?.confirmed ? setConfirming(true) : sync())}
        >
          {syncing ? "Syncing…" : "Sync"}
        </button>
      </footer>
      {confirming &&
        firstSync?.(
          () => setConfirming(false),
          () => {
            setConfirming(false);
            update({ confirmed: true });
            sync();
          },
        )}
    </div>
  );
}

export function DeviceSummary({ device }: { device: Device }) {
  return (
    <SyncPage
      deviceId={device.id}
      noun="iPod"
      device={device}
      writable={device.sync === "supported" || device.sync === "needsHash58"}
      unwritable="Unsub can't write to this iPod model yet. You can still browse and play its music."
      summary={(settings, update) => <SummaryTab device={device} settings={settings} update={update} />}
      capacity={<CapacityBar capacityBytes={device.capacityBytes} segments={segmentsOf(device)} />}
      firstSync={(onCancel, onSync) => <FirstSyncSheet device={device} onCancel={onCancel} onSync={onSync} />}
    />
  );
}

const PLAYER_KINDS: Record<Player["kind"], string> = {
  rockbox: "Rockbox player",
  massStorage: "USB music player",
  mtp: "Phone",
};

function PlayerSummaryTab({ player, settings, update }: { player: Player; settings: SyncSettings; update: Update }) {
  const { profile } = player;
  const formats = profile.formats.map((f) => f.toUpperCase());
  return (
    <>
      <section className="device-box">
        <h2>{PLAYER_KINDS[player.kind]}</h2>
        <div className="device-box-content">
          <dl className="device-facts">
            <dt>Name:</dt>
            <dd>{player.name}</dd>
            <dt>Capacity:</dt>
            <dd>{formatBytes(player.capacityBytes)}</dd>
            <dt>Plays:</dt>
            <dd>{formats.length > 0 ? formats.join(", ") : "Every format"}</dd>
            <dt>Music folder:</dt>
            <dd>/{profile.musicDir}</dd>
            <dt>Playlists:</dt>
            <dd>
              /{profile.playlistDir} ({profile.playlistFormat})
            </dd>
          </dl>
        </div>
      </section>
      <section className="device-box">
        <h2>Options</h2>
        <div className="device-box-content device-options">
          <label className="prefs-check">
            <input type="checkbox" checked={settings.onlyChecked} onChange={(e) => update({ onlyChecked: e.target.checked })} />
            Sync only checked songs
          </label>
          <p className="prefs-note">
            {formats.length > 0
              ? `Songs in other formats become ${formats[0]} on the player; your library keeps the originals. `
              : ""}
            Files you put on the player yourself are never touched.
          </p>
        </div>
      </section>
    </>
  );
}

/** A non-Apple player's page: the same tabs as an iPod's, syncing plain files and playlists. */
export function PlayerSummary({ player }: { player: Player }) {
  const used = Math.max(0, player.capacityBytes - player.freeBytes);
  return (
    <SyncPage
      deviceId={player.id}
      noun="player"
      device={player}
      writable={player.kind !== "mtp"}
      unwritable="Syncing to phones over MTP isn't available yet. Phones that mount as a USB drive can be set up as a player."
      summary={(settings, update) => <PlayerSummaryTab player={player} settings={settings} update={update} />}
      capacity={
        <CapacityBar
          capacityBytes={player.capacityBytes}
          segments={[
            { label: "Used", bytes: used, className: "capacity-other" },
            { label: "Free", bytes: player.freeBytes, className: "capacity-free" },
          ]}
        />
      }
    />
  );
}
