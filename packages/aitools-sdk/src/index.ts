/** Risk tier declared on agent tools (`SPEC-007-R7`, `INV-SEC-04`). */
export type RiskTier =
  | "Tier0ReadOnly"
  | "Tier1StateMutating"
  | "Tier2DestructiveOrExternal";

/** Classified payload signature inferred from ambient desktop context (`SPEC-011-R1`). */
export type ContentSignature =
  | "json"
  | "stack_trace"
  | "url"
  | "file_paths"
  | "image"
  | "plain_text"
  | "empty";

/**
 * Normalized ambient desktop context captured prior to launcher focus and dispatched
 * via the `aitools:context-enter` custom event (`SPEC-011-R1`, `SPEC-016`).
 *
 * Snake-case fields match the canonical Rust `ContextEnvelope` wire format. Optional
 * legacy aliases are preserved for backward compatibility during plugin migration.
 */
export interface ContextEnvelope {
  /** Correlation trace identifier for the wake/capture turn (e.g. `"wake-1"`). */
  trace_id: string;
  /** Frontmost application display name captured before wake (e.g. `"Cursor"`). */
  app_name?: string | null;
  /** Frontmost application bundle identifier (e.g. `"com.apple.Terminal"`). */
  bundle_id?: string | null;
  /** Frontmost window title captured before wake. */
  window_title?: string | null;
  /** Selected text captured prior to launcher focus, truncated to safe bounds. */
  selected_text?: string | null;
  /** Plain-text clipboard preview captured prior to launcher focus. */
  clipboard_preview?: string | null;
  /** Active filesystem path associated with the focused context, if detected. */
  active_path?: string | null;
  /** Opaque handle referencing a captured image attachment, if any. */
  image_handle?: string | null;
  /** Unix epoch timestamp (milliseconds) when the context snapshot was captured. */
  captured_at_ms?: number;
  /** Pre-classified ambient payload signature. */
  signature?: ContentSignature;
  /** @deprecated Use `trace_id`. */
  traceId?: string;
  /** @deprecated Use `selected_text`. */
  selection?: string | null;
  /** @deprecated Use `clipboard_preview`. */
  clipboard?: string | null;
  /** @deprecated Use `app_name`. */
  active_app?: string | null;
  /** @deprecated Use `active_path`. */
  activeFsPath?: string | null;
}

/**
 * PouchDB/LMDB-compatible plugin document (`SPEC-002-R1`, `SPEC-012-R2`).
 * Supports both `{ _id, _rev, data: T }` envelope payloads and flat ZTools JSON fields.
 */
export type DbDoc<T = Record<string, unknown>> = {
  _id: string;
  _rev?: string;
  data?: T;
} & Record<string, unknown>;

/** Result returned by `db.put` and `db.remove`. */
export interface DbWriteResult {
  id: string;
  rev?: string;
  ok?: boolean;
  error?: boolean;
  name?: string;
  message?: string;
}

/** Named AI routing profile managed by the host `ModelBroker` (`SPEC-007-R3`). */
export type AiProfile = "fast" | "balanced" | "deep";

/** Request payload for `aitools.ai.chat` (`SPEC-007-R4`, `SPEC-016`). */
export interface AiChatRequest {
  /** Model tier routed by host `ModelBroker` (`"fast" | "balanced" | "deep"`). */
  profile?: AiProfile;
  /** Optional system instructions prepended to the request. */
  system?: string;
  /** Primary user prompt payload. */
  prompt: string;
  /** Optional JSON Schema when requesting structured JSON output. */
  schema?: Record<string, unknown>;
}

/** Backward-compatible type alias for {@link AiChatRequest}. */
export type ChatRequest = AiChatRequest;

interface TauriInternals {
  invoke<T>(
    cmd: string,
    args?: Record<string, unknown>,
    options?: { headers?: Record<string, string> },
  ): Promise<T>;
}

const getInternals = (): TauriInternals => {
  const internals = (globalThis as unknown as { __TAURI_INTERNALS__?: TauriInternals })
    .__TAURI_INTERNALS__;
  if (!internals) {
    throw new Error("@aitools/sdk requires a Tauri v2 host runtime");
  }
  return internals;
};

const currentTraceHeaders = (): Record<string, string> => {
  const traceId = (globalThis as unknown as { __AITOOLS_ACTIVE_TRACE_ID__?: string })
    .__AITOOLS_ACTIVE_TRACE_ID__;
  return traceId ? { "x-aitools-trace-id": traceId } : {};
};

const invokeHost = <T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> =>
  getInternals().invoke<T>(`plugin:aitools|${cmd}`, args, { headers: currentTraceHeaders() });

/**
 * Normalizes a {@link ContextEnvelope} from either canonical snake_case Rust payloads
 * or legacy camelCase fields.
 */
export function normalizeContextEnvelope(raw: Partial<ContextEnvelope> | null | undefined): ContextEnvelope {
  const src = raw ?? {};
  return {
    trace_id: src.trace_id ?? src.traceId ?? "wake-0",
    app_name: src.app_name ?? src.active_app ?? null,
    bundle_id: src.bundle_id ?? null,
    window_title: src.window_title ?? null,
    selected_text: src.selected_text ?? src.selection ?? null,
    clipboard_preview: src.clipboard_preview ?? src.clipboard ?? null,
    active_path: src.active_path ?? src.activeFsPath ?? null,
    image_handle: src.image_handle ?? null,
    captured_at_ms: src.captured_at_ms,
    signature: src.signature,
  };
}

/**
 * Extracts payload fields from either `{ _id, data: T }` envelopes or flat ZTools documents.
 */
export function unwrapDocData<T = Record<string, unknown>>(doc: DbDoc<T>): T {
  if (doc.data && typeof doc.data === "object" && !Array.isArray(doc.data)) {
    return doc.data;
  }
  const copy: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(doc)) {
    if (k !== "_id" && k !== "_rev" && k !== "data") {
      copy[k] = v;
    }
  }
  return copy as T;
}

export const aitools = Object.freeze({
  /** Returns `true` when running inside a Tauri v2 plugin webview with host IPC available. */
  isRuntimeAvailable(): boolean {
    return (
      typeof (globalThis as unknown as { __TAURI_INTERNALS__?: TauriInternals }).__TAURI_INTERNALS__ !==
      "undefined"
    );
  },

  /** Lifecycle & Context */
  onContextEnter(handler: (ctx: ContextEnvelope) => void | Promise<void>): () => void {
    const listener = (e: Event) => {
      const detail = (e as CustomEvent<Partial<ContextEnvelope>>).detail;
      void handler(normalizeContextEnvelope(detail));
    };
    globalThis.addEventListener("aitools:context-enter", listener);
    return () => globalThis.removeEventListener("aitools:context-enter", listener);
  },

  /** Async-first ACID LMDB Document Storage (SPEC-002 / SPEC-012-R2) */
  db: Object.freeze({
    get<T = Record<string, unknown>>(id: string): Promise<DbDoc<T> | null> {
      return invokeHost("db_get", { id });
    },
    put<T = Record<string, unknown>>(doc: DbDoc<T>): Promise<DbWriteResult> {
      return invokeHost("db_put", { doc });
    },
    remove(id: string, rev?: string): Promise<DbWriteResult> {
      return invokeHost("db_remove", { id, rev });
    },
    allDocs<T = Record<string, unknown>>(prefix?: string): Promise<Array<DbDoc<T>>> {
      return invokeHost("db_all_docs", { prefix });
    },
    unwrap: unwrapDocData,
  }),

  /** Host-Brokered Native AI Service (SPEC-007-R4 — zero per-plugin API key config) */
  ai: Object.freeze({
    chat<T = string>(req: AiChatRequest): Promise<T> {
      return invokeHost("ai_chat", { request: req });
    },
    embed(texts: string[]): Promise<number[][]> {
      return invokeHost("ai_embed", { texts });
    },
  }),

  /** Structured Chronological Telemetry Bridge (SPEC-010 / SPEC-014-R1) */
  log: Object.freeze({
    info(msg: string, fields: Record<string, unknown> = {}): Promise<void> {
      return invokeHost("telemetry_log", { level: "INFO", msg, fields });
    },
    warn(msg: string, fields: Record<string, unknown> = {}): Promise<void> {
      return invokeHost("telemetry_log", { level: "WARN", msg, fields });
    },
    error(msg: string, fields: Record<string, unknown> = {}): Promise<void> {
      return invokeHost("telemetry_log", { level: "ERROR", msg, fields });
    },
  }),
});

export default aitools;
