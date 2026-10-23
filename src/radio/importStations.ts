import { radioImport } from "../api/commands";
import { addMusicPaths, completeMusicWelcome } from "../import/addMusic";
import { importDropped } from "../import/fileDrop";
import { useApp } from "../state/store";
import { pluralize } from "../util/format";

/** Dropped .pls and .m3u files add their stations to My Stations; everything else goes to the library. */
export async function addDropped(paths: string[]) {
  const { showNotice } = useApp.getState();
  await importDropped(paths, {
    addMusic: addMusicPaths,
    musicAdded: completeMusicWelcome,
    addStations: radioImport,
    stationsAdded: (count) => {
      showNotice(count === 0 ? "No stations found" : `Added ${pluralize(count, "station", "stations")}`);
    },
    report: (error) => showNotice(String(error)),
  });
}
