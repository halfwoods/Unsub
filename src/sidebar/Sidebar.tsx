import type { MouseEvent, ReactNode } from "react";
import { artworkUrl, deviceEject } from "../api/commands";
import type { Device } from "../api/types";
import { deviceSource, playSourceOf, useApp, type SourceId } from "../state/store";
import { SourceIcon, type SourceIconName } from "./icons";
import { PlaylistRows } from "./PlaylistRows";
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
  return (
    <div className="artwork-pane">
      <div className="artwork-pane-title">{selectedId !== null ? "Selected Item" : "Now Playing"}</div>
      <div className="artwork-pane-image">
        {trackId !== null ? (
          <img key={`${artSource.type}-${trackId}`} src={artworkUrl(trackId, artSource)} alt="" onError={(e) => (e.currentTarget.style.visibility = "hidden")} />
        ) : (
          <span className="artwork-pane-empty">Nothing Playing</span>
        )}
      </div>
    </div>
  );
}

function Row({ id, label, icon, depth = 0, before, after }: SourceItem & { depth?: number; before?: ReactNode; after?: ReactNode }) {
  const selected = useApp((s) => s.source === id);
  const setSource = useApp((s) => s.setSource);
  return (
    <li
      role="option"
      aria-selected={selected}
      className={depth > 0 ? "sidebar-item sidebar-item--child" : "sidebar-item"}
      onMouseDown={() => setSource(id)}
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
function DeviceRows({ device }: { device: Device }) {
  const expanded = useApp((s) => s.expandedDevices.has(device.id));
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
        after={
          <button className="sidebar-eject" aria-label={`Eject ${device.name}`} onMouseDown={stop} onClick={() => void deviceEject(device.id)}>
            <svg width={11} height={10} viewBox="0 0 11 10" aria-hidden="true">
              <path d="M5.5 0L11 6H0zM0 7.5h11V10H0z" />
            </svg>
          </button>
        }
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
        {devices.length > 0 && (
          <Section heading="Devices">
            {devices.map((device) => (
              <DeviceRows key={device.id} device={device} />
            ))}
          </Section>
        )}
        {rest.map((section) => (
          <Section key={section.heading} heading={section.heading}>
            {section.items.map((item) => (
              <Row key={item.id} {...item} />
            ))}
            {section.heading === "Playlists" && <PlaylistRows />}
          </Section>
        ))}
      </div>
      {showArtwork && <ArtworkPane />}
    </nav>
  );
}
