import { libraryMixFor } from "../api/commands";
import type { Track } from "../api/types";
import { useApp } from "../state/store";
import { pluralize } from "../util/format";

/** Which build was asked for last; an earlier one that finishes afterwards has been overtaken. */
let latestBuild = 0;

/**
 * Builds an on-device mix from `song` and plays it, saying what is happening in the LCD. The build takes a moment, so
 * the result is dropped when a newer mix was asked for, or when the song playing has changed since the click, which
 * is the user choosing something else: playing it then would override that choice.
 */
export async function startMixFrom(song: Pick<Track, "id" | "title">): Promise<void> {
  const build = ++latestBuild;
  const playingAtClick = useApp.getState().nowPlaying?.id;
  const { showNotice, playTrackIds } = useApp.getState();
  const making = `Making a mix from “${song.title}”…`;
  showNotice(making);
  try {
    const related = await libraryMixFor([song.id]);
    if (build !== latestBuild) return;
    if (useApp.getState().nowPlaying?.id !== playingAtClick) {
      if (useApp.getState().notice === making) useApp.setState({ notice: null });
      return;
    }
    if (related.length === 0) return showNotice(`Not enough related songs in your library to make a mix from “${song.title}”`);
    await playTrackIds([song.id, ...related]);
    showNotice(`Playing a mix of ${pluralize(related.length + 1, "song", "songs")} from “${song.title}”`);
  } catch (e) {
    if (build === latestBuild) showNotice(String(e));
  }
}
