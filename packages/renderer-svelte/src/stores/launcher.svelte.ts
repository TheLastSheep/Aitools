import {
  fetchDshLogs,
  fetchZeroQuerySuggestions,
  ingestFrontendLog,
  isTauriRuntime,
  resolveHitlStep,
  searchFeatures,
  setFaultInjection,
  type ContextChip,
  type DagEvent,
  type FaultInjectionConfig,
  type LauncherHit,
  type LauncherMode,
  type LogRecord,
  type QuickAgentAction,
} from "../lib/ipc";

export function createLauncherStore() {
  let mode = $state<LauncherMode>("search");
  let query = $state("");
  let traceId = $state("wake-0");
  let chips = $state<ContextChip[]>([]);
  let suggestions = $state<QuickAgentAction[]>([]);
  let hits = $state<LauncherHit[]>([]);
  let selectedIndex = $state(0);
  let dshOpen = $state(false);
  let dshLogs = $state<LogRecord[]>([]);
  let dagEvents = $state<DagEvent[]>([]);
  let pendingHitl = $state<{
    nodeId: string;
    toolName: string;
    riskTier: string;
    arguments: unknown;
  } | null>(null);
  let faults = $state<FaultInjectionConfig>({
    injectLmdbRevConflict: false,
    injectPermissionDenied: false,
    injectWasmFuelExhaustion: false,
    injectLlmStreamAbort: false,
  });

  async function onQueryChange(next: string) {
    query = next;
    if (next.startsWith("@") && mode !== "agent") {
      mode = "agent";
      return;
    }
    if (mode === "search") {
      const decision = await searchFeatures(next, false, false);
      if ("deterministic_results" in decision) {
        hits = decision.deterministic_results.matches;
        selectedIndex = 0;
      }
    }
  }

  async function promoteToAgent() {
    mode = "agent";
    await ingestFrontendLog("INFO", "renderer::launcher", "Promoted to Agent mode", traceId);
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
    faults = { ...faults, [key]: !faults[key] };
    faults = await setFaultInjection(faults);
  }

  async function approveHitl(resolution: "approve_once" | "approve_session" | "deny") {
    if (!pendingHitl) return;
    const nodeId = pendingHitl.nodeId;
    pendingHitl = null;
    await resolveHitlStep(nodeId, resolution);
  }

  async function mountBridge() {
    suggestions = await fetchZeroQuerySuggestions();
    if (!isTauriRuntime()) return;

    const { Channel, invoke } = await import("@tauri-apps/api/core");
    const { listen } = await import("@tauri-apps/api/event");

    const ch = new Channel<{
      traceId: string;
      chips: ContextChip[];
      suggestions: QuickAgentAction[];
    }>();
    ch.onmessage = (payload) => {
      traceId = payload.traceId;
      chips = payload.chips;
      suggestions = payload.suggestions;
      void ingestFrontendLog("INFO", "renderer::paint", "Wake payload received", payload.traceId);
    };
    await invoke("register_wake_channel", { channel: ch });
    await listen("aitools://toggle-dsh", () => {
      void toggleDsh();
    });
  }

  return {
    get mode() { return mode; },
    set mode(v: LauncherMode) { mode = v; },
    get query() { return query; },
    get traceId() { return traceId; },
    get chips() { return chips; },
    get suggestions() { return suggestions; },
    get hits() { return hits; },
    get selectedIndex() { return selectedIndex; },
    set selectedIndex(v: number) { selectedIndex = v; },
    get dshOpen() { return dshOpen; },
    get dshLogs() { return dshLogs; },
    get dagEvents() { return dagEvents; },
    get pendingHitl() { return pendingHitl; },
    get faults() { return faults; },
    onQueryChange,
    promoteToAgent,
    removeChip,
    refreshDshLogs,
    toggleDsh,
    updateFault,
    approveHitl,
    mountBridge,
  };
}
