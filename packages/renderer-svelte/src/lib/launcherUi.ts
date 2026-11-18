import type { ContextChip, LauncherHit, QuickAgentAction, ResolvedTheme } from "./ipc";

export interface HighlightSegment {
  text: string;
  match: boolean;
}

export type NavigationSectionType =
  | "suggestion"
  | "recent"
  | "pinned"
  | "featured"
  | "bestSearch"
  | "bestMatch"
  | "recommendation"
  | "listItem";

export type NavigationGridItem =
  | { kind: "suggestion"; action: QuickAgentAction }
  | { kind: "hit"; hit: LauncherHit };

export interface NavigationGridRow {
  type: NavigationSectionType;
  items: NavigationGridItem[];
}

export interface BuildAggregateGridOptions {
  hasSearchContent: boolean;
  suggestions: QuickAgentAction[];
  recentItems: LauncherHit[];
  pinnedItems: LauncherHit[];
  featuredItems: LauncherHit[];
  bestSearchResults: LauncherHit[];
  bestMatches: LauncherHit[];
  recommendations: LauncherHit[];
  recentExpanded: boolean;
  pinnedExpanded: boolean;
  featuredExpanded: boolean;
  searchResultsExpanded: boolean;
  bestMatchesExpanded: boolean;
  recommendationsExpanded: boolean;
  columns?: number;
  defaultRows?: number;
}

const SQUIRCLE_PALETTE: Array<{ color: string; bg: string }> = [
  { color: "#047857", bg: "linear-gradient(135deg, #047857 0%, #065f46 100%)" },
  { color: "#0369a1", bg: "linear-gradient(135deg, #0369a1 0%, #075985 100%)" },
  { color: "#4338ca", bg: "linear-gradient(135deg, #4338ca 0%, #3730a3 100%)" },
  { color: "#b45309", bg: "linear-gradient(135deg, #b45309 0%, #92400e 100%)" },
  { color: "#be185d", bg: "linear-gradient(135deg, #be185d 0%, #9d174d 100%)" },
  { color: "#0f766e", bg: "linear-gradient(135deg, #0f766e 0%, #115e59 100%)" },
];

function escapeRegExp(raw: string): string {
  return raw.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/**
 * Splits `text` into matched and unmatched segments for XSS-safe `<mark class="highlight">`
 * rendering inside Svelte `{#each}` blocks (no `{@html}`).
 */
export function highlightSegments(text: string, query: string): HighlightSegment[] {
  if (!text) return [{ text: "", match: false }];
  const trimmed = query.trim();
  if (!trimmed) return [{ text, match: false }];

  const tokens = Array.from(
    new Set(
      [trimmed, ...trimmed.split(/\s+/)]
        .map((t) => t.trim())
        .filter((t) => t.length > 0),
    ),
  ).sort((a, b) => b.length - a.length);

  if (tokens.length === 0) return [{ text, match: false }];

  const pattern = new RegExp(`(${tokens.map(escapeRegExp).join("|")})`, "gi");
  const segments: HighlightSegment[] = [];
  let cursor = 0;

  for (const match of text.matchAll(pattern)) {
    const start = match.index ?? 0;
    const value = match[0];
    if (!value) continue;
    if (start > cursor) {
      segments.push({ text: text.slice(cursor, start), match: false });
    }
    segments.push({ text: value, match: true });
    cursor = start + value.length;
  }

  if (cursor === 0) {
    return [{ text, match: false }];
  }
  if (cursor < text.length) {
    segments.push({ text: text.slice(cursor), match: false });
  }
  return segments;
}

/**
 * Middle-truncates pasted text chips matching ZTools SearchBox (`head 15...tail 10`).
 */
export function truncateMiddle(text: string, maxLength = 30): string {
  if (!text || text.length <= maxLength) return text;
  const headLength = 15;
  const tailLength = 10;
  return `${text.slice(0, headLength)}...${text.slice(text.length - tailLength)}`;
}

/**
 * Mirrors ZTools AdaptiveIcon rule: invert monochrome icon only when its brightness matches the active theme.
 */
export function shouldInvertIcon(
  iconIsDark: boolean | null | undefined,
  resolvedTheme: ResolvedTheme,
): boolean {
  if (iconIsDark === null || iconIsDark === undefined) {
    return false;
  }
  return iconIsDark === (resolvedTheme === "dark");
}

/**
 * Deterministic WCAG-AA-safe squircle gradient and 1-2 character monogram fallback
 * for items without native app/plugin icons.
 */
export function squircleStyle(
  key: string,
  label: string,
): { color: string; bg: string; monogram: string } {
  let hash = 0;
  for (let i = 0; i < key.length; i++) {
    hash = ((hash << 5) - hash + key.charCodeAt(i)) | 0;
  }
  const palette = SQUIRCLE_PALETTE[Math.abs(hash) % SQUIRCLE_PALETTE.length];
  const clean = label.trim();
  if (!clean) {
    return { color: palette.color, bg: palette.bg, monogram: "A" };
  }
  const first = Array.from(clean)[0] ?? "A";
  const isAscii = /^[A-Za-z0-9]$/.test(first);
  if (!isAscii) {
    return { color: palette.color, bg: palette.bg, monogram: first };
  }
  const words = clean.split(/[\s\-_.:/]+/).filter(Boolean);
  if (words.length >= 2 && /^[A-Za-z0-9]/.test(words[1])) {
    return {
      color: palette.color,
      bg: palette.bg,
      monogram: `${words[0][0]}${words[1][0]}`.toUpperCase(),
    };
  }
  return { color: palette.color, bg: palette.bg, monogram: first.toUpperCase() };
}

export function validIconSrc(
  icon: string | null | undefined,
  broken: Record<string, boolean>,
  key: string,
): string | null {
  if (!icon || broken[key]) return null;
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

export function hitKindBadge(kind?: string | null): string {
  switch (kind) {
    case "app":
      return "应用";
    case "list":
      return "列表";
    case "wasm":
      return "Wasm";
    case "macro":
      return "宏";
    default:
      return "插件";
  }
}

export function chipShortLabel(kind: ContextChip["kind"], fallbackLabel: string): string {
  switch (kind) {
    case "clipboard":
      return "Clipboard";
    case "selection":
      return "Selected";
    case "active_app":
      return "App";
    case "active_path":
      return "Path";
    default:
      return fallbackLabel;
  }
}

export function hitIdentityKey(hit: Pick<LauncherHit, "plugin_id" | "feature_code">): string {
  return `${hit.plugin_id}:${hit.feature_code}`;
}

export function getVisibleSectionItems<T>(
  items: T[],
  expanded: boolean,
  defaultRows = 2,
  columns = 9,
): T[] {
  const defaultVisibleCount = columns * defaultRows;
  if (items.length <= defaultVisibleCount) return items;
  return expanded ? items : items.slice(0, defaultVisibleCount);
}

function appendHitRows(
  rows: NavigationGridRow[],
  type: NavigationSectionType,
  hits: LauncherHit[],
  columns = 9,
): void {
  for (let i = 0; i < hits.length; i += columns) {
    rows.push({
      type,
      items: hits.slice(i, i + columns).map((hit) => ({ kind: "hit", hit })),
    });
  }
}

export function buildAggregateNavigationGrid(
  options: BuildAggregateGridOptions,
): NavigationGridRow[] {
  const columns = options.columns ?? 9;
  const defaultRows = options.defaultRows ?? 2;
  const rows: NavigationGridRow[] = [];

  if (options.hasSearchContent) {
    appendHitRows(
      rows,
      "bestSearch",
      getVisibleSectionItems(
        options.bestSearchResults,
        options.searchResultsExpanded,
        defaultRows,
        columns,
      ),
      columns,
    );
    appendHitRows(
      rows,
      "bestMatch",
      getVisibleSectionItems(
        options.bestMatches,
        options.bestMatchesExpanded,
        defaultRows,
        columns,
      ),
      columns,
    );
    appendHitRows(
      rows,
      "recommendation",
      getVisibleSectionItems(
        options.recommendations,
        options.recommendationsExpanded,
        defaultRows,
        columns,
      ),
      columns,
    );
    return rows;
  }

  for (const action of options.suggestions) {
    rows.push({
      type: "suggestion",
      items: [{ kind: "suggestion", action }],
    });
  }

  if (options.recentItems.length > 0) {
    appendHitRows(
      rows,
      "recent",
      getVisibleSectionItems(options.recentItems, options.recentExpanded, defaultRows, columns),
      columns,
    );
  }

  if (options.pinnedItems.length > 0) {
    appendHitRows(
      rows,
      "pinned",
      getVisibleSectionItems(options.pinnedItems, options.pinnedExpanded, defaultRows, columns),
      columns,
    );
  }

  if (
    options.recentItems.length === 0 &&
    options.pinnedItems.length === 0 &&
    options.featuredItems.length > 0
  ) {
    appendHitRows(
      rows,
      "featured",
      getVisibleSectionItems(
        options.featuredItems,
        options.featuredExpanded,
        defaultRows,
        columns,
      ),
      columns,
    );
  }

  return rows;
}

export function getSectionSelectedIndex(
  grid: NavigationGridRow[],
  selectedRow: number,
  selectedCol: number,
  sectionType: NavigationSectionType,
  columns = 9,
): number {
  if (grid.length === 0 || selectedRow < 0 || selectedRow >= grid.length) {
    return -1;
  }
  const current = grid[selectedRow];
  if (current.type !== sectionType) {
    return -1;
  }
  const startRow = grid.findIndex((r) => r.type === sectionType);
  if (startRow < 0) return -1;
  if (sectionType === "suggestion" || sectionType === "listItem") {
    return selectedRow - startRow;
  }
  return (selectedRow - startRow) * columns + selectedCol;
}

export function clampGridCoords(
  grid: NavigationGridRow[],
  row: number,
  col: number,
): { row: number; col: number } {
  if (grid.length === 0) {
    return { row: 0, col: 0 };
  }
  const clampedRow = Math.max(0, Math.min(row, grid.length - 1));
  const rowItems = grid[clampedRow]?.items ?? [];
  if (rowItems.length === 0) {
    return { row: clampedRow, col: 0 };
  }
  const clampedCol = Math.max(0, Math.min(col, rowItems.length - 1));
  return { row: clampedRow, col: clampedCol };
}

export function stepGridSelection(
  grid: NavigationGridRow[],
  row: number,
  col: number,
  key: "ArrowDown" | "ArrowUp" | "ArrowRight" | "ArrowLeft",
  mode: "aggregate" | "list",
): { row: number; col: number } {
  if (grid.length === 0) {
    return { row: 0, col: 0 };
  }
  const cur = clampGridCoords(grid, row, col);

  if (mode === "list") {
    if (key === "ArrowDown") {
      return { row: cur.row < grid.length - 1 ? cur.row + 1 : 0, col: 0 };
    }
    if (key === "ArrowUp") {
      return { row: cur.row > 0 ? cur.row - 1 : grid.length - 1, col: 0 };
    }
    return cur;
  }

  switch (key) {
    case "ArrowDown": {
      const nextRow = cur.row < grid.length - 1 ? cur.row + 1 : 0;
      const maxCol = Math.max(0, grid[nextRow].items.length - 1);
      return { row: nextRow, col: Math.min(cur.col, maxCol) };
    }
    case "ArrowUp": {
      const prevRow = cur.row > 0 ? cur.row - 1 : grid.length - 1;
      const maxCol = Math.max(0, grid[prevRow].items.length - 1);
      return { row: prevRow, col: Math.min(cur.col, maxCol) };
    }
    case "ArrowRight": {
      const rowItems = grid[cur.row].items;
      if (cur.col < rowItems.length - 1) {
        return { row: cur.row, col: cur.col + 1 };
      }
      if (cur.row < grid.length - 1) {
        return { row: cur.row + 1, col: 0 };
      }
      return { row: 0, col: 0 };
    }
    case "ArrowLeft": {
      if (cur.col > 0) {
        return { row: cur.row, col: cur.col - 1 };
      }
      if (cur.row > 0) {
        const prevRow = cur.row - 1;
        return { row: prevRow, col: Math.max(0, grid[prevRow].items.length - 1) };
      }
      const lastRow = grid.length - 1;
      return { row: lastRow, col: Math.max(0, grid[lastRow].items.length - 1) };
    }
  }
}

export function partitionSearchHits(hits: LauncherHit[]): {
  bestSearchResults: LauncherHit[];
  bestMatches: LauncherHit[];
  recommendations: LauncherHit[];
} {
  const bestSearchResults: LauncherHit[] = [];
  const bestMatches: LauncherHit[] = [];
  const recommendations: LauncherHit[] = [];

  for (const hit of hits) {
    const mt = (hit.match_type || "").toLowerCase();
    if (mt === "regex" || mt === "img" || mt === "files") {
      bestMatches.push(hit);
    } else if (mt === "over") {
      recommendations.push(hit);
    } else {
      bestSearchResults.push(hit);
    }
  }

  return { bestSearchResults, bestMatches, recommendations };
}

export function deduplicateHits(hits: LauncherHit[]): LauncherHit[] {
  const seen = new Set<string>();
  const out: LauncherHit[] = [];
  for (const hit of hits) {
    const key =
      hit.kind === "app"
        ? `app:${hit.label}:${hit.target_path ?? hit.feature_code}`
        : hitIdentityKey(hit);
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(hit);
  }
  return out;
}

export function buildListModeResults(hits: LauncherHit[], query: string): LauncherHit[] {
  const deduped = deduplicateHits(hits);
  const q = query.trim().toLowerCase();
  if (!q) return deduped;

  return [...deduped].sort((a, b) => {
    const nameA = (a.label || a.feature_code).toLowerCase();
    const nameB = (b.label || b.feature_code).toLowerCase();

    const exactA = nameA === q;
    const exactB = nameB === q;
    if (exactA && !exactB) return -1;
    if (!exactA && exactB) return 1;

    const prefixA = nameA.startsWith(q);
    const prefixB = nameB.startsWith(q);
    if (prefixA && !prefixB) return -1;
    if (!prefixA && prefixB) return 1;

    const appA = a.kind === "app";
    const appB = b.kind === "app";
    if (appA && !appB) return -1;
    if (!appA && appB) return 1;

    const useA = a.use_count ?? 0;
    const useB = b.use_count ?? 0;
    if (useA !== useB) return useB - useA;

    return b.score - a.score;
  });
}
