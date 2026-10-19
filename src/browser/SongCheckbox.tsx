import type { MouseEvent } from "react";
import { librarySetEnabled } from "../api/commands";
import type { Track } from "../api/types";
import { useApp } from "../state/store";

/** iTunes' per-song checkbox: unchecked songs are skipped by playback and smart playlists can ignore them. */
export function SongCheckbox({ track, selected }: { track: Track; selected: boolean }) {
  const onClick = (e: MouseEvent) => {
    e.stopPropagation();
    const { selection, refresh } = useApp.getState();
    // Clicking a box inside a multi-selection applies to the whole selection, as in iTunes.
    const ids = selected && selection.size > 1 ? [...selection] : [track.id];
    void librarySetEnabled(ids, !track.enabled).then(refresh);
  };
  return (
    <input
      type="checkbox"
      className="song-checkbox"
      checked={track.enabled}
      aria-label={track.enabled ? `Uncheck ${track.title}` : `Check ${track.title}`}
      onChange={() => undefined}
      onMouseDown={(e) => e.stopPropagation()}
      onClick={onClick}
    />
  );
}
