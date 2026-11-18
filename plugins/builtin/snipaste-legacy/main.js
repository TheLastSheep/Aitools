const LOCAL_KEY = "aitools:preview:snipaste-legacy";

const enterModeEl = document.getElementById("enter-mode");
const swatchEl = document.getElementById("swatch");
const colorInputEl = document.getElementById("color-input");
const previewHexEl = document.getElementById("color-preview-hex");
const outHexEl = document.getElementById("out-hex");
const outRgbEl = document.getElementById("out-rgb");
const outHslEl = document.getElementById("out-hsl");
const pinFormEl = document.getElementById("pin-form");
const pinInputEl = document.getElementById("pin-input");
const pinsEl = document.getElementById("pins");
const emptyEl = document.getElementById("empty");
const statusEl = document.getElementById("status");

let pins = [];

function getBridge() {
  return window.utools || window.ztools || window.aitools || null;
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

function copyTextSafe(text) {
  const bridge = getBridge();
  if (bridge && typeof bridge.copyText === "function") {
    bridge.copyText(text);
    showStatus(`Copied "${text}"`, "success");
    return;
  }
  if (navigator.clipboard && typeof navigator.clipboard.writeText === "function") {
    void navigator.clipboard.writeText(text).then(
      () => showStatus(`Copied "${text}"`, "success"),
      () => showStatus("Clipboard copy unavailable in this context.", "error"),
    );
  }
}

function parseColor(raw) {
  const val = raw.trim();
  const hexMatch = /^#?([0-9a-fA-F]{6}|[0-9a-fA-F]{3})$/.exec(val);
  if (hexMatch) {
    let h = hexMatch[1];
    if (h.length === 3) {
      h = h.split("").map((c) => c + c).join("");
    }
    return {
      r: Number.parseInt(h.slice(0, 2), 16),
      g: Number.parseInt(h.slice(2, 4), 16),
      b: Number.parseInt(h.slice(4, 6), 16),
    };
  }
  const rgbMatch = /^rgba?\(\s*(\d{1,3})\s*,\s*(\d{1,3})\s*,\s*(\d{1,3})/i.exec(val);
  if (rgbMatch) {
    return {
      r: Math.min(255, Number.parseInt(rgbMatch[1], 10)),
      g: Math.min(255, Number.parseInt(rgbMatch[2], 10)),
      b: Math.min(255, Number.parseInt(rgbMatch[3], 10)),
    };
  }
  return null;
}

function rgbToHsl(r, g, b) {
  const rn = r / 255;
  const gn = g / 255;
  const bn = b / 255;
  const max = Math.max(rn, gn, bn);
  const min = Math.min(rn, gn, bn);
  let h = 0;
  let s = 0;
  const l = (max + min) / 2;

  if (max !== min) {
    const d = max - min;
    s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
    switch (max) {
      case rn:
        h = (gn - bn) / d + (gn < bn ? 6 : 0);
        break;
      case gn:
        h = (bn - rn) / d + 2;
        break;
      default:
        h = (rn - gn) / d + 4;
    }
    h /= 6;
  }
  return {
    h: Math.round(h * 360),
    s: Math.round(s * 100),
    l: Math.round(l * 100),
  };
}

function updateColor(raw) {
  const parsed = parseColor(raw);
  if (!parsed) return;
  const hex =
    "#" +
    [parsed.r, parsed.g, parsed.b]
      .map((n) => n.toString(16).padStart(2, "0"))
      .join("")
      .toUpperCase();
  const rgb = `rgb(${parsed.r}, ${parsed.g}, ${parsed.b})`;
  const hslObj = rgbToHsl(parsed.r, parsed.g, parsed.b);
  const hsl = `hsl(${hslObj.h}, ${hslObj.s}%, ${hslObj.l}%)`;

  swatchEl.style.backgroundColor = hex;
  previewHexEl.textContent = hex;
  outHexEl.textContent = hex;
  outRgbEl.textContent = rgb;
  outHslEl.textContent = hsl;
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

function unwrapPin(doc) {
  const payload = doc && typeof doc.data === "object" && doc.data !== null ? doc.data : doc;
  return {
    _id: doc._id,
    _rev: doc._rev,
    text: String(payload.text ?? doc.text ?? ""),
  };
}

function loadPins() {
  const db = getBridge()?.db;
  if (db) {
    try {
      const docs = db.allDocs("pin:") || [];
      if (Array.isArray(docs)) {
        return docs.map(unwrapPin);
      }
    } catch {
      // Fallback below
    }
  }
  return readLocal().map(unwrapPin);
}

function savePin(text) {
  const doc = { _id: `pin:${Date.now()}`, text };
  const db = getBridge()?.db;
  if (db) {
    const res = db.put(doc);
    if (res && res.error) {
      throw new Error(res.message || "Failed to save pin");
    }
    return;
  }
  const docs = readLocal();
  docs.unshift({ ...doc, _rev: `preview-${Date.now()}` });
  writeLocal(docs);
}

function removePin(item) {
  const db = getBridge()?.db;
  if (db) {
    db.remove(item);
    return;
  }
  writeLocal(readLocal().filter((d) => d._id !== item._id));
}

function renderPins() {
  pinsEl.innerHTML = "";
  emptyEl.classList.toggle("hidden", pins.length > 0);
  for (const item of pins) {
    const li = document.createElement("li");
    li.className = "pin-item";

    const textEl = document.createElement("span");
    textEl.className = "pin-text";
    textEl.textContent = item.text || item._id;

    const actions = document.createElement("div");
    actions.className = "pin-actions";

    const copyBtn = document.createElement("button");
    copyBtn.type = "button";
    copyBtn.className = "btn btn-ghost";
    copyBtn.textContent = "Copy";
    copyBtn.addEventListener("click", () => copyTextSafe(item.text));

    const delBtn = document.createElement("button");
    delBtn.type = "button";
    delBtn.className = "btn btn-ghost btn-danger";
    delBtn.textContent = "Unpin";
    delBtn.addEventListener("click", () => {
      removePin(item);
      refreshPins();
    });

    actions.append(copyBtn, delBtn);
    li.append(textEl, actions);
    pinsEl.appendChild(li);
  }
}

function refreshPins() {
  pins = loadPins();
  renderPins();
}

colorInputEl.addEventListener("input", () => updateColor(colorInputEl.value));

for (const btn of document.querySelectorAll(".copy-conv")) {
  btn.addEventListener("click", () => {
    const target = document.getElementById(btn.dataset.target || "");
    if (target) copyTextSafe(target.textContent || "");
  });
}

pinFormEl.addEventListener("submit", (e) => {
  e.preventDefault();
  const text = pinInputEl.value.trim();
  if (!text) return;
  try {
    savePin(text);
    if (parseColor(text)) {
      colorInputEl.value = text;
      updateColor(text);
    }
    pinInputEl.value = "";
    refreshPins();
  } catch (err) {
    showStatus(String(err), "error");
  }
});

const bridge = getBridge();
if (bridge && typeof bridge.onPluginEnter === "function") {
  bridge.onPluginEnter((action) => {
    if (!action) return;
    enterModeEl.textContent = action.code || "Active";
    if (typeof action.payload === "string" && action.payload.trim()) {
      const payload = action.payload.trim();
      if (parseColor(payload)) {
        colorInputEl.value = payload;
        updateColor(payload);
      } else {
        pinInputEl.value = payload;
      }
    }
    refreshPins();
  });
}

updateColor(colorInputEl.value);
refreshPins();
