import { useEffect, useState, type ReactNode } from "react";
import { sharingStatus, sharingUpdate } from "../api/commands";
import type { SharingSettings, SharingStatus } from "../api/types";
import { useApp } from "../state/store";
import type { TextSizes } from "./textSizes";
import "./Preferences.css";

type Tab = "general" | "sharing";

const TABS: { id: Tab; label: string; icon: ReactNode }[] = [
  {
    id: "general",
    label: "General",
    icon: <path d="M16 4a12 12 0 1 0 0 24 12 12 0 0 0 0-24zm0 5.5a6.5 6.5 0 1 1 0 13 6.5 6.5 0 0 1 0-13z" />,
  },
  {
    id: "sharing",
    label: "Sharing",
    icon: <path d="M11 10a4 4 0 1 1 0 8 4 4 0 0 1 0-8zm10 0a4 4 0 1 1 0 8 4 4 0 0 1 0-8zM3 27c0-4.5 3.6-7 8-7s8 2.5 8 7zm16.5 0c0-2.5-.9-4.5-2.4-5.8.9-.5 2.4-1.2 3.9-1.2 4.4 0 8 2.5 8 7z" />,
  },
];

function GeneralTab() {
  const sizes = useApp((s) => s.textSizes);
  const update = useApp((s) => s.setTextSizes);
  return (
    <div className="prefs-grid">
      <span className="prefs-label">Source Text:</span>
      <select className="smart-popup" value={sizes.source} onChange={(e) => update({ ...sizes, source: e.target.value as TextSizes["source"] })}>
        <option value="small">Small</option>
        <option value="large">Large</option>
      </select>
      <span className="prefs-label">List Text:</span>
      <select className="smart-popup" value={sizes.list} onChange={(e) => update({ ...sizes, list: e.target.value as TextSizes["list"] })}>
        <option value="small">Small</option>
        <option value="large">Large</option>
      </select>
    </div>
  );
}

function SharingTab() {
  const [status, setStatus] = useState<SharingStatus | null>(null);
  const [draft, setDraft] = useState<SharingSettings | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [revealed, setRevealed] = useState(false);

  useEffect(() => {
    void sharingStatus().then((s) => {
      setStatus(s);
      setDraft(s.settings);
    });
  }, []);

  if (!status || !draft) return null;
  const save = (settings: SharingSettings) => {
    setError(null);
    void sharingUpdate(settings).then(
      (s) => {
        setStatus(s);
        setDraft(s.settings);
      },
      (e) => setError(String(e)),
    );
  };
  const dirty = JSON.stringify(draft) !== JSON.stringify(status.settings);

  return (
    <div className="prefs-sharing">
      <label className="prefs-check">
        <input type="checkbox" checked={draft.enabled} onChange={(e) => save({ ...draft, enabled: e.target.checked })} />
        Share my library with my phone and other devices
      </label>
      <p className="prefs-note">
        Phone apps that speak Subsonic (Symfonium, Amperfy, play:Sub, DSub) can browse and stream your music from this Mac. Only
        devices on your network that know the password can connect.
      </p>
      <div className="prefs-grid">
        <span className="prefs-label">User name:</span>
        <input className="smart-input" value={draft.username} onChange={(e) => setDraft({ ...draft, username: e.target.value })} />
        <span className="prefs-label">Password:</span>
        <span className="prefs-inline">
          <input
            className="smart-input"
            type={revealed ? "text" : "password"}
            value={draft.password}
            onChange={(e) => setDraft({ ...draft, password: e.target.value })}
          />
          <button className="push-button" onClick={() => setRevealed((r) => !r)}>
            {revealed ? "Hide" : "Show"}
          </button>
        </span>
        <span className="prefs-label">Port:</span>
        <input
          className="smart-input smart-input--number"
          type="number"
          min={1024}
          max={65535}
          value={draft.port}
          onChange={(e) => setDraft({ ...draft, port: Number(e.target.value) })}
        />
      </div>
      {dirty && (
        <button className="push-button push-button--default prefs-apply" onClick={() => save(draft)}>
          Apply
        </button>
      )}
      <div className="prefs-status">
        {status.running ? (
          <>
            <strong>Sharing is on.</strong> In your phone app, add a Subsonic server at
            <ul>
              {status.addresses.map((a) => (
                <li key={a}>
                  <code>{a}</code>
                </li>
              ))}
            </ul>
            with the user name and password above. macOS may ask once whether to allow incoming connections.
          </>
        ) : (
          <span>Sharing is off.</span>
        )}
        {(error ?? status.error) && <p className="smart-error">{error ?? status.error}</p>}
      </div>
    </div>
  );
}

/** iTunes 10's Preferences: a toolbar of tabs over each pane, with OK to close. */
export function Preferences() {
  const [tab, setTab] = useState<Tab>("general");
  const setOpen = useApp((s) => s.setPreferencesOpen);
  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && setOpen(false)}>
      <div className="sheet prefs" role="dialog" aria-label="Preferences">
        <div className="prefs-toolbar" role="tablist">
          {TABS.map((t) => (
            <button key={t.id} role="tab" aria-selected={tab === t.id} className="prefs-tab" onClick={() => setTab(t.id)}>
              <svg width={32} height={32} viewBox="0 0 32 32" aria-hidden="true">
                {t.icon}
              </svg>
              {t.label}
            </button>
          ))}
        </div>
        <div className="prefs-pane">{tab === "general" ? <GeneralTab /> : <SharingTab />}</div>
        <div className="sheet-buttons">
          <button className="push-button push-button--default" onClick={() => setOpen(false)}>
            OK
          </button>
        </div>
      </div>
    </div>
  );
}
