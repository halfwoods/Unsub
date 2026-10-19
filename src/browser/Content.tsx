import { OwnershipView } from "../collection/OwnershipView";
import { DeviceSummary } from "../devices/DeviceSummary";
import { MixesView } from "../mixes/MixesView";
import { PartyShuffleView } from "../party/PartyShuffleView";
import { PodcastsView } from "../podcasts/PodcastsView";
import { InboxView, PurchasedView } from "../purchases/PurchasesViews";
import { isDeviceSource, isPlaylistSource, parseDeviceSource, useApp } from "../state/store";
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
  switch (source) {
    case "toOwn":
      return <OwnershipView />;
    case "partyShuffle":
      return <PartyShuffleView />;
    case "mixes":
      return <MixesView />;
    case "podcasts":
      return <PodcastsView />;
    case "inbox":
      return <InboxView />;
    case "purchased":
      return <PurchasedView />;
  }
  const placard = PLACARDS[source];
  // Playlists always show their list, even empty, so songs can be dropped into it.
  if (isPlaylistSource(source) || hasTracks || searching || !loaded || !placard) return <Songs />;
  return (
    <div className="placard">
      <div className="placard-card">
        <h1>{placard.title}</h1>
        <p>{placard.body}</p>
      </div>
    </div>
  );
}
