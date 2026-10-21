import { useRef, type PointerEvent, type WheelEvent } from "react";
import { useApp } from "../state/store";
import "./Volume.css";

const TRACK_WIDTH = 100;
const KNOB = 19;

function Speaker({ waves }: { waves: number }) {
  return (
    <svg className="volume-speaker" width={14} height={11} viewBox="0 0 14 11" aria-hidden="true">
      <path d="M0 3.5h2.5L6 0.5v10L2.5 7.5H0z" />
      {waves > 0 && <path className="volume-wave" d="M8 3.5q1.2 2 0 4" />}
      {waves > 1 && <path className="volume-wave" d="M10 2q2.2 3.5 0 7" />}
      {waves > 2 && <path className="volume-wave" d="M12 0.5q3.2 5 0 10" />}
    </svg>
  );
}

export function Volume() {
  const volume = useApp((s) => s.playback.volume);
  const setVolume = useApp((s) => s.setVolume);
  const trackRef = useRef<HTMLDivElement>(null);

  const volumeAt = (clientX: number) => {
    const rect = trackRef.current?.getBoundingClientRect();
    if (!rect) return volume;
    const usable = rect.width - KNOB;
    return Math.min(1, Math.max(0, (clientX - rect.left - KNOB / 2) / usable));
  };

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    setVolume(volumeAt(e.clientX));
  };

  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    if (e.currentTarget.hasPointerCapture(e.pointerId)) setVolume(volumeAt(e.clientX));
  };

  const onWheel = (e: WheelEvent<HTMLDivElement>) => {
    setVolume(Math.min(1, Math.max(0, volume - e.deltaY / 500)));
  };

  const knobLeft = volume * (TRACK_WIDTH - KNOB);

  return (
    <div className="volume">
      <button className="volume-icon" aria-label="Mute" onClick={() => setVolume(0)}>
        <Speaker waves={0} />
      </button>
      <div
        ref={trackRef}
        className="volume-track"
        role="slider"
        aria-label="Volume"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(volume * 100)}
        tabIndex={0}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onWheel={onWheel}
        onKeyDown={(e) => {
          if (e.key === "ArrowRight" || e.key === "ArrowUp") setVolume(Math.min(1, volume + 0.1));
          if (e.key === "ArrowLeft" || e.key === "ArrowDown") setVolume(Math.max(0, volume - 0.1));
        }}
      >
        <div className="volume-groove">
          <div className="volume-fill" style={{ width: knobLeft + KNOB / 2 }} />
        </div>
        <div className="volume-knob" style={{ left: knobLeft }} />
      </div>
      <button className="volume-icon" aria-label="Maximum volume" onClick={() => setVolume(1)}>
        <Speaker waves={3} />
      </button>
    </div>
  );
}
