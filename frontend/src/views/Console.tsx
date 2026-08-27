export default function Console({ unit }: { unit: string }) {
  const copy = () => {
    if (unit) navigator.clipboard?.writeText(unit);
  };
  return (
    <>
      <div className="view-title">CONSOLE</div>
      <div className="dim" style={{ fontSize: 11, marginBottom: 10 }}>
        last rendered systemd unit (from LOADOUTS preview / apply)
      </div>
      {unit ? (
        <>
          <pre className="unit">{unit}</pre>
          <div className="row" style={{ marginTop: 10 }}>
            <button className="ghost" onClick={copy}>
              COPY
            </button>
          </div>
        </>
      ) : (
        <div className="stub">no unit rendered yet</div>
      )}
    </>
  );
}
