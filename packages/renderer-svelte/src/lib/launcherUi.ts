import type { ContextChip } from "./ipc";

export interface HighlightSegment {
  text: string;
  match: boolean;
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
