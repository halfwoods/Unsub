import { useState, type MouseEvent, type ReactNode } from "react";
import { artworkUrl, cdEject, deviceEject, partyAdd } from "../api/commands";
import type { Device } from "../api/types";
import { cdSource, deviceSource, playerSource, playSourceOf, useApp, type SourceId } from "../state/store";
import { SourceIcon, type SourceIconName } from "./icons";
import { PlaylistRows, TRACKS_MIME } from "./PlaylistRows";
import "./Sidebar.css";

interface SourceItem {
  id: SourceId;
  label: string;
  icon: SourceIconName;
}

// Same section order as iTunes 10; Purchases and Mixes stand in for the Store and Genius.
const SECTIONS: { heading: string; items: SourceItem[] }[] = [
  {
    heading: "Library",
    items: [
      { id: "music", label: "Music", icon: "music" },
      { id: "podcasts", label: "Podcasts", icon: "podcasts" },
      { id: "audiobooks", label: "Audiobooks", icon: "audiobooks" },
    ],
  },
  {
    heading: "Purchases",
    items: [
      { id: "toOwn", label: "To Own", icon: "toOwn" },
      { id: "inbox", label: "Inbox", icon: "inbox" },
      { id: "purchased", label: "Purchased", icon: "purchased" },
    ],
  },
  { heading: "Mixes", items: [{ id: "mixes", label: "Mixes", icon: "mixes" }] },
  {
    heading: "Playlists",
    items: [
      { id: "partyShuffle", label: "Party Shuffle", icon: "partyShuffle" },
    ],
  },
];

function ArtworkPane() {
  const nowPlaying = useApp((s) => s.nowPlaying);
  const playingSource = useApp((s) => s.playingSource);
  // Select the primitive; deriving an object inside a zustand selector re-renders forever.
  const listSource = playSourceOf(useApp((s) => s.source));
  const selectedId = useApp((s) => (s.selection.size === 1 ? [...s.selection][0] : null));
  // iTunes shows the selected item's art, falling back to what's playing.
  const trackId = selectedId ?? nowPlaying?.id ?? null;
  const artSource = selectedId !== null ? listSource : playingSource;
  const cdCover = useApp((s) => (artSource.type === "cd" ? (s.cds.find((cd) => cd.id === artSource.discId)?.coverArtUrl ?? null) : null));
  const src = trackId === null ? null : artSource.type === "cd" ? cdCover : artworkUrl(trackId, artSource);
  return (
    <div className="artwork-pane">
      <div className="artwork-pane-title">{selectedId !== null ? "Selected Item" : "Now Playing"}</div>
      <div className="artwork-pane-image">
        {trackId === null ? (
          <span className="artwork-pane-empty">Nothing Playing</span>
        ) : (
          src && <img key={`${artSource.type}-${trackId}`} src={src} alt="" onError={(e) => (e.currentTarget.style.visibility = "hidden")} />
        )}
      </div>
    </div>
  );
}

function Row({
  id,
  label,
  icon,
  depth = 0,
  before,
  after,
  onDropTracks,
}: SourceItem & { depth?: number; before?: ReactNode; after?: ReactNode; onDropTracks?: (trackIds: number[]) => void }) {
  const selected = useApp((s) => s.source === id);
  const setSource = useApp((s) => s.setSource);
  const [dropTarget, setDropTarget] = useState(false);
  return (
    <li
      role="option"
      aria-selected={selected}
      className={[depth > 0 ? "sidebar-item sidebar-item--child" : "sidebar-item", dropTarget ? "sidebar-item--drop" : ""].join(" ")}
      onMouseDown={() => setSource(id)}
      onDragOver={(e) => {
        if (!onDropTracks || !e.dataTransfer.types.includes(TRACKS_MIME)) return;
        e.preventDefault();
        setDropTarget(true);
      }}
      onDragLeave={() => setDropTarget(false)}
      onDrop={(e) => {
        setDropTarget(false);
        const tracks = e.dataTransfer.getData(TRACKS_MIME);
        if (onDropTracks && tracks) onDropTracks(JSON.parse(tracks) as number[]);
      }}
    >
      {before}
      <SourceIcon name={icon} />
      <span className="sidebar-label">{label}</span>
      {after}
    </li>
  );
}

function Section({ heading, children }: { heading: string; children: ReactNode }) {
  return (
    <section className="sidebar-section">
      <h2 className="sidebar-heading">{heading}</h2>
      <ul role="listbox" aria-label={heading}>
        {children}
      </ul>
    </section>
  );
}

/** Mirrors iTunes 10: the device row opens Summary; expanding it lists what's on the iPod. */
function EjectButton({ name, onEject }: { name: string; onEject: () => void }) {
  return (
    <button className="sidebar-eject" aria-label={`Eject ${name}`} onMouseDown={(e) => e.stopPropagation()} onClick={onEject}>
      <svg width={11} height={10} viewBox="0 0 11 10" aria-hidden="true">
        <path d="M5.5 0L11 6H0zM0 7.5h11V10H0z" />
      </svg>
    </button>
  );
}

/** iTunes swaps the eject button for turning sync arrows while an iPod syncs. */
function SyncingIcon() {
  return (
    <svg className="sidebar-syncing" width={12} height={12} viewBox="0 0 12 12" aria-label="Syncing">
      <path d="M2.2 6a3.8 3.8 0 0 1 6.6-2.6M9.8 6a3.8 3.8 0 0 1-6.6 2.6M9.3 1.4v2.4H6.9M2.7 10.6V8.2h2.4" />
    </svg>
  );
}

function DeviceRows({ device }: { device: Device }) {
  const expanded = useApp((s) => s.expandedDevices.has(device.id));
  const syncing = useApp((s) => s.deviceSync?.deviceId === device.id);
  const toggleDevice = useApp((s) => s.toggleDevice);
  const stop = (e: MouseEvent) => e.stopPropagation();

  const kinds = [
    { kind: "music", label: "Music", icon: "music", count: device.counts.music },
    { kind: "podcast", label: "Podcasts", icon: "podcasts", count: device.counts.podcast },
    { kind: "audiobook", label: "Audiobooks", icon: "audiobooks", count: device.counts.audiobook },
  ] as const;

  return (
    <>
      <Row
        id={deviceSource(device.id)}
        label={device.name}
        icon="ipod"
        before={
          <button
            className={expanded ? "sidebar-disclosure sidebar-disclosure--open" : "sidebar-disclosure"}
            aria-label={expanded ? `Collapse ${device.name}` : `Expand ${device.name}`}
            aria-expanded={expanded}
            onMouseDown={stop}
            onClick={() => toggleDevice(device.id)}
          />
        }
        after={syncing ? <SyncingIcon /> : <EjectButton name={device.name} onEject={() => void deviceEject(device.id)} />}
      />
      {expanded && (
        <>
          {kinds
            .filter((k) => k.count > 0)
            .map((k) => (
              <Row key={k.kind} id={deviceSource(device.id, { kind: k.kind })} label={k.label} icon={k.icon} depth={1} />
            ))}
          {device.playlists
            .filter((p) => !p.isPodcasts)
            .map((p) => (
              <Row
                key={p.id}
                id={deviceSource(device.id, { playlistId: p.id })}
                label={p.name}
                icon={p.isSmart ? "smart" : "playlist"}
                depth={1}
              />
            ))}
        </>
      )}
    </>
  );
}

export function Sidebar() {
  const showArtwork = useApp((s) => s.showArtwork);
  const devices = useApp((s) => s.devices);
  const cds = useApp((s) => s.cds);
  const players = useApp((s) => s.players);
  const syncingId = useApp((s) => s.deviceSync?.deviceId ?? null);
  const showNotice = useApp((s) => s.showNotice);
  const [library, purchases, ...rest] = SECTIONS;

  return (
    <nav className="sidebar">
      <div className="sidebar-scroll">
        {[library, purchases].map((section) => (
          <Section key={section.heading} heading={section.heading}>
            {section.items.map((item) => (
              <Row key={item.id} {...item} />
            ))}
          </Section>
        ))}
        {/* iTunes only shows DEVICES while something is plugged in. */}
        {devices.length + players.length + cds.length > 0 && (
          <Section heading="Devices">
            {devices.map((device) => (
              <DeviceRows key={device.id} device={device} />
            ))}
            {players.map((player) => (
              <Row
                key={player.id}
                id={playerSource(player.id)}
                label={player.name}
                icon="player"
                after={syncingId === player.id ? <SyncingIcon /> : undefined}
              />
            ))}
            {cds.map((cd) => (
              <Row
                key={cd.id}
                id={cdSource(cd.id)}
                label={cd.name}
                icon="cd"
                after={<EjectButton name={cd.name} onEject={() => void cdEject(cd.id).catch((e: unknown) => showNotice(String(e)))} />}
              />
            ))}
          </Section>
        )}
        {rest.map((section) => (
          <Section key={section.heading} heading={section.heading}>
            {section.items.map((item) => (
              <Row
                key={item.id}
                {...item}
                // Songs dropped on Party Shuffle join the end of its queue.
                onDropTracks={item.id === "partyShuffle" ? (ids) => void partyAdd(ids, null) : undefined}
              />
            ))}
            {section.heading === "Playlists" && <PlaylistRows />}
          </Section>
        ))}
      </div>
      {showArtwork && <ArtworkPane />}
    </nav>
  );
}
