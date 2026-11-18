export type LauncherMode = "search" | "list" | "agent";
export type ThemePreference = "system" | "light" | "dark";
export type ResolvedTheme = "light" | "dark";
export type HitKind = "app" | "plugin" | "list" | "wasm" | "macro" | "feature";
export type RiskTier = "Tier0ReadOnly" | "Tier1StateMutating" | "Tier2DestructiveOrExternal";
export type AiProfile = "fast" | "balanced" | "deep";

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
  label: string;
  plugin_name: string;
  description: string;
  icon?: string | null;
  kind?: HitKind | string;
  score: number;
  match_type: string;
  confidence: number;
  explain: string;
}

export type LauncherDecision =
  | { deterministic_results: { matches: LauncherHit[]; top_confidence: number } }
  | { promote_to_agent: { reason: string; query: string } };

export interface ListItem {
  title: string;
  description: string;
  icon?: string | null;
  payload?: unknown;
}

export interface ListPushEvent {
  trace_id: string;
  plugin_id: string;
  handle?: number;
  items: ListItem[];
}

export interface ListSession {
  session_id: string;
  plugin_id: string;
  plugin_name: string;
  feature_code: string;
  placeholder?: string | null;
  items: ListItem[];
}

export type LaunchResult =
  | { kind: "opened_window"; plugin_id: string; window_label: string; hide_main?: boolean }
  | { kind: "launched_app"; app_name: string; hide_main?: boolean }
  | {
      kind: "list_mode";
      session_id: string;
      plugin_id: string;
      plugin_name: string;
      feature_code: string;
      placeholder?: string | null;
      items: ListItem[];
    }
  | {
      kind: "executed_headless";
      plugin_id: string;
      feature_code: string;
      message?: string | null;
      output?: unknown;
      hide_main?: boolean;
    }
  | { kind: "wasm_result"; plugin_id: string; feature_code: string; output: unknown }
  | { kind: "macro_started"; macro_id: string; trace_id: string };

export interface ListSearchResponse {
  session_id: string;
  items: ListItem[];
}

export interface SelectListResult {
  ok: boolean;
  out_plugin?: boolean;
  hide_main?: boolean;
  message?: string | null;
  items?: ListItem[] | null;
}

export interface SyncZtoolsResult {
  dir: string;
  indexed_plugins: number;
  indexed_features: number;
  errors: string[];
}

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
  | {
      hitl_suspended: {
        node_id: string;
        tool_name: string;
        risk_tier: RiskTier | string;
        arguments: unknown;
      };
    }
  | { node_completed: { node_id: string; output: unknown } };

export type AgentStreamEvent =
  | { token: { text: string } }
  | { thinking: { text: string } }
  | { node_started: { node_id: string; tool_name: string } }
  | {
      hitl_suspended: {
        node_id: string;
        tool_name: string;
        risk_tier: RiskTier | string;
        arguments: unknown;
      };
    }
  | { node_completed: { node_id: string; output: unknown } }
  | { node_failed: { node_id: string; error: string } }
  | { completed: { final_text: string; trace_id: string } }
  | { error: { message: string } };

export interface AgentTurnResult {
  trace_id: string;
  final_text: string;
  completed_nodes?: Record<string, unknown>;
}

export interface LaunchItemParams {
  featureCode: string;
  pluginId: string;
  query: string;
  kind?: string;
  traceId?: string;
}

export interface SearchListModeParams {
  sessionId: string;
  pluginId: string;
  featureCode: string;
  query: string;
  traceId?: string;
}

export interface SelectListItemParams {
  sessionId: string;
  pluginId: string;
  featureCode: string;
  item: ListItem;
  index: number;
  traceId?: string;
}

export interface SubmitAgentPromptParams {
  prompt: string;
  traceId: string;
  chips: ContextChip[];
  profile?: AiProfile;
}

export function isTauriRuntime(): boolean {
  return (
    typeof (globalThis as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ !== "undefined"
  );
}

export function isDshRoute(): boolean {
  return typeof window !== "undefined" && window.location.hash === "#dsh";
}

export async function invokeSafe<T>(
  cmd: string,
  args: Record<string, unknown> = {},
): Promise<T | null> {
  if (!isTauriRuntime()) {
    return null;
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

export async function setMainWindowHeight(heightPx: number): Promise<void> {
  if (!isTauriRuntime() || isDshRoute()) {
    return;
  }
  try {
    const { getCurrentWindow, LogicalSize } = await import("@tauri-apps/api/window");
    const win = getCurrentWindow();
    if (win.label === "dsh") {
      return;
    }
    const clamped = Math.max(62, Math.min(560, Math.round(heightPx)));
    await win.setSize(new LogicalSize(780, clamped));
  } catch {
    // Ignored outside desktop capability contexts.
  }
}

export async function hideMainWindow(): Promise<void> {
  await invokeSafe<void>("hide_main_window");
}

const PREVIEW_CATALOG: LauncherHit[] = [
  {
    feature_code: "clipboard-history",
    plugin_id: "clipboard-history",
    label: "Clipboard History",
    plugin_name: "Clipboard History",
    description: "Search, copy, and pin recent clipboard snippets",
    kind: "plugin",
    score: 96,
    match_type: "prefix",
    confidence: 0.96,
    explain: "Search, copy, and pin recent clipboard snippets",
  },
  {
    feature_code: "translate",
    plugin_id: "translation",
    label: "AI Translator",
    plugin_name: "AI Translator",
    description: "Translate technical prose across EN, ZH, JA, and KO",
    kind: "plugin",
    score: 92,
    match_type: "keyword",
    confidence: 0.92,
    explain: "Translate technical prose across EN, ZH, JA, and KO",
  },
  {
    feature_code: "snipaste-ocr",
    plugin_id: "snipaste-legacy",
    label: "Snipaste OCR",
    plugin_name: "Snipaste Pro (Legacy)",
    description: "Headless ZTools list mode for OCR & QR scan",
    kind: "list",
    score: 88,
    match_type: "keyword",
    confidence: 0.88,
    explain: "Recognize text or QR barcodes from clipboard screenshots",
  },
  {
    feature_code: "todo",
    plugin_id: "todo-legacy",
    label: "ToDo Quick Checklist",
    plugin_name: "ToDo Quick Checklist",
    description: "Manage local tasks via synchronous storage",
    kind: "plugin",
    score: 85,
    match_type: "keyword",
    confidence: 0.85,
    explain: "Manage local tasks via synchronous storage",
  },
  {
    feature_code: "format_json",
    plugin_id: "json-codec",
    label: "JSON Formatter & Codec",
    plugin_name: "JSON Formatter & Codec",
    description: "Validate, prettify, and Base64/URL encode payloads",
    kind: "wasm",
    score: 82,
    match_type: "keyword",
    confidence: 0.82,
    explain: "Validate, prettify, or minify JSON inside Wasm sandbox",
  },
  {
    feature_code: "terminal-launchpad",
    plugin_id: "terminal-launchpad",
    label: "Terminal Launchpad",
    plugin_name: "Terminal Shortcuts",
    description: "Project directories and saved shell commands",
    kind: "plugin",
    score: 80,
    match_type: "keyword",
    confidence: 0.8,
    explain: "Saved project directories and command shortcuts",
  },
];

const PREVIEW_LIST_ITEMS: ListItem[] = [
  {
    title: "Capture Region -> OCR",
    description: "Extract text from selected screen area",
    payload: { code: "capture-ocr" },
  },
  {
    title: "Decode QR / Barcode",
    description: "Scan clipboard image for QR payloads",
    payload: { code: "decode-qr" },
  },
];

function previewFilterHits(query: string): LauncherHit[] {
  const q = query.trim().toLowerCase();
  if (!q) return [];
  return PREVIEW_CATALOG.filter(
    (item) =>
      item.label.toLowerCase().includes(q) ||
      item.feature_code.toLowerCase().includes(q) ||
      item.plugin_name.toLowerCase().includes(q) ||
      item.description.toLowerCase().includes(q),
  );
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

  if (tabPressed || (submitted && query.trim().startsWith("@"))) {
    return { promote_to_agent: { reason: tabPressed ? "tab_pressed" : "at_mention_prefix", query } };
  }
  const matches = previewFilterHits(query);
  const topConfidence = matches[0]?.confidence ?? 0;
  if (submitted && matches.length === 0 && query.trim().length > 0) {
    return { promote_to_agent: { reason: "low_deterministic_confidence", query } };
  }
  return {
    deterministic_results: {
      matches,
      top_confidence: topConfidence,
    },
  };
}

export async function fetchZeroQuerySuggestions(): Promise<QuickAgentAction[]> {
  return (await invokeSafe<QuickAgentAction[]>("plugin:aitools|zero_query_suggestions")) ?? [];
}

export async function launchItem(
  params: LaunchItemParams,
  onListPush?: (ev: ListPushEvent) => void,
): Promise<LaunchResult> {
  if (isTauriRuntime()) {
    const { Channel, invoke } = await import("@tauri-apps/api/core");
    const onListEvent = new Channel<ListPushEvent>();
    onListEvent.onmessage = (ev) => {
      onListPush?.(ev);
    };
    return invoke<LaunchResult>("plugin:aitools|launch_item", {
      featureCode: params.featureCode,
      pluginId: params.pluginId,
      query: params.query,
      kind: params.kind,
      traceId: params.traceId,
      onListEvent,
    });
  }

  if (params.kind === "list" || params.featureCode === "snipaste-ocr") {
    return {
      kind: "list_mode",
      session_id: "preview-list-1",
      plugin_id: params.pluginId,
      plugin_name: "Snipaste Pro (Legacy)",
      feature_code: params.featureCode,
      placeholder: "Filter list actions...",
      items: PREVIEW_LIST_ITEMS,
    };
  }
  if (params.kind === "wasm") {
    return {
      kind: "wasm_result",
      plugin_id: params.pluginId,
      feature_code: params.featureCode,
      output: { status: "ok", mode: "browser-preview", input: params.query },
    };
  }
  return {
    kind: "opened_window",
    plugin_id: params.pluginId,
    window_label: `plugin-${params.pluginId}`,
    hide_main: false,
  };
}

export async function searchListMode(
  params: SearchListModeParams,
  onListPush?: (ev: ListPushEvent) => void,
): Promise<ListSearchResponse> {
  if (isTauriRuntime()) {
    const { Channel, invoke } = await import("@tauri-apps/api/core");
    const onListEvent = new Channel<ListPushEvent>();
    onListEvent.onmessage = (ev) => {
      onListPush?.(ev);
    };
    const res = await invoke<ListSearchResponse | ListItem[]>("plugin:aitools|search_list_mode", {
      sessionId: params.sessionId,
      pluginId: params.pluginId,
      featureCode: params.featureCode,
      query: params.query,
      traceId: params.traceId,
      onListEvent,
    });
    if (Array.isArray(res)) {
      return { session_id: params.sessionId, items: res };
    }
    return res;
  }

  const q = params.query.trim().toLowerCase();
  const filtered = PREVIEW_LIST_ITEMS.filter(
    (item) =>
      !q || item.title.toLowerCase().includes(q) || item.description.toLowerCase().includes(q),
  );
  return { session_id: params.sessionId, items: filtered };
}

export async function selectListItem(params: SelectListItemParams): Promise<SelectListResult> {
  const res = await invokeSafe<SelectListResult>("plugin:aitools|select_list_item", {
    sessionId: params.sessionId,
    pluginId: params.pluginId,
    featureCode: params.featureCode,
    item: params.item,
    index: params.index,
    traceId: params.traceId,
  });
  if (res) return res;
  return {
    ok: true,
    out_plugin: true,
    hide_main: false,
    message: `Selected "${params.item.title}"`,
  };
}

export async function syncZtoolsDir(dirPath?: string): Promise<SyncZtoolsResult> {
  const res = await invokeSafe<SyncZtoolsResult>("plugin:aitools|sync_ztools_dir", {
    dirPath: dirPath?.trim() ? dirPath.trim() : null,
  });
  if (res) return res;
  return {
    dir: dirPath?.trim() || "~/.aitools/plugins/ztools",
    indexed_plugins: 0,
    indexed_features: 0,
    errors: ["Sync requires desktop Tauri runtime"],
  };
}

export async function submitAgentPrompt(
  params: SubmitAgentPromptParams,
  onEvent: (ev: AgentStreamEvent) => void,
): Promise<AgentTurnResult> {
  if (isTauriRuntime()) {
    const { Channel, invoke } = await import("@tauri-apps/api/core");
    const channel = new Channel<AgentStreamEvent>();
    channel.onmessage = (ev) => {
      onEvent(ev);
    };
    return invoke<AgentTurnResult>("plugin:aitools|submit_agent_prompt", {
      prompt: params.prompt,
      traceId: params.traceId,
      chips: params.chips,
      profile: params.profile ?? "balanced",
      onEvent: channel,
    });
  }

  onEvent({ node_started: { node_id: "step_1", tool_name: "os.clipboard.read" } });
  await new Promise((r) => setTimeout(r, 80));
  onEvent({ node_completed: { node_id: "step_1", output: { ok: true } } });
  const text = `Preview response for "${params.prompt}" (${params.chips.length} context chip(s) attached).`;
  for (const chunk of text.split(" ")) {
    onEvent({ token: { text: chunk + " " } });
    await new Promise((r) => setTimeout(r, 18));
  }
  onEvent({ completed: { final_text: text, trace_id: params.traceId } });
  return { trace_id: params.traceId, final_text: text };
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
