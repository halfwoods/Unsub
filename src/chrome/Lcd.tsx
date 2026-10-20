import { useCallback, useEffect, useRef, useState, type PointerEvent } from "react";
import { cdStopImport, deviceCancelSync, libraryMixFor, playerSeek } from "../api/commands";
import type { Chapter, RadioStation } from "../api/types";
import { chapterIndexAt, useApp } from "../state/store";
import { formatClock } from "../util/format";
import { VinylMark } from "./VinylMark";
import "./Lcd.css";

/** iTunes alternates the second LCD line between artist and album. */
const SECOND_LINE_MS = 3500;

function useAlternating(enabled: boolean) {
  const [flip, setFlip] = useState(false);
  useEffect(() => {
    if (!enabled) return;
    const timer = setInterval(() => setFlip((f) => !f), SECOND_LINE_MS);
    return () => clearInterval(timer);
  }, [enabled]);
  return flip;
}

function Progress({ positionMs, durationMs }: { positionMs: number; durationMs: number }) {
  const barRef = useRef<HTMLDivElement>(null);
  const [scrubMs, setScrubMs] = useState<number | null>(null);
  const shownMs = scrubMs ?? positionMs;
  const fraction = durationMs > 0 ? Math.min(1, shownMs / durationMs) : 0;

  const msAt = (clientX: number) => {
    const rect = barRef.current?.getBoundingClientRect();
    if (!rect || durationMs <= 0) return 0;
    return Math.min(1, Math.max(0, (clientX - rect.left) / rect.width)) * durationMs;
  };

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    e.currentTarget.setPointerCapture(e.pointerId);
    setScrubMs(msAt(e.clientX));
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    if (scrubMs !== null) setScrubMs(msAt(e.clientX));
  };
  // Seek once on release, like iTunes, so dragging doesn't stutter the decoder.
  const onPointerUp = (e: PointerEvent<HTMLDivElement>) => {
    if (scrubMs === null) return;
    void playerSeek(Math.round(msAt(e.clientX)));
    setScrubMs(null);
  };

  return (
    <div
      ref={barRef}
      className="lcd-progress"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
    >
      <div className="lcd-progress-groove">
        <div className="lcd-progress-fill" style={{ width: `${fraction * 100}%` }} />
      </div>
      <div className="lcd-progress-knob" style={{ left: `${fraction * 100}%` }} />
    </div>
  );
}

/** iTunes 10's "≡" chapter menu: lists the chapters with their start times, and seeks to the one chosen. */
function ChapterMenu({ chapters, positionMs }: { chapters: Chapter[]; positionMs: number }) {
  // The chapter playing when the menu opened is the one marked and scrolled to, not a moving target.
  const [open, setOpen] = useState<{ top: number; left: number; current: number } | null>(null);
  const seekTo = useApp((s) => s.seekTo);
  const scrollToCurrent = useCallback((el: HTMLButtonElement | null) => el?.scrollIntoView({ block: "center" }), []);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(null);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open]);

  return (
    <>
      <button
        className="lcd-chapters-button"
        aria-label="Chapters"
        title="Chapters"
        aria-haspopup="menu"
        onClick={(e) => {
          const rect = e.currentTarget.getBoundingClientRect();
          setOpen({ top: rect.bottom + 2, left: rect.left, current: chapterIndexAt(chapters, positionMs) });
        }}
      >
        <svg width={12} height={10} viewBox="0 0 12 10" aria-hidden="true">
          <path d="M0.5 1.5h11M0.5 5h11M0.5 8.5h11" />
        </svg>
      </button>
      {open && (
        <>
          <div className="lcd-chapters-backdrop" onPointerDown={() => setOpen(null)} />
          <ul className="lcd-chapters" role="menu" style={{ top: open.top, left: open.left }}>
            {chapters.map((chapter, index) => (
              <li key={index} role="none">
                <button
                  role="menuitem"
                  ref={index === open.current ? scrollToCurrent : undefined}
                  onClick={() => {
                    seekTo(chapter.startMs);
                    setOpen(null);
                  }}
                >
                  <span className="lcd-chapters-mark">{index === open.current ? "✓" : ""}</span>
                  <span className="lcd-chapters-title">{chapter.title}</span>
                  <span className="lcd-chapters-time">{formatClock(chapter.startMs)}</span>
                </button>
              </li>
            ))}
          </ul>
        </>
      )}
    </>
  );
}

/**
 * The LCD's progress layout, for work that isn't playback; `fraction` null shows a busy bar.
 * `onCancel` adds iTunes' ⓧ at the end of the bar.
 */
function ProgressLcd({ title, subtitle, fraction, onCancel }: { title: string; subtitle: string; fraction: number | null; onCancel?: () => void }) {
  return (
    <div className="lcd lcd--playing">
      <div className="lcd-title">{title}</div>
      <div className="lcd-subtitle">{subtitle}</div>
      <div className="lcd-timeline">
        <div className={fraction === null ? "lcd-progress lcd-progress--busy" : "lcd-progress"}>
          <div className="lcd-progress-groove">
            {fraction !== null && <div className="lcd-progress-fill" style={{ width: `${Math.round(fraction * 100)}%` }} />}
          </div>
        </div>
        {onCancel && (
          <button className="lcd-cancel" aria-label={`Stop ${title}`} title="Stop" onClick={onCancel}>
            <svg width={12} height={12} viewBox="0 0 12 12" aria-hidden="true">
              <circle cx="6" cy="6" r="6" />
              <path d="M3.9 3.9l4.2 4.2M8.1 3.9L3.9 8.1" />
            </svg>
          </button>
        )}
      </div>
    </div>
  );
}

/** A station: its name, then what it says is playing; time counts up, with nothing to scrub or run out. */
function LiveLcd({ station, notice }: { station: RadioStation; notice: string | null }) {
  const title = useApp((s) => s.liveTitle);
  const buffering = useApp((s) => s.playback.buffering);
  const link = useApp((s) => s.liveLink);
  const positionMs = useApp((s) => s.playback.positionMs);
  const details = [station.genre, station.bitrateKbps ? `${station.bitrateKbps} kbps` : ""].filter(Boolean).join(" · ");
  const playing = title?.title ? (title.artist ? `${title.artist} – ${title.title}` : title.title) : details;
  const status = link === "connecting" ? "Connecting…" : link === "reconnecting" ? "Reconnecting…" : buffering ? "Buffering…" : playing;
  const subtitle = notice ?? status;
  return (
    <div className="lcd lcd--playing">
      <div className="lcd-title">{station.name}</div>
      <div className="lcd-subtitle" key={subtitle}>
        {subtitle}
      </div>
      <div className="lcd-timeline">
        <span className="lcd-time lcd-time--elapsed">{formatClock(positionMs)}</span>
        <div className="lcd-progress">
          <div className="lcd-progress-groove" />
        </div>
      </div>
    </div>
  );
}

export function Lcd() {
  const live = useApp((s) => s.playback.live || s.liveLink === "connecting");
  const liveStation = useApp((s) => s.liveStation);
  const nowPlaying = useApp((s) => s.nowPlaying);
  const positionMs = useApp((s) => s.playback.positionMs);
  const durationMs = useApp((s) => s.playback.durationMs);
  const scan = useApp((s) => s.scan);
  const download = useApp((s) => s.download);
  const downloadingTitle = useApp((s) =>
    s.download ? (s.playingTracks.find((t) => t.id === s.download?.trackId)?.title ?? "") : "",
  );
  const cdRip = useApp((s) => s.cdRip);
  const deviceSync = useApp((s) => s.deviceSync);
  const organize = useApp((s) => s.organize);
  const syncingName = useApp(
    (s) => [...s.devices, ...s.players].find((d) => d.id === s.deviceSync?.deviceId)?.name ?? "device",
  );
  const notice = useApp((s) => s.notice);
  const fromLibrary = useApp((s) => s.playingSource.type === "library");
  const chapters = useApp((s) => s.chapters?.list);
  const playTrackIds = useApp((s) => s.playTrackIds);
  const [showTotal, setShowTotal] = useState(false);
  const flip = useAlternating(nowPlaying !== null);

  if (live && liveStation) return <LiveLcd station={liveStation} notice={notice} />;

  if (nowPlaying) {
    const secondLine = flip && nowPlaying.album ? nowPlaying.album : nowPlaying.artist || nowPlaying.album;
    return (
      <div className="lcd lcd--playing">
        <div className="lcd-title">{nowPlaying.title}</div>
        {fromLibrary && chapters && chapters.length > 0 && <ChapterMenu chapters={chapters} positionMs={positionMs} />}
        {fromLibrary && (
          // iTunes 10 put its Genius button here; ours starts an on-device mix from this song.
          <button
            className="lcd-mix"
            aria-label="Start a Mix from this song"
            title="Start a Mix from this song"
            onClick={() => void libraryMixFor([nowPlaying.id]).then((ids) => playTrackIds([nowPlaying.id, ...ids]))}
          >
            <svg width={14} height={11} viewBox="0 0 14 11" aria-hidden="true">
              <circle cx="4.8" cy="5.5" r="3.8" />
              <circle cx="9.2" cy="5.5" r="3.8" />
            </svg>
          </button>
        )}
        <div className="lcd-subtitle" key={notice ?? secondLine}>
          {notice ?? secondLine}
        </div>
        <div className="lcd-timeline">
          <span className="lcd-time lcd-time--elapsed">{formatClock(positionMs)}</span>
          <Progress positionMs={positionMs} durationMs={durationMs} />
          <button className="lcd-time lcd-time--remaining" onClick={() => setShowTotal((t) => !t)}>
            {showTotal ? formatClock(durationMs) : `-${formatClock(Math.max(0, durationMs - positionMs))}`}
          </button>
        </div>
      </div>
    );
  }

  if (download) return <ProgressLcd title={`Downloading from ${download.from}`} subtitle={downloadingTitle} fraction={download.fraction} />;

  if (deviceSync) {
    const { stage, done, total, current } = deviceSync;
    const subtitle =
      stage === "writingDatabase"
        ? "Updating the iPod's database"
        : stage === "cleaning"
          ? "Removing songs"
          : stage === "playlists"
            ? "Writing playlists"
            : stage === "skipped"
              ? `Skipped: ${current}`
              : `Copying ${Math.min(done + 1, total).toLocaleString()} of ${total.toLocaleString()}: ${current}`;
    return (
      <ProgressLcd
        title={`Syncing “${syncingName}”`}
        subtitle={subtitle}
        fraction={total > 0 ? done / total : null}
        // Only the iPod database's commit can't be stopped halfway.
        onCancel={stage === "writingDatabase" ? undefined : () => void deviceCancelSync()}
      />
    );
  }

  // Each imported song is then added to the library, so the import outranks "Adding to Library".
  if (cdRip) {
    return (
      <ProgressLcd
        title={`Importing “${cdRip.title}”`}
        subtitle={`Song ${cdRip.index + 1} of ${cdRip.count}`}
        fraction={cdRip.fraction}
        onCancel={() => void cdStopImport()}
      />
    );
  }

  if (organize) {
    return (
      <ProgressLcd
        title="Organizing Library"
        subtitle={organize.currentPath.split(/[\\/]/).pop() ?? ""}
        fraction={organize.total > 0 ? organize.done / organize.total : null}
      />
    );
  }

  if (scan) return <ProgressLcd title="Adding to Library" subtitle={scan.currentPath.split(/[\\/]/).pop() ?? ""} fraction={null} />;

  // A notice with nothing playing, such as a station that couldn't be reached.
  if (notice) {
    return (
      <div className="lcd lcd--playing">
        <div className="lcd-title lcd-title--centered">{notice}</div>
      </div>
    );
  }

  return (
    <div className="lcd lcd--idle">
      <VinylMark />
    </div>
  );
}
