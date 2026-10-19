import { OwnershipView } from "../collection/OwnershipView";
import { DeviceSummary } from "../devices/DeviceSummary";
import {
  isDeviceSource,
  isLibrarySource,
  parseDeviceSource,
  useApp,
  type LibrarySource,
  type PlacardSource,
} from "../state/store";
import { SongList } from "./SongList";
import "./Content.css";

// Honest placeholders in iTunes' intro-placard style for sources that land in later milestones.
const PLACARDS: Record<LibrarySource | PlacardSource, { title: string; body: string }> = {
  music: {
    title: "Music",
    body: "Your music library is empty. Drag folders of music onto this window, or choose File › Add to Library…",
  },
  podcasts: {
    title: "Podcasts",
    body: "Podcast episodes you add to your library appear here. Subscribing to feeds arrives in milestone 8.",
  },
  audiobooks: {
    title: "Audiobooks",
    body: "Audiobooks you add (.m4b files, or anything tagged with the Audiobook genre) appear here.",
  },
  inbox: {
    title: "Purchases Inbox",
    body: "Point AntiStreaming at your downloads folder and new Bandcamp, Qobuz and 7digital purchases import themselves. Arrives in milestone 10.",
  },
  purchased: {
    title: "Purchased",
    body: "Every album you bought, wherever you bought it. Arrives in milestone 10.",
  },
  mixes: {
    title: "Mixes",
    body: "Playlists of songs that go great together, built on this computer from your own listening. Arrives in milestone 11.",
  },
  partyShuffle: {
    title: "Party Shuffle",
    body: "A queue that never runs dry: it keeps adding songs from your library, and you can reorder what's up next. Arrives in milestone 4.",
  },
  smart90s: { title: "90’s Music", body: "Smart playlists arrive in milestone 4." },
  smartClassical: { title: "Classical Music", body: "Smart playlists arrive in milestone 4." },
  smartTopRated: { title: "My Top Rated", body: "Smart playlists arrive in milestone 4." },
  smartRecentlyAdded: { title: "Recently Added", body: "Smart playlists arrive in milestone 4." },
  smartRecentlyPlayed: { title: "Recently Played", body: "Smart playlists arrive in milestone 4." },
  smartTop25: { title: "Top 25 Most Played", body: "Smart playlists arrive in milestone 4." },
};

function Placard({ source }: { source: LibrarySource | PlacardSource }) {
  const { title, body } = PLACARDS[source];
  return (
    <div className="placard">
      <div className="placard-card">
        <h1>{title}</h1>
        <p>{body}</p>
      </div>
    </div>
  );
}

export function Content() {
  const source = useApp((s) => s.source);
  const devices = useApp((s) => s.devices);
  const hasTracks = useApp((s) => s.tracks.length > 0);
  const searching = useApp((s) => s.query.search !== null);
  const loaded = useApp((s) => s.stats !== null);

  if (isDeviceSource(source)) {
    const device = parseDeviceSource(source);
    const found = devices.find((d) => d.id === device?.deviceId);
    if (!found) return null;
    return device?.view ? <SongList /> : <DeviceSummary device={found} />;
  }
  if (source === "toOwn") return <OwnershipView />;
  if (isLibrarySource(source) && (hasTracks || searching || !loaded)) return <SongList />;
  return <Placard source={source} />;
}
