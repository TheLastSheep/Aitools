<script lang="ts">
  import type { FaultInjectionConfig, LogRecord } from "../lib/ipc";

  let {
    logs,
    faults,
    activeTraceId,
    onRefresh,
    onToggleFault,
    onClose,
  }: {
    logs: LogRecord[];
    faults: FaultInjectionConfig;
    activeTraceId: string;
    onRefresh: () => void;
    onToggleFault: (key: keyof FaultInjectionConfig) => void;
    onClose: () => void;
  } = $props();

  let traceFilter = $state("");

  const filteredLogs = $derived(
    traceFilter.trim()
      ? logs.filter((r) => (r.trace_id ?? "").includes(traceFilter.trim()))
      : logs,
  );
</script>

<section class="dsh-panel" aria-label="Developer & Diagnostic Harness">
  <header class="dsh-header">
    <div>
      <strong>DSH Diagnostics</strong>
      <span class="mono muted">trace={activeTraceId}</span>
    </div>
    <div class="dsh-actions">
      <input
        type="text"
        class="dsh-filter mono"
        placeholder="Filter trace_id..."
        bind:value={traceFilter}
      />
      <button type="button" class="dsh-btn" onclick={onRefresh}>Refresh</button>
      <button type="button" class="dsh-btn" onclick={onClose}>Esc</button>
    </div>
  </header>

  <div class="fault-bar">
    <span class="muted">Fault Injection (SPEC-013):</span>
    <label>
      <input
        type="checkbox"
        checked={faults.injectLmdbRevConflict}
        onchange={() => onToggleFault("injectLmdbRevConflict")}
      />
      LMDB `_rev` conflict
    </label>
    <label>
      <input
        type="checkbox"
        checked={faults.injectPermissionDenied}
        onchange={() => onToggleFault("injectPermissionDenied")}
      />
      Permission denied
    </label>
    <label>
      <input
        type="checkbox"
        checked={faults.injectWasmFuelExhaustion}
        onchange={() => onToggleFault("injectWasmFuelExhaustion")}
      />
      Wasm fuel cap
    </label>
    <label>
      <input
        type="checkbox"
        checked={faults.injectLlmStreamAbort}
        onchange={() => onToggleFault("injectLlmStreamAbort")}
      />
      LLM abort
    </label>
  </div>

  <div class="waterfall mono">
    {#if filteredLogs.length === 0}
      <div class="empty">No chronological records captured in ring buffer yet.</div>
    {:else}
      {#each filteredLogs as rec (rec.seq)}
        <div class="row" class:anomaly={rec.perf_anomaly}>
          <span class="seq">#{String(rec.seq).padStart(6, "0")}</span>
          <span class="lvl">{rec.level}</span>
          <span class="trace">{rec.trace_id ?? "-"}</span>
          <span class="src">{rec.source}</span>
          {#if rec.duration_us !== undefined}
            <span class="dur">{rec.duration_us}us</span>
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
    padding: 10px 14px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-height: 260px;
  }
  .dsh-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
  }
  .dsh-actions {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .dsh-filter {
    background: var(--bg-elevated);
    border: 1px solid var(--border-strong);
    border-radius: 6px;
    padding: 3px 8px;
    color: var(--text-primary);
    font-size: 12px;
  }
  .dsh-btn {
    padding: 3px 8px;
    border-radius: 6px;
    border: 1px solid var(--border-strong);
    background: var(--bg-secondary);
    font-size: 12px;
  }
  .fault-bar {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    font-size: 12px;
    align-items: center;
  }
  .fault-bar label {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .waterfall {
    overflow-y: auto;
    font-size: 11px;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .row {
    display: flex;
    gap: 8px;
    padding: 2px 4px;
    border-radius: 4px;
  }
  .row.anomaly {
    background: rgba(217, 45, 32, 0.14);
    color: var(--danger);
  }
  .muted { color: var(--text-secondary); margin-left: 6px; font-size: 12px; }
  .seq { color: var(--text-tertiary); }
  .lvl { font-weight: 600; min-width: 44px; }
  .trace { color: var(--accent); }
  .dur { color: var(--warning); }
  .empty { color: var(--text-tertiary); padding: 8px 0; }
</style>
