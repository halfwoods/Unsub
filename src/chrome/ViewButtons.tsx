import { useApp, type ViewMode } from "../state/store";
import "./ViewButtons.css";

const GLYPHS: Record<ViewMode, { label: string; path: string }> = {
  list: { label: "List", path: "M0 0.5h13M0 3.5h13M0 6.5h13M0 9.5h13" },
  albumList: { label: "Album List", path: "M0 0h4v4H0zM6 0.5h7M6 3.5h7M6 6.5h7M6 9.5h7M0 6h4v4H0z" },
  grid: { label: "Grid", path: "M0 0h3.5v4H0zM4.75 0h3.5v4h-3.5zM9.5 0H13v4H9.5zM0 6h3.5v4H0zM4.75 6h3.5v4h-3.5zM9.5 6H13v4H9.5z" },
  coverFlow: { label: "Cover Flow", path: "M4 0h5v10H4zM0.5 1.5v7M2 1v8M11 1v8M12.5 1.5v7" },
};

const MODES: ViewMode[] = ["list", "albumList", "grid", "coverFlow"];

export function ViewButtons() {
  const viewMode = useApp((s) => s.viewMode);
  const setViewMode = useApp((s) => s.setViewMode);
  return (
    <div className="view-buttons" role="radiogroup" aria-label="View">
      {MODES.map((mode) => {
        const { label, path } = GLYPHS[mode];
        const filled = mode === "albumList" || mode === "grid" || mode === "coverFlow";
        return (
          <button
            key={mode}
            role="radio"
            aria-checked={viewMode === mode}
            aria-label={label}
            title={label}
            className="view-button"
            onClick={() => setViewMode(mode)}
          >
            <svg width={13} height={10} viewBox="0 0 13 10" aria-hidden="true">
              <path d={path} className={filled ? "view-glyph view-glyph--mixed" : "view-glyph"} />
            </svg>
          </button>
        );
      })}
    </div>
  );
}
