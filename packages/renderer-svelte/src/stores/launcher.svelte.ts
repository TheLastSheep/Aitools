import {
  fetchDshLogs,
  fetchZeroQuerySuggestions,
  hideMainWindow,
  ingestFrontendLog,
  isDshRoute,
  isTauriRuntime,
  launchItem,
  resolveHitlStep,
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
  type LauncherHit,
  type LauncherMode,
  type ListItem,
  type ListPushEvent,
  type ListSession,
  type LogRecord,
  type QuickAgentAction,
  type ResolvedTheme,
  type SyncZtoolsResult,
  type ThemePreference,
} from "../lib/ipc";

const THEME_STORAGE_KEY = "aitools:theme";

function readInitialThemePref(): ThemePreference {
  if (typeof localStorage === "undefined") return "system";
  const saved = localStorage.getItem(THEME_STORAGE_KEY);
  return saved === "light" || saved === "dark" || saved === "system" ? saved : "system";
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

export function createLauncherStore() {
  let dshStandalone = $state(isDshRoute());
  let mode = $state<LauncherMode>("search");
  let enteredAgentByAt = $state(false);
  let query = $state("");
  let traceId = $state("wake-0");
  let chips = $state<ContextChip[]>([]);
  let suggestions = $state<QuickAgentAction[]>([]);
  let hits = $state<LauncherHit[]>([]);
  let selectedIndex = $state(0);
  let searchLoading = $state(false);
  let searchError = $state<string | null>(null);

  // List-mode state (headless ZTools plugins)
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

  function setThemePreference(next: ThemePreference) {
    themePreference = next;
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(THEME_STORAGE_KEY, next);
    }
    applyThemeAttribute(next === "system" ? osTheme : next);
  }

  function cycleTheme() {
    const next: ThemePreference =
      resolvedTheme === "dark" ? "light" : "dark";
    setThemePreference(next);
  }

  async function onQueryChange(next: string) {
    query = next;
    searchError = null;
    statusBanner = null;

    if (next.startsWith("@") && mode === "search") {
      mode = "agent";
      enteredAgentByAt = true;
      return;
    }

    if (mode === "agent" && enteredAgentByAt && !next.startsWith("@")) {
      mode = "search";
      enteredAgentByAt = false;
    }

    if (mode === "search") {
      if (!next.trim()) {
        hits = [];
        selectedIndex = 0;
        return;
      }
      searchLoading = true;
      try {
        const decision = await searchFeatures(next, false, false);
        if ("deterministic_results" in decision) {
          hits = decision.deterministic_results.matches;
          selectedIndex = 0;
        }
      } catch (err) {
        searchError = err instanceof Error ? err.message : String(err);
      } finally {
        searchLoading = false;
      }
    }
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

  function moveSelection(delta: number) {
    if (mode === "search" && hits.length > 0) {
      selectedIndex = (selectedIndex + delta + hits.length) % hits.length;
    } else if (mode === "list" && listSession && listSession.items.length > 0) {
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
      const result = await launchItem(
        {
          featureCode: hit.feature_code,
          pluginId: hit.plugin_id,
          query,
          kind: hit.kind,
          traceId,
        },
        applyListPush,
      );

      switch (result.kind) {
        case "list_mode":
          mode = "list";
          listSession = {
            session_id: result.session_id,
            plugin_id: result.plugin_id,
            plugin_name: result.plugin_name,
            feature_code: result.feature_code,
            placeholder: result.placeholder,
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

  async function handleSearchEnter() {
    if (mode === "search") {
      if (hits.length > 0) {
        const idx = Math.min(selectedIndex, hits.length - 1);
        await launchHit(hits[idx]);
        return;
      }
      if (query.trim().length > 0) {
        try {
          const decision = await searchFeatures(query, false, true);
          if ("deterministic_results" in decision && decision.deterministic_results.matches.length > 0) {
            hits = decision.deterministic_results.matches;
            selectedIndex = 0;
            await launchHit(hits[0]);
            return;
          }
          if ("promote_to_agent" in decision) {
            await promoteToAgent(decision.promote_to_agent.query || query);
            await submitAgentTurn();
            return;
          }
        } catch (err) {
          searchError = err instanceof Error ? err.message : String(err);
        }
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
    await hideMainWindow();
  }

  async function submitAgentTurn(overridePrompt?: string) {
    const promptText = (overridePrompt ?? query).trim();
    if (!promptText && chips.length === 0) {
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
          chips,
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
      suggestions = await fetchZeroQuerySuggestions();
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

    if (!dshStandalone) {
      const ch = new Channel<{
        traceId: string;
        chips: ContextChip[];
        suggestions: QuickAgentAction[];
      }>();
      ch.onmessage = (payload) => {
        traceId = payload.traceId;
        chips = payload.chips;
        suggestions = payload.suggestions;
        statusBanner = null;
        searchError = null;
        void ingestFrontendLog("INFO", "renderer::paint", "Wake payload received", payload.traceId);
      };
      await invoke("register_wake_channel", { channel: ch });
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
    get query() { return query; },
    get traceId() { return traceId; },
    get chips() { return chips; },
    get suggestions() { return suggestions; },
    get hits() { return hits; },
    get selectedIndex() { return selectedIndex; },
    set selectedIndex(v: number) { selectedIndex = v; },
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
    setThemePreference,
    cycleTheme,
    onQueryChange,
    promoteToAgent,
    exitAgentToSearch,
    exitListToSearch,
    moveSelection,
    launchHit,
    onSubInputChange,
    selectListAt,
    handleSearchEnter,
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
