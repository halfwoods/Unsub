import type { IpodFamily } from "../api/types";

/** Our own drawings of each iPod shape, sized like the Summary page's device image. */
export function IpodArt({ family, color }: { family: IpodFamily; color: string | null }) {
  const dark = color !== null && /black|graphite|slate|space/i.test(color);
  const body = dark ? ["#4a4a4a", "#1d1d1d"] : ["#fbfbfb", "#d9dadc"];
  const wheel = dark ? "#2b2b2b" : "#f4f4f4";
  const wheelInk = dark ? "#9a9a9a" : "#9b9b9b";

  if (family === "shuffle") {
    return (
      <svg width={72} height={88} viewBox="0 0 72 88" aria-hidden="true">
        <rect x="6" y="6" width="60" height="76" rx="9" fill="url(#ipod-body)" stroke="#8d8d8d" />
        <circle cx="36" cy="44" r="20" fill={wheel} stroke="#bdbdbd" />
        <circle cx="36" cy="44" r="7" fill="url(#ipod-body)" stroke="#c6c6c6" />
        <Defs body={body} />
      </svg>
    );
  }

  const nano = family === "nano";
  const [w, h] = nano ? [64, 150] : family === "mini" ? [76, 150] : [96, 160];
  const screenH = nano ? 46 : 62;
  const wheelR = nano ? 21 : family === "mini" ? 25 : 31;
  const wheelY = 10 + screenH + (h - 10 - screenH) / 2;

  return (
    <svg width={w + 8} height={h + 8} viewBox={`0 0 ${w + 8} ${h + 8}`} aria-hidden="true">
      <Defs body={body} />
      <rect x="4" y="4" width={w} height={h} rx={nano ? 9 : 12} fill="url(#ipod-body)" stroke="#8a8a8a" />
      <rect x="11" y="13" width={w - 14} height={screenH} rx="2" fill="#2d3540" />
      <rect x="13" y="15" width={w - 18} height={screenH - 4} fill="url(#ipod-screen)" />
      <g fill="#3b4a5c" opacity="0.65">
        <rect x="16" y="19" width={w - 24} height="5" />
        <rect x="16" y="28" width={(w - 24) * 0.7} height="3" />
        <rect x="16" y="35" width={(w - 24) * 0.8} height="3" />
        <rect x="16" y="42" width={(w - 24) * 0.6} height="3" />
      </g>
      <circle cx={4 + w / 2} cy={wheelY} r={wheelR} fill={wheel} stroke={dark ? "#111" : "#c4c4c4"} />
      <circle cx={4 + w / 2} cy={wheelY} r={wheelR * 0.36} fill="url(#ipod-body)" stroke={dark ? "#111" : "#cfcfcf"} />
      <text x={4 + w / 2} y={wheelY - wheelR * 0.62} fontSize={nano ? 5 : 6} fill={wheelInk} textAnchor="middle" fontFamily="var(--font-ui)">
        MENU
      </text>
    </svg>
  );
}

function Defs({ body }: { body: string[] }) {
  return (
    <defs>
      <linearGradient id="ipod-body" x1="0" y1="0" x2="1" y2="1">
        <stop offset="0" stopColor={body[0]} />
        <stop offset="1" stopColor={body[1]} />
      </linearGradient>
      <linearGradient id="ipod-screen" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0" stopColor="#e9f0f7" />
        <stop offset="1" stopColor="#bccbdb" />
      </linearGradient>
    </defs>
  );
}
