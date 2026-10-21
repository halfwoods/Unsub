import { useEffect, useState, type MouseEvent } from "react";
import { libraryCreatePlaylist, libraryMixes, onLibraryChanged } from "../api/commands";
import type { Mix } from "../api/types";
import { Artwork } from "../browser/Artwork";
import { SEPARATOR, showMenu } from "../browser/songMenu";
import { useApp } from "../state/store";
import { isContextClick } from "../util/platform";
import "./MixesView.css";

const TILE = 180;

/** iTunes 10's Genius Mixes, computed on this Mac: a mosaic tile per mix, click to play. */
export function MixesView() {
  const [mixes, setMixes] = useState<Mix[] | null>(null);
  const playTrackIds = useApp((s) => s.playTrackIds);
  const showNotice = useApp((s) => s.showNotice);
  const refreshPlaylists = useApp((s) => s.refreshPlaylists);
  const setRenamingPlaylist = useApp((s) => s.setRenamingPlaylist);

  useEffect(() => {
    const refresh = () => void libraryMixes().then(setMixes);
    refresh();
    const subscription = onLibraryChanged(refresh);
    return () => void subscription.then((unlisten) => unlisten());
  }, []);

  // The new playlist stays out of the way of the mix being browsed, but its name is ready to type over in the source list.
  const saveAsPlaylist = async (mix: Mix) => {
    const playlist = await libraryCreatePlaylist(mix.name, "regular", null, mix.trackIds);
    await refreshPlaylists();
    setRenamingPlaylist(playlist.id);
  };

  const onContextMenu = (e: MouseEvent, mix: Mix) => {
    e.preventDefault();
    const report = (error: unknown) => showNotice(String(error));
    void showMenu([
      { text: "Play", action: () => void playTrackIds(mix.trackIds).catch(report) },
      { text: "Shuffle", action: () => void playTrackIds(mix.trackIds, true).catch(report) },
      SEPARATOR,
      { text: "Save as Playlist", action: () => void saveAsPlaylist(mix).catch(report) },
    ]);
  };

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
          <button
            key={mix.key}
            className="mix-tile"
            style={{ width: TILE }}
            // Ctrl-click on a Mac is a context click that WebKit reports as a click too, which must not play the mix.
            onClick={(e) => !isContextClick(e) && void playTrackIds(mix.trackIds)}
            onContextMenu={(e) => onContextMenu(e, mix)}
            aria-label={`Play ${mix.name}`}
          >
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
