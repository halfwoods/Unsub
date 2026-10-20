import { libraryAddPaths, radioImport } from "../api/commands";
import { useApp } from "../state/store";
import { pluralize } from "../util/format";

const STATION_LIST = /\.(pls|m3u)$/i;

/** Dropped .pls and .m3u files add their stations to My Stations; everything else goes to the library. */
export function addDropped(paths: string[]) {
  const lists = paths.filter((path) => STATION_LIST.test(path));
  const rest = paths.filter((path) => !STATION_LIST.test(path));
  if (rest.length > 0) void libraryAddPaths(rest);
  if (lists.length === 0) return;
  const { showNotice } = useApp.getState();
  Promise.all(lists.map(radioImport)).then(
    (added) => {
      const count = new Set(added.flat()).size;
      showNotice(count === 0 ? "No stations found" : `Added ${pluralize(count, "station", "stations")}`);
    },
    (e: unknown) => showNotice(String(e)),
  );
}
