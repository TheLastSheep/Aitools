export type LauncherMode = "search" | "agent";

export interface ContextChip {
  kind: "selection" | "clipboard" | "active_app" | "active_path";
  label: string;
  value: string;
}

export interface QuickAgentAction {
  id: string;
  title: string;
  prompt_template: string;
  target_tool?: string | null;
  signature: string;
}

export interface LauncherHit {
  feature_code: string;
  plugin_id: string;
  score: number;
  match_type: string;
  confidence: number;
}

export type LauncherDecision =
  | { deterministic_results: { matches: LauncherHit[]; top_confidence: number } }
  | { promote_to_agent: { reason: string; query: string } };

export interface LogRecord {
  seq: number;
  ts: string;
  level: string;
  trace_id?: string;
  source: string;
  plugin_id?: string;
  span?: string;
  duration_us?: number;
  perf_anomaly?: boolean;
  msg: string;
  fields?: Record<string, unknown>;
}

export interface FaultInjectionConfig {
  injectLmdbRevConflict: boolean;
  injectPermissionDenied: boolean;
  injectWasmFuelExhaustion: boolean;
  injectLlmStreamAbort: boolean;
}

export type DagEvent =
  | { node_started: { node_id: string; tool_name: string } }
  | { hitl_suspended: { node_id: string; tool_name: string; risk_tier: string; arguments: unknown } }
  | { node_completed: { node_id: string; output: unknown } };

export function isTauriRuntime(): boolean {
  return typeof (globalThis as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ !== "undefined";
}

export async function invokeSafe<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T | null> {
  if (!isTauriRuntime()) {
    return null;
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

export async function searchFeatures(
  query: string,
  tabPressed: boolean,
  submitted: boolean,
): Promise<LauncherDecision> {
  const res = await invokeSafe<LauncherDecision>("plugin:aitools|search_features", {
    query,
    tabPressed,
    submitted,
  });
  if (res) return res;

  // Browser preview fallback when running Vite standalone without Tauri host.
  if (tabPressed || (submitted && query.trim().startsWith("@"))) {
    return { promote_to_agent: { reason: "tab_pressed", query } };
  }
  return {
    deterministic_results: {
      matches: query.trim()
        ? [
            {
              feature_code: "json-formatter",
              plugin_id: "builtin",
              score: 90,
              match_type: "prefix",
              confidence: 0.9,
            },
          ]
        : [],
      top_confidence: query.trim() ? 0.9 : 0,
    },
  };
}

export async function fetchZeroQuerySuggestions(): Promise<QuickAgentAction[]> {
  return (await invokeSafe<QuickAgentAction[]>("plugin:aitools|zero_query_suggestions")) ?? [];
}

export async function fetchDshLogs(limit = 100): Promise<LogRecord[]> {
  return (await invokeSafe<LogRecord[]>("plugin:aitools|dsh_recent_logs", { limit })) ?? [];
}

export async function setFaultInjection(
  config: FaultInjectionConfig,
): Promise<FaultInjectionConfig> {
  return (
    (await invokeSafe<FaultInjectionConfig>("plugin:aitools|dsh_set_fault_injection", { config })) ??
    config
  );
}

export async function resolveHitlStep(
  nodeId: string,
  resolution: "approve_once" | "approve_session" | "deny",
): Promise<boolean> {
  return (
    (await invokeSafe<boolean>("plugin:aitools|resolve_hitl_step", { nodeId, resolution })) ?? true
  );
}

export async function ingestFrontendLog(
  level: "INFO" | "WARN" | "ERROR",
  source: string,
  msg: string,
  traceId?: string,
): Promise<void> {
  await invokeSafe<void>("plugin:aitools|dsh_ingest_frontend_log", {
    payload: { level, source, msg, traceId, fields: {} },
  });
}
