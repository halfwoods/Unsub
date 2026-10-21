import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { windowDoubleClickAction } from "../api/commands";
import { useApp } from "../state/store";

const appWindow = getCurrentWindow();

const BARS = ".toolbar, .bottom-bar";
// What reacts to clicks itself. The LCD scrubs and opens menus, and the CD import sheet is drawn inside the bottom bar.
const OWN_CLICKS =
  'button, input, select, textarea, a, label, [role="slider"], [role="radiogroup"], [contenteditable], [tabindex], .lcd, .sheet-backdrop';

const onTitleBar = (target: EventTarget | null) => {
  if (!(target instanceof Element)) return false;
  const bar = target.closest(BARS);
  const control = target.closest(OWN_CLICKS);
  return bar !== null && (control === null || !bar.contains(control));
};

async function doubleClick() {
  const action = await windowDoubleClickAction();
  if (action === "zoom") await appWindow.toggleMaximize();
  else if (action === "minimize") await appWindow.minimize();
}

/**
 * The bars drag the window and answer a double-click from any point that isn't a control, as a native title bar does.
 * Tauri's drag script can't: it only acts on the element that carries data-tauri-drag-region itself, which leaves
 * out the containers between the controls, and it knows nothing of the system's choice between zooming,
 * minimizing and nothing. So the bars carry no such attribute, and this decides by what was clicked.
 */
export function useTitleBar() {
  useEffect(() => {
    const onMouseDown = (e: MouseEvent) => {
      // A full-screen window has nowhere to be moved to, and nothing to zoom.
      if (e.button !== 0 || useApp.getState().windowFullscreen || !onTitleBar(e.target)) return;
      // Keeps the press from starting a text selection or a drag image, and from taking focus off the search field.
      e.preventDefault();
      if (e.detail === 1) void appWindow.startDragging();
      else if (e.detail === 2) void doubleClick();
    };
    document.addEventListener("mousedown", onMouseDown);
    return () => document.removeEventListener("mousedown", onMouseDown);
  }, []);
}
