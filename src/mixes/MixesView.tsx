import { useEffect, useState } from "react";
import { libraryMixes, onLibraryChanged } from "../api/commands";
import type { Mix } from "../api/types";
import { Artwork } from "../browser/Artwork";
import { useApp } from "../state/store";
import "./MixesView.css";

const TILE = 180;

/** iTunes 10's Genius Mixes, computed on this Mac: a mosaic tile per mix, click to play. */
export function MixesView() {
  const [mixes, setMixes] = useState<Mix[] | null>(null);
  const playTrackIds = useApp((s) => s.playTrackIds);

  useEffect(() => {
    const refresh = () => void libraryMixes().then(setMixes);
    refresh();
    const subscription = onLibraryChanged(refresh);
    return () => void subscription.then((unlisten) => unlisten());
  }, []);

  if (mixes === null) return <div className="mixes" />;
  if (mixes.length === 0) {
    return (
      <div className="mixes mixes--empty">
        <h1>Mixes</h1>
        <p>Mixes appear once your library has enough songs in common genres or by the same artists. Add more music, play it for a while, and they'll show up here.</p>
      </div>
    );
  }
  return (
    <div className="mixes">
      <div className="mixes-grid">
        {mixes.map((mix) => (
          <button key={mix.key} className="mix-tile" style={{ width: TILE }} onClick={() => void playTrackIds(mix.trackIds)} aria-label={`Play ${mix.name}`}>
            <span className="mix-mosaic" style={{ width: TILE, height: TILE }}>
              {Array.from({ length: 4 }, (_, i) => {
                const id = mix.artworkTrackIds[i % Math.max(1, mix.artworkTrackIds.length)];
                return id !== undefined ? <Artwork key={`${i}:${id}`} trackId={id} size={TILE / 2} /> : <span key={i} />;
              })}
              <span className="mix-play" aria-hidden="true">
                <svg width={18} height={20} viewBox="0 0 18 20">
                  <path d="M2 1v18l15-9z" />
                </svg>
              </span>
            </span>
            <span className="mix-name">{mix.name}</span>
          </button>
        ))}
      </div>
    </div>
  );
}
