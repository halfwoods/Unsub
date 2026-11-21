import { Lcd } from "./Lcd";
import { TrafficLights } from "./TrafficLights";
import { Transport } from "./Transport";
import { Volume } from "./Volume";
import "./MiniPlayer.css";
import { usesNativeTitleBar } from "./windowPlatform";

const VOLUME_TRACK_WIDTH = 78;

export function MiniPlayer() {
  return (
    <header className="mini-player chrome-surface">
      {!usesNativeTitleBar && <TrafficLights />}
      <Transport />
      <Volume trackWidth={VOLUME_TRACK_WIDTH} />
      <div className="mini-player-lcd">
        <Lcd />
      </div>
    </header>
  );
}
