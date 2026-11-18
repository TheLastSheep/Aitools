<script lang="ts">
  import { onMount, tick } from "svelte";
  import AToolsMark from "./components/AToolsMark.svelte";
  import DshPanel from "./components/DshPanel.svelte";
  import { setMainWindowHeight, type LauncherHit } from "./lib/ipc";
  import { createLauncherStore, isTier2Risk } from "./stores/launcher.svelte";

  const store = createLauncherStore();

  let shellEl: HTMLElement | undefined = $state();
  let searchInput: HTMLInputElement | undefined = $state();
  let subInputEl: HTMLInputElement | undefined = $state();
  let isComposing = $state(false);
  let brokenIcons = $state<Record<string, boolean>>({});
  let lastReportedHeight = 0;

  const isIdleCompact = $derived(
    !store.dshStandalone &&
      store.mode === "search" &&
      !store.query.trim() &&
      store.hits.length === 0 &&
      store.suggestions.length === 0 &&
      store.chips.length === 0 &&
      !store.dshOpen &&
      !store.statusBanner &&
      !store.searchError &&
      !store.searchLoading,
  );

  function syncWindowSize() {
    if (store.dshStandalone) return;
    if (isIdleCompact) {
      if (lastReportedHeight !== 62) {
        lastReportedHeight = 62;
        void setMainWindowHeight(62);
      }
      return;
    }
    if (!shellEl) return;
    const measured = Math.max(62, Math.min(560, Math.ceil(shellEl.scrollHeight)));
    if (measured !== lastReportedHeight) {
      lastReportedHeight = measured;
      void setMainWindowHeight(measured);
    }
  }

  onMount(() => {
    let cleanupBridge: (() => void) | undefined;
    void store.mountBridge().then((fn) => {
      cleanupBridge = fn;
    });

    if (!store.dshStandalone) {
      searchInput?.focus();
    }

    let observer: ResizeObserver | undefined;
    if (typeof ResizeObserver !== "undefined" && shellEl) {
      observer = new ResizeObserver(() => {
        syncWindowSize();
      });
      observer.observe(shellEl);
    }

    syncWindowSize();

    return () => {
      observer?.disconnect();
      cleanupBridge?.();
    };
  });

  $effect(() => {
    // Re-measure when layout-affecting state transitions occur.
    void isIdleCompact;
    void store.mode;
    void store.hits.length;
    void store.suggestions.length;
    void store.chips.length;
    void store.dshOpen;
    void store.statusBanner;
    void store.agentResponse;
    void store.dagEvents.length;
    void store.pendingHitl;
    void store.listSession?.items.length;

    void tick().then(() => {
      syncWindowSize();
      if (store.mode === "list") {
        subInputEl?.focus();
      }
    });
  });

  $effect(() => {
    const idx = store.mode === "list" ? store.listSelectedIndex : store.selectedIndex;
    const prefix = store.mode === "list" ? "list-option-" : "hit-option-";
    if (typeof document !== "undefined") {
      document.getElementById(`${prefix}${idx}`)?.scrollIntoView({ block: "nearest" });
    }
  });

  function eventIsComposing(e: Event): boolean {
    return "isComposing" in e && Boolean((e as { isComposing?: boolean }).isComposing);
  }

  function handleKeydown(e: KeyboardEvent) {
    if (store.dshStandalone) return;
    if (isComposing || e.isComposing || e.keyCode === 229) return;

    if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === "d") {
      e.preventDefault();
      void store.toggleDsh();
      return;
    }

    if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.key >= "1" && e.key <= "9") {
      const slot = Number.parseInt(e.key, 10) - 1;
      if (store.mode === "search" && slot < store.hits.length) {
        e.preventDefault();
        void store.launchHit(store.hits[slot]);
        return;
      }
      if (store.mode === "list" && store.listSession && slot < store.listSession.items.length) {
        e.preventDefault();
        void store.selectListAt(slot);
        return;
      }
    }

    if (e.key === "Tab" && store.mode === "search" && document.activeElement === searchInput) {
      e.preventDefault();
      void store.promoteToAgent();
      return;
    }

    if (e.key === "ArrowDown") {
      if (
        (store.mode === "search" && store.hits.length > 0) ||
        (store.mode === "list" && (store.listSession?.items.length ?? 0) > 0)
      ) {
        e.preventDefault();
        store.moveSelection(1);
        return;
      }
    }

    if (e.key === "ArrowUp") {
      if (
        (store.mode === "search" && store.hits.length > 0) ||
        (store.mode === "list" && (store.listSession?.items.length ?? 0) > 0)
      ) {
        e.preventDefault();
        store.moveSelection(-1);
        return;
      }
    }

    if (e.key === "Enter") {
      const active = document.activeElement;
      const inLauncherInput =
        active === searchInput || active === subInputEl || active === document.body;
      if (inLauncherInput && !e.shiftKey) {
        e.preventDefault();
        void store.handleSearchEnter();
        return;
      }
    }

    if (e.key === "Escape") {
      e.preventDefault();
      void store.handleEscape().then(() => {
        if (store.mode === "search") {
          searchInput?.focus();
        }
      });
    }
  }

  function validIconSrc(icon: string | null | undefined, key: string): string | null {
    if (!icon || brokenIcons[key]) return null;
    if (
      icon.startsWith("data:") ||
      icon.startsWith("aitools-plugin:") ||
      icon.startsWith("https://") ||
      icon.startsWith("http://") ||
      icon.startsWith("/")
    ) {
      return icon;
    }
    return null;
  }

  function markIconBroken(key: string) {
    brokenIcons = { ...brokenIcons, [key]: true };
  }

  function hitKindBadge(hit: LauncherHit): string {
    switch (hit.kind) {
      case "app":
        return "App";
      case "list":
        return "List";
      case "wasm":
        return "Wasm";
      case "macro":
        return "Macro";
      default:
        return "Plugin";
    }
  }

  function riskTierLabel(tier: string): string {
    if (isTier2Risk(tier)) return "Tier 2 · Destructive / External";
    if (tier.toLowerCase().includes("tier1")) return "Tier 1 · State Mutating";
    return "Tier 0 · Read-Only";
  }

  function dagNodeSummary(ev: (typeof store.dagEvents)[number]): {
    status: "started" | "hitl" | "done";
    title: string;
    detail: string;
  } {
    if ("node_started" in ev) {
      return {
        status: "started",
        title: ev.node_started.tool_name,
        detail: `Running ${ev.node_started.node_id}`,
      };
    }
    if ("hitl_suspended" in ev) {
      return {
        status: "hitl",
        title: ev.hitl_suspended.tool_name,
        detail: `Awaiting confirmation (${ev.hitl_suspended.node_id})`,
      };
    }
    return {
      status: "done",
      title: ev.node_completed.node_id,
      detail: "Completed",
    };
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if store.dshStandalone}
  <DshPanel
    logs={store.dshLogs}
    faults={store.faults}
    activeTraceId={store.traceId}
    standalone={true}
    faultPending={store.faultPending}
    faultError={store.faultError}
    syncPending={store.syncPending}
    lastSyncResult={store.lastSyncResult}
    onRefresh={() => void store.refreshDshLogs()}
    onToggleFault={(k) => void store.updateFault(k)}
    onSyncPlugins={() => void store.syncZtoolsPlugins()}
  />
{:else}
  <main bind:this={shellEl} class="shell" class:compact={isIdleCompact}>
    <div class="search-bar" data-tauri-drag-region>
      <span class="brand-wrap" data-tauri-drag-region>
        <AToolsMark size="small" label="aitools" />
      </span>

      <button
        type="button"
        class="mode-badge"
        class:agent={store.mode === "agent"}
        class:list={store.mode === "list"}
        title={store.mode === "agent"
          ? "Switch to Search mode"
          : store.mode === "list"
            ? "Exit List mode"
            : "Switch to Agent mode"}
        onclick={() => {
          if (store.mode === "agent") {
            store.exitAgentToSearch();
          } else if (store.mode === "list") {
            store.exitListToSearch();
          } else {
            void store.promoteToAgent();
          }
          searchInput?.focus();
        }}
      >
        {store.mode === "agent" ? "Agent" : store.mode === "list" ? "List" : "Search"}
      </button>

      <input
        bind:this={searchInput}
        class="search-input"
        type="text"
        role="combobox"
        aria-label={store.mode === "agent"
          ? "Ask AI Agent"
          : "Search apps, Pinyin, commands, or press Tab for Agent"}
        aria-autocomplete="list"
        aria-expanded={store.mode === "search" && store.hits.length > 0}
        aria-controls="launcher-results-listbox"
        aria-activedescendant={store.mode === "search" && store.hits.length > 0
          ? `hit-option-${store.selectedIndex}`
          : undefined}
        value={store.query}
        oncompositionstart={() => {
          isComposing = true;
        }}
        oncompositionend={(e) => {
          isComposing = false;
          void store.onQueryChange((e.currentTarget as HTMLInputElement).value);
        }}
        oninput={(e) => {
          const val = (e.currentTarget as HTMLInputElement).value;
          if (isComposing || eventIsComposing(e)) {
            store.setDraftQuery(val);
            return;
          }
          void store.onQueryChange(val);
        }}
        placeholder={store.mode === "agent"
          ? "Ask AI Agent across plugins & OS tools..."
          : "Search apps, Pinyin, commands — Tab or @ for Agent..."}
      />

      <div class="bar-actions">
        {#if store.mode === "agent"}
          <button
            type="button"
            class="run-btn"
            disabled={store.agentStatus === "streaming" || store.agentStatus === "waiting_hitl"}
            onclick={() => void store.submitAgentTurn()}
          >
            {store.agentStatus === "streaming" ? "Running..." : "Run"}
          </button>
        {/if}

        <button
          type="button"
          class="icon-btn"
          aria-label={store.resolvedTheme === "dark" ? "Switch to light theme" : "Switch to dark theme"}
          title={`Theme: ${store.resolvedTheme}`}
          onclick={() => store.cycleTheme()}
        >
          {#if store.resolvedTheme === "dark"}
            <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
              <path
                fill="currentColor"
                d="M8 1.5a.75.75 0 0 1 .75.75v1a.75.75 0 0 1-1.5 0v-1A.75.75 0 0 1 8 1.5Zm0 9.75a3.25 3.25 0 1 0 0-6.5 3.25 3.25 0 0 0 0 6.5Zm6.5-3.25a.75.75 0 0 1-.75.75h-1a.75.75 0 0 1 0-1.5h1a.75.75 0 0 1 .75.75Zm-11.25.75h-1a.75.75 0 0 1 0-1.5h1a.75.75 0 0 1 0 1.5Zm8.96 4.21a.75.75 0 0 1-1.06 0l-.7-.7a.75.75 0 1 1 1.06-1.06l.7.7a.75.75 0 0 1 0 1.06Zm-7.36-7.36a.75.75 0 0 1-1.06 0l-.7-.7a.75.75 0 0 1 1.06-1.06l.7.7a.75.75 0 0 1 0 1.06Zm7.36-1.76a.75.75 0 0 1 0 1.06l-.7.7a.75.75 0 1 1-1.06-1.06l.7-.7a.75.75 0 0 1 1.06 0Zm-7.36 7.36a.75.75 0 0 1 0 1.06l-.7.7a.75.75 0 1 1-1.06-1.06l.7-.7a.75.75 0 0 1 1.06 0ZM8 12.75a.75.75 0 0 1 .75.75v1a.75.75 0 0 1-1.5 0v-1a.75.75 0 0 1 .75-.75Z"
              />
            </svg>
          {:else}
            <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
              <path
                fill="currentColor"
                d="M6.2 2.13a.75.75 0 0 1 .22.84 5 5 0 0 0 6.61 6.61.75.75 0 0 1 .97.96A6.5 6.5 0 1 1 5.46 1.99a.75.75 0 0 1 .74.14Z"
              />
            </svg>
          {/if}
        </button>

        <button
          type="button"
          class="dsh-toggle mono"
          class:active={store.dshOpen}
          aria-expanded={store.dshOpen}
          aria-controls="dsh-drawer"
          onclick={() => void store.toggleDsh()}
        >
          DSH
        </button>
      </div>
    </div>

    {#if store.chips.length > 0}
      <div class="chips-row">
        {#each store.chips as chip, idx (chip.kind + idx)}
          <span class="chip" title={chip.value}>
            <span class="chip-kind">{chip.label}</span>
            <span class="chip-val">{chip.value}</span>
            <button
              type="button"
              class="chip-remove"
              aria-label={`Remove ${chip.label} context`}
              onclick={() => store.removeChip(idx)}
            >
              ×
            </button>
          </span>
        {/each}
      </div>
    {/if}

    {#if store.statusBanner}
      <div class={`status-banner ${store.statusBanner.kind}`} role="status">
        <span class="status-text">{store.statusBanner.text}</span>
        <button
          type="button"
          class="status-close"
          aria-label="Dismiss notification"
          onclick={() => {
            store.statusBanner = null;
          }}
        >
          ×
        </button>
      </div>
    {/if}

    {#if !isIdleCompact}
      <div class="body">
        {#if store.mode === "search"}
          {#if store.searchError}
            <div class="inline-alert error" role="alert">{store.searchError}</div>
          {:else if store.hits.length > 0}
            <div
              id="launcher-results-listbox"
              class="results"
              role="listbox"
              aria-label="Search results"
            >
              {#each store.hits as hit, idx (`${hit.plugin_id}:${hit.feature_code}:${idx}`)}
                {@const iconKey = `${hit.plugin_id}:${hit.feature_code}`}
                {@const iconUrl = validIconSrc(hit.icon, iconKey)}
                <button
                  type="button"
                  id={`hit-option-${idx}`}
                  role="option"
                  aria-selected={idx === store.selectedIndex}
                  class="result-row"
                  class:selected={idx === store.selectedIndex}
                  onmouseenter={() => {
                    store.selectedIndex = idx;
                  }}
                  onclick={() => void store.launchHit(hit)}
                >
                  <span class="hit-icon" aria-hidden="true">
                    {#if iconUrl}
                      <img
                        src={iconUrl}
                        alt=""
                        onerror={() => markIconBroken(iconKey)}
                      />
                    {:else if hit.kind === "app"}
                      <svg viewBox="0 0 16 16" width="16" height="16">
                        <rect
                          x="2.25"
                          y="2.25"
                          width="11.5"
                          height="11.5"
                          rx="2.5"
                          fill="none"
                          stroke="currentColor"
                          stroke-width="1.4"
                        />
                        <path d="M5.5 6h5M5.5 9.5h3.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
                      </svg>
                    {:else if hit.kind === "wasm"}
                      <svg viewBox="0 0 16 16" width="16" height="16">
                        <path
                          fill="none"
                          stroke="currentColor"
                          stroke-width="1.4"
                          stroke-linejoin="round"
                          d="M8 1.8 13.4 4.9v6.2L8 14.2 2.6 11.1V4.9L8 1.8Z"
                        />
                        <path d="M8 8v6.2M8 8l5.4-3.1M8 8 2.6 4.9" stroke="currentColor" stroke-width="1.2" />
                      </svg>
                    {:else if hit.kind === "list"}
                      <svg viewBox="0 0 16 16" width="16" height="16">
                        <path
                          d="M3 4.5h10M3 8h10M3 11.5h7"
                          stroke="currentColor"
                          stroke-width="1.5"
                          stroke-linecap="round"
                        />
                      </svg>
                    {:else}
                      <svg viewBox="0 0 16 16" width="16" height="16">
                        <path
                          fill="none"
                          stroke="currentColor"
                          stroke-width="1.4"
                          stroke-linejoin="round"
                          d="M9.2 1.8 3.6 9h4.1l-.9 5.2L12.4 7H8.3l.9-5.2Z"
                        />
                      </svg>
                    {/if}
                  </span>

                  <span class="hit-main">
                    <span class="hit-title-row">
                      <span class="hit-title">{hit.label || hit.feature_code}</span>
                      <span class="hit-plugin">{hit.plugin_name || hit.plugin_id}</span>
                    </span>
                    {#if hit.description || hit.explain}
                      <span class="hit-desc">{hit.description || hit.explain}</span>
                    {/if}
                  </span>

                  <span class="hit-meta">
                    <span class="kind-pill">{hitKindBadge(hit)}</span>
                    <span class="meta-score mono tabular">{hit.match_type} · {hit.score}</span>
                    {#if idx < 9}
                      <kbd class="shortcut-hint tabular">⌘{idx + 1}</kbd>
                    {/if}
                  </span>
                </button>
              {/each}
            </div>
          {:else if store.query.trim().length > 0 && !store.searchLoading}
            <div class="empty-search">
              <span>No deterministic command matched "{store.query.trim()}".</span>
              <button
                type="button"
                class="inline-promote-btn"
                onclick={() => {
                  void store.promoteToAgent(store.query);
                  void store.submitAgentTurn();
                }}
              >
                Ask AI Agent <kbd>↵</kbd>
              </button>
            </div>
          {:else if store.suggestions.length > 0}
            <div class="suggestions">
              <div class="section-title">Quick Actions</div>
              {#each store.suggestions as act (act.id)}
                <button
                  type="button"
                  class="suggestion-item"
                  onclick={() => void store.submitAgentTurn(act.prompt_template)}
                >
                  <span class="suggestion-title">{act.title}</span>
                  <span class="meta mono">{act.signature}</span>
                </button>
              {/each}
            </div>
          {/if}
        {:else if store.mode === "list" && store.listSession}
          <div class="list-pane">
            <div class="list-header" data-tauri-drag-region>
              <div class="list-header-left">
                <button
                  type="button"
                  class="back-btn"
                  onclick={() => {
                    store.exitListToSearch();
                    searchInput?.focus();
                  }}
                >
                  ← Back
                </button>
                <span class="list-title">{store.listSession.plugin_name}</span>
                <span class="kind-pill mono">{store.listSession.feature_code}</span>
              </div>
              <span class="meta tabular">{store.listSession.items.length} item(s)</span>
            </div>

            <div class="sub-input-wrap">
              <input
                bind:this={subInputEl}
                type="text"
                class="sub-input"
                role="combobox"
                aria-label="Filter plugin list items"
                aria-autocomplete="list"
                aria-expanded={store.listSession.items.length > 0}
                aria-controls="list-mode-listbox"
                aria-activedescendant={store.listSession.items.length > 0
                  ? `list-option-${store.listSelectedIndex}`
                  : undefined}
                value={store.subInput}
                placeholder={store.listSession.placeholder || "Type to filter plugin options..."}
                oncompositionstart={() => {
                  isComposing = true;
                }}
                oncompositionend={(e) => {
                  isComposing = false;
                  void store.onSubInputChange((e.currentTarget as HTMLInputElement).value);
                }}
                oninput={(e) => {
                  const val = (e.currentTarget as HTMLInputElement).value;
                  if (isComposing || eventIsComposing(e)) return;
                  void store.onSubInputChange(val);
                }}
              />
            </div>

            {#if store.listError}
              <div class="inline-alert error" role="alert">{store.listError}</div>
            {:else if store.listLoading && store.listSession.items.length === 0}
              <div class="hint">Loading items...</div>
            {:else if store.listSession.items.length === 0}
              <div class="hint">No matching items in this plugin list.</div>
            {:else}
              <div
                id="list-mode-listbox"
                class="results"
                role="listbox"
                aria-label="Plugin list options"
              >
                {#each store.listSession.items as item, idx (`${item.title}:${idx}`)}
                  {@const listIcon = validIconSrc(item.icon, `list:${idx}`)}
                  <button
                    type="button"
                    id={`list-option-${idx}`}
                    role="option"
                    aria-selected={idx === store.listSelectedIndex}
                    class="result-row"
                    class:selected={idx === store.listSelectedIndex}
                    onmouseenter={() => {
                      store.listSelectedIndex = idx;
                    }}
                    onclick={() => void store.selectListAt(idx)}
                  >
                    <span class="hit-icon" aria-hidden="true">
                      {#if listIcon}
                        <img
                          src={listIcon}
                          alt=""
                          onerror={() => markIconBroken(`list:${idx}`)}
                        />
                      {:else}
                        <svg viewBox="0 0 16 16" width="16" height="16">
                          <path
                            d="M3 4.5h10M3 8h10M3 11.5h7"
                            stroke="currentColor"
                            stroke-width="1.5"
                            stroke-linecap="round"
                          />
                        </svg>
                      {/if}
                    </span>
                    <span class="hit-main">
                      <span class="hit-title">{item.title}</span>
                      {#if item.description}
                        <span class="hit-desc">{item.description}</span>
                      {/if}
                    </span>
                    <span class="hit-meta">
                      {#if idx < 9}
                        <kbd class="shortcut-hint tabular">⌘{idx + 1}</kbd>
                      {/if}
                    </span>
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        {:else}
          <div class="agent-pane">
            <div class="agent-toolbar">
              <div class="agent-toolbar-left">
                <span class="agent-status-pill" class:error={store.agentStatus === "error"}>
                  {store.agentStatus === "streaming"
                    ? "Streaming"
                    : store.agentStatus === "waiting_hitl"
                      ? "Approval Needed"
                      : store.agentStatus === "done"
                        ? "Completed"
                        : store.agentStatus === "error"
                          ? "Error"
                          : "Ready"}
                </span>
                <span class="meta mono tabular">{store.traceId}</span>
              </div>
              <label class="profile-select-wrap">
                <span class="meta">Profile</span>
                <select
                  class="profile-select"
                  aria-label="AI model profile"
                  value={store.agentProfile}
                  onchange={(e) => {
                    const v = (e.currentTarget as HTMLSelectElement).value;
                    if (v === "fast" || v === "balanced" || v === "deep") {
                      store.agentProfile = v;
                    }
                  }}
                >
                  <option value="fast">Fast</option>
                  <option value="balanced">Balanced</option>
                  <option value="deep">Deep</option>
                </select>
              </label>
            </div>

            {#if store.pendingHitl}
              {@const tier2 = isTier2Risk(store.pendingHitl.riskTier)}
              <div
                class="hitl-card"
                class:tier2={tier2}
                role="alertdialog"
                aria-modal="false"
                aria-labelledby="hitl-dialog-title"
                aria-live="assertive"
              >
                <div class="hitl-header">
                  <strong id="hitl-dialog-title">Confirmation Required</strong>
                  <span class="risk-badge" class:tier2={tier2}>
                    {riskTierLabel(store.pendingHitl.riskTier)}
                  </span>
                </div>
                <div class="hitl-tool mono">{store.pendingHitl.toolName}</div>
                <pre class="hitl-args mono">{JSON.stringify(store.pendingHitl.arguments, null, 2)}</pre>
                {#if tier2}
                  <p class="hitl-note">
                    High-risk external or destructive steps always require explicit per-call approval.
                  </p>
                {/if}
                <div class="hitl-actions">
                  <button
                    type="button"
                    class="btn-primary"
                    onclick={() => void store.approveHitl("approve_once")}
                  >
                    Approve Once
                  </button>
                  {#if !tier2}
                    <button
                      type="button"
                      class="btn-secondary"
                      onclick={() => void store.approveHitl("approve_session")}
                    >
                      Approve Session
                    </button>
                  {/if}
                  <button
                    type="button"
                    class="btn-danger"
                    onclick={() => void store.approveHitl("deny")}
                  >
                    Deny
                  </button>
                </div>
              </div>
            {/if}

            {#if store.dagEvents.length > 0}
              <div class="dag-timeline" aria-label="Execution steps">
                {#each store.dagEvents as ev, idx (`dag-${idx}`)}
                  {@const info = dagNodeSummary(ev)}
                  <div class={`dag-step ${info.status}`}>
                    <span class="dag-dot" aria-hidden="true"></span>
                    <span class="dag-tool mono">{info.title}</span>
                    <span class="dag-detail">{info.detail}</span>
                  </div>
                {/each}
              </div>
            {/if}

            {#if store.agentError}
              <div class="inline-alert error" role="alert">{store.agentError}</div>
            {/if}

            {#if store.agentResponse}
              <div class="agent-response" aria-live="polite">{store.agentResponse}</div>
            {:else if store.agentStatus === "streaming"}
              <div class="hint">Thinking and planning tools...</div>
            {:else if !store.pendingHitl && !store.agentError}
              <div class="hint">
                Press <kbd>Enter</kbd> or click <strong>Run</strong> to invoke the desktop agent with attached context chips.
              </div>
            {/if}
          </div>
        {/if}
      </div>
    {/if}

    {#if store.dshOpen}
      <DshPanel
        logs={store.dshLogs}
        faults={store.faults}
        activeTraceId={store.traceId}
        faultPending={store.faultPending}
        faultError={store.faultError}
        syncPending={store.syncPending}
        lastSyncResult={store.lastSyncResult}
        onRefresh={() => void store.refreshDshLogs()}
        onToggleFault={(k) => void store.updateFault(k)}
        onSyncPlugins={() => void store.syncZtoolsPlugins()}
        onClose={() => void store.toggleDsh()}
      />
    {/if}
  </main>
{/if}

<style>
  .shell {
    background: var(--bg-primary);
    backdrop-filter: blur(24px) saturate(1.5);
    -webkit-backdrop-filter: blur(24px) saturate(1.5);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-shell);
    overflow: hidden;
    display: flex;
    flex-direction: column;
    max-height: 100vh;
    width: 100%;
    animation: fadeIn 140ms var(--ease-out-expo);
  }

  .shell.compact {
    height: 62px;
  }

  .search-bar {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: 0 var(--space-4);
    height: 60px;
    flex: 0 0 auto;
    border-bottom: 1px solid var(--border);
  }

  .shell.compact .search-bar {
    border-bottom: none;
  }

  .brand-wrap {
    display: inline-flex;
    align-items: center;
    flex: 0 0 auto;
  }

  .mode-badge {
    font-size: var(--text-xs);
    font-weight: 600;
    padding: var(--space-1) var(--space-2);
    border-radius: var(--radius-pill);
    background: var(--bg-tertiary);
    color: var(--text-secondary);
    flex: 0 0 auto;
    min-height: 26px;
  }

  .mode-badge:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .mode-badge.agent {
    background: var(--accent-subtle);
    color: var(--accent);
  }

  .mode-badge.list {
    background: var(--success-subtle);
    color: var(--success);
  }

  .search-input {
    flex: 1;
    min-width: 0;
    border: none;
    background: transparent;
    color: var(--text-primary);
    font-size: var(--text-md);
    height: 36px;
  }

  .search-input:focus-visible {
    outline: none;
  }

  .search-bar:focus-within {
    box-shadow: inset 0 -2px 0 0 var(--accent);
  }

  .search-input::placeholder {
    color: var(--text-tertiary);
    opacity: 1;
  }

  .bar-actions {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex: 0 0 auto;
  }

  .run-btn {
    padding: var(--space-1) var(--space-3);
    border-radius: var(--radius-sm);
    background: var(--accent);
    color: var(--accent-foreground);
    font-size: var(--text-xs);
    font-weight: 600;
    min-height: 28px;
  }

  .run-btn:hover:not(:disabled) {
    background: var(--accent-hover);
  }

  .icon-btn {
    width: 28px;
    height: 28px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border-strong);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--text-secondary);
  }

  .icon-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .dsh-toggle {
    padding: var(--space-1) var(--space-2);
    border-radius: var(--radius-sm);
    border: 1px solid var(--border-strong);
    font-size: var(--text-xs);
    font-weight: 600;
    color: var(--text-secondary);
    min-height: 28px;
  }

  .dsh-toggle:hover,
  .dsh-toggle.active {
    background: var(--accent-subtle);
    color: var(--accent);
    border-color: var(--accent);
  }

  .chips-row {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-4);
    border-bottom: 1px solid var(--border);
    flex: 0 0 auto;
  }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    background: var(--accent-subtle);
    color: var(--text-primary);
    border-radius: var(--radius-sm);
    padding: 2px var(--space-2);
    font-size: var(--text-xs);
    max-width: 280px;
    min-width: 0;
  }

  .chip-kind {
    color: var(--accent);
    font-weight: 600;
    flex: 0 0 auto;
  }

  .chip-val {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .chip-remove {
    width: 18px;
    height: 18px;
    border-radius: var(--radius-sm);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--text-secondary);
    flex: 0 0 auto;
  }

  .chip-remove:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .status-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
    padding: var(--space-2) var(--space-4);
    font-size: var(--text-xs);
    border-bottom: 1px solid var(--border);
  }

  .status-banner.success {
    background: var(--success-subtle);
    color: var(--success);
  }

  .status-banner.info {
    background: var(--accent-subtle);
    color: var(--accent);
  }

  .status-banner.error {
    background: var(--danger-subtle);
    color: var(--danger);
  }

  .status-text {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .status-close {
    width: 22px;
    height: 22px;
    border-radius: var(--radius-sm);
    flex: 0 0 auto;
  }

  .body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: var(--space-2) var(--space-3);
  }

  .results {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    min-height: 0;
    max-height: 330px;
    overflow-y: auto;
  }

  .result-row,
  .suggestion-item {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: var(--space-3);
    width: 100%;
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    text-align: left;
    min-width: 0;
  }

  .result-row:hover,
  .result-row.selected,
  .suggestion-item:hover {
    background: var(--bg-hover);
  }

  .result-row.selected {
    box-shadow: inset 2px 0 0 0 var(--accent);
  }

  .hit-icon {
    width: 28px;
    height: 28px;
    border-radius: var(--radius-sm);
    background: var(--bg-tertiary);
    color: var(--accent);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: 0 0 28px;
    overflow: hidden;
  }

  .hit-icon img {
    width: 20px;
    height: 20px;
    object-fit: contain;
  }

  .hit-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .hit-title-row {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
    min-width: 0;
  }

  .hit-title {
    font-size: var(--text-base);
    font-weight: 600;
    color: var(--text-primary);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .hit-plugin {
    font-size: var(--text-xs);
    color: var(--text-tertiary);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .hit-desc {
    font-size: var(--text-xs);
    color: var(--text-secondary);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .hit-meta {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    flex: 0 0 auto;
  }

  .kind-pill {
    font-size: var(--text-xs);
    font-weight: 600;
    padding: 2px var(--space-2);
    border-radius: var(--radius-sm);
    background: var(--bg-tertiary);
    color: var(--text-secondary);
  }

  .meta,
  .meta-score {
    font-size: var(--text-xs);
    color: var(--text-tertiary);
  }

  .shortcut-hint {
    background: var(--bg-tertiary);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    padding: 1px var(--space-1);
    font-size: var(--text-xs);
    color: var(--text-secondary);
  }

  .suggestions {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }

  .suggestion-title {
    font-size: var(--text-base);
    font-weight: 500;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .section-title {
    font-size: var(--text-xs);
    font-weight: 600;
    color: var(--text-tertiary);
    padding: var(--space-1) var(--space-2);
  }

  .empty-search {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-2);
    font-size: var(--text-sm);
    color: var(--text-secondary);
  }

  .inline-promote-btn {
    padding: var(--space-1) var(--space-3);
    border-radius: var(--radius-sm);
    background: var(--accent-subtle);
    color: var(--accent);
    font-size: var(--text-xs);
    font-weight: 600;
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    min-height: 30px;
    flex: 0 0 auto;
  }

  .inline-promote-btn:hover {
    background: var(--accent);
    color: var(--accent-foreground);
  }

  .list-pane,
  .agent-pane {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .list-header,
  .agent-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
    padding: var(--space-1) var(--space-2);
  }

  .list-header-left,
  .agent-toolbar-left {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }

  .back-btn {
    padding: var(--space-1) var(--space-2);
    border-radius: var(--radius-sm);
    border: 1px solid var(--border-strong);
    background: var(--bg-secondary);
    font-size: var(--text-xs);
    font-weight: 600;
    min-height: 28px;
  }

  .back-btn:hover {
    background: var(--bg-hover);
  }

  .list-title {
    font-weight: 600;
    font-size: var(--text-base);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .sub-input-wrap {
    padding: 0 var(--space-1);
  }

  .sub-input {
    width: 100%;
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    border: 1px solid var(--border-strong);
    background: var(--bg-elevated);
    color: var(--text-primary);
    font-size: var(--text-sm);
  }

  .sub-input::placeholder {
    color: var(--text-tertiary);
    opacity: 1;
  }

  .agent-status-pill {
    font-size: var(--text-xs);
    font-weight: 600;
    padding: 2px var(--space-2);
    border-radius: var(--radius-pill);
    background: var(--accent-subtle);
    color: var(--accent);
  }

  .agent-status-pill.error {
    background: var(--danger-subtle);
    color: var(--danger);
  }

  .profile-select-wrap {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
  }

  .profile-select {
    background: var(--bg-elevated);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    padding: 2px var(--space-2);
    font-size: var(--text-xs);
    min-height: 26px;
  }

  .dag-timeline {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    padding: var(--space-2);
    border-radius: var(--radius-md);
    background: var(--bg-secondary);
    border: 1px solid var(--border);
  }

  .dag-step {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    font-size: var(--text-xs);
  }

  .dag-dot {
    width: 8px;
    height: 8px;
    border-radius: var(--radius-pill);
    background: var(--accent);
    flex: 0 0 8px;
  }

  .dag-step.done .dag-dot {
    background: var(--success);
  }

  .dag-step.hitl .dag-dot {
    background: var(--warning);
  }

  .dag-tool {
    font-weight: 600;
    color: var(--text-primary);
  }

  .dag-detail {
    color: var(--text-secondary);
  }

  .agent-response {
    padding: var(--space-3);
    border-radius: var(--radius-md);
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    font-size: var(--text-sm);
    line-height: 1.55;
    white-space: pre-wrap;
    max-height: 200px;
    overflow-y: auto;
    animation: slideUp 160ms var(--ease-out-expo);
  }

  .inline-alert {
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    font-size: var(--text-xs);
  }

  .inline-alert.error {
    background: var(--danger-subtle);
    color: var(--danger);
    border: 1px solid var(--danger);
  }

  .hint {
    color: var(--text-secondary);
    font-size: var(--text-sm);
    padding: var(--space-2);
  }

  kbd {
    background: var(--bg-tertiary);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-sm);
    padding: 1px var(--space-1);
    font-size: var(--text-xs);
  }

  .hitl-card {
    border: 1px solid var(--warning);
    border-radius: var(--radius-md);
    padding: var(--space-3);
    background: var(--bg-secondary);
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .hitl-card.tier2 {
    border-color: var(--danger);
  }

  .hitl-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
  }

  .risk-badge {
    font-size: var(--text-xs);
    font-weight: 600;
    padding: 2px var(--space-2);
    border-radius: var(--radius-pill);
    background: var(--warning-subtle);
    color: var(--warning);
  }

  .risk-badge.tier2 {
    background: var(--danger-subtle);
    color: var(--danger);
  }

  .hitl-tool {
    font-weight: 600;
    color: var(--text-primary);
  }

  .hitl-args {
    max-height: 140px;
    overflow: auto;
    padding: var(--space-2);
    border-radius: var(--radius-sm);
    background: var(--bg-tertiary);
    border: 1px solid var(--border);
  }

  .hitl-note {
    font-size: var(--text-xs);
    color: var(--danger);
  }

  .hitl-actions {
    display: flex;
    gap: var(--space-2);
    flex-wrap: wrap;
  }

  .btn-primary,
  .btn-secondary,
  .btn-danger {
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    font-size: var(--text-xs);
    font-weight: 600;
    min-height: 36px;
  }

  .btn-primary {
    background: var(--accent);
    color: var(--accent-foreground);
  }

  .btn-primary:hover {
    background: var(--accent-hover);
  }

  .btn-secondary {
    background: transparent;
    border: 1px solid var(--border-strong);
    color: var(--text-primary);
  }

  .btn-secondary:hover {
    background: var(--bg-hover);
  }

  .btn-danger {
    background: var(--danger-solid);
    color: var(--danger-foreground);
  }

  .btn-danger:hover {
    filter: brightness(0.92);
  }

  .btn-primary:active,
  .btn-secondary:active,
  .btn-danger:active {
    transform: scale(0.98);
  }
</style>
