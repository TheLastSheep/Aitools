<script lang="ts">
  import type { FaultInjectionConfig, LogRecord, SyncZtoolsResult } from "../lib/ipc";

  let {
    logs,
    faults,
    activeTraceId,
    standalone = false,
    faultPending = false,
    faultError = null,
    syncPending = false,
    lastSyncResult = null,
    onRefresh,
    onToggleFault,
    onSyncPlugins,
    onClose,
  }: {
    logs: LogRecord[];
    faults: FaultInjectionConfig;
    activeTraceId: string;
    standalone?: boolean;
    faultPending?: boolean;
    faultError?: string | null;
    syncPending?: boolean;
    lastSyncResult?: SyncZtoolsResult | null;
    onRefresh: () => void;
    onToggleFault: (key: keyof FaultInjectionConfig) => void;
    onSyncPlugins?: () => void;
    onClose?: () => void;
  } = $props();

  let traceFilter = $state("");

  const filteredLogs = $derived(
    traceFilter.trim()
      ? logs.filter((r) => (r.trace_id ?? "").includes(traceFilter.trim()))
      : logs,
  );

  function levelClass(level: string): string {
    const upper = level.toUpperCase();
    if (upper === "ERROR" || upper === "ERR") return "lvl-error";
    if (upper === "WARN" || upper === "WARNING") return "lvl-warn";
    if (upper === "DEBUG" || upper === "TRACE") return "lvl-debug";
    return "lvl-info";
  }
</script>

<section
  id="dsh-drawer"
  class="dsh-panel"
  class:standalone
  aria-label="Developer & Diagnostic Harness"
>
  <header class="dsh-header">
    <div class="dsh-title-group">
      <strong>Developer Diagnostics</strong>
      <span class="mono muted tabular">trace={activeTraceId}</span>
    </div>
    <div class="dsh-actions">
      <input
        type="text"
        class="dsh-filter mono"
        aria-label="Filter logs by trace ID"
        placeholder="Filter trace_id..."
        bind:value={traceFilter}
      />
      {#if onSyncPlugins}
        <button
          type="button"
          class="dsh-btn"
          disabled={syncPending}
          onclick={onSyncPlugins}
        >
          {syncPending ? "Syncing..." : "Sync Plugins"}
        </button>
      {/if}
      <button type="button" class="dsh-btn" onclick={onRefresh}>Refresh</button>
      {#if onClose && !standalone}
        <button type="button" class="dsh-btn" onclick={onClose}>Close</button>
      {/if}
    </div>
  </header>

  <div class="fault-bar">
    <span class="fault-heading">
      Fault Injection
      {#if faultPending}
        <span class="pending-badge">Updating...</span>
      {/if}
    </span>
    <label class:disabled={faultPending}>
      <input
        type="checkbox"
        disabled={faultPending}
        checked={faults.injectLmdbRevConflict}
        onchange={() => onToggleFault("injectLmdbRevConflict")}
      />
      Revision conflict
    </label>
    <label class:disabled={faultPending}>
      <input
        type="checkbox"
        disabled={faultPending}
        checked={faults.injectPermissionDenied}
        onchange={() => onToggleFault("injectPermissionDenied")}
      />
      Permission denied
    </label>
    <label class:disabled={faultPending}>
      <input
        type="checkbox"
        disabled={faultPending}
        checked={faults.injectWasmFuelExhaustion}
        onchange={() => onToggleFault("injectWasmFuelExhaustion")}
      />
      Wasm fuel cap
    </label>
    <label class:disabled={faultPending}>
      <input
        type="checkbox"
        disabled={faultPending}
        checked={faults.injectLlmStreamAbort}
        onchange={() => onToggleFault("injectLlmStreamAbort")}
      />
      LLM abort
    </label>
  </div>

  {#if faultError}
    <div class="dsh-alert error" role="alert">{faultError}</div>
  {/if}

  {#if lastSyncResult}
    <div class="dsh-alert info">
      Synced {lastSyncResult.indexed_plugins} plugin(s), {lastSyncResult.indexed_features} feature(s) from
      <span class="mono">{lastSyncResult.dir}</span>
    </div>
  {/if}

  <div class="waterfall mono">
    {#if filteredLogs.length === 0}
      <div class="empty">No chronological records captured in ring buffer yet.</div>
    {:else}
      {#each filteredLogs as rec (rec.seq)}
        <div class="row" class:anomaly={rec.perf_anomaly}>
          <span class="seq tabular">#{String(rec.seq).padStart(6, "0")}</span>
          <span class={`lvl ${levelClass(rec.level)}`}>{rec.level.toUpperCase()}</span>
          {#if rec.perf_anomaly}
            <span class="anomaly-badge" title="Latency budget exceeded">
              <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true">
                <path
                  fill="currentColor"
                  d="M8.98 1.56a1.1 1.1 0 0 0-1.96 0L1.14 12.82A1.1 1.1 0 0 0 2.12 14.5h11.76a1.1 1.1 0 0 0 .98-1.68L8.98 1.56ZM8.75 6v3.75a.75.75 0 0 1-1.5 0V6a.75.75 0 0 1 1.5 0ZM8 12.75a.875.875 0 1 1 0-1.75.875.875 0 0 1 0 1.75Z"
                />
              </svg>
              SLOW
            </span>
          {/if}
          <span class="trace">{rec.trace_id ?? "-"}</span>
          <span class="src">{rec.source}</span>
          {#if rec.duration_us !== undefined}
            <span class="dur tabular">{rec.duration_us}us</span>
          {/if}
          <span class="msg">{rec.msg}</span>
        </div>
      {/each}
    {/if}
  </div>
</section>

<style>
  .dsh-panel {
    border-top: 1px solid var(--border);
    background: var(--bg-tertiary);
    padding: var(--space-3) var(--space-4);
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    max-height: 240px;
    min-height: 0;
    min-width: 0;
  }

  .dsh-panel.standalone {
    border-top: none;
    height: 100vh;
    max-height: 100vh;
    background: var(--bg-primary);
    padding: var(--space-4);
    gap: var(--space-3);
  }

  .dsh-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: var(--space-2);
    flex-wrap: wrap;
  }

  .dsh-title-group {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
    min-width: 0;
  }

  .dsh-actions {
    display: flex;
    gap: var(--space-2);
    align-items: center;
    flex-wrap: wrap;
  }

  .dsh-filter {
    background: var(--bg-elevated);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    padding: var(--space-1) var(--space-2);
    color: var(--text-primary);
    font-size: var(--text-xs);
    min-height: 28px;
  }

  .dsh-filter::placeholder {
    color: var(--text-tertiary);
    opacity: 1;
  }

  .dsh-btn {
    padding: var(--space-1) var(--space-3);
    border-radius: var(--radius-sm);
    border: 1px solid var(--border-strong);
    background: var(--bg-secondary);
    color: var(--text-primary);
    font-size: var(--text-xs);
    font-weight: 500;
    min-height: 28px;
  }

  .dsh-btn:hover:not(:disabled) {
    background: var(--bg-hover);
  }

  .dsh-btn:active:not(:disabled) {
    transform: scale(0.98);
  }

  .fault-bar {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-3);
    font-size: var(--text-xs);
    align-items: center;
    color: var(--text-secondary);
  }

  .fault-heading {
    font-weight: 600;
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
  }

  .pending-badge {
    padding: 2px var(--space-2);
    border-radius: var(--radius-pill);
    background: var(--accent-subtle);
    color: var(--accent);
    font-weight: 600;
  }

  .fault-bar label {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    color: var(--text-primary);
    cursor: pointer;
  }

  .fault-bar label.disabled {
    opacity: 0.6;
    cursor: wait;
  }

  .dsh-alert {
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-sm);
    font-size: var(--text-xs);
  }

  .dsh-alert.error {
    background: var(--danger-subtle);
    color: var(--danger);
    border: 1px solid var(--danger);
  }

  .dsh-alert.info {
    background: var(--accent-subtle);
    color: var(--accent);
  }

  .waterfall {
    flex: 1;
    min-height: 0;
    min-width: 0;
    overflow-y: auto;
    overflow-x: auto;
    font-size: var(--text-xs);
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }

  .row {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-1) var(--space-2);
    border-radius: var(--radius-sm);
    min-width: max-content;
  }

  .row.anomaly {
    background: var(--danger-subtle);
    color: var(--danger);
  }

  .muted {
    color: var(--text-secondary);
    font-size: var(--text-xs);
  }

  .seq {
    color: var(--text-tertiary);
  }

  .lvl {
    font-weight: 600;
    padding: 1px var(--space-2);
    border-radius: var(--radius-sm);
    text-align: center;
    min-width: 52px;
  }

  .lvl-info {
    background: var(--accent-subtle);
    color: var(--accent);
  }

  .lvl-debug {
    background: var(--bg-secondary);
    color: var(--text-secondary);
  }

  .lvl-warn {
    background: var(--warning-subtle);
    color: var(--warning);
  }

  .lvl-error {
    background: var(--danger-subtle);
    color: var(--danger);
  }

  .anomaly-badge {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    padding: 1px var(--space-2);
    border-radius: var(--radius-sm);
    background: var(--danger-solid);
    color: var(--danger-foreground);
    font-weight: 700;
  }

  .trace {
    color: var(--accent);
  }

  .src {
    color: var(--text-secondary);
  }

  .dur {
    color: var(--warning);
    font-weight: 600;
  }

  .msg {
    color: var(--text-primary);
    min-width: 0;
  }

  .empty {
    color: var(--text-tertiary);
    padding: var(--space-2) 0;
  }
</style>
