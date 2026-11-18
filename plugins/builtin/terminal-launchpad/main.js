const invoke = window.__TAURI_INTERNALS__?.invoke;
const LOCAL_KEY = "aitools:preview:terminal-launchpad";

const filterEl = document.getElementById("filter");
const formEl = document.getElementById("shortcut-form");
const nameInputEl = document.getElementById("name-input");
const cwdInputEl = document.getElementById("cwd-input");
const cmdInputEl = document.getElementById("cmd-input");
const listEl = document.getElementById("shortcuts");
const emptyEl = document.getElementById("empty");
const statusEl = document.getElementById("status");
const countBadgeEl = document.getElementById("count-badge");

let items = [];

function unwrapDoc(doc) {
  const payload = doc && typeof doc.data === "object" && doc.data !== null ? doc.data : doc;
  return {
    _id: doc._id,
    _rev: doc._rev,
    name: String(payload.name ?? doc.name ?? doc._id.replace(/^cmd:/, "")),
    cwd: String(payload.cwd ?? doc.cwd ?? ""),
    command: String(payload.command ?? doc.command ?? ""),
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

function fullCommand(item) {
  return item.cwd ? `cd ${item.cwd} && ${item.command}` : item.command;
}

function render() {
  const q = filterEl.value.trim().toLowerCase();
  const filtered = items.filter(
    (it) =>
      !q ||
      it.name.toLowerCase().includes(q) ||
      it.cwd.toLowerCase().includes(q) ||
      it.command.toLowerCase().includes(q),
  );

  countBadgeEl.textContent = `${items.length} shortcut${items.length === 1 ? "" : "s"}`;
  listEl.innerHTML = "";
  emptyEl.classList.toggle("hidden", filtered.length > 0);

  for (const item of filtered) {
    const li = document.createElement("li");
    li.className = "cmd-card";

    const info = document.createElement("div");
    info.className = "cmd-info";

    const nameEl = document.createElement("div");
    nameEl.className = "cmd-name";
    nameEl.textContent = item.name;

    if (item.cwd) {
      const cwdEl = document.createElement("div");
      cwdEl.className = "cmd-cwd mono";
      cwdEl.textContent = item.cwd;
      info.append(nameEl, cwdEl);
    } else {
      info.append(nameEl);
    }

    const codeEl = document.createElement("code");
    codeEl.className = "cmd-code";
    codeEl.textContent = item.command;
    info.append(codeEl);

    const actions = document.createElement("div");
    actions.className = "cmd-actions";

    const copyBtn = document.createElement("button");
    copyBtn.type = "button";
    copyBtn.className = "btn btn-ghost";
    copyBtn.textContent = "Copy Cmd";
    copyBtn.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(fullCommand(item));
        showStatus(`Copied "${item.name}" to clipboard.`, "success");
      } catch {
        showStatus("Clipboard copy unavailable in this context.", "error");
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
        showStatus(`Failed to delete shortcut: ${err}`, "error");
      }
    });

    actions.append(copyBtn, delBtn);
    li.append(info, actions);
    listEl.appendChild(li);
  }
}

async function refresh() {
  showStatus("Loading shortcuts...");
  try {
    const raw = await dbAllDocs("cmd:");
    items = (raw || []).map(unwrapDoc);
    showStatus("");
    render();
  } catch (err) {
    showStatus(`Failed to load shortcuts: ${err}`, "error");
  }
}

formEl.addEventListener("submit", async (e) => {
  e.preventDefault();
  const name = nameInputEl.value.trim();
  const cwd = cwdInputEl.value.trim();
  const command = cmdInputEl.value.trim();
  if (!name || !command) return;

  const slug = name.toLowerCase().replace(/[^a-z0-9_-]+/g, "-").replace(/^-|-$/g, "") || String(Date.now());
  try {
    await dbPut({
      _id: `cmd:${slug}`,
      data: { name, cwd, command },
    });
    nameInputEl.value = "";
    cwdInputEl.value = "";
    cmdInputEl.value = "";
    showStatus(`Saved shortcut "${name}".`, "success");
    await refresh();
  } catch (err) {
    showStatus(`Failed to save shortcut: ${err}`, "error");
  }
});

filterEl.addEventListener("input", render);

void refresh();
