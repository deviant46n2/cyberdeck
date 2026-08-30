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
export const bringupReset = () => invoke<void>("bringup_reset");

/** Headless TEST — derive + verify on the test port, never touches live. */
export const testModelStart = (model: string, engine: string) =>
  invoke<void>("test_model_start", { model, engine });

/** Apply a previously-verified profile (skip derive+verify) and bench+record. */
export const testApply = (profile: Profile, fit: FitBreakdown | null) =>
  invoke<void>("apply_cached_profile", { profile, fit });

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

// --- port map (multi-model residency) ---
export interface PortMapSlot {
  engine: string;
  display: string;
  port: number;
  profile: string | null;
  resident: boolean;
  state: "up" | "starting" | "down";
  fit_verdict: string | null;
}

export const portMapStatus = (host: string) =>
  invoke<PortMapSlot[]>("port_map_status", { host });

/** Stop one engine's unit and clear its port-map binding; other residents
 * stay up (the UI door to `deck engines stop`). */
export const engineStop = (engine: string) =>
  invoke<void>("engine_stop", { engine });

// --- blind A/B compare ---
export interface ScoredTrial {
  trial: string;
  task: string;
  run: number;
  ok: boolean;
  error: string | null;
  gen_tokens: number | null;
  prompt_tokens: number | null;
  tok_s: number | null;
  tok_s_kind: string;
  wall_ms: number;
  score: number;
  output: string;
}

export interface CandidateStanding {
  trial: string;
  engine: string;
  model: string;
  ctx: number;
  ok_runs: number;
  trials: number;
  mean_tok_s: number | null;
  mean_score: number;
  failure: string | null;
  verdict: string | null;
}

export interface CompareReport {
  procedure: string;
  tasks: string[];
  candidates: CandidateStanding[];
  trials: ScoredTrial[];
  verdict: string;
}

export const compareRun = (p: {
  model: string;
  engines: string[];
  ollama: string[];
  tasks: string[];
  runs: number;
  maxTokens: number;
  seed: number;
}) =>
  invoke<CompareReport>("compare_run", {
    model: p.model,
    engines: p.engines,
    ollama: p.ollama,
    tasks: p.tasks,
    runs: p.runs,
    maxTokens: p.maxTokens,
    seed: p.seed,
  });

export const opencodeRun = (p: {
  prompt: string;
  dir: string;
  auto: boolean;
  model: string;
}) => invoke<void>("opencode_run", p);
export const opencodeStop = (id: string) =>
  invoke<void>("opencode_stop", { id });

// --- embedded opencode TUIs (HUD canvas panes) ---
export const tuiSpawn = (dir: string, cols: number, rows: number) =>
  invoke<string>("tui_spawn", { dir, cols, rows });
export const tuiWrite = (id: string, bytes: number[]) =>
  invoke<void>("tui_write", { id, bytes });
export const tuiResize = (id: string, cols: number, rows: number) =>
  invoke<void>("tui_resize", { id, cols, rows });
export const tuiStop = (id: string) => invoke<void>("tui_stop", { id });

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

// --- Phase 2..4 intelligence APIs ---

export interface WorkloadTask { label: string; prompt: string; evaluator: string; evaluator_config: string; }
export interface Workload { id: string; label: string; description: string; tasks: WorkloadTask[]; }
export const workloadsList = () => invoke<Workload[]>("workloads_list");

export interface HardwareProfile {
  id: number; gpu: string; vram_mb: number; cpu: string; ram_mb: number;
  os: string; driver: string; cuda: string; cyberdeck_ver: string; engines_json: string;
  captured_at: number; content_hash: string;
}
export const hardwareProfile = () => invoke<HardwareProfile>("hardware_profile");

export interface RankedCandidate { model: string; engine: string; runs: number; success_rate: number; mean_score: number; p50_tok_s: number | null; mean_tok_s: number | null; explain: string; }
export const recommend = (workload: string, objective: string) => invoke<RankedCandidate[]>("recommend", { workload, objective });

export interface Release { source: string; repo: string; rev: string; kind: string; title: string; url: string; published_at: string; payload_json: string; fetched_at: number; }
export interface ScoredRelease { release: Release; score: { total: number; hw: number; family: number; novelty: number; bench: number; recency: number; fits: boolean; reasons: string[] } }
export const feedsList = (limit: number) => invoke<Release[]>("feeds_list", { limit });
export const feedsPoll = (sources: string[]) => invoke<{ fetched: number; inserted: number }>("feeds_poll", { sources });
export const feedsRank = (limit: number, workload?: string | null) => invoke<ScoredRelease[]>("feeds_rank", { limit, workload: workload ?? null });

export const settingsGet = (key: string) => invoke<string | null>("settings_get", { key });
export const settingsSet = (key: string, value: string, reason: string, actor: string) => invoke<void>("settings_set", { key, value, reason, actor });
export const settingsList = () => invoke<[string, string, number][]>("settings_list");

export const engineStart = (engine: string) => invoke<void>("engine_start", { engine });

export interface ToolDef { name: string; description: string; permission: string; }
export const agentTools = () => invoke<ToolDef[]>("agent_tools");
export const analyzeRelevance = (repo: string, workload?: string | null) => invoke<unknown>("analyze_relevance", { repo, workload: workload ?? null });

// --- Infinite Agent Canvas (ROADMAP 8c/8d) ---

export type NodeKind = "Stateless" | "Agentic";
export type WorkflowRunStatus = "Queued" | "Running" | "Done" | "Partial" | "Stopped" | "Error";

export interface ModelBinding {
  role_id: string;
  model_ref: string;
  engine: string | null;
  overrides_json: string;
  active: boolean;
}

export interface WorkflowNode {
  id: string;
  role_id: string;
  binding: ModelBinding;
  kind: NodeKind;
  pos: { x: number; y: number };
  exec: { timeout_s: number; max_tokens: number; max_retries: number };
}

export interface WorkflowEdge {
  id: string;
  from: string;
  to: string;
  from_port: string;
  to_port: string;
  condition: string | null;
}

export interface Workflow {
  id: string;
  name: string;
  description: string;
  version: number;
  nodes: WorkflowNode[];
  edges: WorkflowEdge[];
  exec_settings: {
    max_parallel: number;
    global_retries: number;
    budget_tokens: number;
    budget_wall_s: number;
    max_iterations: number;
  };
  template: boolean;
}

export interface WfRunRow {
  id: string;
  workflow_id: string;
  status: WorkflowRunStatus;
  created_at: number;
  updated_at: number;
  budget_tokens: number;
  tokens_used: number;
  output: string;
}

export interface WfStarted { run_id: string; workflow_id: string; }
export interface WfNodeEvt { run_id: string; node_id: string; ok: boolean; error: string; }
export interface WfDoneEvt { run_id: string; workflow_id: string; status: string; tokens_used: number; nodes_ok: number; nodes_failed: number; }

export const workflowSeed = () => invoke<string>("workflow_seed");
export const workflowSave = (body: string) => invoke<string>("workflow_save", { body });
export const workflowList = () => invoke<Workflow[]>("workflow_list");
export const workflowGet = (workflowId: string) => invoke<Workflow | null>("workflow_get", { workflowId });
export const workflowRun = (workflowId: string, runner: string, dir?: string | null, model?: string | null) =>
  invoke<WfStarted>("workflow_run", { workflowId, runner, dir: dir ?? null, model: model ?? null });
export const workflowStop = (runId: string) => invoke<void>("workflow_stop", { runId });
export const workflowHistory = (workflowId?: string | null) =>
  invoke<WfRunRow[]>("workflow_history", { workflowId: workflowId ?? null });
export interface RoleBenchRow {
  role_id: string;
  engine: string;
  model: string;
  runs: number;
  best_tps: number;
  avg_tps: number;
  last_tps: number;
  last_wall_ms: number;
  last_ttft_ms: number | null;
}
export const workflowPerRoleBench = (workflowId: string) =>
  invoke<RoleBenchRow[]>("workflow_per_role_bench", { workflowId });
