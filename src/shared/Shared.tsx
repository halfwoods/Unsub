import { useState } from "react";
import { remoteConnect } from "../api/commands";
import type { SharedLibrary } from "../api/types";
import { useApp } from "../state/store";
import "./Shared.css";

/** The user name and password fields both sign-in forms share. */
function LoginFields({
  username,
  password,
  onUsername,
  onPassword,
  onSubmit,
  autoFocus = false,
}: {
  autoFocus?: boolean;
  username: string;
  password: string;
  onUsername: (value: string) => void;
  onPassword: (value: string) => void;
  onSubmit: () => void;
}) {
  return (
    <>
      <label className="prefs-label" htmlFor="shared-user">
        User name:
      </label>
      <input id="shared-user" className="smart-input" autoFocus={autoFocus} value={username} onChange={(e) => onUsername(e.target.value)} />
      <label className="prefs-label" htmlFor="shared-password">
        Password:
      </label>
      <input
        id="shared-password"
        className="smart-input"
        type="password"
        value={password}
        onChange={(e) => onPassword(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && onSubmit()}
      />
    </>
  );
}

/** What a shared library shows until it's signed in to: iTunes asked for a shared library's password in place. */
export function SharedLogin({ library }: { library: SharedLibrary }) {
  const [username, setUsername] = useState("unsub");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const connect = async () => {
    setBusy(true);
    setError(null);
    try {
      await remoteConnect(library.id, null, null, username, password);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="placard">
      <div className="placard-card shared-login">
        <h1>{library.name}</h1>
        <p>
          {library.discovered
            ? "This library is shared with a password. Its owner can find the user name and password in Unsub's Preferences › Sharing."
            : "Sign in to this server with your account on it."}
        </p>
        {library.addressChanged && <p className="prefs-note">This library is at a different address than before.</p>}
        <div className="prefs-grid shared-fields">
          <LoginFields autoFocus username={username} password={password} onUsername={setUsername} onPassword={setPassword} onSubmit={() => void connect()} />
        </div>
        {error && <p className="smart-error">{error}</p>}
        <div className="ownership-intro-actions">
          <button className="push-button push-button--default" disabled={busy || !password} onClick={() => void connect()}>
            {busy ? "Connecting…" : "Connect"}
          </button>
        </div>
      </div>
    </div>
  );
}

/** File › Connect to Server…: a Navidrome or other Subsonic server, or a library outside this network. */
export function ConnectToServer() {
  const close = () => useApp.setState({ connectingToServer: false });
  const [url, setUrl] = useState("http://");
  const [name, setName] = useState("");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const connect = async () => {
    setBusy(true);
    setError(null);
    try {
      const library = await remoteConnect(null, url, name || null, username, password);
      close();
      const { refreshShared, setSource } = useApp.getState();
      await refreshShared();
      setSource(`shared:${library}`);
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };
  return (
    <div className="sheet-backdrop" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="sheet connect-sheet" role="dialog" aria-label="Connect to Server">
        <h2 className="first-sync-title">Connect to Server</h2>
        <p className="prefs-note">A Navidrome or other Subsonic server, or an Unsub library you reach by address.</p>
        <div className="prefs-grid">
          <label className="prefs-label" htmlFor="server-url">
            Address:
          </label>
          <input id="server-url" className="smart-input" autoFocus value={url} onChange={(e) => setUrl(e.target.value)} placeholder="http://192.168.1.20:4533" />
          <label className="prefs-label" htmlFor="server-name">
            Name:
          </label>
          <input id="server-name" className="smart-input" value={name} onChange={(e) => setName(e.target.value)} placeholder="Optional" />
          <LoginFields username={username} password={password} onUsername={setUsername} onPassword={setPassword} onSubmit={() => void connect()} />
        </div>
        {error && <p className="smart-error">{error}</p>}
        <div className="sheet-buttons">
          <button className="push-button" onClick={close}>
            Cancel
          </button>
          <button className="push-button push-button--default" disabled={busy || url.length < 10 || !username} onClick={() => void connect()}>
            {busy ? "Connecting…" : "Connect"}
          </button>
        </div>
      </div>
    </div>
  );
}
