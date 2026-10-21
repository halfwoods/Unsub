import { OwnershipView } from "../collection/OwnershipView";
import { DeviceSummary, PlayerSummary } from "../devices/DeviceSummary";
import { MixesView } from "../mixes/MixesView";
import { PartyShuffleView } from "../party/PartyShuffleView";
import { PodcastsView } from "../podcasts/PodcastsView";
import { RadioView } from "../radio/RadioView";
import { InboxView, PurchasedView } from "../purchases/PurchasesViews";
import { SharedLogin } from "../shared/Shared";
import { cdIdOf, isDeviceSource, isLibrarySource, isPlaylistSource, parseDeviceSource, playerIdOf, sharedIdOf, useApp } from "../state/store";
import { AlbumGrid } from "./AlbumGrid";
import { AlbumList } from "./AlbumList";
import { CoverFlow } from "./CoverFlow";
import { SongList } from "./SongList";
import "./Content.css";

// iTunes' intro placards for an empty library.
const PLACARDS: Partial<Record<string, { title: string; body: string }>> = {
  music: {
    title: "Music",
    body: "Your music library is empty. Drag folders of music onto this window, or choose File › Add to Library…",
  },
  audiobooks: {
    title: "Audiobooks",
    body: "Audiobooks you add (.m4b files, or anything tagged with the Audiobook genre) appear here.",
  },
};

function Songs() {
  const viewMode = useApp((s) => s.viewMode);
  if (viewMode === "albumList") return <AlbumList />;
  if (viewMode === "grid") return <AlbumGrid />;
  if (viewMode === "coverFlow") return <CoverFlow />;
  return <SongList />;
}

function DuplicatesBar({ exact }: { exact: boolean }) {
  const showAllSongs = useApp((s) => s.showAllSongs);
  return (
    <div className="party-bar duplicates-bar">
      <span>{exact ? "Displaying Exact Duplicates" : "Displaying Duplicates"}</span>
      <button className="push-button" onClick={showAllSongs}>
        Show All
      </button>
    </div>
  );
}

export function Content() {
  const source = useApp((s) => s.source);
  const devices = useApp((s) => s.devices);
  const players = useApp((s) => s.players);
  const shared = useApp((s) => s.shared);
  const hasTracks = useApp((s) => s.tracks.length > 0);
  const searching = useApp((s) => s.query.search !== null);
  const loaded = useApp((s) => s.stats !== null);
  const duplicates = useApp((s) => s.duplicates);
  const welcoming = useApp((s) => s.welcoming);

  if (isDeviceSource(source)) {
    const device = parseDeviceSource(source);
    const found = devices.find((d) => d.id === device?.deviceId);
    if (!found) return null;
    return device?.view ? <SongList /> : <DeviceSummary device={found} />;
  }
  const playerId = playerIdOf(source);
  if (playerId !== null) {
    const player = players.find((p) => p.id === playerId);
    return player ? <PlayerSummary player={player} /> : null;
  }
  const sharedId = sharedIdOf(source);
  if (sharedId !== null) {
    const library = shared.find((l) => l.id === sharedId);
    if (!library) return null;
    if (!library.hasLogin) return <SharedLogin library={library} />;
    // Songs list as they arrive; the bottom bar's count climbs while it loads.
    return <SongList />;
  }
  // A CD has no albums to browse, so it is always the song list.
  if (cdIdOf(source) !== null) return <SongList />;
  switch (source) {
    case "toOwn":
      return <OwnershipView />;
    case "partyShuffle":
      return <PartyShuffleView />;
    case "mixes":
      return <MixesView />;
    case "podcasts":
      return <PodcastsView />;
    case "radio":
      return <RadioView />;
    case "inbox":
      return <InboxView />;
    case "purchased":
      return <PurchasedView />;
  }
  if (duplicates && isLibrarySource(source)) {
    return (
      <div className="content-with-bar">
        <Songs />
        <DuplicatesBar exact={duplicates.exact} />
      </div>
    );
  }
  const placard = PLACARDS[source];
  // Playlists always show their list, even empty, so songs can be dropped into it.
  if (isPlaylistSource(source) || hasTracks || searching || !loaded || !placard) return <Songs />;
  return (
    <div className="placard">
      {/* The welcome says how to add music itself, and this would show around the edges of its sheet. */}
      {!welcoming && (
        <div className="placard-card">
          <h1>{placard.title}</h1>
          <p>{placard.body}</p>
        </div>
      )}
    </div>
  );
}
