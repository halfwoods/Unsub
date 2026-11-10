import { Lcd } from "./Lcd";
import { TrafficLights } from "./TrafficLights";
import { useTitleBar } from "./titleBar";
import { Transport } from "./Transport";
import { Volume } from "./Volume";
import "./MiniPlayer.css";

const VOLUME_TRACK_WIDTH = 64;

export function MiniPlayer() {
  useTitleBar();
  return (
    <header className="mini-player chrome-surface">
      <TrafficLights />
      <Transport />
      <Volume trackWidth={VOLUME_TRACK_WIDTH} />
      <div className="mini-player-lcd">
        <Lcd />
      </div>
    </header>
  );
}
