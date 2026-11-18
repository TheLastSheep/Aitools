const invoke = window.__TAURI_INTERNALS__?.invoke;
const LOCAL_KEY = "aitools:preview:clipboard-history";

const filterEl = document.getElementById("filter");
const listEl = document.getElementById("clips");
const emptyEl = document.getElementById("empty");
const statusEl = document.getElementById("status");
const summaryEl = document.getElementById("summary-box");
const addFormEl = document.getElementById("add-form");
const newClipEl = document.getElementById("new-clip");
const summarizeBtn = document.getElementById("summarize-btn");
const tabEls = Array.from(document.querySelectorAll(".tab"));

let activeTab = "all";
let allDocs = [];

function unwrapDoc(doc) {
  const payload = doc && typeof doc.data === "object" && doc.data !== null ? doc.data : doc;
  return {
    _id: doc._id,
    _rev: doc._rev,
    text: String(payload.text ?? doc.text ?? ""),
    pinned: Boolean(payload.pinned ?? doc.pinned ?? false),
    createdAt: Number(payload.createdAt ?? doc.createdAt ?? 0),
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

function readLocalDocs() {
  try {
    return JSON.parse(localStorage.getItem(LOCAL_KEY) || "[]");
  } catch {
    return [];
  }
}

function writeLocalDocs(docs) {
  localStorage.setItem(LOCAL_KEY, JSON.stringify(docs));
}

async function dbAllDocs(prefix) {
  if (invoke) {
    return await invoke("plugin:aitools|db_all_docs", { prefix });
  }
  return readLocalDocs().filter((d) => String(d._id || "").startsWith(prefix));
}

async function dbPut(doc) {
  if (invoke) {
    return await invoke("plugin:aitools|db_put", { doc });
  }
  const docs = readLocalDocs();
  const next = { ...doc, _rev: `preview-${Date.now()}` };
  const idx = docs.findIndex((d) => d._id === doc._id);
  if (idx >= 0) docs[idx] = next;
  else docs.unshift(next);
  writeLocalDocs(docs);
  return { id: doc._id, rev: next._rev, ok: true };
}

async function dbRemove(id, rev) {
  if (invoke) {
    return await invoke("plugin:aitools|db_remove", { id, rev });
  }
  const docs = readLocalDocs().filter((d) => d._id !== id);
  writeLocalDocs(docs);
  return { id, ok: true };
}

function render() {
  const q = filterEl.value.trim().toLowerCase();
  const filtered = allDocs
    .filter((item) => (activeTab === "pinned" ? item.pinned : true))
    .filter((item) => !q || item.text.toLowerCase().includes(q) || item._id.toLowerCase().includes(q))
    .sort((a, b) => Number(b.pinned) - Number(a.pinned) || b.createdAt - a.createdAt);

  listEl.innerHTML = "";
  emptyEl.classList.toggle("hidden", filtered.length > 0);

  for (const item of filtered) {
    const li = document.createElement("li");
    li.className = item.pinned ? "clip-card pinned" : "clip-card";

    const textEl = document.createElement("div");
    textEl.className = "clip-text";
    textEl.textContent = item.text || item._id;

    const footer = document.createElement("div");
    footer.className = "clip-footer";

    const meta = document.createElement("span");
    meta.className = "clip-meta";
    const timeStr = item.createdAt ? new Date(item.createdAt).toLocaleString() : item._id;
    meta.textContent = item.pinned ? `Pinned · ${timeStr}` : timeStr;

    const actions = document.createElement("div");
    actions.className = "clip-actions";

    const copyBtn = document.createElement("button");
    copyBtn.type = "button";
    copyBtn.className = "btn btn-ghost";
    copyBtn.textContent = "Copy";
    copyBtn.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(item.text);
        showStatus("Copied snippet to clipboard.", "success");
      } catch {
        showStatus("Clipboard write unavailable in this context.", "error");
      }
    });

    const pinBtn = document.createElement("button");
    pinBtn.type = "button";
    pinBtn.className = "btn btn-ghost";
    pinBtn.textContent = item.pinned ? "Unpin" : "Pin";
    pinBtn.addEventListener("click", async () => {
      try {
        await dbPut({
          _id: item._id,
          _rev: item._rev,
          data: { text: item.text, pinned: !item.pinned, createdAt: item.createdAt },
        });
        await refresh();
      } catch (err) {
        showStatus(String(err), "error");
      }
    });

    const delBtn = document.createElement("button");
    delBtn.type = "button";
    delBtn.className = "btn btn-ghost btn-danger";
    delBtn.textContent = "Delete";
    delBtn.addEventListener("click", async () => {
      try {
        await dbRemove(item._id, item._rev);
        await refresh();
      } catch (err) {
        showStatus(String(err), "error");
      }
    });

    actions.append(copyBtn, pinBtn, delBtn);
    footer.append(meta, actions);
    li.append(textEl, footer);
    listEl.appendChild(li);
  }
}

async function refresh() {
  showStatus("Loading clipboard history...");
  try {
    const raw = await dbAllDocs("clip:");
    allDocs = (raw || []).map(unwrapDoc);
    showStatus("");
    render();
  } catch (err) {
    showStatus(`Failed to load clips: ${err}`, "error");
  }
}

addFormEl.addEventListener("submit", async (e) => {
  e.preventDefault();
  const text = newClipEl.value.trim();
  if (!text) return;
  try {
    await dbPut({
      _id: `clip:${Date.now()}`,
      data: { text, pinned: false, createdAt: Date.now() },
    });
    newClipEl.value = "";
    showStatus("Snippet saved.", "success");
    await refresh();
  } catch (err) {
    showStatus(`Failed to save snippet: ${err}`, "error");
  }
});

filterEl.addEventListener("input", () => render());

for (const tab of tabEls) {
  tab.addEventListener("click", () => {
    activeTab = tab.dataset.filter || "all";
    for (const el of tabEls) {
      const isActive = el === tab;
      el.classList.toggle("active", isActive);
      el.setAttribute("aria-selected", String(isActive));
    }
    render();
  });
}

summarizeBtn.addEventListener("click", async () => {
  const sample = allDocs.slice(0, 8).map((d) => `- ${d.text.slice(0, 140)}`).join("\n");
  if (!sample) {
    showStatus("Save or copy clips before summarizing.", "error");
    return;
  }
  summarizeBtn.disabled = true;
  summaryEl.className = "summary-box";
  summaryEl.textContent = "Summarizing recent clips...";
  try {
    if (!invoke) {
      summaryEl.textContent = `Preview summary (${allDocs.length} clip(s)):\n${sample}`;
      return;
    }
    const output = await invoke("plugin:aitools|ai_chat", {
      request: {
        profile: "fast",
        system: "Summarize these recent clipboard snippets into 3 concise bullet points.",
        prompt: sample,
      },
    });
    summaryEl.textContent = typeof output === "string" ? output : JSON.stringify(output, null, 2);
  } catch (err) {
    summaryEl.className = "summary-box hidden";
    showStatus(`AI summary failed: ${err}`, "error");
  } finally {
    summarizeBtn.disabled = false;
  }
});

globalThis.addEventListener("aitools:context-enter", (e) => {
  const ctx = e.detail || {};
  const text = ctx.selected_text ?? ctx.clipboard_preview ?? ctx.selection ?? ctx.clipboard;
  if (text && !newClipEl.value) {
    newClipEl.value = String(text);
  }
});

void refresh();
