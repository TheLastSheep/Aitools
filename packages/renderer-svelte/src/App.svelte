<script lang="ts">
  import { onMount, tick } from "svelte";
  import AToolsMark from "./components/AToolsMark.svelte";
  import DshPanel from "./components/DshPanel.svelte";
  import {
    setMainWindowHeight,
    type AiProfile,
    type LauncherHit,
    type PastedFileItem,
  } from "./lib/ipc";
  import {
    buildAggregateNavigationGrid,
    buildListModeResults,
    chipShortLabel,
    getSectionSelectedIndex,
    getVisibleSectionItems,
    highlightSegments,
    hitIdentityKey,
    hitKindBadge,
    partitionSearchHits,
    shouldInvertIcon,
    squircleStyle,
    truncateMiddle,
    validIconSrc,
    type NavigationGridRow,
  } from "./lib/launcherUi";
  import { createLauncherStore, isTier2Risk } from "./stores/launcher.svelte";

  type ContextMenuSource = "history" | "pinned" | "featured" | "search" | "recommendation" | "list-header";

  interface ContextMenuItem {
    id: string;
    label: string;
    shortcut?: string;
    danger?: boolean;
    disabled?: boolean;
    action: () => void | Promise<void>;
  }

  interface ContextMenuState {
    x: number;
    y: number;
    rawX: number;
    rawY: number;
    items: ContextMenuItem[];
    activeIndex: number;
  }

  const store = createLauncherStore();

  let shellEl: HTMLElement | undefined = $state();
  let searchBarEl: HTMLElement | undefined = $state();
  let searchActionsEl: HTMLElement | undefined = $state();
  let dropdownEl: HTMLElement | undefined = $state();
  let searchInput: HTMLInputElement | undefined = $state();
  let measureEl: HTMLSpanElement | undefined = $state();
  let menuEl: HTMLElement | undefined = $state();

  let isComposing = $state(false);
  let composingText = $state("");
  let inputWidthPx = $state(2);
  let brokenIcons = $state<Record<string, boolean>>({});
  let lastReportedHeight = 0;

  let isDraggingFilesOver = $state(false);
  let fileDragCounter = 0;

  let draggingPinnedIndex = $state<number | null>(null);
  let dragOverPinnedIndex = $state<number | null>(null);

  let contextMenu = $state<ContextMenuState | null>(null);

  const activeInputValue = $derived(store.mode === "list" ? store.subInput : store.query);

  const hasSearchContent = $derived(
    Boolean(
      store.query.trim() ||
        store.pastedText ||
        (store.pastedFiles && store.pastedFiles.length > 0),
    ),
  );

  const hasAnyChip = $derived(
    store.chips.length > 0 ||
      Boolean(store.pastedText) ||
      Boolean(store.pastedFiles && store.pastedFiles.length > 0),
  );

  const currentPlaceholder = $derived.by(() => {
    if (store.mode === "list" && store.listSession) {
      return store.listSession.placeholder || "过滤插件列表，Esc 返回...";
    }
    if (store.mode === "agent") {
      return "向 AI Agent 提问，Enter 发送...";
    }
    if (hasAnyChip) {
      return "搜索";
    }
    return "搜索应用、插件或输入指令，Tab 切换 AI...";
  });

  const partitionedSearch = $derived(partitionSearchHits(store.hits));
  const bestSearchResults = $derived(partitionedSearch.bestSearchResults);
  const bestMatches = $derived(partitionedSearch.bestMatches);
  const recommendations = $derived(partitionedSearch.recommendations);
  const listModeResults = $derived(
    hasSearchContent ? buildListModeResults(store.hits, store.query) : [],
  );

  const visibleRecentItems = $derived(
    getVisibleSectionItems(store.recentItems, store.recentExpanded, 2, 9),
  );
  const visiblePinnedItems = $derived(
    getVisibleSectionItems(store.pinnedItems, store.pinnedExpanded, 2, 9),
  );
  const visibleFeaturedItems = $derived(
    getVisibleSectionItems(store.featuredItems, store.homeExpanded, 2, 9),
  );

  const visibleBestSearch = $derived(
    getVisibleSectionItems(bestSearchResults, store.searchResultsExpanded, 2, 9),
  );
  const visibleBestMatches = $derived(
    getVisibleSectionItems(bestMatches, store.bestMatchesExpanded, 2, 9),
  );
  const visibleRecommendations = $derived(
    getVisibleSectionItems(recommendations, store.recommendationsExpanded, 2, 9),
  );

  const visibleAggregateSearchItems = $derived<LauncherHit[]>([
    ...visibleBestSearch,
    ...visibleBestMatches,
    ...visibleRecommendations,
  ]);

  const flatSearchHits = $derived(
    store.searchViewMode === "list" ? listModeResults : visibleAggregateSearchItems,
  );

  const navigationGrid = $derived.by<NavigationGridRow[]>(() => {
    if (store.mode !== "search") return [];
    if (hasSearchContent && store.searchViewMode === "list") {
      return listModeResults.map((hit) => ({
        type: "listItem",
        items: [{ kind: "hit", hit }],
      }));
    }
    return buildAggregateNavigationGrid({
      hasSearchContent,
      suggestions: store.suggestions,
      recentItems: store.recentItems,
      pinnedItems: store.pinnedItems,
      featuredItems: store.featuredItems,
      bestSearchResults,
      bestMatches,
      recommendations,
      recentExpanded: store.recentExpanded,
      pinnedExpanded: store.pinnedExpanded,
      featuredExpanded: store.homeExpanded,
      searchResultsExpanded: store.searchResultsExpanded,
      bestMatchesExpanded: store.bestMatchesExpanded,
      recommendationsExpanded: store.recommendationsExpanded,
    });
  });

  const selectedGridItem = $derived.by(() => {
    const row = navigationGrid[store.selectedRow];
    if (!row) return null;
    return row.items[store.selectedCol] ?? null;
  });

  const activeSearchFlatIndex = $derived.by(() => {
    if (store.mode !== "search" || !hasSearchContent) return -1;
    if (store.searchViewMode === "list") {
      return navigationGrid.length > 0 ? store.selectedRow : -1;
    }
    const row = navigationGrid[store.selectedRow];
    if (!row) return -1;
    if (row.type === "bestSearch") {
      return getSectionSelectedIndex(navigationGrid, store.selectedRow, store.selectedCol, "bestSearch");
    }
    if (row.type === "bestMatch") {
      return (
        visibleBestSearch.length +
        getSectionSelectedIndex(navigationGrid, store.selectedRow, store.selectedCol, "bestMatch")
      );
    }
    if (row.type === "recommendation") {
      return (
        visibleBestSearch.length +
        visibleBestMatches.length +
        getSectionSelectedIndex(
          navigationGrid,
          store.selectedRow,
          store.selectedCol,
          "recommendation",
        )
      );
    }
    return -1;
  });

  const showHomeGrid = $derived(
    store.mode === "search" &&
      !hasSearchContent &&
      !store.searchError &&
      (store.hasWoken || store.suggestions.length > 0 || store.chips.length > 0),
  );

  const isIdleCompact = $derived(
    !store.dshStandalone &&
      store.mode === "search" &&
      !hasSearchContent &&
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
        flatSearchHits.length > 0 ||
        (hasSearchContent && !store.searchLoading) ||
        store.suggestions.length > 0 ||
        (showHomeGrid &&
          (store.recentItems.length > 0 ||
            store.pinnedItems.length > 0 ||
            store.featuredItems.length > 0))),
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

  const ariaExpandedState = $derived(
    store.mode === "list"
      ? (store.listSession?.items.length ?? 0) > 0
      : store.mode === "search" && hasSearchContent && flatSearchHits.length > 0,
  );

  const ariaActivedescendantId = $derived.by(() => {
    if (store.mode === "list" && (store.listSession?.items.length ?? 0) > 0) {
      return `list-option-${store.listSelectedIndex}`;
    }
    if (store.mode === "search" && hasSearchContent && activeSearchFlatIndex >= 0) {
      return `hit-option-${activeSearchFlatIndex}`;
    }
    return undefined;
  });

  function updateInputWidth() {
    if (!measureEl || !searchBarEl || !searchActionsEl) return;
    const text = isComposing && composingText ? composingText : activeInputValue;
    const minWidth = 2;
    if (!text) {
      inputWidthPx = minWidth;
      return;
    }
    measureEl.textContent = text;
    const measured = measureEl.offsetWidth + 10;
    const barWidth = searchBarEl.offsetWidth || 800;
    const actionsWidth = searchActionsEl.offsetWidth || 100;
    const siblingsWidth = (searchInput?.parentElement?.offsetLeft ?? 12) + 24;
    const maxWidth = Math.max(80, barWidth - actionsWidth - siblingsWidth);
    inputWidthPx = Math.max(minWidth, Math.min(measured, maxWidth));
  }

  function resetRootScroll() {
    if (typeof document === "undefined") return;
    if (document.documentElement.scrollTop !== 0) {
      document.documentElement.scrollTop = 0;
    }
    if (document.body && document.body.scrollTop !== 0) {
      document.body.scrollTop = 0;
    }
  }

  function focusSearchInput() {
    searchInput?.focus({ preventScroll: true });
    resetRootScroll();
  }

  function scrollIntoDropdownContainer(targetId: string) {
    if (typeof document === "undefined" || !dropdownEl) return;
    const el = document.getElementById(targetId);
    if (!el) return;
    const containerRect = dropdownEl.getBoundingClientRect();
    const targetRect = el.getBoundingClientRect();
    if (targetRect.top < containerRect.top) {
      dropdownEl.scrollTop = Math.max(0, dropdownEl.scrollTop + (targetRect.top - containerRect.top));
    } else if (targetRect.bottom > containerRect.bottom) {
      dropdownEl.scrollTop += targetRect.bottom - containerRect.bottom;
    }
  }

  function syncWindowSize() {
    if (store.dshStandalone) return;
    resetRootScroll();
    if (!dropdownOpen && !contextMenu) {
      if (lastReportedHeight !== 58) {
        lastReportedHeight = 58;
        void setMainWindowHeight(58);
      }
      return;
    }
    const dropdownH = dropdownOpen && dropdownEl ? Math.ceil(dropdownEl.scrollHeight) + 2 : 0;
    const menuH = menuEl ? Math.ceil(menuEl.offsetHeight) : contextMenu ? contextMenu.items.length * 32 + 12 : 0;
    const menuBottom = contextMenu ? contextMenu.y + menuH + 10 : 0;
    const baseHeight = 58 + Math.min(536, dropdownH);
    const measured = Math.max(58, Math.min(594, Math.max(baseHeight, Math.ceil(menuBottom))));
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
      focusSearchInput();
    }

    updateInputWidth();
    syncWindowSize();

    const handleGlobalPointerDown = (e: PointerEvent) => {
      if (contextMenu && menuEl && !menuEl.contains(e.target as Node)) {
        closeContextMenu();
      }
    };
    window.addEventListener("pointerdown", handleGlobalPointerDown);

    return () => {
      window.removeEventListener("pointerdown", handleGlobalPointerDown);
      cleanupBridge?.();
    };
  });

  $effect(() => {
    if (typeof ResizeObserver === "undefined") return;
    const targets = [shellEl, dropdownEl, searchBarEl].filter((el): el is HTMLElement => Boolean(el));
    if (targets.length === 0) return;
    const observer = new ResizeObserver(() => {
      updateInputWidth();
      syncWindowSize();
    });
    for (const el of targets) {
      observer.observe(el);
    }
    return () => observer.disconnect();
  });

  $effect(() => {
    void activeInputValue;
    void composingText;
    void isComposing;
    void store.mode;
    void hasAnyChip;
    void tick().then(() => {
      updateInputWidth();
    });
  });

  $effect(() => {
    void navigationGrid;
    store.clampSelection(navigationGrid);
  });

  function computeContextMenuCoords(rawX: number, rawY: number, mw: number, mh: number): { x: number; y: number } {
    const x = Math.max(8, Math.min(Math.round(rawX), 800 - mw - 8));
    const panelH = shellEl?.getBoundingClientRect().height ?? 0;
    if (panelH >= mh + 16) {
      if (rawY + mh + 8 <= panelH) {
        return { x, y: Math.max(8, Math.round(rawY)) };
      }
      if (rawY - mh >= 8) {
        return { x, y: Math.round(rawY - mh) };
      }
      return { x, y: Math.max(8, Math.round(panelH - mh - 8)) };
    }
    return { x, y: Math.max(8, Math.min(Math.round(rawY), 594 - mh - 8)) };
  }

  $effect(() => {
    void isIdleCompact;
    void dropdownOpen;
    void store.mode;
    void store.searchViewMode;
    void store.hasWoken;
    void store.hits.length;
    void store.recentItems.length;
    void store.pinnedItems.length;
    void store.featuredItems.length;
    void store.recentExpanded;
    void store.pinnedExpanded;
    void store.homeExpanded;
    void store.searchResultsExpanded;
    void store.bestMatchesExpanded;
    void store.recommendationsExpanded;
    void store.suggestions.length;
    void store.chips.length;
    void store.pastedText;
    void store.pastedFiles;
    void store.dshOpen;
    void store.statusBanner;
    void store.searchError;
    void store.searchLoading;
    void store.agentResponse;
    void store.agentError;
    void store.dagEvents.length;
    void store.pendingHitl;
    void store.listSession?.items.length;
    void contextMenu;

    void tick().then(() => {
      if (contextMenu && menuEl) {
        const mw = menuEl.offsetWidth || 220;
        const mh = menuEl.offsetHeight || 160;
        const pos = computeContextMenuCoords(contextMenu.rawX, contextMenu.rawY, mw, mh);
        if (pos.x !== contextMenu.x || pos.y !== contextMenu.y) {
          contextMenu = { ...contextMenu, x: pos.x, y: pos.y };
        }
      }
      syncWindowSize();
    });
  });

  let prevModeForFocus = "search";
  $effect(() => {
    const enteredListMode = store.mode === "list" && prevModeForFocus !== "list";
    prevModeForFocus = store.mode;
    if (enteredListMode) {
      void tick().then(() => {
        focusSearchInput();
        updateInputWidth();
      });
    }
  });

  $effect(() => {
    if (typeof document === "undefined") return;
    if (store.mode === "list") {
      scrollIntoDropdownContainer(`list-option-${store.listSelectedIndex}`);
      return;
    }
    if (store.mode === "search") {
      if (hasSearchContent && activeSearchFlatIndex >= 0) {
        scrollIntoDropdownContainer(`hit-option-${activeSearchFlatIndex}`);
      } else {
        scrollIntoDropdownContainer(`home-option-${store.selectedRow}-${store.selectedCol}`);
      }
    }
  });

  function eventIsComposing(e: Event): boolean {
    return "isComposing" in e && Boolean((e as { isComposing?: boolean }).isComposing);
  }

  function closeContextMenu() {
    contextMenu = null;
    resetRootScroll();
  }

  function openContextMenuAt(x: number, y: number, items: ContextMenuItem[]) {
    if (items.length === 0) return;
    const firstEnabled = items.findIndex((i) => !i.disabled);
    const estW = 228;
    const estH = items.length * 34 + 12;
    const pos = computeContextMenuCoords(x, y, estW, estH);
    contextMenu = {
      x: pos.x,
      y: pos.y,
      rawX: x,
      rawY: y,
      items,
      activeIndex: firstEnabled >= 0 ? firstEnabled : 0,
    };
    resetRootScroll();
    void tick().then(() => {
      syncWindowSize();
      menuEl?.focus({ preventScroll: true });
      resetRootScroll();
    });
  }

  function isLocalTarget(hit: LauncherHit): boolean {
    if (hit.target_path && hit.target_path.trim().length > 0) return true;
    return hit.kind === "app";
  }

  function openHitContextMenu(e: MouseEvent, hit: LauncherHit, source: ContextMenuSource) {
    e.preventDefault();
    e.stopPropagation();

    const items: ContextMenuItem[] = [];

    if (source === "history") {
      items.push({
        id: "remove-recent",
        label: "从使用记录删除",
        danger: true,
        action: async () => {
          await store.mutateState({
            op: "remove_recent",
            plugin_id: hit.plugin_id,
            feature_code: hit.feature_code,
          });
        },
      });
    }

    if (isLocalTarget(hit)) {
      items.push({
        id: "reveal-in-finder",
        label: "打开文件位置",
        action: async () => {
          const ok = await store.revealItem(hit);
          if (!ok) {
            store.statusBanner = {
              kind: "info",
              text: hit.target_path
                ? `Path: ${hit.target_path}`
                : "文件位置仅在桌面应用可用",
            };
          }
        },
      });
    }

    if (source === "recommendation") {
      const recIndex = recommendations.findIndex(
        (r) => hitIdentityKey(r) === hitIdentityKey(hit),
      );
      if (hit.is_recommendation_pinned) {
        items.push({
          id: "unpin-recommendation",
          label: "取消置顶",
          action: async () => {
            await store.mutateState({
              op: "unpin_recommendation",
              plugin_id: hit.plugin_id,
              feature_code: hit.feature_code,
            });
          },
        });
        items.push({
          id: "move-recommendation-to-front",
          label: "排到最前",
          disabled: recIndex <= 0,
          action: async () => {
            await store.mutateState({
              op: "move_recommendation_to_front",
              plugin_id: hit.plugin_id,
              feature_code: hit.feature_code,
            });
          },
        });
      } else {
        items.push({
          id: "pin-recommendation",
          label: "置顶",
          action: async () => {
            await store.mutateState({
              op: "pin_recommendation",
              plugin_id: hit.plugin_id,
              feature_code: hit.feature_code,
            });
          },
        });
      }
    } else {
      const pinned = store.isPinnedHit(hit);
      items.push({
        id: pinned ? "unpin" : "pin",
        label: pinned ? "从搜索框取消固定" : "固定到搜索框",
        action: async () => {
          await store.mutateState({
            op: pinned ? "unpin" : "pin",
            plugin_id: hit.plugin_id,
            feature_code: hit.feature_code,
          });
        },
      });
    }

    items.push({
      id: "toggle-view-mode",
      label: store.searchViewMode === "aggregate" ? "切换为列表视图" : "切换为聚合视图",
      shortcut: "⌘⇧L",
      action: () => {
        store.toggleSearchViewMode();
      },
    });

    items.push({
      id: "toggle-dsh",
      label: store.dshOpen ? "关闭诊断面板" : "打开诊断面板",
      shortcut: "⌘⇧D",
      action: async () => {
        await store.toggleDsh();
      },
    });

    openContextMenuAt(e.clientX, e.clientY, items);
  }

  function openListHeaderMenu(e: MouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    openContextMenuAt(e.clientX - 180, e.clientY + 18, [
      {
        id: "exit-list",
        label: "返回搜索",
        shortcut: "Esc",
        action: () => {
          store.exitListToSearch();
          focusSearchInput();
        },
      },
      {
        id: "toggle-dsh",
        label: store.dshOpen ? "关闭诊断面板" : "打开诊断面板",
        shortcut: "⌘⇧D",
        action: async () => {
          await store.toggleDsh();
        },
      },
    ]);
  }

  async function executeMenuItem(item: ContextMenuItem) {
    if (item.disabled) return;
    closeContextMenu();
    await item.action();
    focusSearchInput();
  }

  function stepMenuIndex(delta: number) {
    if (!contextMenu) return;
    const len = contextMenu.items.length;
    for (let step = 1; step <= len; step++) {
      const idx = (contextMenu.activeIndex + delta * step + len) % len;
      if (!contextMenu.items[idx].disabled) {
        contextMenu = { ...contextMenu, activeIndex: idx };
        return;
      }
    }
  }

  async function activateSelectedGridItem() {
    const item = selectedGridItem;
    if (item) {
      if (item.kind === "suggestion") {
        await store.submitAgentTurn(item.action.prompt_template);
      } else {
        await store.launchHit(item.hit);
      }
      return;
    }
    await store.handleFallbackSearchSubmit();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (store.dshStandalone) return;
    if (isComposing || e.isComposing || e.keyCode === 229) return;

    if (contextMenu) {
      if (e.key === "Escape") {
        e.preventDefault();
        closeContextMenu();
        focusSearchInput();
        return;
      }
      if (e.key === "ArrowDown") {
        e.preventDefault();
        stepMenuIndex(1);
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        stepMenuIndex(-1);
        return;
      }
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        const item = contextMenu.items[contextMenu.activeIndex];
        if (item && !item.disabled) {
          void executeMenuItem(item);
        }
        return;
      }
    }

    if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === "d") {
      e.preventDefault();
      void store.toggleDsh();
      return;
    }

    if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === "l") {
      e.preventDefault();
      store.toggleSearchViewMode();
      focusSearchInput();
      return;
    }

    if (
      (e.metaKey || e.ctrlKey) &&
      !e.shiftKey &&
      !e.altKey &&
      e.key.toLowerCase() === "f" &&
      store.mode === "search" &&
      store.query.trim().length > 0
    ) {
      e.preventDefault();
      void store.convertQueryToFilterChip();
      return;
    }

    if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.key >= "1" && e.key <= "9") {
      const slot = Number.parseInt(e.key, 10) - 1;
      if (store.mode === "search" && hasSearchContent && slot < flatSearchHits.length) {
        e.preventDefault();
        void store.launchHit(flatSearchHits[slot]);
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

    const isPlainDeleteOrBackspace =
      (e.key === "Backspace" || e.key === "Delete") &&
      !e.metaKey &&
      !e.ctrlKey &&
      !e.altKey &&
      !e.shiftKey;

    if (isPlainDeleteOrBackspace && document.activeElement === searchInput) {
      if (store.mode === "list" && e.key === "Backspace" && !store.subInput) {
        e.preventDefault();
        store.exitListToSearch();
        return;
      }
      if (store.mode !== "list" && !store.query && hasAnyChip) {
        e.preventDefault();
        void store.popLastChipToQuery().then((res) => {
          if (res.restoredText !== null) {
            void tick().then(() => {
              focusSearchInput();
              searchInput?.select();
            });
          }
        });
        return;
      }
    }

    if (e.key === "ArrowDown") {
      if (store.mode === "list" && (store.listSession?.items.length ?? 0) > 0) {
        e.preventDefault();
        store.moveListSelection(1);
        return;
      }
      if (store.mode === "search" && navigationGrid.length > 0) {
        e.preventDefault();
        const navMode = hasSearchContent && store.searchViewMode === "list" ? "list" : "aggregate";
        store.moveGridSelection(navigationGrid, "ArrowDown", navMode);
        return;
      }
    }

    if (e.key === "ArrowUp") {
      if (store.mode === "list" && (store.listSession?.items.length ?? 0) > 0) {
        e.preventDefault();
        store.moveListSelection(-1);
        return;
      }
      if (store.mode === "search" && navigationGrid.length > 0) {
        e.preventDefault();
        const navMode = hasSearchContent && store.searchViewMode === "list" ? "list" : "aggregate";
        store.moveGridSelection(navigationGrid, "ArrowUp", navMode);
        return;
      }
    }

    if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
      const isAggregateView =
        store.mode === "search" && (!hasSearchContent || store.searchViewMode === "aggregate");
      const hasInputSelection =
        searchInput &&
        searchInput.selectionStart !== null &&
        searchInput.selectionEnd !== null &&
        searchInput.selectionStart !== searchInput.selectionEnd;

      if (isAggregateView && navigationGrid.length > 0 && !hasInputSelection) {
        e.preventDefault();
        store.moveGridSelection(navigationGrid, e.key, "aggregate");
        return;
      }
    }

    if (e.key === "Enter") {
      const active = document.activeElement;
      const inLauncherInput = active === searchInput || active === document.body;
      if (inLauncherInput && !e.shiftKey) {
        e.preventDefault();
        if (store.mode === "search") {
          void activateSelectedGridItem();
        } else {
          void store.handleFallbackSearchSubmit();
        }
        return;
      }
    }

    if (e.key === "Escape") {
      e.preventDefault();
      void store.handleEscape().then(() => {
        if (store.mode === "search") {
          focusSearchInput();
        }
      });
    }
  }

  function handlePaste(e: ClipboardEvent) {
    if (store.mode === "list" || isComposing) return;
    const files = e.clipboardData?.files;
    if (files && files.length > 0) {
      e.preventDefault();
      const items: PastedFileItem[] = Array.from(files).map((f) => ({
        name: f.name,
        path: (f as File & { path?: string }).path || f.name,
        isDirectory: false,
      }));
      void store.setPastedFiles(items);
      return;
    }

    const text = e.clipboardData?.getData("text/plain") ?? "";
    if (!text) return;

    const hasInputText = Boolean(store.query && store.query.trim().length > 0);
    const hasSelection =
      searchInput &&
      searchInput.selectionStart !== null &&
      searchInput.selectionStart !== searchInput.selectionEnd;

    if (!hasInputText && !hasSelection) {
      e.preventDefault();
      void store.setPastedText(text);
    }
  }

  function hasFileTransfer(e: DragEvent): boolean {
    if (draggingPinnedIndex !== null) return false;
    const types = e.dataTransfer?.types;
    if (!types) return false;
    return Array.from(types).includes("Files");
  }

  function handleFileDragEnter(e: DragEvent) {
    if (!hasFileTransfer(e)) return;
    e.preventDefault();
    e.stopPropagation();
    fileDragCounter++;
    if (fileDragCounter === 1) {
      isDraggingFilesOver = true;
    }
  }

  function handleFileDragOver(e: DragEvent) {
    if (!hasFileTransfer(e)) return;
    e.preventDefault();
    e.stopPropagation();
  }

  function handleFileDragLeave(e: DragEvent) {
    if (!hasFileTransfer(e) && !isDraggingFilesOver) return;
    e.preventDefault();
    e.stopPropagation();
    fileDragCounter = Math.max(0, fileDragCounter - 1);
    if (fileDragCounter === 0) {
      isDraggingFilesOver = false;
    }
  }

  function handleFileDrop(e: DragEvent) {
    if (draggingPinnedIndex !== null) return;
    if (!hasFileTransfer(e)) return;
    e.preventDefault();
    e.stopPropagation();
    isDraggingFilesOver = false;
    fileDragCounter = 0;

    const files = e.dataTransfer?.files;
    if (!files || files.length === 0) return;

    const items: PastedFileItem[] = Array.from(files).map((f) => ({
      name: f.name,
      path: (f as File & { path?: string }).path || f.name,
      isDirectory: f.type === "" && !f.name.includes("."),
    }));
    void store.setPastedFiles(items);
    focusSearchInput();
  }

  function handlePinnedDragStart(e: DragEvent, index: number) {
    draggingPinnedIndex = index;
    dragOverPinnedIndex = index;
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("application/x-aitools-pinned", String(index));
      e.dataTransfer.setData("text/plain", String(index));
    }
  }

  function handlePinnedDragOver(e: DragEvent, index: number) {
    if (draggingPinnedIndex === null) return;
    e.preventDefault();
    if (e.dataTransfer) {
      e.dataTransfer.dropEffect = "move";
    }
    dragOverPinnedIndex = index;
  }

  function handlePinnedDrop(e: DragEvent, targetIndex: number) {
    if (draggingPinnedIndex === null) return;
    e.preventDefault();
    e.stopPropagation();
    const fromIndex = draggingPinnedIndex;
    draggingPinnedIndex = null;
    dragOverPinnedIndex = null;
    if (fromIndex === targetIndex) return;

    const visible = [...visiblePinnedItems];
    if (fromIndex < 0 || fromIndex >= visible.length || targetIndex < 0 || targetIndex >= visible.length) {
      return;
    }
    const [moved] = visible.splice(fromIndex, 1);
    visible.splice(targetIndex, 0, moved);
    const hiddenTail = store.pinnedExpanded ? [] : store.pinnedItems.slice(visible.length);
    void store.reorderPinned([...visible, ...hiddenTail]);
  }

  function handlePinnedDragEnd() {
    draggingPinnedIndex = null;
    dragOverPinnedIndex = null;
  }

  function handleSearchBarPointerUp(e: MouseEvent) {
    const target = e.target as HTMLElement | null;
    if (!target) return;
    if (target.closest("input, button, .pasted-text, .pasted-files, .plugin-tag, .search-actions")) {
      return;
    }
    focusSearchInput();
  }

  function handleDropdownClick(e: MouseEvent) {
    const target = e.target as HTMLElement | null;
    if (!target) return;
    if (
      target.closest(
        "button, input, select, a, [role='menu'], [role='menuitem'], .app-item, .list-item, .push-item, .dsh-panel, .hitl-card",
      )
    ) {
      return;
    }
    focusSearchInput();
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

  function itemTitleTooltip(hit: LauncherHit): string {
    if (hit.description && hit.description !== hit.label) {
      return `${hit.label}\n${hit.description}`;
    }
    return hit.label || hit.plugin_name || hit.feature_code;
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
    searchViewMode={store.searchViewMode}
    onChangeTheme={(next) => store.setThemePreference(next)}
    onChangeSearchViewMode={(next) => store.setSearchViewMode(next)}
    onRefresh={() => void store.refreshDshLogs()}
    onToggleFault={(k) => void store.updateFault(k)}
    onSyncPlugins={() => void store.syncZtoolsPlugins()}
  />
{:else}
  <main
    bind:this={shellEl}
    class="shell"
    class:compact={!dropdownOpen}
    ondragenter={handleFileDragEnter}
    ondragover={handleFileDragOver}
    ondragleave={handleFileDragLeave}
    ondrop={handleFileDrop}
  >
    {#if isDraggingFilesOver}
      <div class="drag-overlay" aria-hidden="true"></div>
    {/if}

    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div
      bind:this={searchBarEl}
      class="search-bar"
      data-tauri-drag-region
      onclick={handleSearchBarPointerUp}
    >
      <div class="search-input-container" data-tauri-drag-region>
        {#if store.mode === "list" && store.listSession}
          {@const listPluginIcon = validIconSrc(
            store.listSession.plugin_icon,
            brokenIcons,
            `list-plugin:${store.listSession.plugin_id}`,
          )}
          <div class="plugin-tag has-cmd">
            <div class="plugin-tag-left">
              {#if listPluginIcon}
                <img
                  class="plugin-tag-icon"
                  class:adaptive-invert={shouldInvertIcon(
                    store.listSession.plugin_icon_is_dark,
                    store.resolvedTheme,
                  )}
                  src={listPluginIcon}
                  alt=""
                  draggable="false"
                  onerror={() => markIconBroken(`list-plugin:${store.listSession?.plugin_id ?? ""}`)}
                />
              {:else}
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
              {/if}
              <span class="plugin-tag-title">{store.listSession.plugin_name}</span>
            </div>
            <span class="plugin-tag-cmd">{store.listSession.feature_code}</span>
            <button
              type="button"
              class="plugin-tag-close"
              title="返回搜索"
              aria-label="退出列表模式"
              onclick={() => {
                store.exitListToSearch();
                focusSearchInput();
              }}
            >
              <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
                <path
                  d="M1.5 1.5L8.5 8.5M8.5 1.5L1.5 8.5"
                  stroke="currentColor"
                  stroke-width="1.5"
                  stroke-linecap="round"
                />
              </svg>
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
              title="返回搜索"
              aria-label="退出 AI 模式"
              onclick={() => {
                store.exitAgentToSearch();
                focusSearchInput();
              }}
            >
              <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
                <path
                  d="M1.5 1.5L8.5 8.5M8.5 1.5L1.5 8.5"
                  stroke="currentColor"
                  stroke-width="1.5"
                  stroke-linecap="round"
                />
              </svg>
            </button>
          </div>
        {/if}

        {#if store.mode !== "list"}
          {#if store.pastedFiles && store.pastedFiles.length > 0}
            <div class="pasted-files" title={store.pastedFiles.map((f) => f.path).join("\n")}>
              <span class="file-icon" aria-hidden="true">
                <svg width="15" height="15" viewBox="0 0 24 24" fill="none">
                  <path
                    d="M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9l-7-7Z"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                  />
                  <path
                    d="M13 2v7h7"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                  />
                </svg>
              </span>
              <span class="file-info">
                <span class="file-name">{truncateMiddle(store.pastedFiles[0].name, 20)}</span>
                {#if store.pastedFiles.length > 1}
                  <span class="file-count">{store.pastedFiles.length}</span>
                {/if}
              </span>
              <button
                type="button"
                class="pasted-remove"
                aria-label="清除文件"
                onclick={() => {
                  void store.clearPastedFiles();
                  focusSearchInput();
                }}
              >
                ×
              </button>
            </div>
          {/if}

          {#if store.pastedText}
            <div class="pasted-text" title={store.pastedText}>
              <button
                type="button"
                class="pasted-chip-body"
                onclick={() => {
                  void store.editPastedTextChip().then(() => {
                    focusSearchInput();
                  });
                }}
              >
                <span class="text-icon" aria-hidden="true">
                  <svg width="14" height="14" viewBox="0 0 24 24" fill="none">
                    <path
                      d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8l-6-6Z"
                      stroke="currentColor"
                      stroke-width="2"
                      stroke-linecap="round"
                      stroke-linejoin="round"
                    />
                    <path
                      d="M14 2v6h6M16 13H8M16 17H8M10 9H8"
                      stroke="currentColor"
                      stroke-width="2"
                      stroke-linecap="round"
                      stroke-linejoin="round"
                    />
                  </svg>
                </span>
                <span class="text-info">
                  <span class="text-content">{truncateMiddle(store.pastedText, 30)}</span>
                </span>
              </button>
              <button
                type="button"
                class="pasted-remove"
                aria-label="清除筛选文本"
                onclick={() => {
                  void store.clearPastedText();
                  focusSearchInput();
                }}
              >
                ×
              </button>
            </div>
          {/if}

          {#each store.chips as chip, idx (chip.kind + idx)}
            <div
              class="pasted-text"
              class:compact-chip={store.chips.length === 3}
              class:many-chips={store.chips.length >= 4}
              title={chip.value}
            >
              <button
                type="button"
                class="pasted-chip-body"
                onclick={() => {
                  void store.editContextChip(idx).then(() => {
                    focusSearchInput();
                  });
                }}
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
                  <span class="text-content">{truncateMiddle(chip.value, 30)}</span>
                </span>
              </button>
              <button
                type="button"
                class="pasted-remove"
                aria-label={`Remove ${chip.label} context`}
                onclick={() => {
                  store.removeChip(idx);
                  focusSearchInput();
                }}
              >
                ×
              </button>
            </div>
          {/each}
        {/if}

        <div class="input-wrapper" data-tauri-drag-region>
          <span bind:this={measureEl} class="measure-text" aria-hidden="true"></span>
          {#if !activeInputValue && !isComposing}
            <div class="placeholder-text" aria-hidden="true">{currentPlaceholder}</div>
          {/if}
          <input
            bind:this={searchInput}
            class="search-input"
            style={`width: ${inputWidthPx}px;`}
            type="text"
            role="combobox"
            aria-label={store.mode === "agent"
              ? "Ask AI Agent"
              : store.mode === "list"
                ? "Filter plugin list items"
                : "搜索应用、插件或输入指令，Tab 切换 AI"}
            aria-autocomplete="list"
            aria-expanded={ariaExpandedState}
            aria-controls={store.mode === "list" ? "list-mode-listbox" : "launcher-results-listbox"}
            aria-activedescendant={ariaActivedescendantId}
            value={activeInputValue}
            placeholder=""
            oncompositionstart={() => {
              isComposing = true;
            }}
            oncompositionend={(e) => {
              isComposing = false;
              composingText = "";
              const val = (e.currentTarget as HTMLInputElement).value;
              if (store.mode === "list") {
                void store.onSubInputChange(val);
              } else {
                void store.onQueryChange(val);
              }
            }}
            oninput={(e) => {
              const val = (e.currentTarget as HTMLInputElement).value;
              if (isComposing || eventIsComposing(e)) {
                composingText = val;
                if (store.mode === "list") {
                  store.setDraftSubInput(val);
                } else {
                  store.setDraftQuery(val);
                }
                return;
              }
              if (store.mode === "list") {
                void store.onSubInputChange(val);
              } else {
                void store.onQueryChange(val);
              }
            }}
            onpaste={handlePaste}
          />
        </div>
      </div>

      <div bind:this={searchActionsEl} class="search-actions">
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
              focusSearchInput();
            }}
          >
            <span class="tab-target-text">AI</span>
            <kbd class="tab-target-key">Tab</kbd>
          </button>
        {/if}

        {#if store.mode === "list" && store.listSession}
          {@const actionPluginIcon = validIconSrc(
            store.listSession.plugin_icon,
            brokenIcons,
            `action-plugin:${store.listSession.plugin_id}`,
          )}
          <button
            type="button"
            class="avatar-wrapper list-avatar-btn"
            class:loading={store.listLoading}
            aria-label="插件列表操作菜单"
            title="插件菜单"
            onclick={openListHeaderMenu}
          >
            <span class="action-indicator" aria-hidden="true">
              <svg width="4" height="18" viewBox="0 0 4 18" fill="none">
                <circle cx="2" cy="2" r="2" fill="currentColor" opacity="0.7" />
                <circle cx="2" cy="9" r="2" fill="currentColor" opacity="0.7" />
                <circle cx="2" cy="16" r="2" fill="currentColor" opacity="0.7" />
              </svg>
            </span>
            <span class="avatar-container">
              {#if actionPluginIcon}
                <img
                  src={actionPluginIcon}
                  alt=""
                  class="search-btn plugin-logo"
                  class:adaptive-invert={shouldInvertIcon(
                    store.listSession.plugin_icon_is_dark,
                    store.resolvedTheme,
                  )}
                  draggable="false"
                  onerror={() => markIconBroken(`action-plugin:${store.listSession?.plugin_id ?? ""}`)}
                />
              {:else}
                <AToolsMark size="badge" circular={true} label="aitools" />
              {/if}
              {#if store.listLoading}
                <span class="avatar-spinner" aria-hidden="true"></span>
              {/if}
            </span>
          </button>
        {:else}
          <button
            type="button"
            class="avatar-wrapper avatar-btn"
            aria-label={store.mode === "agent" ? "Switch back to Search" : "Switch to AI Agent"}
            title={store.mode === "agent" ? "返回搜索模式" : "切换到 AI 模式 (Tab)"}
            onclick={() => {
              if (store.mode === "agent") {
                store.exitAgentToSearch();
              } else {
                void store.promoteToAgent();
              }
              focusSearchInput();
            }}
          >
            <span class="avatar-container">
              <AToolsMark
                size="badge"
                circular={true}
                streaming={store.agentStatus === "streaming" || store.agentStatus === "waiting_hitl"}
                aiState={store.agentStatus === "waiting_hitl"
                  ? "receiving"
                  : store.agentStatus === "streaming"
                    ? "sending"
                    : "idle"}
                label="aitools"
              />
            </span>
          </button>
        {/if}
      </div>
    </div>

    {#if dropdownOpen}
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
      <div bind:this={dropdownEl} class="dropdown-panel" onclick={handleDropdownClick}>
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
          {:else if hasSearchContent}
            {#if flatSearchHits.length > 0}
              {#if store.searchViewMode === "list"}
                <div
                  id="launcher-results-listbox"
                  class="vertical-list"
                  role="listbox"
                  aria-label="Search results"
                >
                  {#each listModeResults as hit, idx (`${hit.plugin_id}:${hit.feature_code}:${idx}`)}
                    {@const iconKey = `list-hit:${hit.plugin_id}:${hit.feature_code}`}
                    {@const iconUrl = validIconSrc(hit.icon, brokenIcons, iconKey)}
                    {@const squircle = squircleStyle(
                      hit.plugin_id || hit.feature_code,
                      hit.label || hit.feature_code,
                    )}
                    {@const titleText = hit.label || hit.feature_code}
                    {@const subText = itemSubtitle(hit)}
                    {@const isSelected = idx === activeSearchFlatIndex}
                    <button
                      type="button"
                      id={`hit-option-${idx}`}
                      role="option"
                      aria-selected={isSelected}
                      class="list-item"
                      class:selected={isSelected}
                      onclick={() => void store.launchHit(hit)}
                      oncontextmenu={(e) =>
                        openHitContextMenu(
                          e,
                          hit,
                          hit.match_type === "over" ? "recommendation" : "search",
                        )}
                    >
                      <span class="item-icon" aria-hidden="true">
                        {#if iconUrl}
                          <img
                            src={iconUrl}
                            alt=""
                            class:adaptive-invert={shouldInvertIcon(
                              hit.icon_is_dark,
                              store.resolvedTheme,
                            )}
                            draggable="false"
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
              {:else}
                <div
                  id="launcher-results-listbox"
                  class="aggregate-view"
                  role="listbox"
                  aria-label="Search results"
                >
                  {#if bestSearchResults.length > 0}
                    {@const selectedBestSearchIdx = getSectionSelectedIndex(
                      navigationGrid,
                      store.selectedRow,
                      store.selectedCol,
                      "bestSearch",
                    )}
                    <section class="collapsible-section" aria-label="最佳搜索结果">
                      {#if bestSearchResults.length > 18}
                        <button
                          type="button"
                          class="section-header clickable"
                          onclick={() => store.toggleSearchResultsExpanded()}
                        >
                          <span class="section-title">最佳搜索结果</span>
                          <span class="expand-btn-text">
                            {store.searchResultsExpanded ? "收起" : `展开 (${bestSearchResults.length})`}
                          </span>
                        </button>
                      {:else}
                        <div class="section-header">
                          <span class="section-title">最佳搜索结果</span>
                        </div>
                      {/if}
                      <div class="app-grid">
                        {#each visibleBestSearch as hit, idx (`bs:${hit.plugin_id}:${hit.feature_code}:${idx}`)}
                          {@const flatIdx = idx}
                          {@const iconKey = `bs:${hit.plugin_id}:${hit.feature_code}`}
                          {@const iconUrl = validIconSrc(hit.icon, brokenIcons, iconKey)}
                          {@const squircle = squircleStyle(
                            hit.plugin_id || hit.feature_code,
                            hit.label || hit.plugin_name,
                          )}
                          {@const titleText = hit.label || hit.plugin_name || hit.feature_code}
                          {@const isSelected = idx === selectedBestSearchIdx}
                          <button
                            type="button"
                            id={`hit-option-${flatIdx}`}
                            role="option"
                            aria-selected={isSelected}
                            class="app-item"
                            class:selected={isSelected}
                            title={itemTitleTooltip(hit)}
                            onclick={() => void store.launchHit(hit)}
                            oncontextmenu={(e) => openHitContextMenu(e, hit, "search")}
                          >
                            <span class="app-icon-wrap" aria-hidden="true">
                              {#if iconUrl}
                                <img
                                  class="app-icon"
                                  class:adaptive-invert={shouldInvertIcon(
                                    hit.icon_is_dark,
                                    store.resolvedTheme,
                                  )}
                                  src={iconUrl}
                                  alt=""
                                  draggable="false"
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
                            <span class="app-name">
                              {#each highlightSegments(titleText, store.query) as seg, sIdx (sIdx)}
                                {#if seg.match}<mark class="highlight">{seg.text}</mark>{:else}{seg.text}{/if}
                              {/each}
                            </span>
                          </button>
                        {/each}
                      </div>
                    </section>
                  {/if}

                  {#if bestMatches.length > 0}
                    {@const selectedBestMatchIdx = getSectionSelectedIndex(
                      navigationGrid,
                      store.selectedRow,
                      store.selectedCol,
                      "bestMatch",
                    )}
                    <section class="collapsible-section" aria-label="最佳匹配">
                      {#if bestMatches.length > 18}
                        <button
                          type="button"
                          class="section-header clickable"
                          onclick={() => store.toggleBestMatchesExpanded()}
                        >
                          <span class="section-title">最佳匹配</span>
                          <span class="expand-btn-text">
                            {store.bestMatchesExpanded ? "收起" : `展开 (${bestMatches.length})`}
                          </span>
                        </button>
                      {:else}
                        <div class="section-header">
                          <span class="section-title">最佳匹配</span>
                        </div>
                      {/if}
                      <div class="app-grid">
                        {#each visibleBestMatches as hit, idx (`bm:${hit.plugin_id}:${hit.feature_code}:${idx}`)}
                          {@const flatIdx = visibleBestSearch.length + idx}
                          {@const iconKey = `bm:${hit.plugin_id}:${hit.feature_code}`}
                          {@const iconUrl = validIconSrc(hit.icon, brokenIcons, iconKey)}
                          {@const squircle = squircleStyle(
                            hit.plugin_id || hit.feature_code,
                            hit.label || hit.plugin_name,
                          )}
                          {@const titleText = hit.label || hit.plugin_name || hit.feature_code}
                          {@const isSelected = idx === selectedBestMatchIdx}
                          <button
                            type="button"
                            id={`hit-option-${flatIdx}`}
                            role="option"
                            aria-selected={isSelected}
                            class="app-item"
                            class:selected={isSelected}
                            title={itemTitleTooltip(hit)}
                            onclick={() => void store.launchHit(hit)}
                            oncontextmenu={(e) => openHitContextMenu(e, hit, "search")}
                          >
                            <span class="app-icon-wrap" aria-hidden="true">
                              {#if iconUrl}
                                <img
                                  class="app-icon"
                                  class:adaptive-invert={shouldInvertIcon(
                                    hit.icon_is_dark,
                                    store.resolvedTheme,
                                  )}
                                  src={iconUrl}
                                  alt=""
                                  draggable="false"
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
                            <span class="app-name">
                              {#each highlightSegments(titleText, store.query) as seg, sIdx (sIdx)}
                                {#if seg.match}<mark class="highlight">{seg.text}</mark>{:else}{seg.text}{/if}
                              {/each}
                            </span>
                          </button>
                        {/each}
                      </div>
                    </section>
                  {/if}

                  {#if recommendations.length > 0}
                    {@const selectedRecIdx = getSectionSelectedIndex(
                      navigationGrid,
                      store.selectedRow,
                      store.selectedCol,
                      "recommendation",
                    )}
                    <section class="collapsible-section" aria-label="匹配推荐">
                      {#if recommendations.length > 18}
                        <button
                          type="button"
                          class="section-header clickable"
                          onclick={() => store.toggleRecommendationsExpanded()}
                        >
                          <span class="section-title">匹配推荐</span>
                          <span class="expand-btn-text">
                            {store.recommendationsExpanded
                              ? "收起"
                              : `展开 (${recommendations.length})`}
                          </span>
                        </button>
                      {:else}
                        <div class="section-header">
                          <span class="section-title">匹配推荐</span>
                        </div>
                      {/if}
                      <div class="app-grid">
                        {#each visibleRecommendations as hit, idx (`rec:${hit.plugin_id}:${hit.feature_code}:${idx}`)}
                          {@const flatIdx = visibleBestSearch.length + visibleBestMatches.length + idx}
                          {@const iconKey = `rec:${hit.plugin_id}:${hit.feature_code}`}
                          {@const iconUrl = validIconSrc(hit.icon, brokenIcons, iconKey)}
                          {@const squircle = squircleStyle(
                            hit.plugin_id || hit.feature_code,
                            hit.label || hit.plugin_name,
                          )}
                          {@const titleText = hit.label || hit.plugin_name || hit.feature_code}
                          {@const isSelected = idx === selectedRecIdx}
                          <button
                            type="button"
                            id={`hit-option-${flatIdx}`}
                            role="option"
                            aria-selected={isSelected}
                            class="app-item"
                            class:selected={isSelected}
                            title={itemTitleTooltip(hit)}
                            onclick={() => void store.launchHit(hit)}
                            oncontextmenu={(e) => openHitContextMenu(e, hit, "recommendation")}
                          >
                            <span class="app-icon-wrap" aria-hidden="true">
                              {#if iconUrl}
                                <img
                                  class="app-icon"
                                  class:adaptive-invert={shouldInvertIcon(
                                    hit.icon_is_dark,
                                    store.resolvedTheme,
                                  )}
                                  src={iconUrl}
                                  alt=""
                                  draggable="false"
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
                              {#if hit.is_recommendation_pinned}
                                <span class="app-pin-badge" title="已置顶">
                                  <svg viewBox="0 0 16 16" width="10" height="10" aria-hidden="true">
                                    <path
                                      fill="currentColor"
                                      d="M9.828 1.172a1 1 0 0 1 1.414 0l3.586 3.586a1 1 0 0 1-.44 1.668l-2.66.76-.846 2.538a1 1 0 0 1-.242.404l-.707.707a1 1 0 0 1-1.414 0L6.68 8.996l-3.474 3.474a.75.75 0 1 1-1.06-1.06l3.473-3.475-1.839-1.839a1 1 0 0 1 0-1.414l.707-.707a1 1 0 0 1 .404-.242l2.538-.846.76-2.66a1 1 0 0 1 .254-.451Z"
                                    />
                                  </svg>
                                </span>
                              {/if}
                            </span>
                            <span class="app-name">
                              {#each highlightSegments(titleText, store.query) as seg, sIdx (sIdx)}
                                {#if seg.match}<mark class="highlight">{seg.text}</mark>{:else}{seg.text}{/if}
                              {/each}
                            </span>
                          </button>
                        {/each}
                      </div>
                    </section>
                  {/if}
                </div>
              {/if}
            {:else if !store.searchLoading}
              <div class="empty-search">
                <span>
                  未找到与 “{(store.query || store.pastedText || "所选文件").trim()}” 匹配的本地应用或指令。
                </span>
                <button
                  type="button"
                  class="inline-promote-btn"
                  onclick={() => {
                    void store.promoteToAgent(store.query || store.pastedText || "");
                    void store.submitAgentTurn();
                  }}
                >
                  使用 AI 处理 <kbd>↵</kbd>
                </button>
              </div>
            {/if}
          {:else}
            {#if store.suggestions.length > 0}
              {@const selectedSuggestionIdx = getSectionSelectedIndex(
                navigationGrid,
                store.selectedRow,
                store.selectedCol,
                "suggestion",
              )}
              <section class="main-push-section" aria-label="智能推荐">
                <div class="section-header">
                  <span class="section-title">智能推荐</span>
                </div>
                <div class="push-list" role="listbox" aria-label="智能推荐">
                  {#each store.suggestions as act, idx (act.id)}
                    {@const isSelected = idx === selectedSuggestionIdx}
                    <button
                      type="button"
                      id={`home-option-${idx}-0`}
                      role="option"
                      aria-selected={isSelected}
                      class="push-item"
                      class:selected={isSelected}
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

            {#if showHomeGrid && store.recentItems.length > 0}
              {@const selectedRecentIdx = getSectionSelectedIndex(
                navigationGrid,
                store.selectedRow,
                store.selectedCol,
                "recent",
              )}
              {@const recentRowOffset = store.suggestions.length}
              <section class="collapsible-section" aria-label="最近使用">
                {#if store.recentItems.length > 18}
                  <button
                    type="button"
                    class="section-header clickable"
                    onclick={() => store.toggleRecentExpanded()}
                  >
                    <span class="section-title">最近使用</span>
                    <span class="expand-btn-text">
                      {store.recentExpanded ? "收起" : `展开 (${store.recentItems.length})`}
                    </span>
                  </button>
                {:else}
                  <div class="section-header">
                    <span class="section-title">最近使用</span>
                  </div>
                {/if}
                <div class="app-grid">
                  {#each visibleRecentItems as item, idx (`recent:${item.plugin_id}:${item.feature_code}:${idx}`)}
                    {@const iconKey = `recent:${item.plugin_id}:${item.feature_code}`}
                    {@const iconUrl = validIconSrc(item.icon, brokenIcons, iconKey)}
                    {@const squircle = squircleStyle(
                      item.plugin_id || item.feature_code,
                      item.label || item.plugin_name,
                    )}
                    {@const isSelected = idx === selectedRecentIdx}
                    {@const gridRow = recentRowOffset + Math.floor(idx / 9)}
                    {@const gridCol = idx % 9}
                    <button
                      type="button"
                      id={`home-option-${gridRow}-${gridCol}`}
                      role="option"
                      aria-selected={isSelected}
                      class="app-item"
                      class:selected={isSelected}
                      title={itemTitleTooltip(item)}
                      onclick={() => void store.launchHit(item)}
                      oncontextmenu={(e) => openHitContextMenu(e, item, "history")}
                    >
                      <span class="app-icon-wrap" aria-hidden="true">
                        {#if iconUrl}
                          <img
                            class="app-icon"
                            class:adaptive-invert={shouldInvertIcon(
                              item.icon_is_dark,
                              store.resolvedTheme,
                            )}
                            src={iconUrl}
                            alt=""
                            draggable="false"
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

            {#if showHomeGrid && store.pinnedItems.length > 0}
              {@const selectedPinnedIdx = getSectionSelectedIndex(
                navigationGrid,
                store.selectedRow,
                store.selectedCol,
                "pinned",
              )}
              {@const pinnedRowOffset =
                store.suggestions.length + Math.ceil(visibleRecentItems.length / 9)}
              <section class="collapsible-section" aria-label="已固定">
                {#if store.pinnedItems.length > 18}
                  <button
                    type="button"
                    class="section-header clickable"
                    onclick={() => store.togglePinnedExpanded()}
                  >
                    <span class="section-title">已固定</span>
                    <span class="expand-btn-text">
                      {store.pinnedExpanded ? "收起" : `展开 (${store.pinnedItems.length})`}
                    </span>
                  </button>
                {:else}
                  <div class="section-header">
                    <span class="section-title">已固定</span>
                  </div>
                {/if}
                <div class="app-grid">
                  {#each visiblePinnedItems as item, idx (`pinned:${item.plugin_id}:${item.feature_code}`)}
                    {@const iconKey = `pinned:${item.plugin_id}:${item.feature_code}`}
                    {@const iconUrl = validIconSrc(item.icon, brokenIcons, iconKey)}
                    {@const squircle = squircleStyle(
                      item.plugin_id || item.feature_code,
                      item.label || item.plugin_name,
                    )}
                    {@const isSelected = idx === selectedPinnedIdx}
                    {@const gridRow = pinnedRowOffset + Math.floor(idx / 9)}
                    {@const gridCol = idx % 9}
                    <button
                      type="button"
                      id={`home-option-${gridRow}-${gridCol}`}
                      role="option"
                      aria-selected={isSelected}
                      draggable="true"
                      class="app-item draggable-item"
                      class:selected={isSelected}
                      class:chosen={draggingPinnedIndex === idx}
                      class:ghost={dragOverPinnedIndex === idx &&
                        draggingPinnedIndex !== null &&
                        draggingPinnedIndex !== idx}
                      title={itemTitleTooltip(item)}
                      onclick={() => void store.launchHit(item)}
                      oncontextmenu={(e) => openHitContextMenu(e, item, "pinned")}
                      ondragstart={(e) => handlePinnedDragStart(e, idx)}
                      ondragover={(e) => handlePinnedDragOver(e, idx)}
                      ondrop={(e) => handlePinnedDrop(e, idx)}
                      ondragend={handlePinnedDragEnd}
                    >
                      <span class="app-icon-wrap" aria-hidden="true">
                        {#if iconUrl}
                          <img
                            class="app-icon"
                            class:adaptive-invert={shouldInvertIcon(
                              item.icon_is_dark,
                              store.resolvedTheme,
                            )}
                            src={iconUrl}
                            alt=""
                            draggable="false"
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

            {#if showHomeGrid && store.recentItems.length === 0 && store.pinnedItems.length === 0 && visibleFeaturedItems.length > 0}
              {@const selectedFeaturedIdx = getSectionSelectedIndex(
                navigationGrid,
                store.selectedRow,
                store.selectedCol,
                "featured",
              )}
              <section class="collapsible-section" aria-label="最佳匹配">
                {#if store.featuredItems.length > 18}
                  <button
                    type="button"
                    class="section-header clickable"
                    onclick={() => store.toggleHomeExpanded()}
                  >
                    <span class="section-title">最佳匹配</span>
                    <span class="expand-btn-text">
                      {store.homeExpanded ? "收起" : `展开 (${store.featuredItems.length})`}
                    </span>
                  </button>
                {:else}
                  <div class="section-header">
                    <span class="section-title">最佳匹配</span>
                  </div>
                {/if}
                <div class="app-grid">
                  {#each visibleFeaturedItems as item, idx (`featured:${item.plugin_id}:${item.feature_code}:${idx}`)}
                    {@const iconKey = `featured:${item.plugin_id}:${item.feature_code}`}
                    {@const iconUrl = validIconSrc(item.icon, brokenIcons, iconKey)}
                    {@const squircle = squircleStyle(
                      item.plugin_id || item.feature_code,
                      item.label || item.plugin_name,
                    )}
                    {@const isSelected = idx === selectedFeaturedIdx}
                    {@const gridRow = store.suggestions.length + Math.floor(idx / 9)}
                    {@const gridCol = idx % 9}
                    <button
                      type="button"
                      id={`home-option-${gridRow}-${gridCol}`}
                      role="option"
                      aria-selected={isSelected}
                      class="app-item"
                      class:selected={isSelected}
                      title={itemTitleTooltip(item)}
                      onclick={() => void store.launchHit(item)}
                      oncontextmenu={(e) => openHitContextMenu(e, item, "featured")}
                    >
                      <span class="app-icon-wrap" aria-hidden="true">
                        {#if iconUrl}
                          <img
                            class="app-icon"
                            class:adaptive-invert={shouldInvertIcon(
                              item.icon_is_dark,
                              store.resolvedTheme,
                            )}
                            src={iconUrl}
                            alt=""
                            draggable="false"
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
                    onclick={() => void store.selectListAt(idx)}
                  >
                    <span class="item-icon" aria-hidden="true">
                      {#if listIcon}
                        <img
                          src={listIcon}
                          alt=""
                          draggable="false"
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
            searchViewMode={store.searchViewMode}
            onChangeTheme={(next) => store.setThemePreference(next)}
            onChangeSearchViewMode={(next) => store.setSearchViewMode(next)}
            onRefresh={() => void store.refreshDshLogs()}
            onToggleFault={(k) => void store.updateFault(k)}
            onSyncPlugins={() => void store.syncZtoolsPlugins()}
            onClose={() => void store.toggleDsh()}
          />
        {/if}
      </div>
    {/if}
  </main>

  {#if contextMenu}
    <div
      bind:this={menuEl}
      class="context-menu"
      role="menu"
      tabindex="-1"
      aria-label="操作菜单"
      style={`left: ${contextMenu.x}px; top: ${contextMenu.y}px;`}
    >
      {#each contextMenu.items as item, idx (item.id)}
        <button
          type="button"
          role="menuitem"
          class="context-menu-item"
          class:active={idx === contextMenu.activeIndex}
          class:danger={Boolean(item.danger)}
          disabled={Boolean(item.disabled)}
          aria-disabled={Boolean(item.disabled)}
          onclick={() => void executeMenuItem(item)}
        >
          <span class="context-menu-label">{item.label}</span>
          {#if item.shortcut}
            <kbd class="context-menu-shortcut">{item.shortcut}</kbd>
          {/if}
        </button>
      {/each}
    </div>
  {/if}
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

  .drag-overlay {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.28);
    border-radius: var(--radius-lg);
    z-index: 150;
    pointer-events: none;
    animation: fadeIn 150ms ease;
  }

  .search-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 12px;
    height: 58px;
    flex: 0 0 58px;
    width: 100%;
    min-width: 0;
    border-radius: var(--radius-lg) var(--radius-lg) 0 0;
    user-select: none;
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
    height: 100%;
    overflow: hidden;
  }

  .input-wrapper {
    position: relative;
    flex: 1 1 0;
    min-width: 64px;
    height: 100%;
    display: flex;
    align-items: center;
    cursor: text;
  }

  .measure-text {
    position: absolute;
    left: -9999px;
    white-space: pre;
    font-size: 22px;
    line-height: 1.3;
    font-family: var(--font-sans);
    font-weight: 400;
    pointer-events: none;
    visibility: hidden;
  }

  .placeholder-text {
    position: absolute;
    left: 0;
    color: var(--text-tertiary);
    font-size: 22px;
    font-weight: 400;
    line-height: 1.3;
    pointer-events: none;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 100%;
    font-family: var(--font-sans);
  }

  .search-input {
    width: auto;
    flex: 0 0 auto;
    min-width: 2px;
    border: none;
    outline: none;
    background: transparent;
    color: var(--text-primary);
    font-size: 22px;
    font-weight: 400;
    line-height: 1.3;
    height: 44px;
    font-family: var(--font-sans);
    user-select: text;
  }

  .search-input:focus-visible {
    outline: none;
  }

  .pasted-files,
  .pasted-text {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 36px;
    max-width: 210px;
    padding: 0 6px 0 10px;
    border-radius: 6px;
    background: var(--control-bg);
    flex: 0 1 auto;
    min-width: 60px;
    user-select: none;
  }

  .pasted-files {
    border: 1px solid var(--border-strong);
  }

  .pasted-text {
    border: 1px dashed var(--border-strong);
    transition: background-color 150ms ease-out, border-color 150ms ease-out;
  }

  @media (hover: hover) {
    .pasted-text:hover {
      background: var(--hover-bg);
      border-color: var(--text-tertiary);
    }
  }

  .pasted-text.compact-chip {
    max-width: 150px;
  }

  .pasted-text.many-chips {
    max-width: 112px;
  }

  .pasted-chip-body {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    flex: 1 1 auto;
    height: 100%;
    text-align: left;
  }

  .file-icon,
  .text-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--accent-text);
    flex: 0 0 auto;
  }

  .file-info,
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

  .file-name,
  .text-content {
    color: var(--text-primary);
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .file-count {
    font-size: 11px;
    font-weight: 600;
    color: var(--accent-text);
    background: var(--accent-subtle);
    padding: 1px 6px;
    border-radius: 999px;
    flex: 0 0 auto;
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
    background: var(--hover-bg);
    color: var(--text-primary);
  }

  .plugin-tag {
    display: inline-flex;
    align-items: center;
    border-radius: 20px;
    background: var(--control-bg);
    border: 1px solid var(--border);
    flex: 0 0 auto;
    max-width: 280px;
    height: 36px;
    overflow: hidden;
    padding-right: 4px;
    transition: background-color 150ms ease-out;
  }

  .plugin-tag.has-cmd {
    background: color-mix(in srgb, var(--text-primary) 4%, transparent);
  }

  .plugin-tag-left {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 0 18px 0 10px;
    height: 100%;
    background: color-mix(in srgb, var(--text-primary) 7%, transparent);
    clip-path: polygon(0 0, 100% 0, calc(100% - 10px) 100%, 0 100%);
    min-width: 0;
  }

  .plugin-tag-icon {
    width: 20px;
    height: 20px;
    border-radius: 4px;
    object-fit: contain;
    flex: 0 0 20px;
  }

  .plugin-tag-glyph {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--accent-text);
    flex: 0 0 auto;
  }

  .plugin-tag-title {
    font-size: 15px;
    font-weight: 600;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    line-height: 1;
  }

  .plugin-tag-cmd {
    font-size: 13px;
    font-weight: 600;
    color: var(--text-secondary);
    padding: 0 6px 0 4px;
    max-width: 120px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    line-height: 1;
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
    padding: 0;
    transition: background-color 150ms ease-out, color 150ms ease-out, transform 100ms ease-out;
  }

  .plugin-tag-close:hover {
    background: var(--hover-bg);
    color: var(--text-primary);
  }

  .plugin-tag-close:active {
    transform: scale(0.9);
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
    transition: background-color 150ms ease-out, color 150ms ease-out;
  }

  .tab-target-hint:hover {
    background: var(--hover-bg);
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

  .avatar-wrapper {
    position: relative;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    flex: 0 0 auto;
    border-radius: 999px;
  }

  .avatar-container {
    position: relative;
    width: 36px;
    height: 36px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }

  .action-indicator {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--text-secondary);
    opacity: 0.7;
    transition: opacity 150ms ease-out;
  }

  .avatar-wrapper:hover .action-indicator {
    opacity: 1;
  }

  .search-btn {
    width: 36px;
    height: 36px;
    border-radius: 50%;
    object-fit: cover;
    position: relative;
    z-index: 1;
  }

  .search-btn.plugin-logo {
    border-radius: 6px;
    object-fit: contain;
  }

  .avatar-btn:hover {
    transform: scale(0.96);
  }

  .avatar-btn:active,
  .list-avatar-btn:active {
    transform: scale(0.94);
  }

  .avatar-spinner {
    position: absolute;
    inset: -4px;
    border-radius: 50%;
    border: 2px solid color-mix(in srgb, var(--accent) 22%, transparent);
    border-top-color: var(--accent);
    animation: avatar-spin 0.8s linear infinite;
    pointer-events: none;
  }

  @keyframes avatar-spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
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

  .aggregate-view {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .collapsible-section,
  .main-push-section {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-bottom: 2px;
  }

  .section-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    width: 100%;
    padding: 2px 10px;
    min-height: 28px;
    border-radius: 6px;
    text-align: left;
  }

  .section-header.clickable {
    cursor: pointer;
    transition: background-color 150ms ease-out;
  }

  @media (hover: hover) {
    .section-header.clickable:hover {
      background: var(--hover-bg);
    }

    .section-header.clickable:hover .expand-btn-text {
      color: var(--text-primary);
    }
  }

  .section-title {
    font-size: 13.5px;
    font-weight: 600;
    color: var(--text-secondary);
    line-height: 22px;
  }

  .expand-btn-text {
    font-size: 12.5px;
    font-weight: 500;
    color: var(--text-tertiary);
    white-space: nowrap;
    user-select: none;
    transition: color 150ms ease-out;
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
    user-select: none;
    transition: background-color 150ms ease-out, transform 150ms ease-out;
  }

  @media (hover: hover) {
    .list-item:hover:not(.selected) {
      background: var(--hover-bg);
    }
  }

  .list-item.selected {
    background: var(--primary-light-bg);
  }

  .list-item:active {
    transform: scale(0.97);
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

  .item-title mark.highlight,
  .app-name mark.highlight {
    background-color: transparent;
    color: var(--highlight);
    font-weight: 600;
  }

  .list-item.selected .item-title mark.highlight,
  .app-item.selected .app-name mark.highlight {
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
    user-select: none;
    transition: background-color 150ms ease-out, transform 150ms ease-out;
  }

  @media (hover: hover) {
    .push-item:hover:not(.selected) {
      background: var(--hover-bg);
    }
  }

  .push-item.selected {
    background: var(--primary-light-bg);
  }

  .push-item:active {
    transform: scale(0.97);
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
    gap: 0;
    padding: 0 4px;
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
    user-select: none;
    transition: background-color 150ms ease-out, transform 150ms ease-out;
  }

  @media (hover: hover) {
    .app-item:hover:not(.selected) {
      background: var(--hover-bg);
    }
  }

  .app-item.selected {
    background: var(--active-bg);
  }

  .app-item:active {
    transform: scale(0.97);
  }

  .app-item.ghost {
    opacity: 0.5;
    background: var(--border-strong);
  }

  .app-item.chosen {
    opacity: 0.8;
  }

  .app-item[draggable="true"] .app-icon-wrap,
  .app-item[draggable="true"] .app-name {
    pointer-events: none;
  }

  .app-icon-wrap {
    position: relative;
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

  img.adaptive-invert {
    filter: invert(1) brightness(1.1);
  }

  .app-pin-badge {
    position: absolute;
    top: -4px;
    right: -4px;
    display: inline-flex;
    width: 16px;
    height: 16px;
    align-items: center;
    justify-content: center;
    border: 1px solid var(--bg-primary);
    border-radius: 50%;
    background: var(--accent);
    color: var(--accent-foreground);
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.18);
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

  .agent-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
    padding: var(--space-1) var(--space-2);
  }

  .agent-toolbar-left {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
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

  .context-menu {
    position: fixed;
    z-index: 200;
    min-width: 210px;
    max-width: 280px;
    padding: 6px;
    border-radius: 10px;
    background: var(--bg-elevated);
    border: 1px solid var(--border-strong);
    box-shadow: 0 10px 28px rgba(0, 0, 0, 0.2);
    display: flex;
    flex-direction: column;
    gap: 2px;
    outline: none;
  }

  .context-menu-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    width: 100%;
    padding: 6px 10px;
    border-radius: 6px;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-primary);
    text-align: left;
    transition: background-color 120ms ease-out;
  }

  @media (hover: hover) {
    .context-menu-item:hover:not(:disabled) {
      background: var(--hover-bg);
    }
  }

  .context-menu-item.active:not(:disabled) {
    background: var(--active-bg);
  }

  .context-menu-item.danger:not(:disabled) {
    color: var(--danger);
  }

  @media (hover: hover) {
    .context-menu-item.danger:hover:not(:disabled) {
      background: var(--danger-subtle);
    }
  }

  .context-menu-item:disabled {
    color: var(--text-tertiary);
    opacity: 1;
    cursor: not-allowed;
  }

  .context-menu-label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .context-menu-shortcut {
    flex: 0 0 auto;
    font-size: 11px;
    color: var(--text-secondary);
    background: var(--control-bg);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 5px;
  }
</style>
