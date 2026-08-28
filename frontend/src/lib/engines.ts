// Engine registry access: one cached fetch of `engine_list` shared by every
// picker, so the whole UI derives its runtime menu from the backend descriptor
// table instead of hardcoded buttons. No DOM here — components consume the hook.

import { useEffect, useState } from "react";

import { EngineDescriptor, EngineSource, engineList } from "../api";

let cached: Promise<EngineDescriptor[]> | null = null;

function load(): Promise<EngineDescriptor[]> {
  if (!cached) cached = engineList();
  return cached;
}

/** Registered engines, optionally filtered by model source. Empty until the
 * first (cached) fetch resolves. */
export function useEngineList(source?: EngineSource): EngineDescriptor[] {
  const [engines, setEngines] = useState<EngineDescriptor[]>([]);
  useEffect(() => {
    let live = true;
    void load().then((all) => {
      if (!live) return;
      setEngines(source ? all.filter((e) => e.model_source === source) : all);
    });
    return () => {
      live = false;
    };
  }, [source]);
  return engines;
}