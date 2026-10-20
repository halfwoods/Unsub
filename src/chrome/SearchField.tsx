import { useEffect, useState } from "react";
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
  const [text, setText] = useState("");

  useEffect(() => setText(""), [source]);

  useEffect(() => {
    const timer = setTimeout(() => setSearch(text), DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [text, setSearch]);

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
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => e.key === "Escape" && setText("")}
      />
      {text && (
        <button className="search-clear" aria-label="Clear search" onClick={() => setText("")}>
          <svg width={7} height={7} viewBox="0 0 7 7" aria-hidden="true">
            <path d="M1 1l5 5M6 1L1 6" />
          </svg>
        </button>
      )}
    </label>
  );
}
