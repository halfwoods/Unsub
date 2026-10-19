import { OwnershipView } from "../collection/OwnershipView";
import { PartyShuffleView } from "../party/PartyShuffleView";
import { DeviceSummary } from "../devices/DeviceSummary";
import {
  isDeviceSource,
  isLibrarySource,
  isPlaylistSource,
  parseDeviceSource,
  useApp,
  type LibrarySource,
  type PlacardSource,
} from "../state/store";
import { AlbumGrid } from "./AlbumGrid";
import { AlbumList } from "./AlbumList";
import { CoverFlow } from "./CoverFlow";
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
  const viewMode = useApp((s) => s.viewMode);

  if (isDeviceSource(source)) {
    const device = parseDeviceSource(source);
    const found = devices.find((d) => d.id === device?.deviceId);
    if (!found) return null;
    return device?.view ? <SongList /> : <DeviceSummary device={found} />;
  }
  if (source === "toOwn") return <OwnershipView />;
  if (source === "partyShuffle") return <PartyShuffleView />;
  // Playlists always show their list, even empty, so songs can be dropped into it.
  if (isPlaylistSource(source) || (isLibrarySource(source) && (hasTracks || searching || !loaded))) {
    if (viewMode === "albumList") return <AlbumList />;
    if (viewMode === "grid") return <AlbumGrid />;
    if (viewMode === "coverFlow") return <CoverFlow />;
    return <SongList />;
  }
  return <Placard source={source} />;
}
