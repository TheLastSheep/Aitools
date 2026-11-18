const invoke = window.__TAURI_INTERNALS__?.invoke;

const sourceEl = document.getElementById("source");
const targetLangEl = document.getElementById("target-lang");
const profileEl = document.getElementById("profile");
const charCountEl = document.getElementById("char-count");
const translateBtn = document.getElementById("translate-btn");
const clearBtn = document.getElementById("clear-btn");
const copyBtn = document.getElementById("copy-btn");
const resultEl = document.getElementById("result");
const statusEl = document.getElementById("status");

let hasOutput = false;

function showStatus(msg, kind = "info") {
  if (!msg) {
    statusEl.className = "status hidden";
    statusEl.textContent = "";
    return;
  }
  statusEl.className = `status ${kind}`;
  statusEl.textContent = msg;
}

function updateCharCount() {
  charCountEl.textContent = `${sourceEl.value.length} chars`;
}

async function runTranslate() {
  const text = sourceEl.value.trim();
  if (!text) {
    showStatus("Enter or paste text to translate.", "error");
    return;
  }
  const targetLang = targetLangEl.value;
  const profile = profileEl.value;

  translateBtn.disabled = true;
  translateBtn.textContent = "Translating...";
  showStatus("");

  try {
    let translated;
    if (!invoke) {
      translated = `[Preview · ${profile} -> ${targetLang}]\n${text}`;
    } else {
      const response = await invoke("plugin:aitools|ai_chat", {
        request: {
          profile,
          system: "Translate accurately. Preserve technical identifiers, file paths, and inline code spans verbatim.",
          prompt: `Translate the following text into ${targetLang}:\n\n${text}`,
        },
      });
      translated =
        typeof response === "string"
          ? response
          : response && typeof response.content === "string"
            ? response.content
            : JSON.stringify(response, null, 2);
    }
    resultEl.textContent = translated;
    hasOutput = true;
    copyBtn.disabled = false;
  } catch (err) {
    showStatus(`Translation failed: ${err}`, "error");
  } finally {
    translateBtn.disabled = false;
    translateBtn.textContent = "Translate";
  }
}

sourceEl.addEventListener("input", updateCharCount);
translateBtn.addEventListener("click", () => void runTranslate());
clearBtn.addEventListener("click", () => {
  sourceEl.value = "";
  resultEl.textContent = "Translation will appear here.";
  hasOutput = false;
  copyBtn.disabled = true;
  showStatus("");
  updateCharCount();
  sourceEl.focus();
});

copyBtn.addEventListener("click", async () => {
  if (!hasOutput) return;
  try {
    await navigator.clipboard.writeText(resultEl.textContent || "");
    showStatus("Translation copied to clipboard.", "success");
  } catch {
    showStatus("Clipboard copy unavailable in this context.", "error");
  }
});

globalThis.addEventListener("aitools:context-enter", (e) => {
  const ctx = e.detail || {};
  const candidate = ctx.selected_text ?? ctx.clipboard_preview ?? ctx.selection ?? ctx.clipboard;
  if (candidate && !sourceEl.value.trim()) {
    sourceEl.value = String(candidate);
    updateCharCount();
  }
});

updateCharCount();
