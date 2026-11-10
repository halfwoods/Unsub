import { Lcd } from "./Lcd";
import { SearchField } from "./SearchField";
import { TrafficLights } from "./TrafficLights";
import { Transport } from "./Transport";
import { ViewButtons } from "./ViewButtons";
import { Volume } from "./Volume";
import "./Toolbar.css";

export function Toolbar() {
  return (
    <header className="toolbar chrome-surface">
      <TrafficLights />
      <Transport />
      <Volume />
      <div className="toolbar-lcd">
        <Lcd />
      </div>
      <div className="toolbar-right">
        <ViewButtons />
        <SearchField />
      </div>
    </header>
  );
}
