export const IS_MAC = navigator.userAgent.includes("Mac");
export const IS_LINUX = navigator.userAgent.includes("Linux");
export const IS_WINDOWS = navigator.userAgent.includes("Windows");

/** A right-click, or Ctrl-click on a Mac: the clicks that open a context menu rather than select. */
export const isContextClick = (e: { button: number; ctrlKey: boolean }) => e.button === 2 || (IS_MAC && e.ctrlKey);
