import type { MouseEvent } from "react";
import { librarySetRating } from "../api/commands";
import { useApp } from "../state/store";

const STAR = "M5 0.6l1.3 2.9 3.1.3-2.4 2.1.7 3.1L5 7.4 2.3 9l.7-3.1L.6 3.8l3.1-.3z";

/** Five stars stored as 0–100 in steps of 20, like iTunes. Empty slots show dots on selected rows. */
export function Rating({ trackId, rating, selected }: { trackId: number; rating: number; selected: boolean }) {
  const refresh = useApp((s) => s.refresh);
  const stars = Math.round(rating / 20);

  const onClick = (e: MouseEvent<HTMLSpanElement>) => {
    e.stopPropagation();
    const rect = e.currentTarget.getBoundingClientRect();
    const clicked = Math.min(5, Math.max(0, Math.ceil(((e.clientX - rect.left) / rect.width) * 5)));
    // Clicking the current rating's last star clears it, as in iTunes.
    const next = clicked === stars ? 0 : clicked;
    void librarySetRating(trackId, next * 20).then(refresh);
  };

  return (
    <span className="rating" onClick={onClick} onDoubleClick={(e) => e.stopPropagation()}>
      {[0, 1, 2, 3, 4].map((i) =>
        i < stars ? (
          <svg key={i} width={10} height={10} viewBox="0 0 10 10" aria-hidden="true">
            <path d={STAR} />
          </svg>
        ) : (
          <span key={i} className={selected ? "rating-dot rating-dot--shown" : "rating-dot"} />
        ),
      )}
    </span>
  );
}
