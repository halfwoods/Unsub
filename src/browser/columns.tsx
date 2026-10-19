import type { ReactNode } from "react";
import type { SortKey, Track } from "../api/types";
import { formatClock } from "../util/format";
import { Rating } from "./Rating";
import { SongCheckbox } from "./SongCheckbox";

export interface Column {
  id: string;
  title: string;
  width: number;
  /** Flexible columns share leftover width in proportion to `width`. */
  flex?: boolean;
  align?: "left" | "right" | "center";
  sort?: SortKey;
  render: (track: Track, selected: boolean) => ReactNode;
}

// iTunes 10's default Music columns, in its order.
export const COLUMNS: Column[] = [
  { id: "enabled", title: "", width: 22, align: "center", render: (t, selected) => <SongCheckbox track={t} selected={selected} /> },
  { id: "name", title: "Name", width: 240, flex: true, sort: "name", render: (t) => t.title },
  { id: "time", title: "Time", width: 52, align: "right", sort: "time", render: (t) => formatClock(t.durationMs) },
  { id: "artist", title: "Artist", width: 170, flex: true, sort: "artist", render: (t) => t.artist },
  { id: "album", title: "Album", width: 170, flex: true, sort: "album", render: (t) => t.album },
  { id: "genre", title: "Genre", width: 100, sort: "genre", render: (t) => t.genre },
  {
    id: "rating",
    title: "Rating",
    width: 74,
    sort: "rating",
    render: (t, selected) => <Rating trackId={t.id} rating={t.rating} selected={selected} />,
  },
  { id: "plays", title: "Plays", width: 44, align: "right", sort: "plays", render: (t) => (t.playCount > 0 ? t.playCount : "") },
];

export const STATUS_COLUMN_WIDTH = 20;
