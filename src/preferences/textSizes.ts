/** iTunes' General › Source Text / List Text sizes. A per-viewer convenience, so it lives in localStorage. */
export interface TextSizes {
  source: "small" | "large";
  list: "small" | "large";
}

const KEY = "antistreaming.textSizes";
const DEFAULTS: TextSizes = { source: "small", list: "small" };

export function loadTextSizes(): TextSizes {
  try {
    return { ...DEFAULTS, ...(JSON.parse(localStorage.getItem(KEY) ?? "{}") as Partial<TextSizes>) };
  } catch {
    return DEFAULTS;
  }
}

export function saveTextSizes(sizes: TextSizes) {
  try {
    localStorage.setItem(KEY, JSON.stringify(sizes));
  } catch {
    // Private or locked-down storage: the choice still applies for this session.
  }
}

/** Song rows grow with List Text, as iTunes' did; virtualized lists need the number, not just CSS. */
export const listRowHeight = (sizes: TextSizes) => (sizes.list === "large" ? 22 : 18);

/** Overrides the measured defaults in tokens.css; Large matches iTunes' 13px lists and taller rows. */
export function applyTextSizes(sizes: TextSizes) {
  const root = document.documentElement.style;
  root.setProperty("--font-size-sidebar", sizes.source === "large" ? "14px" : "12px");
  root.setProperty("--sidebar-row", sizes.source === "large" ? "28px" : "24px");
  root.setProperty("--font-size", sizes.list === "large" ? "13px" : "11px");
  root.setProperty("--list-row", `${listRowHeight(sizes)}px`);
}
