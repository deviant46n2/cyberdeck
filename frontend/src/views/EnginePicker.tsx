// The engine dropdown derived from the backend registry (`engine_list`). Every
// "which runtime?" choice in the app uses this, so a new engine appears
// everywhere the moment it's registered — no per-view button edits.

import { EngineId, EngineSource } from "../api";
import { useEngineList } from "../lib/engines";

interface Props {
  value: EngineId;
  onChange: (id: EngineId) => void;
  /** Filter the menu by model source (default and typical: local GGUFs). */
  source?: EngineSource;
  title?: string;
  disabled?: boolean;
}

export default function EnginePicker({ value, onChange, source, title, disabled }: Props) {
  const engines = useEngineList(source);
  if (engines.length === 0) return null;
  return (
    <select
      className="ghost"
      style={{ fontSize: 9, padding: "1px 4px", color: "var(--fg)" }}
      value={value}
      title={title ?? "runtime to use for this action"}
      disabled={disabled}
      onChange={(e) => onChange(e.target.value as EngineId)}
    >
      {engines.map((en) => (
        <option key={en.id} value={en.id}>
          {en.display}
        </option>
      ))}
    </select>
  );
}