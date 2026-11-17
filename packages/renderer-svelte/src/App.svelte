<script lang="ts">
  import { onMount } from "svelte";
  import AToolsMark from "./components/AToolsMark.svelte";
  import DshPanel from "./components/DshPanel.svelte";
  import { createLauncherStore } from "./stores/launcher.svelte";

  const store = createLauncherStore();
  let searchInput: HTMLInputElement | undefined = $state();

  onMount(() => {
    searchInput?.focus();
    void store.mountBridge();
  });

  function handleKeydown(e: KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === "d") {
      e.preventDefault();
      void store.toggleDsh();
      return;
    }
    if (e.key === "Tab" && store.mode === "search") {
      e.preventDefault();
      void store.promoteToAgent();
      return;
    }
    if (e.key === "Escape") {
      if (store.dshOpen) {
        void store.toggleDsh();
      } else if (store.mode === "agent") {
        store.mode = "search";
      }
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<main class="shell">
  <div class="search-bar">
    <AToolsMark size="small" label="aitools" />
    <span class="mode-badge" class:agent={store.mode === "agent"}>
      {store.mode === "agent" ? "Agent" : "Search"}
    </span>
    <input
      bind:this={searchInput}
      class="search-input"
      type="text"
      value={store.query}
      oninput={(e) => void store.onQueryChange((e.currentTarget as HTMLInputElement).value)}
      placeholder={store.mode === "agent"
        ? "Ask AI Agent across plugins & OS tools..."
        : "Search apps, Pinyin, commands — press Tab or @ for Agent..."}
    />
    <button type="button" class="dsh-toggle mono" onclick={() => void store.toggleDsh()}>
      DSH
    </button>
  </div>

  {#if store.chips.length > 0}
    <div class="chips-row">
      {#each store.chips as chip, idx (chip.kind + idx)}
        <span class="chip">
          <span class="chip-kind">{chip.label}:</span>
          <span class="chip-val">{chip.value.slice(0, 42)}</span>
          <button type="button" aria-label="Remove context chip" onclick={() => store.removeChip(idx)}>
            ×
          </button>
        </span>
      {/each}
    </div>
  {/if}

  <div class="body">
    {#if store.mode === "search"}
      {#if store.hits.length > 0}
        <ul class="results">
          {#each store.hits as hit, idx (hit.plugin_id + hit.feature_code)}
            <li class:selected={idx === store.selectedIndex}>
              <span class="title">{hit.feature_code}</span>
              <span class="meta mono">{hit.plugin_id} · {hit.match_type} ({hit.score})</span>
            </li>
          {/each}
        </ul>
      {:else if store.suggestions.length > 0}
        <div class="suggestions">
          <div class="section-title">Zero-Query Suggestions (SPEC-011)</div>
          {#each store.suggestions as act (act.id)}
            <button
              type="button"
              class="suggestion-item"
              onclick={() => {
                void store.promoteToAgent();
                void store.onQueryChange(act.prompt_template);
              }}
            >
              <span>{act.title}</span>
              <span class="meta mono">{act.signature}</span>
            </button>
          {/each}
        </div>
      {:else}
        <div class="hint">
          Type to search synchronously (&lt; 5 ms) or press <kbd>Tab</kbd> / <kbd>@</kbd> for Agent mode.
        </div>
      {/if}
    {:else}
      <div class="agent-pane">
        {#if store.pendingHitl}
          <div class="hitl-card">
            <strong>HITL Approval Required ({store.pendingHitl.riskTier})</strong>
            <div class="mono">{store.pendingHitl.toolName}</div>
            <pre class="mono">{JSON.stringify(store.pendingHitl.arguments, null, 2)}</pre>
            <div class="hitl-actions">
              <button type="button" onclick={() => void store.approveHitl("approve_once")}>Approve Once</button>
              <button type="button" onclick={() => void store.approveHitl("approve_session")}>Approve Session</button>
              <button type="button" class="danger" onclick={() => void store.approveHitl("deny")}>Deny</button>
            </div>
          </div>
        {:else}
          <div class="hint">
            Agent Session active (<span class="mono">{store.traceId}</span>). Context chips above are attached to this turn.
          </div>
        {/if}
      </div>
    {/if}
  </div>

  {#if store.dshOpen}
    <DshPanel
      logs={store.dshLogs}
      faults={store.faults}
      activeTraceId={store.traceId}
      onRefresh={() => void store.refreshDshLogs()}
      onToggleFault={(k) => void store.updateFault(k)}
      onClose={() => void store.toggleDsh()}
    />
  {/if}
</main>

<style>
  .shell {
    background: var(--bg-primary);
    border: 1px solid var(--border-strong);
    border-radius: 14px;
    overflow: hidden;
    display: flex;
    flex-direction: column;
    min-height: 100vh;
  }
  .search-bar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 14px;
    border-bottom: 1px solid var(--border);
  }
  .mode-badge {
    font-size: 11px;
    font-weight: 600;
    padding: 2px 8px;
    border-radius: 999px;
    background: var(--bg-tertiary);
    color: var(--text-secondary);
  }
  .mode-badge.agent {
    background: var(--accent-subtle);
    color: var(--accent);
  }
  .search-input {
    flex: 1;
    border: none;
    outline: none;
    background: transparent;
    color: var(--text-primary);
    font-size: 15px;
  }
  .dsh-toggle {
    padding: 3px 8px;
    border-radius: 6px;
    border: 1px solid var(--border-strong);
    font-size: 11px;
    color: var(--text-secondary);
  }
  .chips-row {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 8px 14px;
    border-bottom: 1px solid var(--border);
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    background: var(--accent-subtle);
    color: var(--text-primary);
    border-radius: 6px;
    padding: 2px 8px;
    font-size: 12px;
  }
  .chip-kind { color: var(--accent); font-weight: 600; }
  .body {
    flex: 1;
    padding: 10px 14px;
  }
  .results {
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .results li, .suggestion-item {
    display: flex;
    justify-content: space-between;
    align-items: center;
    width: 100%;
    padding: 8px 10px;
    border-radius: 8px;
    text-align: left;
  }
  .results li.selected, .suggestion-item:hover {
    background: var(--bg-hover);
  }
  .section-title {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-tertiary);
    margin-bottom: 6px;
  }
  .meta { font-size: 11px; color: var(--text-secondary); }
  .hint { color: var(--text-secondary); font-size: 13px; padding: 12px 4px; }
  kbd {
    background: var(--bg-tertiary);
    border: 1px solid var(--border-strong);
    border-radius: 4px;
    padding: 1px 5px;
    font-size: 11px;
  }
  .hitl-card {
    border: 1px solid var(--warning);
    border-radius: 10px;
    padding: 10px;
    background: var(--bg-secondary);
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .hitl-actions { display: flex; gap: 8px; }
  .hitl-actions button {
    padding: 4px 10px;
    border-radius: 6px;
    background: var(--accent);
    color: var(--accent-foreground);
    font-size: 12px;
  }
  .hitl-actions button.danger { background: var(--danger); }
</style>
