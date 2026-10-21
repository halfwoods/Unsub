import { Menu } from "@tauri-apps/api/menu";
import { castOpenAirplay, castSelect, castSpeakers } from "../api/commands";
import type { SpeakerKind } from "../api/types";
import { SEPARATOR, type MenuItems } from "../browser/songMenu";
import { loadBitPerfect } from "../preferences/playbackPrefs";
import { useApp } from "../state/store";
import { IS_MAC } from "../util/platform";

/** The speaker menu's groups, in order. */
const GROUPS: [SpeakerKind, string][] = [
  ["chromecast", "Chromecast"],
  ["sonos", "Sonos"],
  ["upnp", "Other speakers"],
];

/** Kept until the next click: closing a menu as it is dismissed could drop the event of the item just chosen. */
let lastMenu: Menu | null = null;

/** The bottom bar's speaker menu: this computer, then the speakers found, grouped, with the one playing checked. */
export async function showSpeakerMenu() {
  const showNotice = useApp.getState().showNotice;
  const { devices, target, airplay } = await castSpeakers();
  const choose = (id: string | null) => void castSelect(id).catch((e: unknown) => showNotice(String(e)));
  // Bit-perfect output holds the device it started on, so the engine doesn't move when the Mac's output does.
  const bitPerfect = loadBitPerfect();
  const viaAirplay = airplay && target === null && !bitPerfect;

  const items: MenuItems = [{ text: "Computer", checked: target === null && !viaAirplay, action: () => choose(null) }];
  for (const [kind, heading] of GROUPS) {
    const speakers = devices.filter((d) => d.kind === kind);
    if (speakers.length === 0) continue;
    items.push(SEPARATOR, { text: heading, enabled: false });
    for (const speaker of speakers) items.push({ text: speaker.name, checked: target?.id === speaker.id, action: () => choose(speaker.id) });
  }
  if (devices.length === 0) items.push(SEPARATOR, { text: "No speakers found", enabled: false });

  items.push(SEPARATOR);
  // AirPlay speakers are the Mac's own outputs; picking one is the system's to do, and the engine follows it.
  if (IS_MAC) {
    const open = () =>
      bitPerfect
        ? showNotice("AirPlay needs Bit-perfect output turned off, in Preferences.")
        : void castOpenAirplay().catch((e: unknown) => showNotice(String(e)));
    items.push({ text: "AirPlay…", checked: viaAirplay, action: open });
  } else {
    items.push({ text: "AirPlay isn’t available on Windows", enabled: false });
  }

  const menu = await Menu.new({ items });
  void lastMenu?.close();
  lastMenu = menu;
  await menu.popup();
}
