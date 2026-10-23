/** Native file drags are separate from dragging songs within the library. */
export type FileDrop =
  | { type: "enter"; paths: string[] }
  | { type: "over" }
  | { type: "leave" }
  | { type: "drop"; paths: string[] };

type DropListener = (handler: (event: FileDrop) => void) => Promise<() => void>;

/** A subscription can finish after React has already disposed its effect. */
export function watchFileDrops(
  listen: DropListener,
  dragging: (active: boolean) => void,
  drop: (paths: string[]) => Promise<void>,
  report: (error: unknown) => void,
) {
  let active = true;
  let stop: (() => void) | undefined;
  void listen((event) => {
    if (!active) return;
    if (event.type === "enter") dragging(event.paths.length > 0);
    else if (event.type !== "over") {
      dragging(false);
      if (event.type === "drop" && event.paths.length > 0) void drop(event.paths).catch(report);
    }
  }).then(
    (unlisten) => {
      if (active) stop = unlisten;
      else unlisten();
    },
    (error: unknown) => active && report(error),
  );
  return () => {
    active = false;
    stop?.();
  };
}

interface DropImports {
  addMusic: (paths: string[]) => Promise<boolean>;
  musicAdded: () => Promise<void>;
  addStations: (path: string) => Promise<number[]>;
  stationsAdded: (count: number) => void;
  report: (error: unknown) => void;
}

const STATION_LIST = /\.(pls|m3u)$/i;

/** A mixed drop still adds its music when a station list fails, and vice versa. */
export async function importDropped(paths: string[], actions: DropImports) {
  const lists = paths.filter((path) => STATION_LIST.test(path));
  const music = paths.filter((path) => !STATION_LIST.test(path));
  await Promise.all([
    (async () => {
      if (music.length === 0) return;
      try {
        if (await actions.addMusic(music)) await actions.musicAdded();
      } catch (error) {
        actions.report(error);
      }
    })(),
    (async () => {
      if (lists.length === 0) return;
      try {
        const added = await Promise.all(lists.map(actions.addStations));
        actions.stationsAdded(new Set(added.flat()).size);
      } catch (error) {
        actions.report(error);
      }
    })(),
  ]);
}
