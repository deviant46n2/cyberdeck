import * as api from "../api";

export default function Vault({
  models,
  dups,
}: {
  models: api.ModelRow[];
  dups: api.DupRow[];
}) {
  const dupIds = new Set(dups.flatMap((d) => d.members));

  return (
    <>
      <div className="view-title">VAULT</div>

      {dups.length > 0 && (
        <div className="card" style={{ marginBottom: 16, borderColor: "var(--oom)" }}>
          <h3 style={{ color: "var(--oom)" }}>DUPLICATE SHARDS — WASTED SPACE</h3>
          {dups.map((d) => (
            <div key={d.identity} style={{ margin: "8px 0" }}>
              <span className="badge oom">{d.wasted_gib.toFixed(2)} GiB</span>{" "}
              <span className="mono">{d.identity}</span>
              <ul className="dim" style={{ margin: "4px 0 0 18px", fontSize: 11 }}>
                {d.members.map((m) => (
                  <li key={m}>{m}</li>
                ))}
              </ul>
            </div>
          ))}
        </div>
      )}

      <div className="card">
        <table>
          <thead>
            <tr>
              <th>NAME</th>
              <th>QUANT</th>
              <th>ARCH</th>
              <th>TRAIN CTX</th>
              <th>SIZE</th>
              <th>PATH</th>
            </tr>
          </thead>
          <tbody>
            {models.map((m) => {
              const dup = dupIds.has(m.path);
              return (
                <tr key={m.path} style={dup ? { background: "rgba(255,59,59,0.06)" } : undefined}>
                  <td>{m.name}</td>
                  <td>{m.quant ?? "—"}</td>
                  <td>{m.arch ?? "—"}</td>
                  <td className="mono">{m.ctx_train ? m.ctx_train.toLocaleString() : "—"}</td>
                  <td className="mono">{m.footprint_gib.toFixed(2)} GiB</td>
                  <td className="dim" style={{ maxWidth: 320, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                    {m.path}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </>
  );
}
