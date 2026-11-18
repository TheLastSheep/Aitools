'use strict';

const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const Module = require('node:module');
const { Console } = require('node:console');
const { Worker, isMainThread, parentPort, workerData } = require('node:worker_threads');

const CHUNK_BYTES = 512 * 1024;
const CTRL_OFFSETS = 20; // 5 x Int32 = 20 bytes

if (!isMainThread) {
  const { sab } = workerData;
  const ctrl = new Int32Array(sab, 0, 5);
  const dataBuf = new Uint8Array(sab, CTRL_OFFSETS);
  const readBuf = Buffer.alloc(65536);
  let acc = '';
  let ackExpected = 0;

  function pushSyncReply(replyId, lineBytes) {
    const total = lineBytes.length;
    let offset = 0;
    while (offset < total || total === 0) {
      const end = Math.min(offset + dataBuf.length, total);
      const chunkLen = end - offset;
      if (chunkLen > 0) {
        dataBuf.set(lineBytes.subarray(offset, end), 0);
      }
      const isLast = end >= total;
      Atomics.store(ctrl, 1, chunkLen);
      Atomics.store(ctrl, 2, replyId);
      Atomics.store(ctrl, 3, isLast ? 1 : 0);
      Atomics.add(ctrl, 0, 1);
      Atomics.notify(ctrl, 0);
      offset = end;
      if (isLast) {
        break;
      }
      // Wait for main thread to copy this chunk before overwriting dataBuf.
      ackExpected += 1;
      while (Atomics.load(ctrl, 4) < ackExpected) {
        Atomics.wait(ctrl, 4, Atomics.load(ctrl, 4), 1000);
      }
    }
  }

  while (true) {
    let n = 0;
    try {
      n = fs.readSync(0, readBuf, 0, readBuf.length, null);
    } catch (err) {
      if (err && err.code === 'EAGAIN') {
        Atomics.wait(ctrl, 4, Atomics.load(ctrl, 4), 2);
        continue;
      }
      break;
    }
    if (n === 0) {
      Atomics.store(ctrl, 3, 2);
      Atomics.add(ctrl, 0, 1);
      Atomics.notify(ctrl, 0);
      parentPort.postMessage({ kind: 'eof' });
      break;
    }
    acc += readBuf.toString('utf8', 0, n);
    let nlIdx;
    while ((nlIdx = acc.indexOf('\n')) !== -1) {
      const line = acc.slice(0, nlIdx);
      acc = acc.slice(nlIdx + 1);
      if (!line.trim()) continue;
      let msg;
      try {
        msg = JSON.parse(line);
      } catch (err) {
        parentPort.postMessage({ kind: 'parse_error', error: String(err && err.message ? err.message : err) });
        continue;
      }
      if (msg && msg.type === 'rpc_reply' && typeof msg.rpc_reply_id === 'number') {
        pushSyncReply(msg.rpc_reply_id, Buffer.from(line, 'utf8'));
      } else {
        parentPort.postMessage({ kind: 'msg', msg });
      }
    }
  }
  process.exit(0);
}

// --- Main Thread ---

// Keep fd 1 exclusive to NDJSON protocol frames so plugin console.log never corrupts stdio.
function writeProtocolFrame(obj) {
  const payload = JSON.stringify(obj) + '\n';
  fs.writeSync(1, payload, null, 'utf8');
}

const stderrConsole = new Console({ stdout: process.stderr, stderr: process.stderr });
global.console = stderrConsole;
process.stdout.write = function(chunk, encoding, callback) {
  return process.stderr.write(chunk, encoding, callback);
};

const majorNode = Number(String(process.versions.node || '0').split('.')[0]);
if (!Number.isFinite(majorNode) || majorNode < 18) {
  writeProtocolFrame({
    type: 'startup_error',
    error: `Node.js >= 18 required, found ${process.versions.node || 'unknown'}`,
  });
  process.exit(1);
}

const sab = new SharedArrayBuffer(CTRL_OFFSETS + CHUNK_BYTES);
const ctrl = new Int32Array(sab, 0, 5);
const dataBuf = new Uint8Array(sab, CTRL_OFFSETS);

const stdinWorker = new Worker(__filename, { workerData: { sab } });
stdinWorker.on('error', (err) => {
  process.stderr.write('[aitools-node-runner] stdin worker error: ' + String(err) + '\n');
  process.exit(1);
});

let nextRpcId = 1;
const SYNC_RPC_TIMEOUT_MS = 15000;

function syncHostCall(op, payload) {
  const myId = nextRpcId++;
  let seqSeen = Atomics.load(ctrl, 0);
  writeProtocolFrame({
    type: 'rpc_request',
    rpc_id: myId,
    op: String(op || ''),
    payload: payload === undefined ? null : payload,
  });

  const chunks = [];
  const deadline = Date.now() + SYNC_RPC_TIMEOUT_MS;
  while (true) {
    while (Atomics.load(ctrl, 0) === seqSeen) {
      const remaining = deadline - Date.now();
      if (remaining <= 0) {
        throw new Error(`Host RPC timed out after ${SYNC_RPC_TIMEOUT_MS}ms for op '${op}'`);
      }
      const status = Atomics.wait(ctrl, 0, seqSeen, remaining);
      if (status === 'timed-out') {
        throw new Error(`Host RPC timed out after ${SYNC_RPC_TIMEOUT_MS}ms for op '${op}'`);
      }
    }
    seqSeen = Atomics.load(ctrl, 0);
    const flags = Atomics.load(ctrl, 3);
    if (flags === 2) {
      throw new Error(`Host stdio closed while waiting for RPC '${op}'`);
    }
    const replyId = Atomics.load(ctrl, 2);
    const len = Atomics.load(ctrl, 1);
    if (replyId !== myId) {
      if (flags === 0) {
        Atomics.add(ctrl, 4, 1);
        Atomics.notify(ctrl, 4);
      }
      continue;
    }
    if (len > 0) {
      chunks.push(Buffer.from(dataBuf.slice(0, len)));
    }
    if (flags === 0) {
      Atomics.add(ctrl, 4, 1);
      Atomics.notify(ctrl, 4);
      continue;
    }
    break;
  }

  const raw = Buffer.concat(chunks).toString('utf8');
  const parsed = JSON.parse(raw);
  if (!parsed || parsed.ok !== true) {
    throw new Error(String((parsed && parsed.error) || `Host RPC failed for '${op}'`));
  }
  return parsed.data;
}

// --- Pre-require DOM & Electron Facade ---

global.window = global;
global.self = global;
globalThis.window = globalThis;

function createStoragePolyfill() {
  const store = new Map();
  return {
    get length() {
      return store.size;
    },
    key(index) {
      const keys = Array.from(store.keys());
      return index >= 0 && index < keys.length ? keys[index] : null;
    },
    getItem(k) {
      const key = String(k);
      return store.has(key) ? store.get(key) : null;
    },
    setItem(k, v) {
      store.set(String(k), String(v));
    },
    removeItem(k) {
      store.delete(String(k));
    },
    clear() {
      store.clear();
    },
  };
}

class StubEvent {
  constructor(type, init) {
    this.type = String(type || '');
    this.bubbles = Boolean(init && init.bubbles);
    this.cancelable = Boolean(init && init.cancelable);
    this.defaultPrevented = false;
  }
  preventDefault() {
    this.defaultPrevented = true;
  }
  stopPropagation() {}
}

class StubCustomEvent extends StubEvent {
  constructor(type, init) {
    super(type, init);
    this.detail = init && init.detail !== undefined ? init.detail : null;
  }
}

class StubNode {
  constructor() {
    this.childNodes = [];
    this.parentNode = null;
    this.textContent = '';
  }
  appendChild(child) {
    if (child && typeof child === 'object') {
      child.parentNode = this;
      this.childNodes.push(child);
    }
    return child;
  }
  removeChild(child) {
    const idx = this.childNodes.indexOf(child);
    if (idx >= 0) this.childNodes.splice(idx, 1);
    return child;
  }
  addEventListener() {}
  removeEventListener() {}
  dispatchEvent() {
    return true;
  }
}

class StubElement extends StubNode {
  constructor(tagName) {
    super();
    this.tagName = String(tagName || 'DIV').toUpperCase();
    this.style = {};
    this.attributes = Object.create(null);
    const classes = new Set();
    this.classList = {
      add: (...c) => c.forEach((x) => classes.add(String(x))),
      remove: (...c) => c.forEach((x) => classes.delete(String(x))),
      toggle: (x, force) => {
        const key = String(x);
        const on = force !== undefined ? Boolean(force) : !classes.has(key);
        if (on) classes.add(key);
        else classes.delete(key);
        return on;
      },
      contains: (x) => classes.has(String(x)),
    };
  }
  setAttribute(name, val) {
    this.attributes[String(name)] = String(val);
  }
  getAttribute(name) {
    return Object.prototype.hasOwnProperty.call(this.attributes, String(name))
      ? this.attributes[String(name)]
      : null;
  }
  removeAttribute(name) {
    delete this.attributes[String(name)];
  }
  remove() {
    if (this.parentNode && typeof this.parentNode.removeChild === 'function') {
      this.parentNode.removeChild(this);
    }
  }
  querySelector() {
    return null;
  }
  querySelectorAll() {
    return [];
  }
}

class StubHTMLElement extends StubElement {}

global.Event = global.Event || StubEvent;
global.CustomEvent = global.CustomEvent || StubCustomEvent;
global.Node = global.Node || StubNode;
global.Element = global.Element || StubElement;
global.HTMLElement = global.HTMLElement || StubHTMLElement;

const stubDocRoot = new StubHTMLElement('html');
const stubHead = new StubHTMLElement('head');
const stubBody = new StubHTMLElement('body');
global.document = {
  readyState: 'complete',
  title: '',
  cookie: '',
  documentElement: stubDocRoot,
  head: stubHead,
  body: stubBody,
  createElement: (tag) => new StubHTMLElement(tag),
  createTextNode: (text) => {
    const n = new StubNode();
    n.textContent = String(text == null ? '' : text);
    return n;
  },
  getElementById: () => null,
  querySelector: () => null,
  querySelectorAll: () => [],
  getElementsByTagName: () => [],
  getElementsByClassName: () => [],
  addEventListener: () => {},
  removeEventListener: () => {},
  dispatchEvent: () => true,
  execCommand: () => false,
};

const navStub = {
  userAgent: 'Mozilla/5.0 (aitools; NodePreload)',
  platform: process.platform === 'darwin' ? 'MacIntel' : (process.platform === 'win32' ? 'Win32' : 'Linux x86_64'),
  language: 'zh-CN',
  languages: ['zh-CN', 'en-US'],
  clipboard: {
    writeText: async (text) => syncHostCall('clipboard/copyText', { text: String(text == null ? '' : text) }),
    readText: async () => {
      try {
        return String(syncHostCall('clipboard/readText', {}) || '');
      } catch (_) {
        return '';
      }
    },
  },
};
try {
  Object.defineProperty(global, 'navigator', {
    value: navStub,
    configurable: true,
    writable: true,
  });
} catch (_) {}

const locStub = {
  href: 'aitools-plugin://unknown/index.html',
  protocol: 'aitools-plugin:',
  host: 'unknown',
  hostname: 'unknown',
  pathname: '/index.html',
  search: '',
  hash: '',
  origin: 'aitools-plugin://unknown',
};
try {
  Object.defineProperty(global, 'location', {
    value: locStub,
    configurable: true,
    writable: true,
  });
} catch (_) {}

global.localStorage = createStoragePolyfill();
global.sessionStorage = createStoragePolyfill();
global.matchMedia = (q) => ({
  matches: String(q || '').includes('dark')
    ? (() => {
        try {
          return Boolean(syncHostCall('system/isDarkColors', {}));
        } catch (_) {
          return false;
        }
      })()
    : false,
  media: String(q || ''),
  addEventListener: () => {},
  removeEventListener: () => {},
  addListener: () => {},
  removeListener: () => {},
});
global.addEventListener = () => {};
global.removeEventListener = () => {};
global.dispatchEvent = () => true;

function parsePngDimensions(buf) {
  if (Buffer.isBuffer(buf) && buf.length >= 24 &&
      buf[0] === 0x89 && buf[1] === 0x50 && buf[2] === 0x4e && buf[3] === 0x47) {
    return { width: buf.readUInt32BE(16), height: buf.readUInt32BE(20) };
  }
  return { width: 0, height: 0 };
}

function makeNativeImage(buf, mimeType) {
  const buffer = Buffer.isBuffer(buf) ? buf : Buffer.alloc(0);
  const mime = mimeType || 'image/png';
  const size = parsePngDimensions(buffer);
  return {
    isEmpty: () => buffer.length === 0,
    toDataURL: () => (buffer.length === 0 ? '' : `data:${mime};base64,${buffer.toString('base64')}`),
    toPNG: () => buffer,
    toJPEG: () => buffer,
    toBitmap: () => buffer,
    getSize: () => ({ width: size.width, height: size.height }),
    resize: () => makeNativeImage(buffer, mime),
    crop: () => makeNativeImage(buffer, mime),
  };
}

function createElectronShim() {
  const ipcListeners = new Map();
  return {
    clipboard: {
      readText: () => {
        try {
          return String(syncHostCall('clipboard/readText', {}) || '');
        } catch (_) {
          return '';
        }
      },
      writeText: (text) => {
        syncHostCall('clipboard/copyText', { text: String(text == null ? '' : text) });
      },
      readHTML: () => {
        try {
          return String(syncHostCall('clipboard/readHTML', {}) || '');
        } catch (_) {
          return '';
        }
      },
      writeHTML: (html) => {
        try {
          syncHostCall('clipboard/writeHTML', { html: String(html == null ? '' : html) });
        } catch (_) {}
      },
      readImage: () => {
        try {
          const raw = syncHostCall('clipboard/readImage', {});
          if (typeof raw === 'string' && raw.startsWith('data:')) {
            const idx = raw.indexOf(',');
            const meta = raw.slice(5, idx);
            const mime = meta.split(';')[0] || 'image/png';
            return makeNativeImage(Buffer.from(raw.slice(idx + 1), 'base64'), mime);
          }
        } catch (_) {}
        return makeNativeImage(Buffer.alloc(0), 'image/png');
      },
      writeImage: (img) => {
        const dataUrl = img && typeof img.toDataURL === 'function' ? img.toDataURL() : String(img || '');
        syncHostCall('clipboard/copyImage', { image: dataUrl });
      },
      readBuffer: () => Buffer.alloc(0),
      writeBuffer: () => {},
      availableFormats: () => [],
      clear: () => {
        try {
          syncHostCall('clipboard/clear', {});
        } catch (_) {}
      },
    },
    nativeImage: {
      createEmpty: () => makeNativeImage(Buffer.alloc(0), 'image/png'),
      createFromPath: (filePath) => {
        try {
          const p = String(filePath || '');
          const data = fs.readFileSync(p);
          const ext = path.extname(p).toLowerCase();
          const mime = ext === '.jpg' || ext === '.jpeg' ? 'image/jpeg'
            : (ext === '.svg' ? 'image/svg+xml' : (ext === '.webp' ? 'image/webp' : 'image/png'));
          return makeNativeImage(data, mime);
        } catch (_) {
          return makeNativeImage(Buffer.alloc(0), 'image/png');
        }
      },
      createFromDataURL: (dataUrl) => {
        const str = String(dataUrl || '');
        const match = /^data:([^;,]+)?(?:;base64)?,(.*)$/i.exec(str);
        if (!match) return makeNativeImage(Buffer.alloc(0), 'image/png');
        return makeNativeImage(Buffer.from(match[2] || '', 'base64'), match[1] || 'image/png');
      },
      createFromBuffer: (buf) => makeNativeImage(Buffer.isBuffer(buf) ? buf : Buffer.from(buf || []), 'image/png'),
    },
    shell: {
      openExternal: (url) => Promise.resolve().then(() => {
        syncHostCall('shell/openExternal', { url: String(url || '') });
      }),
      openPath: (fullPath) => Promise.resolve().then(() => {
        syncHostCall('shell/openPath', { path: String(fullPath || '') });
        return '';
      }),
      showItemInFolder: (fullPath) => {
        syncHostCall('shell/showItemInFolder', { path: String(fullPath || '') });
      },
      trashItem: (fullPath) => Promise.resolve().then(() => {
        syncHostCall('shell/trashItem', { path: String(fullPath || '') });
      }),
      beep: () => {
        syncHostCall('shell/beep', {});
      },
    },
    ipcRenderer: {
      on(channel, fn) {
        const key = String(channel);
        if (!ipcListeners.has(key)) ipcListeners.set(key, []);
        ipcListeners.get(key).push(fn);
        return this;
      },
      once(channel, fn) {
        const wrapper = (...args) => {
          this.removeListener(channel, wrapper);
          fn(...args);
        };
        return this.on(channel, wrapper);
      },
      off(channel, fn) {
        return this.removeListener(channel, fn);
      },
      removeListener(channel, fn) {
        const arr = ipcListeners.get(String(channel));
        if (arr) {
          const idx = arr.indexOf(fn);
          if (idx >= 0) arr.splice(idx, 1);
        }
        return this;
      },
      removeAllListeners(channel) {
        if (channel === undefined) ipcListeners.clear();
        else ipcListeners.delete(String(channel));
        return this;
      },
      send(channel, ...args) {
        try {
          syncHostCall('window/ipcSend', { channel: String(channel), args });
        } catch (_) {}
      },
      sendSync(channel, ...args) {
        try {
          return syncHostCall('window/ipcSendSync', { channel: String(channel), args });
        } catch (_) {
          return null;
        }
      },
      invoke(channel, ...args) {
        return Promise.resolve().then(() => {
          try {
            return syncHostCall('window/ipcInvoke', { channel: String(channel), args });
          } catch (_) {
            return null;
          }
        });
      },
    },
    screen: {
      getPrimaryDisplay: () => syncHostCall('display/getPrimaryDisplay', {}),
      getAllDisplays: () => syncHostCall('display/getAllDisplays', {}),
      getCursorScreenPoint: () => syncHostCall('display/getCursorScreenPoint', {}),
      getDisplayNearestPoint: (pt) => syncHostCall('display/getDisplayNearestPoint', pt || { x: 0, y: 0 }),
      dipToScreenPoint: (pt) => syncHostCall('display/dipToScreenPoint', pt || { x: 0, y: 0 }),
      screenToDipPoint: (pt) => syncHostCall('display/screenToDipPoint', pt || { x: 0, y: 0 }),
    },
    dialog: {
      showOpenDialogSync: (winOrOpts, maybeOpts) => {
        const opts = (maybeOpts && typeof maybeOpts === 'object') ? maybeOpts
          : (winOrOpts && typeof winOrOpts === 'object' && !winOrOpts.webContents ? winOrOpts : {});
        return syncHostCall('dialog/showOpenDialog', opts);
      },
      showOpenDialog: (winOrOpts, maybeOpts) => Promise.resolve().then(() => {
        const opts = (maybeOpts && typeof maybeOpts === 'object') ? maybeOpts
          : (winOrOpts && typeof winOrOpts === 'object' && !winOrOpts.webContents ? winOrOpts : {});
        const filePaths = syncHostCall('dialog/showOpenDialog', opts);
        const list = Array.isArray(filePaths) ? filePaths : [];
        return { canceled: list.length === 0, filePaths: list };
      }),
      showSaveDialogSync: (winOrOpts, maybeOpts) => {
        const opts = (maybeOpts && typeof maybeOpts === 'object') ? maybeOpts
          : (winOrOpts && typeof winOrOpts === 'object' && !winOrOpts.webContents ? winOrOpts : {});
        return syncHostCall('dialog/showSaveDialog', opts);
      },
      showSaveDialog: (winOrOpts, maybeOpts) => Promise.resolve().then(() => {
        const opts = (maybeOpts && typeof maybeOpts === 'object') ? maybeOpts
          : (winOrOpts && typeof winOrOpts === 'object' && !winOrOpts.webContents ? winOrOpts : {});
        const filePath = syncHostCall('dialog/showSaveDialog', opts);
        return { canceled: !filePath, filePath: filePath || undefined };
      }),
    },
    app: {
      getPath: (name) => syncHostCall('system/getPath', { name: String(name || '') }),
      getName: () => syncHostCall('system/getAppName', {}),
      getVersion: () => syncHostCall('system/getAppVersion', {}),
    },
  };
}

const electronShim = createElectronShim();
const origModuleLoad = Module._load;
Module._load = function(request, parent, isMain) {
  if (request === 'electron' || request === 'node:electron') {
    return electronShim;
  }
  return origModuleLoad.apply(this, arguments);
};

// --- Runtime State & Introspection ---

const FORBIDDEN_SEGMENTS = new Set(['__proto__', 'prototype', 'constructor']);
const INTERNAL_WINDOW_KEYS = new Set([
  'aitools', 'ztools', 'utools', 'rubick', 'exports',
  'window', 'self', 'global', 'globalThis',
  'document', 'navigator', 'location', 'localStorage', 'sessionStorage',
  'Event', 'CustomEvent', 'Node', 'Element', 'HTMLElement',
  'matchMedia', 'addEventListener', 'removeEventListener', 'dispatchEvent',
  'console',
]);

let loadedModuleExports = null;
let preRequireWindowKeys = new Set();

function isPlainSerializable(val, seen, depth) {
  if (val === null || val === undefined) return true;
  const t = typeof val;
  if (t === 'string' || t === 'boolean') return true;
  if (t === 'number') return Number.isFinite(val);
  if (t !== 'object') return false;
  if (depth > 4) return false;
  if (seen.has(val)) return false;
  seen.add(val);
  if (Array.isArray(val)) {
    return val.every((item) => isPlainSerializable(item, seen, depth + 1));
  }
  const proto = Object.getPrototypeOf(val);
  if (proto !== Object.prototype && proto !== null) return false;
  for (const key of Object.keys(val)) {
    if (!isPlainSerializable(val[key], seen, depth + 1)) return false;
  }
  return true;
}

function describeProp(propPath, value, seen, depth) {
  if (typeof value === 'function') {
    return { kind: 'function', path: propPath };
  }
  if (value === null || value === undefined || typeof value !== 'object') {
    const safeVal = (typeof value === 'number' && !Number.isFinite(value)) || value === undefined ? null : value;
    return { kind: 'value', path: propPath, value: safeVal };
  }
  if (Array.isArray(value) || isPlainSerializable(value, new WeakSet(), 0)) {
    try {
      return { kind: 'value', path: propPath, value: JSON.parse(JSON.stringify(value)) };
    } catch (_) {
      return { kind: 'value', path: propPath, value: null };
    }
  }
  if (depth >= 4 || seen.has(value)) {
    return null;
  }
  seen.add(value);
  const entries = [];
  for (const key of Object.keys(value)) {
    if (FORBIDDEN_SEGMENTS.has(key)) continue;
    let child;
    try {
      child = value[key];
    } catch (_) {
      continue;
    }
    if (child === undefined) continue;
    const childPath = propPath + '.' + key;
    const desc = describeProp(childPath, child, seen, depth + 1);
    if (desc) entries.push(desc);
  }
  return { kind: 'object', path: propPath, entries };
}

function collectExportedProps() {
  const out = [];
  const seen = new WeakSet();
  const keys = Object.keys(global.window);
  for (const key of keys) {
    if (INTERNAL_WINDOW_KEYS.has(key) || key.startsWith('__aitools_') || key.startsWith('__AITOOLS_')) {
      continue;
    }
    const isServicesWithItems = key === 'services' && global.window.services && Object.keys(global.window.services).length > 0;
    if (preRequireWindowKeys.has(key) && !isServicesWithItems) {
      continue;
    }
    let val;
    try {
      val = global.window[key];
    } catch (_) {
      continue;
    }
    if (val === undefined) continue;
    const desc = describeProp(key, val, seen, 0);
    if (desc) out.push(desc);
  }
  return out;
}

function getExportsRegistry() {
  const winExp = global.window.exports && typeof global.window.exports === 'object' ? global.window.exports : {};
  const modExp = loadedModuleExports && typeof loadedModuleExports === 'object' ? loadedModuleExports : {};
  return Object.assign({}, modExp, winExp);
}

function findFeatureEntry(featureCode) {
  const reg = getExportsRegistry();
  if (featureCode && Object.prototype.hasOwnProperty.call(reg, featureCode)) {
    return reg[featureCode];
  }
  return null;
}

function collectExportFeatures() {
  const reg = getExportsRegistry();
  const out = [];
  for (const [code, entry] of Object.entries(reg)) {
    if (entry && typeof entry === 'object' && entry.args) {
      out.push({ code, mode: String(entry.mode || 'list') });
    }
  }
  return out;
}

function callbackSetList(items) {
  const rawList = Array.isArray(items) ? items : [];
  const normalized = rawList.map((item) => {
    const obj = item && typeof item === 'object' ? item : { title: String(item == null ? '' : item) };
    return {
      title: String(obj.title == null ? '' : obj.title),
      description: String(obj.description == null ? '' : obj.description),
      icon: obj.icon == null ? null : String(obj.icon),
      payload: obj.payload !== undefined ? obj.payload : obj,
    };
  });
  syncHostCall('callbackSetList', { items: normalized });
  return true;
}

async function handleRequest(method, params) {
  const p = params && typeof params === 'object' ? params : {};
  switch (method) {
    case 'load': {
      const pluginId = String(p.pluginId || 'unknown');
      const pluginDir = fs.realpathSync(String(p.pluginDir || process.cwd()));
      const rawPreload = String(p.preloadPath || '');
      const candidate = path.isAbsolute(rawPreload) ? rawPreload : path.resolve(pluginDir, rawPreload);
      const resolvedPreload = fs.realpathSync(candidate);
      const rel = path.relative(pluginDir, resolvedPreload);
      if (rel === '..' || rel.startsWith('..' + path.sep) || path.isAbsolute(rel)) {
        throw new Error(`preload path escapes plugin directory: ${rawPreload}`);
      }

      const encodedId = encodeURIComponent(pluginId);
      locStub.href = `aitools-plugin://${encodedId}/index.html`;
      locStub.host = encodedId;
      locStub.hostname = encodedId;
      locStub.origin = `aitools-plugin://${encodedId}`;

      global.__AITOOLS_PLUGIN_ID__ = pluginId;
      global.__aitools_sync_call__ = syncHostCall;
      global.window.exports = {};
      global.window.services = {};

      if (typeof p.shimSource === 'string' && p.shimSource.length > 0) {
        vm.runInThisContext(p.shimSource, { filename: 'aitools-triple-global-shim.js' });
      }

      preRequireWindowKeys = new Set(Object.keys(global.window));
      const modExports = require(resolvedPreload);
      loadedModuleExports = (modExports && typeof modExports === 'object') ? modExports : null;

      const lc = global.__aitools_lifecycle__ || {};
      return {
        exportedProps: collectExportedProps(),
        exportFeatures: collectExportFeatures(),
        hasOnPluginEnter: Array.isArray(lc.enter) && lc.enter.length > 0,
        hasOnMainPush: Boolean(lc.mainPush && typeof lc.mainPush.cb === 'function'),
        hasOnPluginOut: Array.isArray(lc.out) && lc.out.length > 0,
      };
    }

    case 'enter': {
      const featureCode = String(p.featureCode || '');
      const action = p.action && typeof p.action === 'object' ? p.action : { code: featureCode };
      const entry = findFeatureEntry(featureCode);
      if (entry && entry.args && typeof entry.args.enter === 'function') {
        const res = await entry.args.enter(action, callbackSetList);
        return { handled: true, mode: String(entry.mode || 'list'), result: res === undefined ? null : res };
      }
      const lc = global.__aitools_lifecycle__ || {};
      if (Array.isArray(lc.enter) && lc.enter.length > 0) {
        if (!lc.readyFired && Array.isArray(lc.ready)) {
          lc.readyFired = true;
          for (const rcb of lc.ready.slice()) {
            await rcb(action);
          }
        }
        for (const cb of lc.enter.slice()) {
          await cb(action);
        }
        return { handled: true, mode: 'ui', result: null };
      }
      return { handled: false, mode: 'none', result: null };
    }

    case 'search': {
      const featureCode = String(p.featureCode || '');
      const action = p.action && typeof p.action === 'object' ? p.action : { code: featureCode };
      const query = String(p.query == null ? '' : p.query);
      const entry = findFeatureEntry(featureCode);
      if (entry && entry.args && typeof entry.args.search === 'function') {
        const res = await entry.args.search(action, query, callbackSetList);
        return { handled: true, result: res === undefined ? null : res };
      }
      const lc = global.__aitools_lifecycle__ || {};
      if (lc.subInput && typeof lc.subInput.onChange === 'function') {
        const res = await lc.subInput.onChange({ text: query });
        return { handled: true, result: res === undefined ? null : res };
      }
      return { handled: false, result: null };
    }

    case 'select': {
      const featureCode = String(p.featureCode || '');
      const action = p.action && typeof p.action === 'object' ? p.action : { code: featureCode };
      const rawItem = p.item && typeof p.item === 'object' ? p.item : { payload: p.item };
      const itemData = (rawItem.payload && typeof rawItem.payload === 'object' && !Array.isArray(rawItem.payload))
        ? Object.assign({}, rawItem, rawItem.payload)
        : rawItem;
      const entry = findFeatureEntry(featureCode);
      if (entry && entry.args && typeof entry.args.select === 'function') {
        const res = await entry.args.select(action, itemData, callbackSetList);
        return { handled: true, result: res === undefined ? null : res };
      }
      const lc = global.__aitools_lifecycle__ || {};
      if (lc.mainPush && typeof lc.mainPush.selectCb === 'function') {
        const res = await lc.mainPush.selectCb({ code: featureCode, option: itemData, action });
        return { handled: true, result: res === undefined ? null : res };
      }
      return { handled: false, result: null };
    }

    case 'main_push': {
      const featureCode = String(p.featureCode || '');
      const action = Object.assign({ code: featureCode }, p.action && typeof p.action === 'object' ? p.action : {});
      const lc = global.__aitools_lifecycle__ || {};
      if (lc.mainPush && typeof lc.mainPush.cb === 'function') {
        const res = await lc.mainPush.cb(action);
        return res === undefined ? [] : res;
      }
      return [];
    }

    case 'invoke_window_prop': {
      const propPath = String(p.propPath || '');
      const args = Array.isArray(p.args) ? p.args : [];
      const segments = propPath.split('.').filter(Boolean);
      if (segments.length === 0) {
        throw new Error('invoke_window_prop requires a non-empty propPath');
      }
      let parent = global.window;
      let target = global.window;
      for (const seg of segments) {
        if (FORBIDDEN_SEGMENTS.has(seg)) {
          throw new Error(`forbidden property segment: ${seg}`);
        }
        if (target == null) {
          throw new Error(`property path '${propPath}' is undefined at '${seg}'`);
        }
        parent = target;
        target = target[seg];
      }
      if (typeof target === 'function') {
        const res = await Reflect.apply(target, parent, args);
        return res === undefined ? null : res;
      }
      return target === undefined ? null : target;
    }

    case 'get_exported_props': {
      return collectExportedProps();
    }

    case 'plugin_out': {
      const isKill = Boolean(p.isKill);
      const lc = global.__aitools_lifecycle__ || {};
      if (Array.isArray(lc.out)) {
        for (const cb of lc.out.slice()) {
          await cb(isKill);
        }
      }
      return true;
    }

    case 'shutdown': {
      const lc = global.__aitools_lifecycle__ || {};
      if (Array.isArray(lc.detach)) {
        for (const cb of lc.detach.slice()) {
          try {
            await cb({});
          } catch (_) {}
        }
      }
      return true;
    }

    default:
      throw new Error(`unknown NodePreloadRuntime method: ${method}`);
  }
}

stdinWorker.on('message', async (envelope) => {
  if (!envelope || envelope.kind === 'eof') {
    process.exit(0);
    return;
  }
  if (envelope.kind === 'parse_error') {
    process.stderr.write('[aitools-node-runner] invalid JSON from host: ' + envelope.error + '\n');
    return;
  }
  const msg = envelope.msg;
  if (!msg || msg.type !== 'request' || typeof msg.id !== 'number') {
    return;
  }
  try {
    const result = await handleRequest(msg.method, msg.params);
    writeProtocolFrame({
      type: 'response',
      id: msg.id,
      ok: true,
      result: result === undefined ? null : result,
    });
    if (msg.method === 'shutdown') {
      setImmediate(() => process.exit(0));
    }
  } catch (err) {
    writeProtocolFrame({
      type: 'response',
      id: msg.id,
      ok: false,
      error: (err && err.stack) ? String(err.stack) : String(err && err.message ? err.message : err),
    });
  }
});

writeProtocolFrame({
  type: 'ready',
  nodeVersion: String(process.versions.node || ''),
  pid: process.pid,
});
