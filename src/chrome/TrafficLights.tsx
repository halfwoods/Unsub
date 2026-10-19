import { getCurrentWindow } from "@tauri-apps/api/window";
import "./TrafficLights.css";

const appWindow = getCurrentWindow();

// iTunes 10 stacked the window buttons vertically to save toolbar width.
export function TrafficLights() {
  return (
    <div className="traffic-lights">
      <button className="traffic-light traffic-light--close" aria-label="Close" onClick={() => appWindow.close()}>
        <svg viewBox="0 0 8 8" aria-hidden="true">
          <path d="M1.5 1.5l5 5M6.5 1.5l-5 5" />
        </svg>
      </button>
      <button className="traffic-light traffic-light--minimize" aria-label="Minimize" onClick={() => appWindow.minimize()}>
        <svg viewBox="0 0 8 8" aria-hidden="true">
          <path d="M1 4h6" />
        </svg>
      </button>
      <button className="traffic-light traffic-light--zoom" aria-label="Zoom" onClick={() => appWindow.toggleMaximize()}>
        <svg viewBox="0 0 8 8" aria-hidden="true">
          <path d="M1 4h6M4 1v6" />
        </svg>
      </button>
    </div>
  );
}
