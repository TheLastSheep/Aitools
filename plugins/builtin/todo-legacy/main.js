const LOCAL_KEY = "aitools:preview:todo-legacy";

const formEl = document.getElementById("todo-form");
const inputEl = document.getElementById("todo-input");
const listEl = document.getElementById("todos");
const emptyEl = document.getElementById("empty");
const statusEl = document.getElementById("status");
const summaryCountEl = document.getElementById("summary-count");
const clearDoneBtn = document.getElementById("clear-done");
const tabEls = Array.from(document.querySelectorAll(".tab"));

let activeFilter = "all";
let todos = [];

function getBridgeDb() {
  const bridge = window.utools || window.ztools || window.aitools;
  return bridge && bridge.db ? bridge.db : null;
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

function unwrapTodo(doc) {
  const payload = doc && typeof doc.data === "object" && doc.data !== null ? doc.data : doc;
  return {
    _id: doc._id,
    _rev: doc._rev,
    text: String(payload.text ?? doc.text ?? ""),
    done: Boolean(payload.done ?? doc.done ?? false),
    createdAt: Number(payload.createdAt ?? doc.createdAt ?? 0),
  };
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

function loadDocs() {
  const db = getBridgeDb();
  if (db) {
    try {
      const raw = db.allDocs("todo:") || [];
      if (Array.isArray(raw)) {
        return raw.map(unwrapTodo);
      }
    } catch (err) {
      showStatus(`Storage read error: ${err}`, "error");
    }
  }
  return readLocal()
    .filter((d) => String(d._id || "").startsWith("todo:"))
    .map(unwrapTodo);
}

function putDoc(doc) {
  const db = getBridgeDb();
  if (db) {
    const res = db.put(doc);
    if (res && res.error) {
      throw new Error(res.message || res.name || "Failed to save todo");
    }
    return res;
  }
  const docs = readLocal();
  const next = { ...doc, _rev: `preview-${Date.now()}` };
  const idx = docs.findIndex((d) => d._id === doc._id);
  if (idx >= 0) docs[idx] = next;
  else docs.unshift(next);
  writeLocal(docs);
  return { id: doc._id, rev: next._rev, ok: true };
}

function removeDoc(doc) {
  const db = getBridgeDb();
  if (db) {
    const res = db.remove(doc);
    if (res && res.error) {
      throw new Error(res.message || res.name || "Failed to delete todo");
    }
    return res;
  }
  writeLocal(readLocal().filter((d) => d._id !== doc._id));
  return { id: doc._id, ok: true };
}

function render() {
  const visible = todos
    .filter((t) => {
      if (activeFilter === "active") return !t.done;
      if (activeFilter === "done") return t.done;
      return true;
    })
    .sort((a, b) => Number(a.done) - Number(b.done) || b.createdAt - a.createdAt);

  const remaining = todos.filter((t) => !t.done).length;
  summaryCountEl.textContent = `${remaining} remaining`;

  listEl.innerHTML = "";
  emptyEl.classList.toggle("hidden", visible.length > 0);

  for (const item of visible) {
    const li = document.createElement("li");
    li.className = item.done ? "todo-row done" : "todo-row";

    const label = document.createElement("label");
    label.className = "todo-label";

    const checkbox = document.createElement("input");
    checkbox.type = "checkbox";
    checkbox.checked = item.done;
    checkbox.addEventListener("change", () => {
      try {
        putDoc({
          _id: item._id,
          _rev: item._rev,
          text: item.text,
          done: checkbox.checked,
          createdAt: item.createdAt,
        });
        refresh();
      } catch (err) {
        showStatus(String(err), "error");
      }
    });

    const span = document.createElement("span");
    span.className = "todo-text";
    span.textContent = item.text || item._id;

    label.append(checkbox, span);

    const delBtn = document.createElement("button");
    delBtn.type = "button";
    delBtn.className = "btn btn-ghost btn-danger";
    delBtn.textContent = "Delete";
    delBtn.addEventListener("click", () => {
      try {
        removeDoc(item);
        refresh();
      } catch (err) {
        showStatus(String(err), "error");
      }
    });

    li.append(label, delBtn);
    listEl.appendChild(li);
  }
}

function refresh() {
  todos = loadDocs();
  render();
}

formEl.addEventListener("submit", (e) => {
  e.preventDefault();
  const text = inputEl.value.trim();
  if (!text) return;
  try {
    putDoc({
      _id: `todo:${Date.now()}`,
      text,
      done: false,
      createdAt: Date.now(),
    });
    inputEl.value = "";
    showStatus("");
    refresh();
  } catch (err) {
    showStatus(String(err), "error");
  }
});

clearDoneBtn.addEventListener("click", () => {
  try {
    for (const item of todos.filter((t) => t.done)) {
      removeDoc(item);
    }
    refresh();
  } catch (err) {
    showStatus(String(err), "error");
  }
});

for (const tab of tabEls) {
  tab.addEventListener("click", () => {
    activeFilter = tab.dataset.filter || "all";
    for (const el of tabEls) {
      const isActive = el === tab;
      el.classList.toggle("active", isActive);
      el.setAttribute("aria-selected", String(isActive));
    }
    render();
  });
}

const bridge = window.utools || window.ztools || window.aitools;
if (bridge && typeof bridge.onPluginEnter === "function") {
  bridge.onPluginEnter((action) => {
    if (action && typeof action.payload === "string" && action.type === "over") {
      inputEl.value = action.payload;
    }
    refresh();
  });
}

refresh();
