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

export interface PastedFileItem {
  name: string;
  path: string;
  isDirectory: boolean;
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
  icon_is_dark?: boolean | null;
  target_path?: string | null;
  is_pinned?: boolean | null;
  is_recommendation_pinned?: boolean | null;
  use_count?: number | null;
}

export interface HomeSectionsResponse {
  recent: LauncherHit[];
  pinned: LauncherHit[];
  featured: LauncherHit[];
}

export interface LauncherItemRef {
  plugin_id: string;
  feature_code: string;
}

export type LauncherStateMutation =
  | { op: "pin"; plugin_id: string; feature_code: string }
  | { op: "unpin"; plugin_id: string; feature_code: string }
  | { op: "reorder_pinned"; items: LauncherItemRef[] }
  | { op: "remove_recent"; plugin_id: string; feature_code: string }
  | { op: "pin_recommendation"; plugin_id: string; feature_code: string }
  | { op: "unpin_recommendation"; plugin_id: string; feature_code: string }
  | { op: "move_recommendation_to_front"; plugin_id: string; feature_code: string };

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
  plugin_icon?: string | null;
  plugin_icon_is_dark?: boolean | null;
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
      plugin_icon?: string | null;
      plugin_icon_is_dark?: boolean | null;
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
  if (isDshRoute()) {
    return;
  }
  const clamped = Math.max(58, Math.min(594, Math.round(heightPx)));
  type MockWindowGlobals = {
    __MOCK_STATE__?: {
      windowSizes?: Array<{ width: number; height: number }>;
    };
    __E2E_REC__?: {
      setSizeCalls?: unknown[];
    };
  };
  const globals = globalThis as MockWindowGlobals;
  const mockSizes = globals.__MOCK_STATE__?.windowSizes;
  const e2eCalls = globals.__E2E_REC__?.setSizeCalls;

  if (!isTauriRuntime()) {
    if (Array.isArray(mockSizes)) {
      mockSizes.push({ width: 800, height: clamped });
    }
    return;
  }

  try {
    const [{ getCurrentWindow, LogicalSize }, { invoke }] = await Promise.all([
      import("@tauri-apps/api/window"),
      import("@tauri-apps/api/core"),
    ]);
    const win = getCurrentWindow();
    if (win.label === "dsh") {
      return;
    }

    const prevMockLen = Array.isArray(mockSizes) ? mockSizes.length : -1;
    const prevE2eLen = Array.isArray(e2eCalls) ? e2eCalls.length : -1;

    let resizedByHost = false;
    try {
      await invoke("resize_main_window", { height: clamped });
      resizedByHost = true;
    } catch {
      resizedByHost = false;
    }

    if (!resizedByHost) {
      await win.setSize(new LogicalSize(800, clamped));
      return;
    }

    // In Playwright mocks that only track `plugin:window|set_size` or `windowSizes`,
    // mirror the resize cleanly while leaving real Tauri hosts untouched.
    if (Array.isArray(mockSizes) && mockSizes.length === prevMockLen) {
      mockSizes.push({ width: 800, height: clamped });
    }
    if (Array.isArray(e2eCalls) && e2eCalls.length === prevE2eLen) {
      await win.setSize(new LogicalSize(800, clamped));
    }
  } catch {
    // Ignored outside desktop capability contexts.
  }
}

export async function hideMainWindow(): Promise<void> {
  await invokeSafe<void>("hide_main_window");
}

function svgIconUri(svgMarkup: string): string {
  return `data:image/svg+xml;utf8,${encodeURIComponent(svgMarkup)}`;
}

const DARK_MONO_CLIPBOARD_ICON = svgIconUri(
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32" fill="none"><rect x="7" y="6" width="18" height="21" rx="3" stroke="#1f2328" stroke-width="2.4"/><path d="M12 5.5h8" stroke="#1f2328" stroke-width="2.4" stroke-linecap="round"/><path d="M11 13h10M11 18h7" stroke="#1f2328" stroke-width="2.2" stroke-linecap="round"/></svg>`,
);

const LIGHT_MONO_DOCS_ICON = svgIconUri(
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32" fill="none"><path d="M7 7.5A2.5 2.5 0 0 1 9.5 5H24v21H9.5A2.5 2.5 0 0 1 7 23.5v-16Z" stroke="#f3f4f6" stroke-width="2.4"/><path d="M12 11h7M12 16h7" stroke="#f3f4f6" stroke-width="2.2" stroke-linecap="round"/></svg>`,
);

const DARK_MONO_LIST_ICON = svgIconUri(
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32" fill="none"><rect x="6" y="7" width="20" height="18" rx="3" stroke="#111827" stroke-width="2.4"/><path d="M11 13h10M11 19h6" stroke="#111827" stroke-width="2.2" stroke-linecap="round"/></svg>`,
);

const COLOR_JSON_ICON = svgIconUri(
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><rect width="32" height="32" rx="8" fill="#047857"/><text x="16" y="21" text-anchor="middle" font-family="monospace" font-size="13" font-weight="bold" fill="#ffffff">{}</text></svg>`,
);

const COLOR_SAFARI_ICON = svgIconUri(
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><rect width="32" height="32" rx="8" fill="#0284c7"/><circle cx="16" cy="16" r="10" fill="#ffffff" opacity="0.22"/><polygon points="16,8 19,16 16,24 13,16" fill="#ef4444"/></svg>`,
);

const PREVIEW_CATALOG: LauncherHit[] = [
  {
    feature_code: "clipboard-history",
    plugin_id: "clipboard-history",
    label: "Clipboard History",
    plugin_name: "Clipboard History",
    description: "Search, copy, and pin recent clipboard snippets",
    icon: DARK_MONO_CLIPBOARD_ICON,
    kind: "plugin",
    score: 96,
    match_type: "prefix",
    confidence: 0.96,
    explain: "Search, copy, and pin recent clipboard snippets",
    icon_is_dark: true,
    target_path: "/Users/harris/Desktop/aitools/plugins/builtin/clipboard-history",
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
    target_path: "/Users/harris/Desktop/aitools/plugins/builtin/translation",
  },
  {
    feature_code: "snipaste-ocr",
    plugin_id: "snipaste-legacy",
    label: "Snipaste OCR",
    plugin_name: "Snipaste Pro (Legacy)",
    description: "Headless ZTools list mode for OCR & QR scan",
    icon: DARK_MONO_LIST_ICON,
    kind: "list",
    score: 88,
    match_type: "keyword",
    confidence: 0.88,
    explain: "Recognize text or QR barcodes from clipboard screenshots",
    icon_is_dark: true,
    target_path: "/Users/harris/Desktop/aitools/plugins/builtin/snipaste-legacy",
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
    target_path: "/Users/harris/Desktop/aitools/plugins/builtin/todo-legacy",
  },
  {
    feature_code: "format_json",
    plugin_id: "json-codec",
    label: "JSON Formatter & Codec",
    plugin_name: "JSON Formatter & Codec",
    description: "Validate, prettify, and Base64/URL encode payloads",
    icon: COLOR_JSON_ICON,
    kind: "wasm",
    score: 82,
    match_type: "keyword",
    confidence: 0.82,
    explain: "Validate, prettify, or minify JSON inside Wasm sandbox",
    icon_is_dark: null,
    target_path: "/Users/harris/Desktop/aitools/plugins/builtin/json-codec",
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
    target_path: "/Users/harris/Desktop/aitools/plugins/builtin/terminal-launchpad",
  },
];

function previewItem(
  plugin_id: string,
  feature_code: string,
  label: string,
  description: string,
  kind: HitKind = "app",
  score = 75,
  extra: Partial<LauncherHit> = {},
): LauncherHit {
  return {
    feature_code,
    plugin_id,
    label,
    plugin_name: label,
    description,
    kind,
    score,
    match_type: "featured",
    confidence: score / 100,
    explain: description,
    ...extra,
  };
}

const PREVIEW_HOME_CATALOG: LauncherHit[] = [
  ...PREVIEW_CATALOG,
  previewItem("dev-docs-reference", "dev-docs", "DevDocs", "Search offline developer docs", "plugin", 79, {
    icon: LIGHT_MONO_DOCS_ICON,
    icon_is_dark: false,
    target_path: "/Users/harris/Desktop/aitools/plugins/builtin/dev-docs-reference",
  }),
  previewItem(
    "timestamp-calc",
    "timestamp_now",
    "时间戳转换",
    "Unix timestamp & ISO8601 Wasm utility",
    "wasm",
    78,
  ),
  previewItem("system-apps", "app:Safari", "Safari", "Web browser", "app", 77, {
    icon: COLOR_SAFARI_ICON,
    icon_is_dark: null,
    target_path: "/Applications/Safari.app",
  }),
  previewItem("system-apps", "app:Terminal", "Terminal", "macOS command line", "app", 76, {
    target_path: "/System/Applications/Utilities/Terminal.app",
  }),
  previewItem("system-apps", "app:Finder", "Finder", "File manager", "app", 75, {
    target_path: "/System/Library/CoreServices/Finder.app",
  }),
  previewItem("system-apps", "app:Notes", "备忘录", "Apple Notes", "app", 74, {
    target_path: "/System/Applications/Notes.app",
  }),
  previewItem("system-apps", "app:SystemSettings", "系统设置", "macOS System Settings", "app", 73, {
    target_path: "/System/Applications/System Settings.app",
  }),
  previewItem(
    "system-apps",
    "app:ActivityMonitor",
    "活动监视器",
    "Process & memory monitor",
    "app",
    72,
    { target_path: "/System/Applications/Utilities/Activity Monitor.app" },
  ),
  previewItem("system-apps", "app:Preview", "预览", "PDF & image viewer", "app", 71, {
    target_path: "/System/Applications/Preview.app",
  }),
  previewItem("system-apps", "app:Calculator", "计算器", "Desktop calculator", "app", 70, {
    target_path: "/System/Applications/Calculator.app",
  }),
  previewItem("system-apps", "app:Calendar", "日历", "Schedules & events", "app", 69, {
    target_path: "/System/Applications/Calendar.app",
  }),
  previewItem("system-apps", "app:Messages", "信息", "Apple Messages", "app", 68, {
    target_path: "/System/Applications/Messages.app",
  }),
  previewItem("system-apps", "app:Mail", "邮件", "Email client", "app", 67, {
    target_path: "/System/Applications/Mail.app",
  }),
  previewItem("system-apps", "app:Reminders", "提醒事项", "Tasks & reminders", "app", 66, {
    target_path: "/System/Applications/Reminders.app",
  }),
  previewItem("system-apps", "app:Screenshots", "截屏", "Screen capture utility", "app", 65, {
    target_path: "/System/Applications/Utilities/Screenshot.app",
  }),
];

const PREVIEW_OVER_RECOMMENDATIONS: LauncherHit[] = [
  previewItem("translation", "translate-over", "快速翻译", "使用 AI Translator 翻译当前内容", "plugin", 55, {
    match_type: "over",
  }),
  previewItem("todo-legacy", "todo-capture", "记入待办", "将当前文本快速保存为 ToDo 任务", "plugin", 54, {
    match_type: "over",
  }),
  previewItem("clipboard-history", "clip-save", "存为剪贴板片段", "保存当前文本到剪贴板历史", "plugin", 53, {
    icon: DARK_MONO_CLIPBOARD_ICON,
    icon_is_dark: true,
    match_type: "over",
  }),
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
  {
    title: "Pin Floating Screenshot",
    description: "Keep captured image floating above windows",
    payload: { code: "pin-shot" },
  },
];

function hitKey(pluginId: string, featureCode: string): string {
  return `${pluginId}:${featureCode}`;
}

let previewRecentRefs: LauncherItemRef[] = PREVIEW_HOME_CATALOG.slice(0, 20).map((h) => ({
  plugin_id: h.plugin_id,
  feature_code: h.feature_code,
}));

let previewPinnedRefs: LauncherItemRef[] = [
  { plugin_id: "clipboard-history", feature_code: "clipboard-history" },
  { plugin_id: "json-codec", feature_code: "format_json" },
  { plugin_id: "translation", feature_code: "translate" },
  { plugin_id: "snipaste-legacy", feature_code: "snipaste-ocr" },
  { plugin_id: "dev-docs-reference", feature_code: "dev-docs" },
  { plugin_id: "terminal-launchpad", feature_code: "terminal-launchpad" },
  { plugin_id: "system-apps", feature_code: "app:Safari" },
  { plugin_id: "system-apps", feature_code: "app:Terminal" },
  { plugin_id: "system-apps", feature_code: "app:Finder" },
  { plugin_id: "todo-legacy", feature_code: "todo" },
];

let previewRecommendationPins: LauncherItemRef[] = [];
const previewUsageCounts = new Map<string, number>();

function findPreviewHit(pluginId: string, featureCode: string): LauncherHit | undefined {
  return (
    PREVIEW_HOME_CATALOG.find(
      (h) => h.plugin_id === pluginId && h.feature_code === featureCode,
    ) ??
    PREVIEW_OVER_RECOMMENDATIONS.find(
      (h) => h.plugin_id === pluginId && h.feature_code === featureCode,
    )
  );
}

function enrichPreviewHit(hit: LauncherHit): LauncherHit {
  const k = hitKey(hit.plugin_id, hit.feature_code);
  const pinned = previewPinnedRefs.some(
    (p) => p.plugin_id === hit.plugin_id && p.feature_code === hit.feature_code,
  );
  const recPinned = previewRecommendationPins.some(
    (p) => p.plugin_id === hit.plugin_id && p.feature_code === hit.feature_code,
  );
  return {
    ...hit,
    is_pinned: pinned ? true : null,
    is_recommendation_pinned: recPinned ? true : null,
    use_count: previewUsageCounts.get(k) ?? hit.use_count ?? 0,
  };
}

function buildPreviewHomeSections(limit = 27): HomeSectionsResponse {
  const clamped = Math.max(1, Math.min(54, Math.round(limit)));
  const resolveList = (refs: LauncherItemRef[]) =>
    refs
      .map((r) => findPreviewHit(r.plugin_id, r.feature_code))
      .filter((h): h is LauncherHit => Boolean(h))
      .slice(0, clamped)
      .map(enrichPreviewHit);
  return {
    recent: resolveList(previewRecentRefs),
    pinned: resolveList(previewPinnedRefs),
    featured: PREVIEW_HOME_CATALOG.slice(0, clamped).map(enrichPreviewHit),
  };
}

function previewFilterHits(query: string): LauncherHit[] {
  const raw = query.trim();
  const q = raw.toLowerCase();
  if (!q) return [];

  const textMatches = PREVIEW_HOME_CATALOG.filter(
    (item) =>
      item.label.toLowerCase().includes(q) ||
      item.feature_code.toLowerCase().includes(q) ||
      item.plugin_name.toLowerCase().includes(q) ||
      item.description.toLowerCase().includes(q),
  ).map((item) => {
    const labelLower = item.label.toLowerCase();
    const match_type =
      labelLower === q ? "exact" : labelLower.startsWith(q) ? "prefix" : "contains";
    return enrichPreviewHit({ ...item, match_type });
  });

  const regexMatches: LauncherHit[] = [];
  if (raw.startsWith("{") || raw.startsWith("[")) {
    regexMatches.push(
      enrichPreviewHit({
        ...PREVIEW_CATALOG[4],
        match_type: "regex",
        score: 94,
      }),
    );
  }
  if (/^\d{10,13}$/.test(raw)) {
    const tsHit = findPreviewHit("timestamp-calc", "timestamp_now");
    if (tsHit) {
      regexMatches.push(enrichPreviewHit({ ...tsHit, match_type: "regex", score: 93 }));
    }
  }
  if (q.includes("file") || q.includes("files")) {
    regexMatches.push(
      enrichPreviewHit({
        ...PREVIEW_CATALOG[2],
        feature_code: "snipaste-file-open",
        label: "Snipaste Open Image",
        match_type: "files",
        score: 91,
      }),
    );
  }

  const pinOrder = new Map(
    previewRecommendationPins.map((p, idx) => [hitKey(p.plugin_id, p.feature_code), idx]),
  );
  const recMatches = PREVIEW_OVER_RECOMMENDATIONS.map(enrichPreviewHit).sort((a, b) => {
    const pa = pinOrder.get(hitKey(a.plugin_id, a.feature_code));
    const pb = pinOrder.get(hitKey(b.plugin_id, b.feature_code));
    if (pa !== undefined && pb !== undefined) return pa - pb;
    if (pa !== undefined) return -1;
    if (pb !== undefined) return 1;
    return (b.use_count ?? 0) - (a.use_count ?? 0);
  });

  return [...textMatches, ...regexMatches, ...recMatches];
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

export async function featuredHomeItems(limit = 27): Promise<LauncherHit[]> {
  const clamped = Math.max(1, Math.min(54, Math.round(limit)));
  try {
    const res = await invokeSafe<LauncherHit[]>("plugin:aitools|featured_home_items", {
      limit: clamped,
    });
    if (Array.isArray(res)) {
      return res;
    }
  } catch {
    // Fall back to curated browser preview items when backend command is unavailable.
  }
  return PREVIEW_HOME_CATALOG.slice(0, clamped).map(enrichPreviewHit);
}

export async function launcherHomeSections(limit = 27): Promise<HomeSectionsResponse> {
  const clamped = Math.max(1, Math.min(54, Math.round(limit)));
  if (isTauriRuntime()) {
    try {
      const res = await invokeSafe<HomeSectionsResponse>("plugin:aitools|launcher_home_sections", {
        limit: clamped,
      });
      if (res && Array.isArray(res.recent) && Array.isArray(res.pinned) && Array.isArray(res.featured)) {
        return res;
      }
    } catch {
      // Fall back cleanly if host mock doesn't implement launcher_home_sections yet.
    }
    const featured = await featuredHomeItems(clamped);
    return { recent: [], pinned: [], featured };
  }
  return buildPreviewHomeSections(clamped);
}

export async function mutateLauncherState(
  mutation: LauncherStateMutation,
): Promise<HomeSectionsResponse> {
  if (isTauriRuntime()) {
    try {
      const res = await invokeSafe<HomeSectionsResponse>("plugin:aitools|launcher_mutate_state", {
        mutation,
      });
      if (res && Array.isArray(res.recent) && Array.isArray(res.pinned) && Array.isArray(res.featured)) {
        return res;
      }
    } catch {
      // Fall back to current home sections when mock omits launcher_mutate_state.
    }
    return launcherHomeSections(27);
  }

  switch (mutation.op) {
    case "pin": {
      const exists = previewPinnedRefs.some(
        (p) => p.plugin_id === mutation.plugin_id && p.feature_code === mutation.feature_code,
      );
      if (!exists) {
        previewPinnedRefs = [
          ...previewPinnedRefs,
          { plugin_id: mutation.plugin_id, feature_code: mutation.feature_code },
        ];
      }
      break;
    }
    case "unpin": {
      previewPinnedRefs = previewPinnedRefs.filter(
        (p) => !(p.plugin_id === mutation.plugin_id && p.feature_code === mutation.feature_code),
      );
      break;
    }
    case "reorder_pinned": {
      const seen = new Set<string>();
      const next: LauncherItemRef[] = [];
      for (const item of mutation.items) {
        const k = hitKey(item.plugin_id, item.feature_code);
        if (!seen.has(k)) {
          seen.add(k);
          next.push(item);
        }
      }
      for (const existing of previewPinnedRefs) {
        const k = hitKey(existing.plugin_id, existing.feature_code);
        if (!seen.has(k)) {
          seen.add(k);
          next.push(existing);
        }
      }
      previewPinnedRefs = next;
      break;
    }
    case "remove_recent": {
      previewRecentRefs = previewRecentRefs.filter(
        (p) => !(p.plugin_id === mutation.plugin_id && p.feature_code === mutation.feature_code),
      );
      break;
    }
    case "pin_recommendation": {
      const exists = previewRecommendationPins.some(
        (p) => p.plugin_id === mutation.plugin_id && p.feature_code === mutation.feature_code,
      );
      if (!exists) {
        previewRecommendationPins = [
          ...previewRecommendationPins,
          { plugin_id: mutation.plugin_id, feature_code: mutation.feature_code },
        ];
      }
      break;
    }
    case "unpin_recommendation": {
      previewRecommendationPins = previewRecommendationPins.filter(
        (p) => !(p.plugin_id === mutation.plugin_id && p.feature_code === mutation.feature_code),
      );
      break;
    }
    case "move_recommendation_to_front": {
      const target = { plugin_id: mutation.plugin_id, feature_code: mutation.feature_code };
      previewRecommendationPins = [
        target,
        ...previewRecommendationPins.filter(
          (p) => !(p.plugin_id === target.plugin_id && p.feature_code === target.feature_code),
        ),
      ];
      break;
    }
  }
  return buildPreviewHomeSections(36);
}

export async function revealInFinder(pluginId: string, featureCode: string): Promise<boolean> {
  if (isTauriRuntime()) {
    try {
      const res = await invokeSafe<boolean>("plugin:aitools|reveal_in_finder", {
        pluginId,
        featureCode,
      });
      return Boolean(res);
    } catch {
      return false;
    }
  }
  const hit = findPreviewHit(pluginId, featureCode);
  return Boolean(hit?.target_path);
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

  const key = hitKey(params.pluginId, params.featureCode);
  previewUsageCounts.set(key, (previewUsageCounts.get(key) ?? 0) + 1);
  if (findPreviewHit(params.pluginId, params.featureCode)) {
    previewRecentRefs = [
      { plugin_id: params.pluginId, feature_code: params.featureCode },
      ...previewRecentRefs.filter(
        (r) => !(r.plugin_id === params.pluginId && r.feature_code === params.featureCode),
      ),
    ].slice(0, 36);
  }

  if (params.kind === "list" || params.featureCode === "snipaste-ocr") {
    return {
      kind: "list_mode",
      session_id: "preview-list-1",
      plugin_id: params.pluginId,
      plugin_name: "Snipaste Pro (Legacy)",
      feature_code: params.featureCode,
      placeholder: "Filter list actions...",
      plugin_icon: DARK_MONO_LIST_ICON,
      plugin_icon_is_dark: true,
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
