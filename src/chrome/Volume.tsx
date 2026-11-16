import { useRef, type CSSProperties, type PointerEvent, type WheelEvent } from "react";
import { useApp } from "../state/store";
import "./Volume.css";

const DEFAULT_TRACK_WIDTH = 100;
const KNOB = 19;
const MAX_VOLUME = 1.3;
const VOLUME_STEP = 0.01;
const WHEEL_STEP = 0.03;

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

export function Volume({ trackWidth = DEFAULT_TRACK_WIDTH }: { trackWidth?: number }) {
  const volume = useApp((s) => s.playback.volume);
  const setVolume = useApp((s) => s.setVolume);
  const trackRef = useRef<HTMLDivElement>(null);
  const dragStart = useRef<{ x: number; volume: number } | null>(null);

  const volumeAt = (clientX: number) => {
    const rect = trackRef.current?.getBoundingClientRect();
    if (!rect) return volume;
    const usable = rect.width - KNOB;
    return Math.min(MAX_VOLUME, Math.max(0, ((clientX - rect.left - KNOB / 2) / usable) * MAX_VOLUME));
  };

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    dragStart.current = { x: e.clientX, volume };
    if (!e.shiftKey) setVolume(volumeAt(e.clientX));
  };

  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    if (!e.currentTarget.hasPointerCapture(e.pointerId)) return;
    if (e.shiftKey && dragStart.current) {
      const usable = Math.max(1, (trackRef.current?.getBoundingClientRect().width ?? trackWidth) - KNOB);
      const fineChange = ((e.clientX - dragStart.current.x) / usable) * MAX_VOLUME / 5;
      setVolume(Math.min(MAX_VOLUME, Math.max(0, dragStart.current.volume + fineChange)));
    } else {
      setVolume(volumeAt(e.clientX));
    }
  };

  const onWheel = (e: WheelEvent<HTMLDivElement>) => {
    e.preventDefault();
    setVolume(Math.min(MAX_VOLUME, Math.max(0, volume - Math.sign(e.deltaY) * WHEEL_STEP)));
  };

  const knobLeft = (volume / MAX_VOLUME) * (trackWidth - KNOB);
  const percent = Math.round(volume * 100);
  const boostHint = volume > 1 ? "; above 100% can clip loud tracks" : "";

  return (
    <div className="volume">
      <button className="volume-icon" aria-label="Mute" onClick={() => setVolume(0)}>
        <Speaker waves={0} />
      </button>
      <div
        ref={trackRef}
        className="volume-track"
        style={{ width: trackWidth, "--volume-unity-position": `${(trackWidth - KNOB) / MAX_VOLUME + KNOB / 2}px` } as CSSProperties}
        role="slider"
        aria-label="Volume"
        aria-valuemin={0}
        aria-valuemax={130}
        aria-valuenow={percent}
        aria-valuetext={`${percent}%${boostHint}`}
        title={`Volume ${percent}%${boostHint}`}
        tabIndex={0}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={() => { dragStart.current = null; }}
        onPointerCancel={() => { dragStart.current = null; }}
        onWheel={onWheel}
        onKeyDown={(e) => {
          const step = e.shiftKey ? VOLUME_STEP / 10 : VOLUME_STEP;
          if (e.key === "ArrowRight" || e.key === "ArrowUp") setVolume(Math.min(MAX_VOLUME, volume + step));
          if (e.key === "ArrowLeft" || e.key === "ArrowDown") setVolume(Math.max(0, volume - step));
          if (e.key === "Home") setVolume(0);
          if (e.key === "End") setVolume(MAX_VOLUME);
        }}
      >
        <div className="volume-groove">
          <div className="volume-fill" style={{ width: knobLeft + KNOB / 2 }} />
        </div>
        <div className="volume-knob" style={{ left: knobLeft }} />
      </div>
      <button className="volume-icon" aria-label="Maximum volume, 130%" onClick={() => setVolume(MAX_VOLUME)}>
        <Speaker waves={3} />
      </button>
    </div>
  );
}
