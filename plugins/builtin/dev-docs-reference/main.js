const invoke = window.__TAURI_INTERNALS__?.invoke;
const LOCAL_KEY = "aitools:preview:dev-docs";

const BUILTIN_DOCS = [
  {
    id: "rust-result",
    category: "rust",
    tag: "Rust",
    title: "Result & Option Combinators",
    description: "Idiomatic propagation without unwrap() in production paths.",
    snippet: "let val = opt.ok_or_else(|| MyError::Missing)?;\nlet mapped = res.map_err(|e| MyError::Io(e.to_string()))?;",
  },
  {
    id: "rust-tokio-select",
    category: "rust",
    tag: "Rust",
    title: "Tokio Bounded Timeout",
    description: "Prevent hung async IPC or HTTP streams with explicit timeouts.",
    snippet: "tokio::time::timeout(Duration::from_secs(10), fut).await\n  .map_err(|_| AiError::Provider(\"timeout\".into()))?",
  },
  {
    id: "tauri-cap",
    category: "tauri",
    tag: "Tauri v2",
    title: "Least-Privilege Capability Scope",
    description: "Split main-shell vs plugin-* webview permissions.",
    snippet: "{\"identifier\": \"plugin-sandbox\", \"webviews\": [\"plugin-*\"], \"permissions\": [\"aitools:guest-sandbox\"]}",
  },
  {
    id: "tauri-channel",
    category: "tauri",
    tag: "Tauri v2",
    title: "Streaming IPC Channel",
    description: "Stream incremental tokens or list items from Rust to frontend.",
    snippet: "const ch = new Channel();\nch.onmessage = (ev) => handle(ev);\nawait invoke(\"plugin:aitools|submit_agent_prompt\", { prompt, onEvent: ch });",
  },
  {
    id: "http-codes",
    category: "http",
    tag: "HTTP",
    title: "Core Status Codes",
    description: "200 OK · 204 No Content · 400 Bad Request · 403 Forbidden · 404 Not Found · 409 Conflict · 429 Rate Limit",
    snippet: "403 Forbidden: caller webview_label mismatch\n409 Conflict: optimistic _rev mismatch\n413 Payload Too Large: exceeds 256 KiB",
  },
  {
    id: "regex-basics",
    category: "regex",
    tag: "RegExp",
    title: "Common Validation Patterns",
    description: "Anchored ASCII slug, semantic version, and ISO date.",
    snippet: "^[a-z0-9][a-z0-9_-]{1,62}$\n^\\d+\\.\\d+\\.\\d+(?:-[0-9A-Za-z.-]+)?$\n^\\d{4}-\\d{2}-\\d{2}$",
  },
];

const searchEl = document.getElementById("search");
const cardsEl = document.getElementById("cards");
const emptyEl = document.getElementById("empty");
const statusEl = document.getElementById("status");
const formEl = document.getElementById("bookmark-form");
const bmTitleEl = document.getElementById("bm-title");
const bmSnippetEl = document.getElementById("bm-snippet");
const tabEls = Array.from(document.querySelectorAll(".tab"));

let activeCat = "all";
let bookmarks = [];

function unwrapBookmark(doc) {
  const payload = doc && typeof doc.data === "object" && doc.data !== null ? doc.data : doc;
  return {
    id: doc._id,
    _rev: doc._rev,
    category: "bookmarks",
    tag: "Bookmark",
    title: String(payload.title ?? doc.title ?? doc._id),
    description: String(payload.description ?? doc.description ?? "Saved personal snippet"),
    snippet: String(payload.snippet ?? payload.text ?? doc.snippet ?? ""),
    isBookmark: true,
  };
}

function showStatus(msg, kind = "info") {
  if (!msg) {
    statusEl.className = "status hidden";
    statusEl.textContent = "";
    return;
  }
  statusEl.className = `status ${kind}`;
  statusEl.textContent = msg;
}

function readLocal() {
  try {
    return JSON.parse(localStorage.getItem(LOCAL_KEY) || "[]");
  } catch {
    return [];
  }
}

function writeLocal(docs) {
  localStorage.setItem(LOCAL_KEY, JSON.stringify(docs));
}

async function dbAllDocs(prefix) {
  if (invoke) {
    return await invoke("plugin:aitools|db_all_docs", { prefix });
  }
  return readLocal().filter((d) => String(d._id || "").startsWith(prefix));
}

async function dbPut(doc) {
  if (invoke) {
    return await invoke("plugin:aitools|db_put", { doc });
  }
  const docs = readLocal();
  const next = { ...doc, _rev: `preview-${Date.now()}` };
  const idx = docs.findIndex((d) => d._id === doc._id);
  if (idx >= 0) docs[idx] = next;
  else docs.unshift(next);
  writeLocal(docs);
  return { id: doc._id, rev: next._rev, ok: true };
}

async function dbRemove(id, rev) {
  if (invoke) {
    return await invoke("plugin:aitools|db_remove", { id, rev });
  }
  writeLocal(readLocal().filter((d) => d._id !== id));
  return { id, ok: true };
}

function render() {
  const q = searchEl.value.trim().toLowerCase();
  const combined = [...bookmarks, ...BUILTIN_DOCS].filter((item) => {
    if (activeCat !== "all" && item.category !== activeCat) return false;
    if (!q) return true;
    return (
      item.title.toLowerCase().includes(q) ||
      item.description.toLowerCase().includes(q) ||
      item.snippet.toLowerCase().includes(q) ||
      item.tag.toLowerCase().includes(q)
    );
  });

  cardsEl.innerHTML = "";
  emptyEl.classList.toggle("hidden", combined.length > 0);

  for (const item of combined) {
    const card = document.createElement("article");
    card.className = "ref-card";

    const head = document.createElement("div");
    head.className = "ref-head";

    const title = document.createElement("div");
    title.className = "ref-title";
    title.textContent = item.title;

    const tag = document.createElement("span");
    tag.className = "ref-tag";
    tag.textContent = item.tag;
    head.append(title, tag);

    const desc = document.createElement("p");
    desc.className = "ref-desc";
    desc.textContent = item.description;

    const code = document.createElement("pre");
    code.className = "ref-code";
    code.textContent = item.snippet;

    const actions = document.createElement("div");
    actions.className = "ref-actions";

    const copyBtn = document.createElement("button");
    copyBtn.type = "button";
    copyBtn.className = "btn btn-ghost";
    copyBtn.textContent = "Copy Snippet";
    copyBtn.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(item.snippet);
        showStatus(`Copied "${item.title}" snippet.`, "success");
      } catch {
        showStatus("Clipboard copy unavailable in this context.", "error");
      }
    });
    actions.append(copyBtn);

    if (item.isBookmark) {
      const delBtn = document.createElement("button");
      delBtn.type = "button";
      delBtn.className = "btn btn-ghost btn-danger";
      delBtn.textContent = "Delete";
      delBtn.addEventListener("click", async () => {
        try {
          await dbRemove(item.id, item._rev);
          await refresh();
        } catch (err) {
          showStatus(`Failed to delete bookmark: ${err}`, "error");
        }
      });
      actions.append(delBtn);
    }

    card.append(head, desc, code, actions);
    cardsEl.appendChild(card);
  }
}

async function refresh() {
  try {
    const raw = await dbAllDocs("doc:");
    bookmarks = (raw || []).map(unwrapBookmark);
    render();
  } catch (err) {
    showStatus(`Failed to load saved bookmarks: ${err}`, "error");
    render();
  }
}

formEl.addEventListener("submit", async (e) => {
  e.preventDefault();
  const title = bmTitleEl.value.trim();
  const snippet = bmSnippetEl.value.trim();
  if (!title || !snippet) return;
  try {
    await dbPut({
      _id: `doc:${Date.now()}`,
      data: { title, snippet, description: "Custom developer bookmark" },
    });
    bmTitleEl.value = "";
    bmSnippetEl.value = "";
    showStatus(`Saved bookmark "${title}".`, "success");
    await refresh();
  } catch (err) {
    showStatus(`Failed to save bookmark: ${err}`, "error");
  }
});

searchEl.addEventListener("input", render);

for (const tab of tabEls) {
  tab.addEventListener("click", () => {
    activeCat = tab.dataset.cat || "all";
    for (const el of tabEls) {
      const isActive = el === tab;
      el.classList.toggle("active", isActive);
      el.setAttribute("aria-selected", String(isActive));
    }
    render();
  });
}

void refresh();
