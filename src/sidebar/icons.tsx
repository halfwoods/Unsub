// 16px monochrome source-list icons, embossed like iTunes 10's gray sidebar glyphs.

const PATHS = {
  music: { fill: "M6 2.5l8-2v9.2a2.3 2 0 1 1-1.4-1.85V3.4L7.4 4.7v6.9A2.3 2 0 1 1 6 9.75z", stroke: "" },
  podcasts: {
    fill: "M8 5.2a1.8 1.8 0 1 1 0 3.6 1.8 1.8 0 0 1 0-3.6zM6.9 9.6h2.2l-.4 5.4H7.3z",
    stroke: "M4.6 10.4a4.4 4.4 0 1 1 6.8 0M2.7 12.2a7 7 0 1 1 10.6 0",
  },
  audiobooks: { fill: "", stroke: "M8 4C6 2.6 3.5 2.4 1.5 2.8v10c2-.4 4.5-.2 6.5 1.2 2-1.4 4.5-1.6 6.5-1.2v-10C12.5 2.4 10 2.6 8 4zm0 0v10" },
  toOwn: { fill: "", stroke: "M8.5 1.5h5v5L7 13 1.5 7.5zM11 3.2a.9.9 0 1 0 0 1.8.9.9 0 0 0 0-1.8z" },
  inbox: { fill: "", stroke: "M1.5 9.5h4l1 2h3l1-2h4M1.5 9.5v4h13v-4l-2-5h-9zM8 1v6M5.5 4.8L8 7.2l2.5-2.4" },
  purchased: { fill: "", stroke: "M2.5 5.5h11l-1 9h-9zM5.5 7V4a2.5 2.5 0 0 1 5 0v3" },
  mixes: { fill: "", stroke: "M6 4.5a4 4 0 1 0 0 8 4 4 0 0 0 0-8zM10 3.5a4 4 0 1 0 0 8 4 4 0 0 0 0-8z" },
  partyShuffle: { fill: "", stroke: "M1 4.5h3l6 7h3M11.5 9.5l2 2-2 2M1 11.5h3l2-2.4M8.2 6.9L10 4.5h3M11.5 2.5l2 2-2 2" },
  ipod: {
    fill: "M4.5 0.5h7a2 2 0 0 1 2 2v11a2 2 0 0 1-2 2h-7a2 2 0 0 1-2-2v-11a2 2 0 0 1 2-2zM4.5 2v4.5h7V2zM8 8.2a2.9 2.9 0 1 0 0 5.8 2.9 2.9 0 0 0 0-5.8zm0 1.9a1 1 0 1 1 0 2 1 1 0 0 1 0-2z",
    stroke: "",
  },
  playlist: { fill: "M7 4.5l6-1.5v7.2a1.8 1.6 0 1 1-1.2-1.5V5.1L8.2 6v5.7A1.8 1.6 0 1 1 7 10.2z", stroke: "M1.5 3.5h3M1.5 6.5h3M1.5 9.5h3" },
  smart: {
    fill: "M7 1h2l.4 1.8 1.4.6 1.6-1 1.4 1.4-1 1.6.6 1.4 1.6.4v2l-1.8.4-.6 1.4 1 1.6-1.4 1.4-1.6-1-1.4.6L9 15H7l-.4-1.8-1.4-.6-1.6 1-1.4-1.4 1-1.6-.6-1.4L1 9V7l1.8-.4.6-1.4-1-1.6 1.4-1.4 1.6 1 1.4-.6zm1 4.5a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5z",
    stroke: "",
  },
} as const;

export type SourceIconName = keyof typeof PATHS;

export function SourceIcon({ name }: { name: SourceIconName }) {
  const { fill, stroke } = PATHS[name];
  return (
    <svg className="source-icon" width={16} height={16} viewBox="0 0 16 16" aria-hidden="true">
      {fill && <path className="source-icon-fill" d={fill} fillRule="evenodd" />}
      {stroke && <path className="source-icon-stroke" d={stroke} />}
    </svg>
  );
}
