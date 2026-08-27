import { useEffect, useState } from "react";
import * as api from "./api";
import Hud from "./views/Hud";
import Vault from "./views/Vault";
import Loadouts from "./views/Loadouts";
import Signals from "./views/Signals";
import Market from "./views/Market";
import Console from "./views/Console";

const VIEWS = ["HUD", "VAULT", "SIGNALS", "MARKET", "LOADOUTS", "CONSOLE"];

export default function App() {
  const [view, setView] = useState("HUD");
  const [scanlines, setScanlines] = useState(true);
  const [booted, setBooted] = useState(false);
  const [models, setModels] = useState<api.ModelRow[]>([]);
  const [dups, setDups] = useState<api.DupRow[]>([]);
  const [profiles, setProfiles] = useState<api.ProfileRow[]>([]);
  const [unit, setUnit] = useState<string>("");

  const refresh = async () => {
    const r = await api.scan();
    setModels(r.models);
    setDups(r.dups);
    setProfiles(await api.listProfiles());
  };

  useEffect(() => {
    refresh();
    const t = setTimeout(() => setBooted(true), 1200);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">CYBERDECK</div>
        <div className="brand-sub">LOCAL LLM FLEET</div>
        <nav className="nav">
          {VIEWS.map((v) => (
            <button
              key={v}
              className={view === v ? "active" : ""}
              onClick={() => setView(v)}
            >
              {v}
            </button>
          ))}
        </nav>
        <div className="spacer" />
        <label className="toggle">
          <input
            type="checkbox"
            checked={scanlines}
            onChange={(e) => setScanlines(e.target.checked)}
          />{" "}
          SCANLINES
        </label>
      </aside>

      <main className="main">
        {view === "HUD" && (
          <Hud
            models={models}
            dups={dups}
            profiles={profiles}
            onUnit={setUnit}
            onChanged={refresh}
          />
        )}
        {view === "VAULT" && <Vault models={models} dups={dups} />}
        {view === "SIGNALS" && <Signals />}
        {view === "MARKET" && <Market />}
        {view === "LOADOUTS" && (
          <Loadouts profiles={profiles} onUnit={setUnit} />
        )}
        {view === "CONSOLE" && <Console unit={unit} />}
      </main>

      {scanlines && <div className="scanlines" />}
      <div
        className={"boot" + (booted ? " fade-out" : "")}
        style={{ pointerEvents: booted ? "none" : "auto" }}
      >
        <h1>CYBERDECK</h1>
        <p>BOOTING FLEET CONTROL</p>
      </div>
    </div>
  );
}
