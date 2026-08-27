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
export const marketFiles = (repoId: string) =>
  invoke<MarketFileRow[]>("market_files", { repoId });
export const marketDownload = (repoId: string, rfilename: string) =>
  invoke<string>("market_download", { repoId, rfilename });

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
