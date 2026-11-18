<script lang="ts">
  import { onMount, tick } from "svelte";
  import AToolsMark from "./components/AToolsMark.svelte";
  import DshPanel from "./components/DshPanel.svelte";
  import { setMainWindowHeight, type AiProfile, type LauncherHit } from "./lib/ipc";
  import {
    chipShortLabel,
    highlightSegments,
    hitKindBadge,
    squircleStyle,
    validIconSrc,
  } from "./lib/launcherUi";
  import { createLauncherStore, isTier2Risk } from "./stores/launcher.svelte";

  const store = createLauncherStore();

  let shellEl: HTMLElement | undefined = $state();
  let dropdownEl: HTMLElement | undefined = $state();
  let searchInput: HTMLInputElement | undefined = $state();
  let subInputEl: HTMLInputElement | undefined = $state();
  let isComposing = $state(false);
  let brokenIcons = $state<Record<string, boolean>>({});
  let lastReportedHeight = 0;
  let prevMode = "search";

  const visibleHomeItems = $derived(
    store.homeExpanded ? store.homeItems : store.homeItems.slice(0, 18),
  );

  const showHomeGrid = $derived(
    store.mode === "search" &&
      !store.query.trim() &&
      store.hits.length === 0 &&
      !store.searchError &&
      (store.hasWoken || store.suggestions.length > 0 || store.chips.length > 0),
  );

  const isIdleCompact = $derived(
    !store.dshStandalone &&
      store.mode === "search" &&
      !store.query.trim() &&
      store.hits.length === 0 &&
      !store.hasWoken &&
      store.suggestions.length === 0 &&
      store.chips.length === 0 &&
      !store.dshOpen &&
      !store.statusBanner &&
      !store.searchError &&
      !store.searchLoading,
  );

  const hasSearchPanelContent = $derived(
    store.mode === "search" &&
      (Boolean(store.searchError) ||
        store.hits.length > 0 ||
        (store.query.trim().length > 0 && !store.searchLoading) ||
        store.suggestions.length > 0 ||
        (showHomeGrid && visibleHomeItems.length > 0)),
  );

  const dropdownOpen = $derived(
    !store.dshStandalone &&
      !isIdleCompact &&
      (Boolean(store.statusBanner) ||
        store.dshOpen ||
        (store.mode === "list" && Boolean(store.listSession)) ||
        store.mode === "agent" ||
        hasSearchPanelContent),
  );

  function syncWindowSize() {
    if (store.dshStandalone) return;
    if (!dropdownOpen) {
      if (lastReportedHeight !== 58) {
        lastReportedHeight = 58;
        void setMainWindowHeight(58);
      }
      return;
    }
    if (!dropdownEl) return;
    const measured = 58 + Math.min(536, Math.ceil(dropdownEl.scrollHeight) + 2);
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

    syncWindowSize();

    return () => {
      cleanupBridge?.();
    };
  });

  $effect(() => {
    if (typeof ResizeObserver === "undefined") return;
    const targets = [shellEl, dropdownEl].filter((el): el is HTMLElement => Boolean(el));
    if (targets.length === 0) return;
    const observer = new ResizeObserver(() => {
      syncWindowSize();
    });
    for (const el of targets) {
      observer.observe(el);
    }
    return () => observer.disconnect();
  });

  $effect(() => {
    // Re-measure when layout-affecting state transitions occur.
    void isIdleCompact;
    void dropdownOpen;
    void store.mode;
    void store.hasWoken;
    void store.hits.length;
    void store.homeItems.length;
    void store.homeExpanded;
    void store.suggestions.length;
    void store.chips.length;
    void store.dshOpen;
    void store.statusBanner;
    void store.searchError;
    void store.searchLoading;
    void store.agentResponse;
    void store.agentError;
    void store.dagEvents.length;
    void store.pendingHitl;
    void store.listSession?.items.length;

    const enteredListMode = store.mode === "list" && prevMode !== "list";
    prevMode = store.mode;

    void tick().then(() => {
      syncWindowSize();
      if (enteredListMode) {
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
      if (showHomeGrid && visibleHomeItems.length > 0) {
        e.preventDefault();
        store.moveHomeSelection(store.homeSelectedIndex < 0 ? 0 : 9);
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
      if (showHomeGrid && visibleHomeItems.length > 0) {
        e.preventDefault();
        store.moveHomeSelection(-9);
        return;
      }
    }

    if (
      (e.key === "ArrowRight" || e.key === "ArrowLeft") &&
      showHomeGrid &&
      visibleHomeItems.length > 0 &&
      !store.query
    ) {
      e.preventDefault();
      store.moveHomeSelection(e.key === "ArrowRight" ? 1 : -1);
      return;
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

  function markIconBroken(key: string) {
    brokenIcons = { ...brokenIcons, [key]: true };
  }

  function agentProfileLabel(profile: AiProfile): string {
    switch (profile) {
      case "fast":
        return "Fast";
      case "deep":
        return "Deep";
      default:
        return "Balanced";
    }
  }

  function suggestionHint(targetTool: string | null | undefined): string {
    return targetTool ? "AI 工具快捷执行" : "AI 上下文推荐";
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

  function itemSubtitle(hit: LauncherHit): string {
    return hit.description || hit.plugin_name || hit.explain || "";
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
    themePreference={store.themePreference}
    onChangeTheme={(next) => store.setThemePreference(next)}
    onRefresh={() => void store.refreshDshLogs()}
    onToggleFault={(k) => void store.updateFault(k)}
    onSyncPlugins={() => void store.syncZtoolsPlugins()}
  />
{:else}
  <main bind:this={shellEl} class="shell" class:compact={!dropdownOpen}>
    <div class="search-bar" data-tauri-drag-region>
      <div class="search-input-container" data-tauri-drag-region>
        {#if store.mode === "list" && store.listSession}
          <div class="plugin-tag has-cmd">
            <div class="plugin-tag-left">
              <span class="plugin-tag-glyph" aria-hidden="true">
                <svg viewBox="0 0 16 16" width="14" height="14">
                  <path
                    d="M3 4.5h10M3 8h10M3 11.5h7"
                    stroke="currentColor"
                    stroke-width="1.6"
                    stroke-linecap="round"
                  />
                </svg>
              </span>
              <span class="plugin-tag-title">{store.listSession.plugin_name}</span>
            </div>
            <span class="plugin-tag-cmd">{store.listSession.feature_code}</span>
            <button
              type="button"
              class="plugin-tag-close"
              aria-label="退出列表模式"
              onclick={() => {
                store.exitListToSearch();
                searchInput?.focus();
              }}
            >
              ×
            </button>
          </div>
        {:else if store.mode === "agent"}
          <div class="plugin-tag has-cmd">
            <div class="plugin-tag-left">
              <span class="plugin-tag-glyph" aria-hidden="true">
                <svg viewBox="0 0 16 16" width="14" height="14">
                  <path
                    fill="currentColor"
                    d="M8 1.5 9.6 5.4l3.9 1.6-3.9 1.6L8 12.5 6.4 8.6 2.5 7l3.9-1.6L8 1.5Z"
                  />
                </svg>
              </span>
              <span class="plugin-tag-title">AI Agent</span>
            </div>
            <span class="plugin-tag-cmd">{agentProfileLabel(store.agentProfile)}</span>
            <button
              type="button"
              class="plugin-tag-close"
              aria-label="退出 AI 模式"
              onclick={() => {
                store.exitAgentToSearch();
                searchInput?.focus();
              }}
            >
              ×
            </button>
          </div>
        {/if}

        {#if store.mode !== "list" && store.chips.length > 0}
          {#each store.chips as chip, idx (chip.kind + idx)}
            <div
              class="pasted-text"
              class:compact-chip={store.chips.length === 3}
              class:many-chips={store.chips.length >= 4}
              title={chip.value}
            >
              <span class="text-icon" aria-hidden="true">
                {#if chip.kind === "clipboard"}
                  <svg viewBox="0 0 16 16" width="13" height="13">
                    <rect
                      x="3.5"
                      y="3"
                      width="9"
                      height="11"
                      rx="1.6"
                      fill="none"
                      stroke="currentColor"
                      stroke-width="1.35"
                    />
                    <path
                      d="M6 2.2h4"
                      stroke="currentColor"
                      stroke-width="1.5"
                      stroke-linecap="round"
                    />
                  </svg>
                {:else}
                  <svg viewBox="0 0 16 16" width="13" height="13">
                    <path
                      d="M3 4.5h10M3 8h7M3 11.5h5"
                      stroke="currentColor"
                      stroke-width="1.4"
                      stroke-linecap="round"
                    />
                  </svg>
                {/if}
              </span>
              <span class="text-info">
                <span class="pasted-label">{chipShortLabel(chip.kind, chip.label)}:</span>
                <span class="text-content">{chip.value}</span>
              </span>
              <button
                type="button"
                class="pasted-remove"
                aria-label={`Remove ${chip.label} context`}
                onclick={() => {
                  store.removeChip(idx);
                  searchInput?.focus();
                }}
              >
                ×
              </button>
            </div>
          {/each}
        {/if}

        <input
          bind:this={searchInput}
          class="search-input"
          type="text"
          role="combobox"
          aria-label={store.mode === "agent"
            ? "Ask AI Agent"
            : "搜索应用、插件或输入指令，Tab 切换 AI"}
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
            ? "向 AI Agent 提问，Enter 发送..."
            : store.mode === "list"
              ? "在下方过滤插件列表，Esc 返回..."
              : "搜索应用、插件或输入指令，Tab 切换 AI..."}
        />
      </div>

      <div class="search-actions">
        {#if store.mode === "agent"}
          <button
            type="button"
            class="run-btn"
            disabled={store.agentStatus === "streaming" || store.agentStatus === "waiting_hitl"}
            onclick={() => void store.submitAgentTurn()}
          >
            {store.agentStatus === "streaming" ? "Running..." : "Run"}
          </button>
        {:else if store.mode === "search"}
          <button
            type="button"
            class="tab-target-hint"
            tabindex="-1"
            title="按 Tab 切换到 AI 模式"
            onclick={() => {
              void store.promoteToAgent();
              searchInput?.focus();
            }}
          >
            <span class="tab-target-text">AI</span>
            <kbd class="tab-target-key">Tab</kbd>
          </button>
        {/if}

        <button
          type="button"
          class="avatar-btn"
          aria-label={store.mode === "agent" ? "Switch back to Search" : "Switch to AI Agent"}
          title={store.mode === "agent" ? "返回搜索模式" : "切换到 AI 模式 (Tab)"}
          onclick={() => {
            if (store.mode === "agent") {
              store.exitAgentToSearch();
            } else {
              void store.promoteToAgent();
            }
            searchInput?.focus();
          }}
        >
          <AToolsMark
            size="badge"
            circular={true}
            streaming={store.agentStatus === "streaming" || store.agentStatus === "waiting_hitl"}
            label="aitools"
          />
        </button>
      </div>
    </div>

    {#if dropdownOpen}
      <div bind:this={dropdownEl} class="dropdown-panel">
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

        {#if store.mode === "search"}
          {#if store.searchError}
            <div class="inline-alert error" role="alert">{store.searchError}</div>
          {:else if store.hits.length > 0}
            <div
              id="launcher-results-listbox"
              class="vertical-list"
              role="listbox"
              aria-label="Search results"
            >
              {#each store.hits as hit, idx (`${hit.plugin_id}:${hit.feature_code}:${idx}`)}
                {@const iconKey = `hit:${hit.plugin_id}:${hit.feature_code}`}
                {@const iconUrl = validIconSrc(hit.icon, brokenIcons, iconKey)}
                {@const squircle = squircleStyle(hit.plugin_id || hit.feature_code, hit.label || hit.feature_code)}
                {@const titleText = hit.label || hit.feature_code}
                {@const subText = itemSubtitle(hit)}
                <button
                  type="button"
                  id={`hit-option-${idx}`}
                  role="option"
                  aria-selected={idx === store.selectedIndex}
                  class="list-item"
                  class:selected={idx === store.selectedIndex}
                  onmouseenter={() => {
                    store.selectedIndex = idx;
                  }}
                  onclick={() => void store.launchHit(hit)}
                >
                  <span class="item-icon" aria-hidden="true">
                    {#if iconUrl}
                      <img
                        src={iconUrl}
                        alt=""
                        onerror={() => markIconBroken(iconKey)}
                      />
                    {:else}
                      <span
                        class="squircle-icon"
                        style={`background-color: ${squircle.color}; background-image: ${squircle.bg};`}
                      >
                        {squircle.monogram}
                      </span>
                    {/if}
                  </span>

                  <span class="item-content">
                    <span class="item-title">
                      {#each highlightSegments(titleText, store.query) as seg, sIdx (sIdx)}
                        {#if seg.match}<mark class="highlight">{seg.text}</mark>{:else}{seg.text}{/if}
                      {/each}
                    </span>
                    {#if subText}
                      <span class="item-subtitle">{subText}</span>
                    {/if}
                  </span>

                  <span class="hit-meta">
                    <span class="item-badge">{hitKindBadge(hit.kind)}</span>
                    {#if idx < 9}
                      <kbd class="shortcut-hint tabular">⌘{idx + 1}</kbd>
                    {/if}
                  </span>
                </button>
              {/each}
            </div>
          {:else if store.query.trim().length > 0 && !store.searchLoading}
            <div class="empty-search">
              <span>未找到与 “{store.query.trim()}” 匹配的本地应用或指令。</span>
              <button
                type="button"
                class="inline-promote-btn"
                onclick={() => {
                  void store.promoteToAgent(store.query);
                  void store.submitAgentTurn();
                }}
              >
                使用 AI 处理 <kbd>↵</kbd>
              </button>
            </div>
          {:else}
            {#if store.suggestions.length > 0}
              <section class="main-push-section" aria-label="智能推荐">
                <div class="section-header">
                  <span class="section-title">智能推荐</span>
                </div>
                <div class="push-list">
                  {#each store.suggestions as act (act.id)}
                    <button
                      type="button"
                      class="push-item"
                      onclick={() => void store.submitAgentTurn(act.prompt_template)}
                    >
                      <span class="push-icon" aria-hidden="true">
                        <svg viewBox="0 0 16 16" width="14" height="14">
                          <path
                            fill="currentColor"
                            d="M8 1.5 9.6 5.4l3.9 1.6-3.9 1.6L8 12.5 6.4 8.6 2.5 7l3.9-1.6L8 1.5Z"
                          />
                        </svg>
                      </span>
                      <span class="push-content">
                        <span class="push-title">{act.title}</span>
                      </span>
                      <span class="item-badge">{suggestionHint(act.target_tool)}</span>
                    </button>
                  {/each}
                </div>
              </section>
            {/if}

            {#if showHomeGrid && visibleHomeItems.length > 0}
              <section class="home-section" aria-label="最佳匹配">
                <div class="section-header">
                  <span class="section-title">最佳匹配</span>
                  {#if store.homeItems.length > 18}
                    <button
                      type="button"
                      class="expand-btn-text"
                      onclick={() => store.toggleHomeExpanded()}
                    >
                      {store.homeExpanded ? "收起" : `展开 (${store.homeItems.length})`}
                    </button>
                  {/if}
                </div>
                <div class="app-grid">
                  {#each visibleHomeItems as item, idx (`home:${item.plugin_id}:${item.feature_code}:${idx}`)}
                    {@const iconKey = `home:${item.plugin_id}:${item.feature_code}`}
                    {@const iconUrl = validIconSrc(item.icon, brokenIcons, iconKey)}
                    {@const squircle = squircleStyle(item.plugin_id || item.feature_code, item.label || item.plugin_name)}
                    <button
                      type="button"
                      class="app-item"
                      class:selected={idx === store.homeSelectedIndex}
                      title={item.description || item.label}
                      onmouseenter={() => {
                        store.homeSelectedIndex = idx;
                      }}
                      onclick={() => void store.launchHit(item)}
                    >
                      <span class="app-icon-wrap" aria-hidden="true">
                        {#if iconUrl}
                          <img
                            class="app-icon"
                            src={iconUrl}
                            alt=""
                            onerror={() => markIconBroken(iconKey)}
                          />
                        {:else}
                          <span
                            class="app-icon-placeholder"
                            style={`background-color: ${squircle.color}; background-image: ${squircle.bg};`}
                          >
                            {squircle.monogram}
                          </span>
                        {/if}
                      </span>
                      <span class="app-name">{item.label || item.plugin_name || item.feature_code}</span>
                    </button>
                  {/each}
                </div>
              </section>
            {/if}
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
                <span class="item-badge mono">{store.listSession.feature_code}</span>
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
                class="vertical-list"
                role="listbox"
                aria-label="Plugin list options"
              >
                {#each store.listSession.items as item, idx (`${item.title}:${idx}`)}
                  {@const listIcon = validIconSrc(item.icon, brokenIcons, `list:${idx}`)}
                  {@const squircle = squircleStyle(`list:${idx}:${item.title}`, item.title)}
                  <button
                    type="button"
                    id={`list-option-${idx}`}
                    role="option"
                    aria-selected={idx === store.listSelectedIndex}
                    class="list-item"
                    class:selected={idx === store.listSelectedIndex}
                    onmouseenter={() => {
                      store.listSelectedIndex = idx;
                    }}
                    onclick={() => void store.selectListAt(idx)}
                  >
                    <span class="item-icon" aria-hidden="true">
                      {#if listIcon}
                        <img
                          src={listIcon}
                          alt=""
                          onerror={() => markIconBroken(`list:${idx}`)}
                        />
                      {:else}
                        <span
                          class="squircle-icon"
                          style={`background-color: ${squircle.color}; background-image: ${squircle.bg};`}
                        >
                          {squircle.monogram}
                        </span>
                      {/if}
                    </span>
                    <span class="item-content">
                      <span class="item-title">{item.title}</span>
                      {#if item.description}
                        <span class="item-subtitle">{item.description}</span>
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
                Press <kbd>Enter</kbd> or click <strong>Run</strong> to invoke the AI Agent with attached context.
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
            themePreference={store.themePreference}
            onChangeTheme={(next) => store.setThemePreference(next)}
            onRefresh={() => void store.refreshDshLogs()}
            onToggleFault={(k) => void store.updateFault(k)}
            onSyncPlugins={() => void store.syncZtoolsPlugins()}
            onClose={() => void store.toggleDsh()}
          />
        {/if}
      </div>
    {/if}
  </main>
{/if}

<style>
  .shell {
    position: relative;
    background: var(--bg-primary);
    backdrop-filter: blur(24px) saturate(1.5);
    -webkit-backdrop-filter: blur(24px) saturate(1.5);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-shell);
    display: flex;
    flex-direction: column;
    width: 100%;
    animation: fadeIn 140ms var(--ease-out-expo);
  }

  .shell::after {
    content: "";
    position: absolute;
    inset: 0;
    border-radius: var(--radius-lg);
    border: 1px solid var(--shell-border);
    pointer-events: none;
    z-index: 50;
  }

  .shell.compact {
    height: 58px;
  }

  .search-bar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 5px 12px;
    height: 58px;
    flex: 0 0 58px;
    width: 100%;
    min-width: 0;
    border-radius: var(--radius-lg) var(--radius-lg) 0 0;
  }

  .shell.compact .search-bar {
    border-radius: var(--radius-lg);
  }

  .search-input-container {
    flex: 1 1 0;
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    overflow: hidden;
  }

  .pasted-text {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 34px;
    max-width: 220px;
    padding: 0 6px 0 9px;
    border-radius: 6px;
    background: var(--control-bg);
    border: 1px dashed var(--border-strong);
    flex: 0 1 auto;
    min-width: 56px;
  }

  .pasted-text.compact-chip {
    max-width: 150px;
  }

  .pasted-text.many-chips {
    max-width: 112px;
  }

  .text-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--accent-text);
    flex: 0 0 auto;
  }

  .text-info {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    flex: 1 1 auto;
    font-size: var(--text-xs);
  }

  .pasted-label {
    color: var(--accent-text);
    font-weight: 600;
    flex: 0 0 auto;
  }

  .text-content {
    color: var(--text-primary);
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .pasted-remove {
    width: 18px;
    height: 18px;
    border-radius: 4px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--text-secondary);
    flex: 0 0 auto;
    font-size: 13px;
    line-height: 1;
  }

  .pasted-remove:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .plugin-tag {
    display: inline-flex;
    align-items: center;
    border-radius: 18px;
    background: var(--control-bg);
    border: 1px solid var(--border);
    flex: 0 0 auto;
    max-width: 280px;
    height: 34px;
    overflow: hidden;
    padding-right: 4px;
  }

  .plugin-tag-left {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 0 14px 0 10px;
    height: 100%;
    background: var(--bg-hover);
    clip-path: polygon(0 0, 100% 0, calc(100% - 8px) 100%, 0 100%);
    min-width: 0;
  }

  .plugin-tag-glyph {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--accent-text);
    flex: 0 0 auto;
  }

  .plugin-tag-title {
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .plugin-tag-cmd {
    font-size: var(--text-xs);
    font-weight: 600;
    color: var(--text-secondary);
    padding: 0 6px 0 4px;
    max-width: 110px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .plugin-tag-close {
    width: 20px;
    height: 20px;
    border-radius: 50%;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--text-secondary);
    flex: 0 0 auto;
    font-size: 14px;
    line-height: 1;
  }

  .plugin-tag-close:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .search-input {
    flex: 1 1 160px;
    min-width: 120px;
    border: none;
    outline: none;
    background: transparent;
    color: var(--text-primary);
    font-size: 21px;
    line-height: 1.3;
    height: 40px;
  }

  .search-input:focus-visible {
    outline: none;
  }

  .search-input::placeholder {
    color: var(--text-tertiary);
    opacity: 1;
    font-weight: 400;
  }

  .search-actions {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    flex: 0 0 auto;
  }

  .tab-target-hint {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 3px 6px;
    border-radius: 6px;
    color: var(--text-tertiary);
    white-space: nowrap;
  }

  .tab-target-hint:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .tab-target-text {
    font-size: var(--text-xs);
    font-weight: 500;
  }

  .tab-target-key {
    font-size: 11px;
    font-weight: 600;
    color: inherit;
    background: var(--control-bg);
    border: 1px solid var(--border-strong);
    border-radius: 4px;
    padding: 1px 5px;
    line-height: 1.25;
  }

  .avatar-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 36px;
    height: 36px;
    border-radius: 50%;
    flex: 0 0 36px;
  }

  .avatar-btn:hover {
    transform: scale(0.96);
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

  .dropdown-panel {
    max-height: 536px;
    overflow-y: auto;
    overflow-x: hidden;
    border-top: 1px solid var(--border);
    padding: 6px 8px 8px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    border-radius: 0 0 var(--radius-lg) var(--radius-lg);
  }

  .status-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
    padding: 6px 10px;
    border-radius: var(--radius-sm);
    font-size: var(--text-xs);
  }

  .status-banner.success {
    background: var(--success-subtle);
    color: var(--success);
  }

  .status-banner.info {
    background: var(--accent-subtle);
    color: var(--accent-text);
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

  .vertical-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .list-item {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    padding: 8px 12px;
    border-radius: 6px;
    text-align: left;
    min-width: 0;
  }

  .list-item:hover {
    background: var(--bg-hover);
  }

  .list-item.selected {
    background: var(--bg-selected);
  }

  .item-icon {
    position: relative;
    flex: 0 0 32px;
    width: 32px;
    height: 32px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
    border-radius: 7px;
  }

  .item-icon img {
    width: 32px;
    height: 32px;
    object-fit: contain;
    image-rendering: auto;
    border-radius: 7px;
  }

  .squircle-icon,
  .app-icon-placeholder {
    width: 32px;
    height: 32px;
    border-radius: 8px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: #ffffff;
    font-size: 13px;
    font-weight: 700;
    letter-spacing: -0.02em;
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.18);
  }

  .item-content {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .item-title {
    font-size: 14px;
    font-weight: 500;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .item-title mark.highlight {
    background-color: transparent;
    color: var(--highlight);
    font-weight: 600;
  }

  .list-item.selected .item-title mark.highlight {
    color: var(--highlight-selected);
  }

  .item-subtitle {
    font-size: 12px;
    color: var(--text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .hit-meta {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex: 0 0 auto;
  }

  .item-badge {
    flex-shrink: 0;
    padding: 2px 8px;
    border-radius: 4px;
    font-size: 11px;
    font-weight: 500;
    color: var(--text-secondary);
    background: var(--control-bg);
  }

  .shortcut-hint {
    background: var(--control-bg);
    border: 1px solid var(--border-strong);
    border-radius: 4px;
    padding: 1px 5px;
    font-size: 11px;
    color: var(--text-secondary);
  }

  .meta {
    font-size: var(--text-xs);
    color: var(--text-tertiary);
  }

  .main-push-section,
  .home-section {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .section-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 2px 10px;
    min-height: 24px;
  }

  .section-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--text-secondary);
    line-height: 20px;
  }

  .expand-btn-text {
    font-size: 12px;
    font-weight: 500;
    color: var(--text-tertiary);
    padding: 2px 6px;
    border-radius: 4px;
  }

  .expand-btn-text:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .push-list {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .push-item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 6px 10px;
    border-radius: 6px;
    text-align: left;
    min-width: 0;
  }

  .push-item:hover {
    background: var(--bg-hover);
  }

  .push-icon {
    width: 22px;
    height: 22px;
    border-radius: 6px;
    background: var(--accent-subtle);
    color: var(--accent-text);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: 0 0 22px;
  }

  .push-content {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
  }

  .push-title {
    font-size: 13px;
    font-weight: 500;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .app-grid {
    display: grid;
    grid-template-columns: repeat(9, minmax(0, 1fr));
    gap: 2px;
    padding: 0 2px;
  }

  .app-item {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 8px 4px;
    border-radius: 8px;
    width: 100%;
    min-width: 0;
    overflow: hidden;
  }

  .app-item:hover {
    background: var(--bg-hover);
  }

  .app-item.selected {
    background: var(--bg-selected);
    box-shadow: inset 0 0 0 1px var(--accent);
  }

  .app-icon-wrap {
    width: 32px;
    height: 32px;
    margin-bottom: 6px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: 0 0 32px;
  }

  .app-icon {
    width: 32px;
    height: 32px;
    border-radius: 7px;
    object-fit: contain;
    image-rendering: auto;
  }

  .app-name {
    font-size: 12px;
    font-weight: 500;
    line-height: 16px;
    height: 32px;
    color: var(--text-primary);
    text-align: center;
    width: 100%;
    padding: 0 3px;
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    overflow: hidden;
    word-break: break-word;
  }

  .empty-search {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    padding: 10px 12px;
    font-size: var(--text-sm);
    color: var(--text-secondary);
  }

  .inline-promote-btn {
    padding: var(--space-1) var(--space-3);
    border-radius: var(--radius-sm);
    background: var(--accent-subtle);
    color: var(--accent-text);
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
    color: var(--accent-text);
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
    max-height: 240px;
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
