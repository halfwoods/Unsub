import { useEffect, useState } from "react";
import { playerEqualizerPresets, playerSetEqualizer } from "../api/commands";
import { loadEqualizer, saveEqualizer, type EqualizerPrefs } from "../preferences/playbackPrefs";
import { useApp } from "../state/store";
import "./EqualizerPanel.css";

const FREQUENCIES = ["32", "64", "125", "250", "500", "1K", "2K", "4K", "8K", "16K"];

/** Sends the saved equalizer to the engine; called at launch and on every change. */
export function applyEqualizer(prefs: EqualizerPrefs) {
  void playerSetEqualizer(prefs.on ? { preampDb: prefs.preampDb, bands: prefs.bands } : null, prefs.preventClipping);
}

/** One of iTunes' vertical sliders: a horizontal range turned upright, which every WebKit draws. */
function VerticalSlider({ label, value, onChange }: { label: string; value: number; onChange: (value: number) => void }) {
  return (
    <span className="eq-slider">
      <input
        type="range"
        min={-12}
        max={12}
        step={0.5}
        value={value}
        aria-label={label}
        aria-valuetext={`${value > 0 ? "+" : ""}${value} dB`}
        onChange={(e) => onChange(Number(e.target.value))}
        // Double-clicking a slider centres it, as in iTunes.
        onDoubleClick={() => onChange(0)}
      />
    </span>
  );
}

/** Window › Equalizer: iTunes 10's equalizer, as a palette that stays open while you listen. */
export function EqualizerPanel() {
  const [prefs, setPrefs] = useState<EqualizerPrefs>(loadEqualizer);
  const [presets, setPresets] = useState<{ name: string; bands: number[] }[]>([]);
  const bitPerfect = useApp((s) => s.playback.bitPerfect);
  const castTarget = useApp((s) => s.castTarget);
  const close = () => useApp.setState({ equalizerOpen: false });

  useEffect(() => {
    void playerEqualizerPresets().then(setPresets);
  }, []);

  const update = (next: EqualizerPrefs) => {
    setPrefs(next);
    saveEqualizer(next);
    applyEqualizer(next);
  };
  const setBand = (index: number, db: number) =>
    // Moving a slider by hand turns any preset into "Manual", and switches the equalizer on.
    update({ ...prefs, on: true, preset: null, bands: prefs.bands.map((b, i) => (i === index ? db : b)) });
  const choosePreset = (name: string) => {
    const preset = presets.find((p) => p.name === name);
    if (preset) update({ ...prefs, on: true, preset: preset.name, bands: [...preset.bands] });
  };

  return (
    <div className="eq-panel" role="dialog" aria-label="Equalizer">
      <header className="eq-titlebar">
        <button className="eq-close" aria-label="Close the equalizer" onClick={close} />
        <span>Equalizer</span>
      </header>
      <div className="eq-controls">
        <label className="prefs-check">
          <input type="checkbox" checked={prefs.on} onChange={(e) => update({ ...prefs, on: e.target.checked })} />
          On
        </label>
        <select className="smart-popup" aria-label="Preset" value={prefs.preset ?? ""} onChange={(e) => choosePreset(e.target.value)}>
          {prefs.preset === null && <option value="">Manual</option>}
          {presets.map((p) => (
            <option key={p.name} value={p.name}>
              {p.name}
            </option>
          ))}
        </select>
      </div>
      <div className={prefs.on ? "eq-sliders" : "eq-sliders eq-sliders--off"}>
        <div className="eq-column eq-column--preamp">
          <VerticalSlider label="Preamp" value={prefs.preampDb} onChange={(preampDb) => update({ ...prefs, on: true, preampDb })} />
          <span className="eq-label">Preamp</span>
        </div>
        <div className="eq-scale" aria-hidden="true">
          <span>+12 dB</span>
          <span>0 dB</span>
          <span>−12 dB</span>
        </div>
        {FREQUENCIES.map((frequency, i) => (
          <div key={frequency} className="eq-column">
            <VerticalSlider label={`${frequency}Hz`} value={prefs.bands[i]} onChange={(db) => setBand(i, db)} />
            <span className="eq-label">{frequency}</span>
          </div>
        ))}
      </div>
      <label className="prefs-check eq-clipping">
        <input type="checkbox" checked={prefs.preventClipping} onChange={(e) => update({ ...prefs, preventClipping: e.target.checked })} />
        Prevent clipping
      </label>
      {bitPerfect && <p className="eq-note">The equalizer is off while bit-perfect output is on.</p>}
      {castTarget && <p className="eq-note">The equalizer can’t change what plays on {castTarget.name}.</p>}
    </div>
  );
}
