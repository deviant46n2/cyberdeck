import { useEffect, useState, useSyncExternalStore } from "react";
import * as api from "./api";
import * as dls from "./lib/dl";
import Hud from "./views/Hud";
import Vault from "./views/Vault";
import Loadouts from "./views/Loadouts";
import Signals from "./views/Signals";
import Market from "./views/Market";
import Downloads from "./views/Downloads";
import Bringup from "./views/Bringup";
import Bench from "./views/Bench";
import Canvas from "./views/Canvas";
import Compare from "./views/Compare";
import Feeds from "./views/Feeds";

const VIEWS = ["HUD", "VAULT", "SIGNALS", "FEEDS", "MARKET", "DOWNLOADS", "LOADOUTS", "COMPARE", "CANVAS", "BENCH"];

export default function App() {
  const [view, setView] = useState("HUD");
  const [booted, setBooted] = useState(false);
  const [models, setModels] = useState<api.ModelRow[]>([]);
  const [dups, setDups] = useState<api.DupRow[]>([]);
  const [profiles, setProfiles] = useState<api.ProfileRow[]>([]);
  const activeCount = useSyncExternalStore(dls.subscribe, dls.activeCount);

  const refresh = async () => {
    const r = await api.scanWithEvent();
    setModels(r.models);
    setDups(r.dups);
    setProfiles(await api.listProfiles());
  };

  const reload = async () => {
    setModels(await api.listModels());
    setDups(await api.dedup());
    setProfiles(await api.listProfiles());
  };

  useEffect(() => {
    refresh();
    const t = setTimeout(() => setBooted(true), 500);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Attach the global download store at boot so MARKET transfers work
  // even before the DOWNLOADS tab is opened; any completed file triggers a
  // debounced rescan so the vault stays in sync with disk.
  useEffect(() => {
    dls.init();
    let t: number | undefined;
    const off = dls.onDone(() => {
      window.clearTimeout(t);
      t = window.setTimeout(() => void refresh(), 800);
    });
    return () => {
      off();
      window.clearTimeout(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">cyberdeck</div>
        <div className="brand-sub">local llm fleet · v0.1</div>
        <nav className="nav">
          {VIEWS.map((v) => (
            <button
              key={v}
              className={view === v ? "active" : ""}
              onClick={() => setView(v)}
            >
              {v}
              {v === "DOWNLOADS" && activeCount > 0 && (
                <span className="dl-badge">{activeCount}</span>
              )}
            </button>
          ))}
        </nav>
        <div className="side-status">
          <div>{models.length} model{models.length !== 1 ? "s" : ""} · {profiles.length} loadout{profiles.length !== 1 ? "s" : ""}</div>
          <div className="status-line">
            <span className={"dot " + (models.length ? "up" : "down")} />
            <span>{models.length ? "vault indexed" : "vault empty — scan ~/models"}</span>
          </div>
        </div>
        <div className="spacer" />
      </aside>

      <main className="main">
        {view === "HUD" && (
          <Hud
            models={models}
            dups={dups}
            profiles={profiles}
            onChanged={refresh}
          />
        )}
        {view === "VAULT" && <Vault models={models} dups={dups} onRefresh={refresh} onReload={reload} />}
        {view === "SIGNALS" && <Signals />}
        {view === "FEEDS" && <Feeds />}
        {view === "MARKET" && <Market />}
        {view === "DOWNLOADS" && <Downloads />}
        {view === "LOADOUTS" && (
          <Loadouts profiles={profiles} onChanged={refresh} />
        )}
        {view === "COMPARE" && <Compare />}
        {view === "CANVAS" && <Canvas />}
        {view === "BENCH" && <Bench />}
      </main>

      <Bringup />

      <div
        className={"boot" + (booted ? " fade-out" : "")}
        style={{ pointerEvents: booted ? "none" : "auto" }}
      >
        <h1>cyberdeck</h1>
        <p>loading fleet…</p>
      </div>
    </div>
  );
}
