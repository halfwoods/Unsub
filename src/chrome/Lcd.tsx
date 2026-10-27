import { useCallback, useEffect, useRef, useState, type PointerEvent } from "react";
import { cdStopImport, detailsSetPaused, jobCancel, playerSeek } from "../api/commands";
import type { Chapter, RadioStation, Track } from "../api/types";
import { startMixFrom } from "../mixes/startMix";
import { chapterIndexAt, useApp } from "../state/store";
import { formatClock } from "../util/format";
import { VinylMark } from "./VinylMark";
import "./Lcd.css";

/** iTunes alternates the second LCD line between artist and album. */
const SECOND_LINE_MS = 3500;

/** Which of `lines` the second LCD line is showing; each stays for a while in turn. */
function useAlternating(enabled: boolean, lines: number) {
  const [turn, setTurn] = useState(0);
  useEffect(() => {
    if (!enabled) return;
    const timer = setInterval(() => setTurn((t) => t + 1), SECOND_LINE_MS);
    return () => clearInterval(timer);
  }, [enabled]);
  return turn % lines;
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
    // A right or middle click doesn't start a scrub.
    if (e.button !== 0) return;
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

/** iTunes 10 put its Genius button here; ours starts an on-device mix from this song. Building it takes a moment, so the LCD says what is happening rather than leaving the click without a sign. */
function MixButton({ song }: { song: Track }) {
  const [building, setBuilding] = useState(false);
  const start = () => {
    setBuilding(true);
    void startMixFrom(song).finally(() => setBuilding(false));
  };
  return (
    <button className="lcd-mix" aria-label="Start a mix from this song" title="Start a mix from this song" disabled={building} onClick={start}>
      {/* One song, drawn larger, branching out to others like it. Not a circle pair, which is the sidebar's Mixes list, nor arcs around a dot, which the sidebar uses for radio. */}
      <svg width={14} height={11} viewBox="0 0 14 11" aria-hidden="true">
        <circle className="lcd-mix-dot" cx="2.2" cy="5.5" r="2" />
        <path d="M4.2 5.5C6.6 5.5 7 1.7 9.8 1.7M4.4 5.5h5.4M4.2 5.5C6.6 5.5 7 9.3 9.8 9.3" />
        <circle className="lcd-mix-dot" cx="11.7" cy="1.7" r="1.3" />
        <circle className="lcd-mix-dot" cx="11.7" cy="5.5" r="1.3" />
        <circle className="lcd-mix-dot" cx="11.7" cy="9.3" r="1.3" />
      </svg>
    </button>
  );
}

/**
 * The LCD's progress layout, for work that isn't playback; `fraction` null shows a busy bar.
 * `onCancel` adds iTunes' ⓧ at the end of the bar, and `onPause` a button before it that pauses the work, or resumes it if `paused`.
 */
function ProgressLcd({
  title,
  subtitle,
  fraction,
  onCancel,
  paused,
  onPause,
}: {
  title: string;
  subtitle: string;
  fraction: number | null;
  onCancel?: () => void;
  paused?: boolean;
  onPause?: () => void;
}) {
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
        {onPause && (
          <button className="lcd-pause" aria-label={`${paused ? "Resume" : "Pause"} ${title}`} title={paused ? "Resume" : "Pause"} onClick={onPause}>
            <svg width={12} height={12} viewBox="0 0 12 12" aria-hidden="true">
              <circle cx="6" cy="6" r="6" />
              {paused ? <path d="M4.8 3.6v4.8L8.6 6z" /> : <path d="M4.5 3.8v4.4M7.5 3.8v4.4" />}
            </svg>
          </button>
        )}
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
  // Downloads go one at a time, so at most one has started; until one does, any of those waiting stands in.
  const podcastDownload = useApp((s) => {
    const downloads = Object.values(s.podcastDownloads);
    return downloads.find((d) => d.started) ?? downloads[0];
  });
  const cdRip = useApp((s) => s.cdRip);
  const deviceSync = useApp((s) => s.deviceSync);
  const organize = useApp((s) => s.organize);
  const cleanUp = useApp((s) => s.cleanUp);
  const details = useApp((s) => s.details);
  const importProgress = useApp((s) => s.importProgress);
  const syncingName = useApp(
    (s) => [...s.devices, ...s.players].find((d) => d.id === s.deviceSync?.deviceId)?.name ?? "device",
  );
  const notice = useApp((s) => s.notice);
  const fromLibrary = useApp((s) => s.playingSource.type === "library");
  const chapters = useApp((s) => s.chapters?.list);
  const castTarget = useApp((s) => s.castTarget);
  const [showTotal, setShowTotal] = useState(false);
  // The speaker joins artist and album in the rotation, so it is always one glance away.
  const turn = useAlternating(nowPlaying !== null, castTarget ? 3 : 2);

  if (live && liveStation) return <LiveLcd station={liveStation} notice={notice} />;

  if (nowPlaying) {
    const secondLine =
      castTarget && turn === 2
        ? `Playing on ${castTarget.name}`
        : turn === 1 && nowPlaying.album
          ? nowPlaying.album
          : nowPlaying.artist || nowPlaying.album;
    return (
      <div className="lcd lcd--playing">
        <div className="lcd-title">{nowPlaying.title}</div>
        {fromLibrary && chapters && chapters.length > 0 && <ChapterMenu chapters={chapters} positionMs={positionMs} />}
        {fromLibrary && nowPlaying.kind === "music" && <MixButton song={nowPlaying} />}
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
    const rescuing = stage === "rescuing" || stage === "exportingVideos";
    // Before the first song there is nothing to count yet, only the iPod being read.
    const subtitle = rescuing
      ? total === 0
        ? current
        : `${Math.min(done + 1, total).toLocaleString()} of ${total.toLocaleString()}: ${current}`
      : stage === "writingDatabase"
        ? "Updating the iPod's database"
        : stage === "cleaning"
          ? "Removing songs"
          : stage === "playlists"
            ? "Writing playlists"
            : stage === "skipped"
              ? `Skipped: ${current}`
              : `${stage === "downloading" ? "Downloading from iCloud" : "Copying"} ${Math.min(done + 1, total).toLocaleString()} of ${total.toLocaleString()}: ${current}`;
    return (
      <ProgressLcd
        title={stage === "exportingVideos" ? `Copying videos from “${syncingName}”` : rescuing ? `Copying from “${syncingName}”` : `Syncing “${syncingName}”`}
        subtitle={subtitle}
        fraction={total > 0 ? done / total : null}
        // Only the iPod database's commit can't be stopped halfway.
        onCancel={stage === "writingDatabase" ? undefined : () => void jobCancel("sync")}
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

  if (cleanUp) {
    return (
      <ProgressLcd
        title="Cleaning Up Song Info"
        subtitle={`${cleanUp.done.toLocaleString()} of ${cleanUp.total.toLocaleString()}`}
        fraction={cleanUp.total > 0 ? cleanUp.done / cleanUp.total : null}
        onCancel={() => void jobCancel("cleanUp")}
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

  if (importProgress) {
    const { stage, done, total } = importProgress;
    const subtitle = {
      reading: "Reading your Music library",
      locating: total === null ? `Looking through the folder: ${done.toLocaleString()} files` : `Finding files: ${done.toLocaleString()} of ${total.toLocaleString()}`,
      adding: `Adding songs: ${done.toLocaleString()} of ${(total ?? 0).toLocaleString()}`,
      merging: "Merging play counts and playlists",
      readingExport: "Reading your export",
      matching: "Finding the songs you own",
      applying: `Adding playlists: ${done.toLocaleString()} of ${(total ?? 0).toLocaleString()}`,
      plays: `Adding plays: ${done.toLocaleString()} of ${(total ?? 0).toLocaleString()}`,
    }[stage];
    const fromElsewhere = stage === "readingExport" || stage === "matching" || stage === "applying" || stage === "plays";
    return (
      <ProgressLcd
        title={fromElsewhere ? "Importing Playlists and Plays" : "Importing Music Library"}
        subtitle={subtitle}
        fraction={total && done > 0 ? done / total : null}
      />
    );
  }

  if (scan) return <ProgressLcd title="Adding to Library" subtitle={scan.currentPath.split(/[\\/]/).pop() ?? ""} fraction={null} />;

  if (podcastDownload) {
    const { title, fraction } = podcastDownload;
    // A notice shares the subtitle, as on the now-playing screen, since the download goes on for as long as the notice is up.
    const subtitle = notice ?? (fraction === null ? "Starting…" : `${Math.round(fraction * 100)}%`);
    return <ProgressLcd title={`Downloading “${title}”`} subtitle={subtitle} fraction={fraction} />;
  }

  // A notice with nothing playing, such as a station that couldn't be reached.
  if (notice) {
    return (
      <div className="lcd lcd--playing">
        <div className="lcd-title lcd-title--centered">{notice}</div>
      </div>
    );
  }

  // Quietest of all: it can run for hours, so anything the user starts shows in its place, and a notice for as long as it lasts.
  if (details) {
    const done = details.read + details.skipped + details.missing;
    const counted = `${done.toLocaleString()} of ${details.total.toLocaleString()}`;
    const subtitle = {
      running: `${counted}: ${details.title}`,
      paused: `Paused at ${counted}`,
      lowSpace: "Paused: the disk is almost full",
      noResponse: "Paused: iCloud isn't responding",
      filesMissing: "Paused: the songs' files can't be found",
      notGivenBack: "Paused: iCloud isn't taking songs back",
    }[details.phase];
    return (
      <ProgressLcd
        title="Getting Song Details from iCloud"
        subtitle={subtitle}
        fraction={details.total > 0 ? Math.min(1, done / details.total) : null}
        paused={details.phase !== "running"}
        onPause={() => void detailsSetPaused(details.phase === "running")}
        onCancel={() => void jobCancel("details")}
      />
    );
  }

  return (
    <div className="lcd lcd--idle">
      <VinylMark />
    </div>
  );
}
