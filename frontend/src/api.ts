import { invoke } from "@tauri-apps/api/core";

export interface ModelRow {
  name: string;
  quant: string | null;
  arch: string | null;
  ctx_train: number;
  footprint_gib: number;
  path: string;
}

export interface DupRow {
  identity: string;
  wasted_gib: number;
  members: string[];
}

export interface ProfileRow {
  name: string;
  engine: string;
  alias: string;
  port: number;
  ctx: number;
  model: string;
}

export interface FitRow {
  model: string;
  ctx: number;
  weights_mb: number;
  kv_mb: number;
  buffers_mb: number;
  model_vram_mb: number;
  weights_ram_mb: number;
  overhead_mb: number;
  available_for_model_mb: number;
  verdict: string;
}

export interface BenchRow {
  id: number;
  engine: string;
  host: string;
  port: number;
  model: string;
  ctx: number;
  tps: number;
  at: number;
}

// --- engine registry (mirrors deck_core::profile::EngineDescriptor) ---

export type EngineId = "llamacpp" | "freetoken" | "ollama";
export type EngineSource = "LocalPath" | "OllamaStore";

/** One registered runtime: store id, display name, ports, where models come
 * from, and the HTTP protocol it speaks. The UI derives engine menus from this
 * — never from hardcoded buttons. */
export interface EngineDescriptor {
  id: EngineId;
  display: string;
  unit_name: string;
  default_port: number;
  test_port: number;
  model_source: EngineSource;
  protocol: "OpenAiChat" | "OllamaChat";
}

export const engineList = () => invoke<EngineDescriptor[]>("engine_list");

/** Per-engine executable config row — the one machine-specific fact the
 * engine menu needs (None = engine default resolution). */
export interface EngineBinRow {
  engine_id: EngineId;
  display: string;
  bin: string | null;
}

export const engineBinList = () => invoke<EngineBinRow[]>("engine_bin_list");
export const engineBinSet = (storeId: string, bin: string) => invoke<void>("engine_bin_set", { storeId, bin });
export const engineBinClear = (storeId: string) => invoke<void>("engine_bin_clear", { storeId });

/** Full editable loadout shape — mirrors deck_core::profile::Profile. */
export interface Profile {
  name: string;
  engine: "LlamaCpp" | "FreeToken";
  bin: string;
  model: string;
  alias: string;
  host: string;
  port: number;
  metrics: boolean;
  ctx_size: number;
  ctx_ladder: number[];
  n_gpu_layers: number;
  ubatch_size: number;
  flash_attn: boolean;
  kv_cache_type_k: string | null;
  kv_cache_type_v: string | null;
  load_mode: string | null;
  spec_type: string | null;
  draft_model: string | null;
  temperature: number;
  top_p: number;
  top_k: number;
  parallel: number;
  reasoning: string | null;
  reasoning_format: string | null;
  reasoning_effort: string | null;
  reasoning_budget: number | null;
  ft_backend: string | null;
  ft_moe_cache_size: number | null;
  mem_max_mb: number | null;
  mem_swap_max_mb: number | null;
}

export interface EngineStatus {
  engine: string;
  host: string;
  port: number;
  up: boolean;
}

export interface ScanResult {
  indexed: number;
  pruned: number;
  models: ModelRow[];
  dups: DupRow[];
}

export interface UseResult {
  name: string;
  applied: boolean;
  dry_run: boolean;
  unit: string;
}

export interface SignalRow {
  id: string;
  author: string;
  created_at: string;
  downloads: number;
  likes: number;
  pipeline_tag: string | null;
  tags: string[];
}

export const scan = () => invoke<ScanResult>("scan");
export const listModels = () => invoke<ModelRow[]>("list_models");
export const listProfiles = () => invoke<ProfileRow[]>("list_profiles");
export const dedup = () => invoke<DupRow[]>("dedup");
export const fit = (p: {
  model: string;
  ctx: number;
  kv_bytes: number;
  n_gpu_layers: number;
  kv_layers: number | null;
  reserve: number;
  offload: boolean;
}) => invoke<FitRow>("fit", p);
export const useProfile = (name: string, dryRun: boolean) =>
  invoke<UseResult>("use_profile", { name, dryRun });

export const signalsCheck = (limit: number) =>
  invoke<SignalRow[]>("signals_check", { limit });
export const watchlist = () => invoke<string[]>("watchlist");
export const watchAdd = (org: string) => invoke<void>("watch_add", { org });
export const watchRemove = (org: string) => invoke<void>("watch_remove", { org });

export interface MarketHit {
  id: string;
  downloads: number;
  likes: number;
  pipeline_tag: string | null;
  tags: string[];
  created_at: string;
}

export interface MarketFileRow {
  rfilename: string;
  size: number | null;
}

export const marketSearch = (query: string, limit: number) =>
  invoke<MarketHit[]>("market_search", { query, limit });
export const browseOrg = (org: string, limit: number) =>
  invoke<MarketHit[]>("browse_org", { org, limit });
export const marketFiles = (repoId: string) =>
  invoke<MarketFileRow[]>("market_files", { repoId });

// --- background downloads (progress via dl-* events, see lib/dl.ts) ---

export const downloadStart = (repoId: string, rfilename: string) =>
  invoke<{ key: string }>("download_start", { repoId, rfilename });
export const downloadCancel = (key: string) =>
  invoke<void>("download_cancel", { key });
/** Cancel (if active) and drop the partial `.part` for a download. */
export const downloadRemove = (key: string, rfilename: string) =>
  invoke<void>("download_remove", { key, rfilename });

/** Index an explicit set of landed files into the vault (no full rescan). */
export const indexDownloaded = (paths: string[]) =>
  invoke<number>("index_downloaded", { paths });

// --- bring-up (one-click load pipeline) ---

export interface BringupResult {
  ok: boolean;
  summary: string;
  name: string;
  port: number;
  ctx: number;
  tps: number | null;
  fit: FitBreakdown | null;
}

export interface FitBreakdown {
  weights_mb: number;
  weights_gpu_mb: number;
  weights_ram_mb: number;
  kv_mb: number;
  buffers_mb: number;
  model_vram_mb: number;
  overhead_mb: number;
  available_mb: number;
  available_for_model_mb: number;
  headroom_mb: number;
  verdict: string;
}

export const bringupStart = (model: string, engine: string, fast = false) =>
  invoke<void>("bringup_start", { model, engine, fast });

/** Headless TEST — derive + verify on the test port, never touches live. */
export const testModelStart = (model: string, engine: string) =>
  invoke<void>("test_model_start", { model, engine });

export const benchNow = (p: {
  engine: string;
  host: string;
  port: number;
  model: string;
  ctx: number;
}) => invoke<BenchRow>("bench_now", p);
export const benchHistory = () => invoke<BenchRow[]>("bench_history");
export const engineStatus = (engine: string, host: string, port: number) =>
  invoke<EngineStatus>("engine_status", { engine, host, port });

export const opencodeRun = (p: {
  prompt: string;
  dir: string;
  auto: boolean;
  model: string;
}) => invoke<void>("opencode_run", p);
export const opencodeStop = (id: string) =>
  invoke<void>("opencode_stop", { id });

// --- loadout editing ---
export const saveProfile = (p: Profile) => invoke<void>("save_profile", { profile: p });
export const deleteProfile = (name: string) =>
  invoke<void>("delete_profile", { name });
export const renderProfileUnit = (p: Profile) =>
  invoke<string>("render_profile_unit", { profile: p });

export const TEST_PORTS: Record<Profile["engine"], number> = {
  LlamaCpp: 18999,
  FreeToken: 18998,
};
export const testLoadout = (p: Profile, testPort: number) =>
  invoke<void>("test_loadout", { profile: p, testPort });
export const testStop = () => invoke<void>("test_stop");

// --- browse / remote fit ---

export interface HwInfo {
  vram_mb: number | null;
  detected: boolean;
}

export interface BrowseFitResult {
  arch: string | null;
  quant: string | null;
  params: number | null;
  n_layers: number | null;
  n_embd: number | null;
  truncated: boolean;
  weights_mb: number;
  kv_mb: number;
  buffers_mb: number;
  model_vram_mb: number;
  weights_ram_mb: number;
  overhead_mb: number;
  available_for_model_mb: number;
  verdict: string;
}

export const hwInfo = () => invoke<HwInfo>("hw_info");
export const browseFitRemote = (p: {
  repoId: string;
  rfilename: string;
  ctx: number;
  kvBytes: number;
  nGpuLayers: number;
  kvLayers: number | null;
  reserve: number;
  offload: boolean;
}) => invoke<BrowseFitResult>("browse_fit_remote", p);

export const scanWithEvent = () =>
  invoke<ScanResult>("scan_with_event");

export const deleteModel = (path: string, deleteFile: boolean) =>
  invoke<number>("delete_model", { path, deleteFile });

export const dedupDelete = (identity: string, deleteFile: boolean) =>
  invoke<number>("dedup_delete", { identity, deleteFile });

export interface DupRow {
  identity: string;
  wasted_gib: number;
  members: string[];
}

export const tweakProfile = (p: {
  profile: Profile;
  ctxOverride?: number;
  kvBytesOverride?: number;
  offloadOverride?: boolean;
  nglOverride?: number;
}) => invoke<TweakResult>("tweak_profile", p);

export interface TweakResult {
  ok: boolean;
  summary: string;
  ctx: number;
  tps: number | null;
}
