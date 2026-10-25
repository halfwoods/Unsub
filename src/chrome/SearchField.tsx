import { useEffect, useRef, useState } from "react";
import { useApp, type SourceId } from "../state/store";
import "./SearchField.css";

/** iTunes filters as you type; a short debounce keeps big libraries responsive. */
const DEBOUNCE_MS = 150;

const PLACEHOLDER: Partial<Record<SourceId, string>> = {
  music: "Search Music",
  podcasts: "Search Podcasts",
  audiobooks: "Search Audiobooks",
  radio: "Search Radio",
};

export function SearchField() {
  const source = useApp((s) => s.source);
  const setSearch = useApp((s) => s.setSearch);
  const [text, setText] = useState(() => useApp.getState().query.search ?? "");
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const applyingInput = useRef(false);
  const cancelInput = () => { clearTimeout(timer.current); timer.current = undefined; };

  useEffect(() => {
    // Navigation can reset search without changing the source. Subscribe synchronously
    // so a pending keystroke cannot restore a filter after Go to Current Song.
    const unsubscribe = useApp.subscribe((state, previous) => {
      const revealed = state.songReveal !== null && state.songReveal !== previous.songReveal;
      if (state.source !== previous.source || revealed || (!applyingInput.current && state.query.search !== previous.query.search)) {
        cancelInput();
        setText(state.query.search ?? "");
      }
    });
    setText(useApp.getState().query.search ?? "");
    return () => { unsubscribe(); cancelInput(); };
  }, []);

  const changeText = (value: string) => {
    setText(value);
    cancelInput();
    timer.current = setTimeout(() => {
      timer.current = undefined;
      applyingInput.current = true;
      try { setSearch(value); } finally { applyingInput.current = false; }
    }, DEBOUNCE_MS);
  };

  return (
    <label className="search-field">
      <svg className="search-glass" width={14} height={12} viewBox="0 0 14 12" aria-hidden="true">
        <circle cx="4.5" cy="4.5" r="3.5" />
        <path d="M7 7l3 3" />
        <path className="search-caret" d="M10.5 3.5h3l-1.5 2z" />
      </svg>
      <input
        type="text"
        value={text}
        placeholder={PLACEHOLDER[source] ?? "Search"}
        spellCheck={false}
        onChange={(e) => changeText(e.target.value)}
        onKeyDown={(e) => e.key === "Escape" && changeText("")}
      />
      {text && (
        <button className="search-clear" aria-label="Clear search" onClick={() => changeText("")}>
          <svg width={7} height={7} viewBox="0 0 7 7" aria-hidden="true">
            <path d="M1 1l5 5M6 1L1 6" />
          </svg>
        </button>
      )}
    </label>
  );
}
