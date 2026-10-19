import type { Device } from "../api/types";
import { formatBytes } from "../util/format";
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

function CapacityBar({ device }: { device: Device }) {
  const segments = segmentsOf(device);
  const total = Math.max(1, device.capacityBytes);
  return (
    <div className="capacity">
      <div className="capacity-total">
        <strong>Capacity</strong>
        {formatBytes(device.capacityBytes)}
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

export function DeviceSummary({ device }: { device: Device }) {
  const model = device.model;
  return (
    <div className="device-summary">
      <div className="pill-tabs" role="tablist">
        <span role="tab" aria-selected="true" className="pill-tab">
          Summary
        </span>
      </div>
      <div className="device-body">
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
      </div>
      <footer className="device-footer">
        <CapacityBar device={device} />
        <button className="push-button" disabled title="Syncing arrives in milestone 13">
          Sync
        </button>
      </footer>
    </div>
  );
}
