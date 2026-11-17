export type RiskTier = "Tier0ReadOnly" | "Tier1StateMutating" | "Tier2DestructiveOrExternal";

export interface ContextEnvelope {
  traceId: string;
  activeApp?: { bundleId: string; appName: string; windowTitle: string };
  activeFsPath?: string;
  selection?: { rawText: string; detectedMime: string; astHint?: string };
}

export interface DbDoc<T = Record<string, unknown>> {
  _id: string;
  _rev?: string;
  data: T;
}

export interface ChatRequest {
  profile?: "fast" | "reasoning";
  system?: string;
  prompt: string;
  schema?: Record<string, unknown>;
}

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

export const aitools = Object.freeze({
  /** Lifecycle & Context */
  onContextEnter(handler: (ctx: ContextEnvelope) => void | Promise<void>): () => void {
    const listener = (e: Event) => handler((e as CustomEvent<ContextEnvelope>).detail);
    globalThis.addEventListener("aitools:context-enter", listener);
    return () => globalThis.removeEventListener("aitools:context-enter", listener);
  },

  /** Async-first ACID LMDB Document Storage (SPEC-002 / SPEC-012-R2) */
  db: Object.freeze({
    get<T = Record<string, unknown>>(id: string): Promise<DbDoc<T> | null> {
      return invokeHost("db_get", { id });
    },
    put<T = Record<string, unknown>>(
      doc: DbDoc<T>,
    ): Promise<{ id: string; rev: string; ok: boolean }> {
      return invokeHost("db_put", { doc });
    },
    remove(id: string, rev?: string): Promise<{ id: string; ok: boolean }> {
      return invokeHost("db_remove", { id, rev });
    },
    allDocs<T = Record<string, unknown>>(prefix?: string): Promise<Array<DbDoc<T>>> {
      return invokeHost("db_all_docs", { prefix });
    },
  }),

  /** Host-Brokered Native AI Service (SPEC-007-R4 — zero per-plugin API key config) */
  ai: Object.freeze({
    chat<T = string>(req: ChatRequest): Promise<T> {
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
