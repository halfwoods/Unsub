import { useState } from "react";
import { artworkUrl } from "../api/commands";
import "./Artwork.css";

/** Album art from the library, or iTunes' gray note placeholder when a track has none. */
export function Artwork({ trackId, size, className = "" }: { trackId: number; size: number; className?: string }) {
  const [failed, setFailed] = useState(false);
  const style = { width: size, height: size };
  if (failed) {
    return (
      <div className={`artwork artwork--empty ${className}`} style={style}>
        <svg viewBox="0 0 32 32" width={size * 0.42} height={size * 0.42} aria-hidden="true">
          <path d="M12 6l14-3v18.5a4.5 4 0 1 1-2.6-3.6V8.6L14.6 10.5v13.8a4.5 4 0 1 1-2.6-3.6z" />
        </svg>
      </div>
    );
  }
  return (
    <img
      className={`artwork ${className}`}
      style={style}
      src={artworkUrl(trackId, { type: "library" }, size)}
      alt=""
      loading="lazy"
      draggable={false}
      onError={() => setFailed(true)}
    />
  );
}
