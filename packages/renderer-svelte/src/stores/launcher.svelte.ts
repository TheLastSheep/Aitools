import {
  fetchDshLogs,
  fetchZeroQuerySuggestions,
  hideMainWindow,
  ingestFrontendLog,
  isDshRoute,
  isTauriRuntime,
  launchItem,
  launcherHomeSections,
  mutateLauncherState,
  resolveHitlStep,
  revealInFinder,
  searchFeatures,
  searchListMode,
  selectListItem,
  setFaultInjection,
  submitAgentPrompt,
  syncZtoolsDir,
  type AgentStreamEvent,
  type AiProfile,
  type ContextChip,
  type DagEvent,
  type FaultInjectionConfig,
  type HomeSectionsResponse,
  type LauncherHit,
  type LauncherMode,
  type LauncherStateMutation,
  type ListItem,
  type ListPushEvent,
  type ListSession,
  type LogRecord,
  type PastedFileItem,
  type QuickAgentAction,
  type ResolvedTheme,
  type SyncZtoolsResult,
  type ThemePreference,
} from "../lib/ipc";
import {
  clampGridCoords,
  hitIdentityKey,
  stepGridSelection,
  type NavigationGridRow,
} from "../lib/launcherUi";

export type SearchViewMode = "aggregate" | "list";

const THEME_STORAGE_KEY = "aitools:theme";
const SEARCH_MODE_STORAGE_KEY = "aitools:search-mode";

function readInitialThemePref(): ThemePreference {
  if (typeof localStorage === "undefined") return "system";
  const saved = localStorage.getItem(THEME_STORAGE_KEY);
  return saved === "light" || saved === "dark" || saved === "system" ? saved : "system";
}

function readInitialSearchViewMode(): SearchViewMode {
  if (typeof localStorage === "undefined") return "aggregate";
  const saved = localStorage.getItem(SEARCH_MODE_STORAGE_KEY);
  return saved === "list" ? "list" : "aggregate";
}

function resolveOsTheme(): ResolvedTheme {
  if (typeof window !== "undefined" && window.matchMedia?.("(prefers-color-scheme: dark)").matches) {
    return "dark";
  }
  return "light";
}

function applyThemeAttribute(theme: ResolvedTheme): void {
  if (typeof document !== "undefined") {
    document.documentElement.dataset.atoolsTheme = theme;
  }
}

export function isTier2Risk(riskTier: string | undefined): boolean {
  const normalized = (riskTier ?? "").toLowerCase();
  return normalized.includes("tier2") || normalized.includes("destructive");
}

function filterHitsBySubstring(source: LauncherHit[], filterText: string): LauncherHit[] {
  const q = filterText.trim().toLowerCase();
  if (!q) return source;
  return source.filter((hit) => {
    const fields = [hit.label, hit.plugin_name, hit.description, hit.feature_code, hit.explain];
    return fields.some((f) => (f || "").toLowerCase().includes(q));
  });
}

export function createLauncherStore() {
  let dshStandalone = $state(isDshRoute());
  let mode = $state<LauncherMode>("search");
  let searchViewMode = $state<SearchViewMode>(readInitialSearchViewMode());
  let enteredAgentByAt = $state(false);
  let query = $state("");
  let traceId = $state("wake-0");
  let chips = $state<ContextChip[]>([]);
  let pastedText = $state<string | null>(null);
  let pastedFiles = $state<PastedFileItem[] | null>(null);
  let pastedBaseHits = $state<LauncherHit[]>([]);
  let suggestions = $state<QuickAgentAction[]>([]);

  // Home sections
  let recentItems = $state<LauncherHit[]>([]);
  let pinnedItems = $state<LauncherHit[]>([]);
  let featuredItems = $state<LauncherHit[]>([]);
  let recentExpanded = $state(false);
  let pinnedExpanded = $state(false);
  let homeExpanded = $state(false);
  let searchResultsExpanded = $state(false);
  let bestMatchesExpanded = $state(false);
  let recommendationsExpanded = $state(false);

  let hasWoken = $state(false);
  let selectedRow = $state(0);
  let selectedCol = $state(0);
  let lastWakeTraceId = "";
  let hits = $state<LauncherHit[]>([]);
  let searchLoading = $state(false);
  let searchError = $state<string | null>(null);

  // List-mode state (headless ZTools plugins, hosted in single top search bar)
  let listSession = $state<ListSession | null>(null);
  let subInput = $state("");
  let listSelectedIndex = $state(0);
  let listLoading = $state(false);
  let listError = $state<string | null>(null);

  // Inline feedback banner (Wasm / headless result / errors)
  let statusBanner = $state<{ kind: "info" | "success" | "error"; text: string } | null>(null);

  // Interactive Agent & HITL state
  let agentProfile = $state<AiProfile>("balanced");
  let agentStatus = $state<"idle" | "streaming" | "waiting_hitl" | "done" | "error">("idle");
  let agentPromptSubmitted = $state("");
  let agentResponse = $state("");
  let agentError = $state<string | null>(null);
  let dagEvents = $state<DagEvent[]>([]);
  let pendingHitl = $state<{
    nodeId: string;
    toolName: string;
    riskTier: string;
    arguments: unknown;
  } | null>(null);

  // DSH & Fault Injection state
  let dshOpen = $state(false);
  let dshLogs = $state<LogRecord[]>([]);
  let faults = $state<FaultInjectionConfig>({
    injectLmdbRevConflict: false,
    injectPermissionDenied: false,
    injectWasmFuelExhaustion: false,
    injectLlmStreamAbort: false,
  });
  let faultPending = $state(false);
  let faultError = $state<string | null>(null);
  let syncPending = $state(false);
  let lastSyncResult = $state<SyncZtoolsResult | null>(null);

  // Theme state
  const initialThemePref = readInitialThemePref();
  const initialOsTheme = resolveOsTheme();
  let themePreference = $state<ThemePreference>(initialThemePref);
  let osTheme = $state<ResolvedTheme>(initialOsTheme);
  const resolvedTheme = $derived<ResolvedTheme>(
    themePreference === "system" ? osTheme : themePreference,
  );

  applyThemeAttribute(initialThemePref === "system" ? initialOsTheme : initialThemePref);

  function setDraftQuery(next: string) {
    query = next;
  }

  function setDraftSubInput(next: string) {
    subInput = next;
  }

  function setThemePreference(next: ThemePreference) {
    themePreference = next;
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(THEME_STORAGE_KEY, next);
    }
    applyThemeAttribute(next === "system" ? osTheme : next);
  }

  function cycleTheme() {
    const next: ThemePreference = resolvedTheme === "dark" ? "light" : "dark";
    setThemePreference(next);
  }

  function setSearchViewMode(next: SearchViewMode) {
    searchViewMode = next;
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(SEARCH_MODE_STORAGE_KEY, next);
    }
    selectedRow = 0;
    selectedCol = 0;
  }

  function toggleSearchViewMode() {
    setSearchViewMode(searchViewMode === "aggregate" ? "list" : "aggregate");
  }

  function applyHomeSectionsResponse(res: HomeSectionsResponse) {
    recentItems = res.recent;
    pinnedItems = res.pinned;
    featuredItems = res.featured;
    const pinnedSet = new Set(res.pinned.map(hitIdentityKey));
    const syncHitFlags = (list: LauncherHit[]) =>
      list.map((h) => ({
        ...h,
        is_pinned: pinnedSet.has(hitIdentityKey(h)) ? true : null,
      }));
    if (hits.length > 0) {
      hits = syncHitFlags(hits);
    }
    if (pastedBaseHits.length > 0) {
      pastedBaseHits = syncHitFlags(pastedBaseHits);
    }
  }

  async function refreshHomeItems(limit = 36) {
    const res = await launcherHomeSections(limit);
    applyHomeSectionsResponse(res);
  }

  function resetSelection() {
    selectedRow = 0;
    selectedCol = 0;
  }

  function resetCollapseState() {
    recentExpanded = false;
    pinnedExpanded = false;
    homeExpanded = false;
    searchResultsExpanded = false;
    bestMatchesExpanded = false;
    recommendationsExpanded = false;
  }

  function toggleRecentExpanded() {
    recentExpanded = !recentExpanded;
  }

  function togglePinnedExpanded() {
    pinnedExpanded = !pinnedExpanded;
  }

  function toggleHomeExpanded() {
    homeExpanded = !homeExpanded;
  }

  function toggleSearchResultsExpanded() {
    searchResultsExpanded = !searchResultsExpanded;
  }

  function toggleBestMatchesExpanded() {
    bestMatchesExpanded = !bestMatchesExpanded;
  }

  function toggleRecommendationsExpanded() {
    recommendationsExpanded = !recommendationsExpanded;
  }

  function clampSelection(grid: NavigationGridRow[]) {
    const next = clampGridCoords(grid, selectedRow, selectedCol);
    selectedRow = next.row;
    selectedCol = next.col;
  }

  function moveGridSelection(
    grid: NavigationGridRow[],
    key: "ArrowDown" | "ArrowUp" | "ArrowRight" | "ArrowLeft",
    navMode: "aggregate" | "list",
  ) {
    const next = stepGridSelection(grid, selectedRow, selectedCol, key, navMode);
    selectedRow = next.row;
    selectedCol = next.col;
  }

  function isPinnedHit(hit: Pick<LauncherHit, "plugin_id" | "feature_code" | "is_pinned">): boolean {
    if (hit.is_pinned) return true;
    const k = hitIdentityKey(hit);
    return pinnedItems.some((p) => hitIdentityKey(p) === k);
  }

  async function runSearchForText(text: string): Promise<LauncherHit[]> {
    const decision = await searchFeatures(text, false, false);
    if ("deterministic_results" in decision) {
      return decision.deterministic_results.matches;
    }
    return [];
  }

  async function onQueryChange(next: string) {
    query = next;
    searchError = null;
    statusBanner = null;
    resetSelection();
    resetCollapseState();

    if (next.startsWith("@") && mode === "search" && !pastedText && !pastedFiles) {
      mode = "agent";
      enteredAgentByAt = true;
      return;
    }

    if (mode === "agent" && enteredAgentByAt && !next.startsWith("@")) {
      mode = "search";
      enteredAgentByAt = false;
    }

    if (mode === "search") {
      if (pastedText || (pastedFiles && pastedFiles.length > 0)) {
        hits = next.trim() ? filterHitsBySubstring(pastedBaseHits, next) : pastedBaseHits;
        return;
      }

      if (!next.trim()) {
        hits = [];
        return;
      }

      searchLoading = true;
      try {
        hits = await runSearchForText(next);
      } catch (err) {
        searchError = err instanceof Error ? err.message : String(err);
      } finally {
        searchLoading = false;
      }
    }
  }

  async function setPastedText(text: string) {
    const trimmed = text.trim();
    if (!trimmed) return;
    pastedFiles = null;
    pastedText = text;
    query = "";
    searchError = null;
    statusBanner = null;
    resetSelection();
    resetCollapseState();

    searchLoading = true;
    try {
      pastedBaseHits = await runSearchForText(text);
      hits = pastedBaseHits;
    } catch (err) {
      searchError = err instanceof Error ? err.message : String(err);
    } finally {
      searchLoading = false;
    }
  }

  async function setPastedFiles(files: PastedFileItem[]) {
    if (files.length === 0) return;
    pastedText = null;
    pastedFiles = files;
    query = "";
    searchError = null;
    statusBanner = null;
    resetSelection();
    resetCollapseState();

    searchLoading = true;
    try {
      const fileQuery = files.length === 1 ? files[0].name : "files";
      const [byKind, byName] = await Promise.all([
        runSearchForText("files"),
        runSearchForText(fileQuery),
      ]);
      const seen = new Set<string>();
      const merged: LauncherHit[] = [];
      for (const item of [...byKind, ...byName]) {
        const k = hitIdentityKey(item);
        if (seen.has(k)) continue;
        seen.add(k);
        merged.push(item);
      }
      pastedBaseHits = merged;
      hits = merged;
    } catch (err) {
      searchError = err instanceof Error ? err.message : String(err);
    } finally {
      searchLoading = false;
    }
  }

  async function clearPastedText() {
    pastedText = null;
    pastedBaseHits = [];
    if (query.trim()) {
      await onQueryChange(query);
    } else {
      hits = [];
      resetSelection();
    }
  }

  async function clearPastedFiles() {
    pastedFiles = null;
    pastedBaseHits = [];
    if (query.trim()) {
      await onQueryChange(query);
    } else {
      hits = [];
      resetSelection();
    }
  }

  async function convertQueryToFilterChip(): Promise<boolean> {
    const trimmed = query.trim();
    if (!trimmed || mode !== "search") return false;
    await setPastedText(trimmed);
    return true;
  }

  async function editPastedTextChip() {
    if (!pastedText) return;
    const text = pastedText;
    pastedText = null;
    pastedBaseHits = [];
    query = text;
    void onQueryChange(text);
  }

  async function editContextChip(index: number) {
    const chip = chips[index];
    if (!chip) return;
    chips = chips.filter((_, idx) => idx !== index);
    query = chip.value;
    void onQueryChange(chip.value);
  }

  async function popLastChipToQuery(): Promise<{ restoredText: string | null; handled: boolean }> {
    if (pastedFiles && pastedFiles.length > 0) {
      await clearPastedFiles();
      return { restoredText: null, handled: true };
    }
    if (pastedText !== null) {
      const text = pastedText;
      pastedText = null;
      pastedBaseHits = [];
      query = text;
      void onQueryChange(text);
      return { restoredText: text, handled: true };
    }
    if (chips.length > 0) {
      const last = chips[chips.length - 1];
      chips = chips.slice(0, -1);
      query = last.value;
      void onQueryChange(last.value);
      return { restoredText: last.value, handled: true };
    }
    return { restoredText: null, handled: false };
  }

  async function mutateState(mutation: LauncherStateMutation) {
    const res = await mutateLauncherState(mutation);
    applyHomeSectionsResponse(res);

    if (
      mutation.op === "pin_recommendation" ||
      mutation.op === "unpin_recommendation" ||
      mutation.op === "move_recommendation_to_front"
    ) {
      if (pastedText) {
        pastedBaseHits = await runSearchForText(pastedText);
        hits = query.trim() ? filterHitsBySubstring(pastedBaseHits, query) : pastedBaseHits;
      } else if (query.trim()) {
        hits = await runSearchForText(query);
      }
    }
  }

  async function reorderPinned(nextOrder: LauncherHit[]) {
    pinnedItems = nextOrder;
    await mutateState({
      op: "reorder_pinned",
      items: nextOrder.map((h) => ({
        plugin_id: h.plugin_id,
        feature_code: h.feature_code,
      })),
    });
  }

  async function revealItem(hit: Pick<LauncherHit, "plugin_id" | "feature_code">): Promise<boolean> {
    return revealInFinder(hit.plugin_id, hit.feature_code);
  }

  async function promoteToAgent(prefillPrompt?: string) {
    mode = "agent";
    enteredAgentByAt = false;
    if (prefillPrompt !== undefined) {
      query = prefillPrompt;
    }
    await ingestFrontendLog("INFO", "renderer::launcher", "Promoted to Agent mode", traceId);
  }

  function exitAgentToSearch() {
    mode = "search";
    enteredAgentByAt = false;
    pendingHitl = null;
    if (agentStatus === "streaming" || agentStatus === "waiting_hitl") {
      agentStatus = "idle";
    }
  }

  function exitListToSearch() {
    mode = "search";
    listSession = null;
    subInput = "";
    listSelectedIndex = 0;
    listError = null;
  }

  function moveListSelection(delta: number) {
    if (mode === "list" && listSession && listSession.items.length > 0) {
      const len = listSession.items.length;
      listSelectedIndex = (listSelectedIndex + delta + len) % len;
    }
  }

  function applyListPush(ev: ListPushEvent) {
    if (!listSession || ev.plugin_id !== listSession.plugin_id) return;
    listSession = { ...listSession, items: ev.items };
    listSelectedIndex = 0;
  }

  async function launchHit(hit: LauncherHit) {
    statusBanner = null;
    searchError = null;
    try {
      const effectiveQuery = pastedText ? (query ? `${pastedText} ${query}` : pastedText) : query;
      const result = await launchItem(
        {
          featureCode: hit.feature_code,
          pluginId: hit.plugin_id,
          query: effectiveQuery,
          kind: hit.kind,
          traceId,
        },
        applyListPush,
      );

      void refreshHomeItems(36);

      switch (result.kind) {
        case "list_mode":
          mode = "list";
          listSession = {
            session_id: result.session_id,
            plugin_id: result.plugin_id,
            plugin_name: result.plugin_name,
            feature_code: result.feature_code,
            placeholder: result.placeholder,
            plugin_icon: result.plugin_icon ?? hit.icon ?? null,
            plugin_icon_is_dark: result.plugin_icon_is_dark ?? hit.icon_is_dark ?? null,
            items: result.items,
          };
          subInput = "";
          listSelectedIndex = 0;
          listError = null;
          break;
        case "opened_window":
        case "launched_app":
          if (result.hide_main !== false) {
            await hideMainWindow();
          } else {
            statusBanner = {
              kind: "success",
              text:
                result.kind === "opened_window"
                  ? `Opened ${hit.label || hit.feature_code}`
                  : `Launched ${result.app_name}`,
            };
          }
          break;
        case "executed_headless":
          if (result.hide_main) {
            await hideMainWindow();
          } else {
            statusBanner = {
              kind: "success",
              text: result.message || `Executed ${hit.label || hit.feature_code}`,
            };
          }
          break;
        case "wasm_result": {
          const rendered =
            typeof result.output === "string"
              ? result.output
              : JSON.stringify(result.output);
          statusBanner = {
            kind: "success",
            text: `${hit.label || hit.feature_code}: ${rendered}`,
          };
          break;
        }
        case "macro_started":
          mode = "agent";
          statusBanner = {
            kind: "info",
            text: `Running macro ${result.macro_id} (${result.trace_id})`,
          };
          break;
      }
    } catch (err) {
      statusBanner = {
        kind: "error",
        text: err instanceof Error ? err.message : String(err),
      };
    }
  }

  async function onSubInputChange(next: string) {
    subInput = next;
    if (!listSession) return;
    listLoading = true;
    listError = null;
    try {
      const res = await searchListMode(
        {
          sessionId: listSession.session_id,
          pluginId: listSession.plugin_id,
          featureCode: listSession.feature_code,
          query: next,
          traceId,
        },
        applyListPush,
      );
      if (listSession) {
        listSession = { ...listSession, items: res.items };
        listSelectedIndex = 0;
      }
    } catch (err) {
      listError = err instanceof Error ? err.message : String(err);
    } finally {
      listLoading = false;
    }
  }

  async function selectListAt(index: number) {
    if (!listSession || index < 0 || index >= listSession.items.length) return;
    const item = listSession.items[index];
    listLoading = true;
    listError = null;
    try {
      const res = await selectListItem({
        sessionId: listSession.session_id,
        pluginId: listSession.plugin_id,
        featureCode: listSession.feature_code,
        item,
        index,
        traceId,
      });
      if (res.items) {
        listSession = { ...listSession, items: res.items };
        listSelectedIndex = 0;
      }
      if (res.out_plugin) {
        exitListToSearch();
      }
      if (res.message) {
        statusBanner = { kind: "success", text: res.message };
      }
      if (res.hide_main) {
        await hideMainWindow();
      }
    } catch (err) {
      listError = err instanceof Error ? err.message : String(err);
    } finally {
      listLoading = false;
    }
  }

  async function handleFallbackSearchSubmit() {
    if (mode === "search" && query.trim().length > 0) {
      try {
        const decision = await searchFeatures(query, false, true);
        if ("deterministic_results" in decision && decision.deterministic_results.matches.length > 0) {
          hits = decision.deterministic_results.matches;
          resetSelection();
          await launchHit(hits[0]);
          return;
        }
        if ("promote_to_agent" in decision) {
          await promoteToAgent(decision.promote_to_agent.query || query);
          await submitAgentTurn();
        }
      } catch (err) {
        searchError = err instanceof Error ? err.message : String(err);
      }
    } else if (mode === "list") {
      await selectListAt(listSelectedIndex);
    } else if (mode === "agent") {
      await submitAgentTurn();
    }
  }

  async function handleEscape(): Promise<void> {
    if (dshOpen) {
      await toggleDsh();
      return;
    }
    if (mode === "list") {
      if (subInput.length > 0) {
        await onSubInputChange("");
        return;
      }
      if (pastedText || (pastedFiles && pastedFiles.length > 0)) {
        pastedText = null;
        pastedFiles = null;
        return;
      }
      exitListToSearch();
      return;
    }
    if (mode === "agent") {
      exitAgentToSearch();
      return;
    }
    if (statusBanner) {
      statusBanner = null;
      return;
    }
    if (query.length > 0) {
      await onQueryChange("");
      return;
    }
    if (pastedText || (pastedFiles && pastedFiles.length > 0) || chips.length > 0) {
      pastedText = null;
      pastedFiles = null;
      pastedBaseHits = [];
      chips = [];
      hits = [];
      resetSelection();
      return;
    }
    hasWoken = false;
    resetCollapseState();
    resetSelection();
    suggestions = [];
    await hideMainWindow();
  }

  async function submitAgentTurn(overridePrompt?: string) {
    const promptText = (overridePrompt ?? query).trim();
    const effectiveChips: ContextChip[] = [
      ...chips,
      ...(pastedText
        ? [{ kind: "clipboard" as const, label: "Filter", value: pastedText }]
        : []),
    ];
    if (!promptText && effectiveChips.length === 0) {
      agentError = "Enter a prompt or attach desktop context to run Agent.";
      return;
    }
    if (overridePrompt !== undefined) {
      query = overridePrompt;
    }
    mode = "agent";
    agentStatus = "streaming";
    agentPromptSubmitted = promptText || "Analyze attached desktop context";
    agentResponse = "";
    agentError = null;
    dagEvents = [];
    pendingHitl = null;

    const handleStreamEvent = (ev: AgentStreamEvent) => {
      if ("token" in ev) {
        agentResponse += ev.token.text;
      } else if ("node_started" in ev) {
        dagEvents = [...dagEvents, { node_started: ev.node_started }];
      } else if ("hitl_suspended" in ev) {
        dagEvents = [...dagEvents, { hitl_suspended: ev.hitl_suspended }];
        pendingHitl = {
          nodeId: ev.hitl_suspended.node_id,
          toolName: ev.hitl_suspended.tool_name,
          riskTier: String(ev.hitl_suspended.risk_tier),
          arguments: ev.hitl_suspended.arguments,
        };
        agentStatus = "waiting_hitl";
      } else if ("node_completed" in ev) {
        dagEvents = [...dagEvents, { node_completed: ev.node_completed }];
        if (pendingHitl?.nodeId === ev.node_completed.node_id) {
          pendingHitl = null;
          agentStatus = "streaming";
        }
      } else if ("node_failed" in ev) {
        agentError = `Step ${ev.node_failed.node_id} failed: ${ev.node_failed.error}`;
      } else if ("completed" in ev) {
        if (!agentResponse && ev.completed.final_text) {
          agentResponse = ev.completed.final_text;
        }
        agentStatus = "done";
      } else if ("error" in ev) {
        agentError = ev.error.message;
        agentStatus = "error";
      }
    };

    try {
      const result = await submitAgentPrompt(
        {
          prompt: agentPromptSubmitted,
          traceId,
          chips: effectiveChips,
          profile: agentProfile,
        },
        handleStreamEvent,
      );
      if (!agentResponse && result.final_text) {
        agentResponse = result.final_text;
      }
      if (agentStatus === "streaming") {
        agentStatus = "done";
      }
    } catch (err) {
      agentError = err instanceof Error ? err.message : String(err);
      agentStatus = "error";
    }
  }

  function removeChip(index: number) {
    chips = chips.filter((_, idx) => idx !== index);
  }

  async function refreshDshLogs() {
    dshLogs = await fetchDshLogs(120);
  }

  async function toggleDsh() {
    dshOpen = !dshOpen;
    if (dshOpen) {
      await refreshDshLogs();
    }
  }

  async function updateFault(key: keyof FaultInjectionConfig) {
    const previous = { ...faults };
    faults = { ...faults, [key]: !faults[key] };
    faultPending = true;
    faultError = null;
    try {
      faults = await setFaultInjection(faults);
    } catch (err) {
      faults = previous;
      faultError = err instanceof Error ? err.message : String(err);
    } finally {
      faultPending = false;
    }
  }

  async function syncZtoolsPlugins(dirPath?: string) {
    syncPending = true;
    faultError = null;
    try {
      lastSyncResult = await syncZtoolsDir(dirPath);
      await refreshHomeItems(36);
      if (dshOpen || dshStandalone) {
        await refreshDshLogs();
      }
    } catch (err) {
      faultError = err instanceof Error ? err.message : String(err);
    } finally {
      syncPending = false;
    }
  }

  async function approveHitl(resolution: "approve_once" | "approve_session" | "deny") {
    if (!pendingHitl) return;
    if (resolution === "approve_session" && isTier2Risk(pendingHitl.riskTier)) {
      return;
    }
    const nodeId = pendingHitl.nodeId;
    pendingHitl = null;
    agentStatus = resolution === "deny" ? "error" : "streaming";
    if (resolution === "deny") {
      agentError = "Action denied by user.";
    }
    try {
      await resolveHitlStep(nodeId, resolution);
    } catch (err) {
      agentError = err instanceof Error ? err.message : String(err);
      agentStatus = "error";
    }
  }

  async function mountBridge(): Promise<() => void> {
    const cleanups: Array<() => void> = [];

    if (typeof window !== "undefined") {
      const onHash = () => {
        dshStandalone = isDshRoute();
        if (dshStandalone) {
          void refreshDshLogs();
        }
      };
      window.addEventListener("hashchange", onHash);
      cleanups.push(() => window.removeEventListener("hashchange", onHash));

      const media = window.matchMedia?.("(prefers-color-scheme: dark)");
      if (media) {
        const onMedia = (ev: MediaQueryListEvent) => {
          osTheme = ev.matches ? "dark" : "light";
          if (themePreference === "system") {
            applyThemeAttribute(osTheme);
          }
        };
        media.addEventListener("change", onMedia);
        cleanups.push(() => media.removeEventListener("change", onMedia));
      }
    }

    if (dshStandalone) {
      await refreshDshLogs();
    } else {
      const [initialSuggestions, sections] = await Promise.all([
        fetchZeroQuerySuggestions(),
        launcherHomeSections(36),
      ]);
      suggestions = initialSuggestions;
      applyHomeSectionsResponse(sections);
    }

    if (!isTauriRuntime()) {
      return () => {
        for (const fn of cleanups) fn();
      };
    }

    const { Channel, invoke } = await import("@tauri-apps/api/core");
    const { listen } = await import("@tauri-apps/api/event");
    const { getCurrentWindow } = await import("@tauri-apps/api/window");

    try {
      const currentTheme = await getCurrentWindow().theme();
      if (currentTheme === "light" || currentTheme === "dark") {
        osTheme = currentTheme;
        if (themePreference === "system") {
          applyThemeAttribute(currentTheme);
        }
      }
      const unlistenWinTheme = await getCurrentWindow().onThemeChanged(({ payload }) => {
        if (payload === "light" || payload === "dark") {
          osTheme = payload;
          if (themePreference === "system") {
            applyThemeAttribute(payload);
          }
        }
      });
      cleanups.push(unlistenWinTheme);
    } catch {
      // Optional window theme listener unavailable.
    }

    type WakePayloadMsg = {
      traceId: string;
      chips: ContextChip[];
      suggestions: QuickAgentAction[];
    };

    const applyWakePayload = (payload: WakePayloadMsg) => {
      const isDuplicate = payload.traceId && payload.traceId === lastWakeTraceId && hasWoken;
      lastWakeTraceId = payload.traceId || lastWakeTraceId;
      traceId = payload.traceId;
      chips = payload.chips ?? [];
      suggestions = payload.suggestions ?? [];
      hasWoken = true;
      resetSelection();
      resetCollapseState();
      statusBanner = null;
      searchError = null;
      if (!isDuplicate) {
        void refreshHomeItems(36);
        void ingestFrontendLog("INFO", "renderer::paint", "Wake payload received", payload.traceId);
      }
    };

    if (!dshStandalone) {
      const ch = new Channel<WakePayloadMsg>();
      ch.onmessage = applyWakePayload;
      await invoke("register_wake_channel", { channel: ch });

      const unlistenWake = await listen<WakePayloadMsg>("aitools://wake", (event) => {
        if (event.payload) {
          applyWakePayload(event.payload);
        }
      });
      cleanups.push(unlistenWake);
    }

    const unlistenDsh = await listen("aitools://toggle-dsh", () => {
      if (!dshStandalone) {
        void toggleDsh();
      } else {
        void refreshDshLogs();
      }
    });
    cleanups.push(unlistenDsh);

    const unlistenList = await listen<ListPushEvent>("aitools://list-push", (event) => {
      applyListPush(event.payload);
    });
    cleanups.push(unlistenList);

    const unlistenTheme = await listen<{ theme?: ThemePreference }>(
      "aitools://theme-changed",
      (event) => {
        const next = event.payload?.theme;
        if (next === "system" || next === "light" || next === "dark") {
          setThemePreference(next);
        }
      },
    );
    cleanups.push(unlistenTheme);

    return () => {
      for (const fn of cleanups) fn();
    };
  }

  return {
    get dshStandalone() { return dshStandalone; },
    get mode() { return mode; },
    set mode(v: LauncherMode) { mode = v; },
    get searchViewMode() { return searchViewMode; },
    get query() { return query; },
    get traceId() { return traceId; },
    get chips() { return chips; },
    get pastedText() { return pastedText; },
    get pastedFiles() { return pastedFiles; },
    get suggestions() { return suggestions; },
    get recentItems() { return recentItems; },
    get pinnedItems() { return pinnedItems; },
    get featuredItems() { return featuredItems; },
    get homeItems() { return featuredItems; },
    get recentExpanded() { return recentExpanded; },
    get pinnedExpanded() { return pinnedExpanded; },
    get homeExpanded() { return homeExpanded; },
    get searchResultsExpanded() { return searchResultsExpanded; },
    get bestMatchesExpanded() { return bestMatchesExpanded; },
    get recommendationsExpanded() { return recommendationsExpanded; },
    get hasWoken() { return hasWoken; },
    get selectedRow() { return selectedRow; },
    get selectedCol() { return selectedCol; },
    get hits() { return hits; },
    get searchLoading() { return searchLoading; },
    get searchError() { return searchError; },
    get listSession() { return listSession; },
    get subInput() { return subInput; },
    get listSelectedIndex() { return listSelectedIndex; },
    set listSelectedIndex(v: number) { listSelectedIndex = v; },
    get listLoading() { return listLoading; },
    get listError() { return listError; },
    get statusBanner() { return statusBanner; },
    set statusBanner(v: { kind: "info" | "success" | "error"; text: string } | null) {
      statusBanner = v;
    },
    get agentProfile() { return agentProfile; },
    set agentProfile(v: AiProfile) { agentProfile = v; },
    get agentStatus() { return agentStatus; },
    get agentPromptSubmitted() { return agentPromptSubmitted; },
    get agentResponse() { return agentResponse; },
    get agentError() { return agentError; },
    get dagEvents() { return dagEvents; },
    get pendingHitl() { return pendingHitl; },
    get dshOpen() { return dshOpen; },
    get dshLogs() { return dshLogs; },
    get faults() { return faults; },
    get faultPending() { return faultPending; },
    get faultError() { return faultError; },
    get syncPending() { return syncPending; },
    get lastSyncResult() { return lastSyncResult; },
    get themePreference() { return themePreference; },
    get resolvedTheme() { return resolvedTheme; },
    setDraftQuery,
    setDraftSubInput,
    setThemePreference,
    cycleTheme,
    setSearchViewMode,
    toggleSearchViewMode,
    refreshHomeItems,
    resetSelection,
    resetCollapseState,
    toggleRecentExpanded,
    togglePinnedExpanded,
    toggleHomeExpanded,
    toggleSearchResultsExpanded,
    toggleBestMatchesExpanded,
    toggleRecommendationsExpanded,
    clampSelection,
    moveGridSelection,
    isPinnedHit,
    onQueryChange,
    setPastedText,
    setPastedFiles,
    clearPastedText,
    clearPastedFiles,
    convertQueryToFilterChip,
    editPastedTextChip,
    editContextChip,
    popLastChipToQuery,
    mutateState,
    reorderPinned,
    revealItem,
    promoteToAgent,
    exitAgentToSearch,
    exitListToSearch,
    moveListSelection,
    launchHit,
    onSubInputChange,
    selectListAt,
    handleFallbackSearchSubmit,
    handleEscape,
    submitAgentTurn,
    removeChip,
    refreshDshLogs,
    toggleDsh,
    updateFault,
    syncZtoolsPlugins,
    approveHitl,
    mountBridge,
  };
}
