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
  overhead_mb: number;
  available_for_model_mb: number;
  verdict: string;
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
  ngl: number;
  kv_layers: number | null;
  reserve: number;
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
