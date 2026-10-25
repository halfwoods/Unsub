import { useState } from "react";
import { artworkUrl } from "../api/commands";
import { artistInitials } from "./browseHelpers";

/** At most four local album covers; no network lookup or per-tile library query. */
export function ArtistArtwork({ name, trackIds, size }: { name: string; trackIds: number[]; size: number }) {
  const [failed, setFailed] = useState<ReadonlySet<number>>(new Set());
  const covers = trackIds.filter((id) => !failed.has(id)).slice(0, 4);
  return (
    <div className={`album-tile-art artist-mosaic${covers.length > 1 ? " artist-mosaic--multiple" : ""}${covers.length === 2 ? " artist-mosaic--two" : ""}`}
      style={{ width: size, height: size }} aria-hidden="true">
      {covers.length === 0 ? <span className="artist-mosaic-initials">{artistInitials(name)}</span> : covers.map((id) => (
        <img key={id} src={artworkUrl(id, { type: "library" }, covers.length > 1 ? size / 2 : size)}
          alt="" loading="lazy" draggable={false} onError={() => setFailed((previous) => new Set([...previous, id]))} />
      ))}
      {covers.length === 3 && <span className="artist-mosaic-initials">{artistInitials(name)}</span>}
    </div>
  );
}
