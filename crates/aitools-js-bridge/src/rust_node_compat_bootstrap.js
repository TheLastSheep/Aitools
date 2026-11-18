(function() {
  'use strict';

  globalThis.global = globalThis;
  globalThis.window = globalThis;
  globalThis.self = globalThis;

  function nativeCall(op, args) {
    const raw = globalThis.__aitools_native_node_op(
      String(op || ''),
      JSON.stringify(args === undefined ? {} : args)
    );
    const res = JSON.parse(raw);
    if (!res || res.ok !== true) {
      if (res && res.compatGap) {
        throw new Error('__AITOOLS_NODE_COMPAT_GAP__:' + String(res.compatGap));
      }
      const err = new Error(String((res && res.error) || ('native node op failed: ' + op)));
      if (res && res.code) {
        err.code = String(res.code);
      }
      throw err;
    }
    return res.data;
  }

  function hostBridgeCall(op, payload) {
    const raw = globalThis.__aitools_host_bridge_op(
      String(op || ''),
      JSON.stringify(payload === undefined ? null : payload)
    );
    const res = JSON.parse(raw);
    if (!res || res.ok !== true) {
      const err = new Error(String((res && res.error) || ('host rpc failed: ' + op)));
      if (res && res.code) {
        err.code = String(res.code);
      }
      throw err;
    }
    return res.data;
  }

  globalThis.__aitools_sync_call__ = function(op, payload) {
    return hostBridgeCall(op, payload);
  };

  // --- Console (routed to Rust tracing, never corrupts stdout) ---
  function formatLogArg(v) {
    if (typeof v === 'string') return v;
    try { return JSON.stringify(v); } catch (_) { return String(v); }
  }
  function emitLog(level, args) {
    try {
      nativeCall('console_log', {
        level,
        text: Array.prototype.map.call(args, formatLogArg).join(' ')
      });
    } catch (_) {}
  }
  globalThis.console = {
    log() { emitLog('info', arguments); },
    info() { emitLog('info', arguments); },
    debug() { emitLog('debug', arguments); },
    warn() { emitLog('warn', arguments); },
    error() { emitLog('error', arguments); }
  };

  // --- UTF-8 / Base64 / Hex helpers ---
  function utf8Encode(str) {
    const s = String(str);
    const out = [];
    for (let i = 0; i < s.length; i++) {
      let cp = s.charCodeAt(i);
      if (cp >= 0xd800 && cp <= 0xdbff && i + 1 < s.length) {
        const next = s.charCodeAt(i + 1);
        if (next >= 0xdc00 && next <= 0xdfff) {
          cp = 0x10000 + ((cp - 0xd800) << 10) + (next - 0xdc00);
          i++;
        }
      }
      if (cp < 0x80) out.push(cp);
      else if (cp < 0x800) out.push(0xc0 | (cp >> 6), 0x80 | (cp & 0x3f));
      else if (cp < 0x10000) out.push(0xe0 | (cp >> 12), 0x80 | ((cp >> 6) & 0x3f), 0x80 | (cp & 0x3f));
      else out.push(0xf0 | (cp >> 18), 0x80 | ((cp >> 12) & 0x3f), 0x80 | ((cp >> 6) & 0x3f), 0x80 | (cp & 0x3f));
    }
    return new Uint8Array(out);
  }

  function utf8Decode(bytes) {
    let out = '';
    let i = 0;
    while (i < bytes.length) {
      const b0 = bytes[i++];
      if (b0 < 0x80) {
        out += String.fromCharCode(b0);
      } else if ((b0 & 0xe0) === 0xc0 && i < bytes.length) {
        const b1 = bytes[i++];
        out += String.fromCharCode(((b0 & 0x1f) << 6) | (b1 & 0x3f));
      } else if ((b0 & 0xf0) === 0xe0 && i + 1 < bytes.length) {
        const b1 = bytes[i++];
        const b2 = bytes[i++];
        out += String.fromCharCode(((b0 & 0x0f) << 12) | ((b1 & 0x3f) << 6) | (b2 & 0x3f));
      } else if ((b0 & 0xf8) === 0xf0 && i + 2 < bytes.length) {
        const b1 = bytes[i++];
        const b2 = bytes[i++];
        const b3 = bytes[i++];
        let cp = ((b0 & 0x07) << 18) | ((b1 & 0x3f) << 12) | ((b2 & 0x3f) << 6) | (b3 & 0x3f);
        cp -= 0x10000;
        out += String.fromCharCode(0xd800 + (cp >> 10), 0xdc00 + (cp & 0x3ff));
      } else {
        out += '\ufffd';
      }
    }
    return out;
  }

  const B64_ALPHA = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
  function bytesToBase64(bytes) {
    let out = '';
    for (let i = 0; i < bytes.length; i += 3) {
      const a = bytes[i];
      const b = i + 1 < bytes.length ? bytes[i + 1] : 0;
      const c = i + 2 < bytes.length ? bytes[i + 2] : 0;
      const n = (a << 16) | (b << 8) | c;
      out += B64_ALPHA[(n >> 18) & 63];
      out += B64_ALPHA[(n >> 12) & 63];
      out += i + 1 < bytes.length ? B64_ALPHA[(n >> 6) & 63] : '=';
      out += i + 2 < bytes.length ? B64_ALPHA[n & 63] : '=';
    }
    return out;
  }

  function base64ToBytes(b64) {
    const clean = String(b64 || '').replace(/-/g, '+').replace(/_/g, '/').replace(/[^A-Za-z0-9+/=]/g, '');
    const out = [];
    for (let i = 0; i < clean.length; i += 4) {
      const c1 = B64_ALPHA.indexOf(clean[i]);
      const c2 = B64_ALPHA.indexOf(clean[i + 1]);
      const c3 = clean[i + 2] === '=' || i + 2 >= clean.length ? 0 : B64_ALPHA.indexOf(clean[i + 2]);
      const c4 = clean[i + 3] === '=' || i + 3 >= clean.length ? 0 : B64_ALPHA.indexOf(clean[i + 3]);
      if (c1 < 0 || c2 < 0) break;
      const n = (c1 << 18) | (c2 << 12) | (c3 << 6) | c4;
      out.push((n >> 16) & 255);
      if (clean[i + 2] && clean[i + 2] !== '=') out.push((n >> 8) & 255);
      if (clean[i + 3] && clean[i + 3] !== '=') out.push(n & 255);
    }
    return new Uint8Array(out);
  }

  function bytesToHex(bytes) {
    let out = '';
    for (let i = 0; i < bytes.length; i++) {
      out += bytes[i].toString(16).padStart(2, '0');
    }
    return out;
  }

  function hexToBytes(hex) {
    const clean = String(hex || '').trim();
    const len = Math.floor(clean.length / 2);
    const out = new Uint8Array(len);
    for (let i = 0; i < len; i++) {
      out[i] = parseInt(clean.slice(i * 2, i * 2 + 2), 16) || 0;
    }
    return out;
  }

  function encodeBytes(bytes, encoding) {
    const enc = String(encoding || 'utf8').toLowerCase();
    if (enc === 'utf8' || enc === 'utf-8') return utf8Decode(bytes);
    if (enc === 'hex') return bytesToHex(bytes);
    if (enc === 'base64') return bytesToBase64(bytes);
    if (enc === 'base64url') return bytesToBase64(bytes).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
    if (enc === 'latin1' || enc === 'binary' || enc === 'ascii') {
      let s = '';
      for (let i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i] & (enc === 'ascii' ? 0x7f : 0xff));
      return s;
    }
    return utf8Decode(bytes);
  }

  function decodeStringToBytes(str, encoding) {
    const enc = String(encoding || 'utf8').toLowerCase();
    const s = String(str);
    if (enc === 'utf8' || enc === 'utf-8') return utf8Encode(s);
    if (enc === 'hex') return hexToBytes(s);
    if (enc === 'base64' || enc === 'base64url') return base64ToBytes(s);
    if (enc === 'latin1' || enc === 'binary' || enc === 'ascii') {
      const out = new Uint8Array(s.length);
      for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i) & 0xff;
      return out;
    }
    return utf8Encode(s);
  }

  // --- TextEncoder / TextDecoder ---
  class TextEncoderPolyfill {
    get encoding() { return 'utf-8'; }
    encode(input) { return utf8Encode(input == null ? '' : String(input)); }
    encodeInto(src, dest) {
      const bytes = utf8Encode(src == null ? '' : String(src));
      const written = Math.min(bytes.length, dest.length);
      dest.set(bytes.subarray(0, written));
      return { read: src.length, written };
    }
  }
  class TextDecoderPolyfill {
    constructor(label) { this.encoding = String(label || 'utf-8').toLowerCase(); }
    decode(input) {
      if (!input) return '';
      const bytes = input instanceof Uint8Array
        ? input
        : (ArrayBuffer.isView(input) ? new Uint8Array(input.buffer, input.byteOffset, input.byteLength) : new Uint8Array(input));
      return utf8Decode(bytes);
    }
  }
  globalThis.TextEncoder = globalThis.TextEncoder || TextEncoderPolyfill;
  globalThis.TextDecoder = globalThis.TextDecoder || TextDecoderPolyfill;

  // --- Buffer ---
  class Buffer extends Uint8Array {
    static isBuffer(obj) {
      return Boolean(obj && (obj instanceof Buffer || obj._isBuffer === true));
    }
    static isEncoding(enc) {
      return ['utf8', 'utf-8', 'hex', 'base64', 'base64url', 'latin1', 'binary', 'ascii'].includes(String(enc || '').toLowerCase());
    }
    static from(value, encodingOrOffset, length) {
      if (typeof value === 'string') {
        return new Buffer(decodeStringToBytes(value, encodingOrOffset));
      }
      if (value instanceof ArrayBuffer) {
        const off = Number(encodingOrOffset || 0);
        const len = length === undefined ? (value.byteLength - off) : Number(length);
        return new Buffer(new Uint8Array(value, off, len));
      }
      if (ArrayBuffer.isView(value)) {
        return new Buffer(new Uint8Array(value.buffer, value.byteOffset, value.byteLength));
      }
      if (Array.isArray(value)) {
        return new Buffer(value);
      }
      if (value && typeof value === 'object' && value.type === 'Buffer' && Array.isArray(value.data)) {
        return new Buffer(value.data);
      }
      if (value && typeof value.length === 'number') {
        return new Buffer(Array.from(value));
      }
      throw new TypeError('First argument must be a string, Buffer, ArrayBuffer, or Array');
    }
    static alloc(size, fill, encoding) {
      const len = Math.max(0, Number(size) | 0);
      const buf = new Buffer(len);
      if (fill !== undefined && len > 0) {
        if (typeof fill === 'number') {
          buf.fill(fill & 0xff);
        } else {
          const pattern = Buffer.isBuffer(fill) ? fill : decodeStringToBytes(String(fill), encoding);
          if (pattern.length > 0) {
            for (let i = 0; i < len; i++) buf[i] = pattern[i % pattern.length];
          }
        }
      }
      return buf;
    }
    static allocUnsafe(size) {
      return new Buffer(Math.max(0, Number(size) | 0));
    }
    static byteLength(value, encoding) {
      if (typeof value === 'string') return decodeStringToBytes(value, encoding).length;
      if (ArrayBuffer.isView(value) || value instanceof ArrayBuffer) return value.byteLength;
      return 0;
    }
    static concat(list, totalLength) {
      const arrs = Array.isArray(list) ? list : [];
      let len = totalLength;
      if (len === undefined) {
        len = 0;
        for (const item of arrs) len += item ? item.length : 0;
      }
      const out = Buffer.alloc(len);
      let offset = 0;
      for (const item of arrs) {
        if (!item) continue;
        const view = item instanceof Uint8Array ? item : new Uint8Array(item);
        const take = Math.min(view.length, len - offset);
        if (take <= 0) break;
        out.set(view.subarray(0, take), offset);
        offset += take;
      }
      return out;
    }
    static compare(a, b) {
      const x = Buffer.isBuffer(a) ? a : Buffer.from(a);
      const y = Buffer.isBuffer(b) ? b : Buffer.from(b);
      const minLen = Math.min(x.length, y.length);
      for (let i = 0; i < minLen; i++) {
        if (x[i] !== y[i]) return x[i] < y[i] ? -1 : 1;
      }
      return x.length === y.length ? 0 : (x.length < y.length ? -1 : 1);
    }
    toString(encoding, start, end) {
      const s = start === undefined ? 0 : Math.max(0, Number(start));
      const e = end === undefined ? this.length : Math.min(this.length, Number(end));
      return encodeBytes(this.subarray(s, e), encoding);
    }
    slice(start, end) {
      return new Buffer(super.subarray(start, end));
    }
    subarray(start, end) {
      return new Buffer(super.subarray(start, end));
    }
    copy(target, targetStart = 0, sourceStart = 0, sourceEnd = this.length) {
      const slice = this.subarray(sourceStart, sourceEnd);
      const count = Math.min(slice.length, Math.max(0, target.length - targetStart));
      target.set(slice.subarray(0, count), targetStart);
      return count;
    }
    write(str, offset = 0, length = this.length - offset, encoding = 'utf8') {
      if (typeof offset === 'string') { encoding = offset; offset = 0; length = this.length; }
      else if (typeof length === 'string') { encoding = length; length = this.length - offset; }
      const bytes = decodeStringToBytes(str, encoding);
      const count = Math.min(bytes.length, length, Math.max(0, this.length - offset));
      this.set(bytes.subarray(0, count), offset);
      return count;
    }
    equals(other) {
      return Buffer.compare(this, other) === 0;
    }
    compare(other) {
      return Buffer.compare(this, other);
    }
    toJSON() {
      return { type: 'Buffer', data: Array.from(this) };
    }
  }
  Buffer.prototype._isBuffer = true;
  globalThis.Buffer = Buffer;

  function toRawBytes(data, encoding) {
    if (data instanceof Uint8Array) return data;
    if (data instanceof ArrayBuffer) return new Uint8Array(data);
    if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
    if (Array.isArray(data)) return new Uint8Array(data);
    return decodeStringToBytes(String(data == null ? '' : data), encoding);
  }

  // --- EventEmitter ---
  class EventEmitter {
    constructor() {
      this._events = Object.create(null);
      this._maxListeners = 10;
    }
    setMaxListeners(n) { this._maxListeners = Number(n); return this; }
    getMaxListeners() { return this._maxListeners; }
    on(event, fn) {
      const key = String(event);
      if (!this._events[key]) this._events[key] = [];
      this._events[key].push(fn);
      return this;
    }
    addListener(event, fn) { return this.on(event, fn); }
    prependListener(event, fn) {
      const key = String(event);
      if (!this._events[key]) this._events[key] = [];
      this._events[key].unshift(fn);
      return this;
    }
    once(event, fn) {
      const wrapper = (...args) => {
        this.removeListener(event, wrapper);
        fn.apply(this, args);
      };
      wrapper.listener = fn;
      return this.on(event, wrapper);
    }
    off(event, fn) { return this.removeListener(event, fn); }
    removeListener(event, fn) {
      const key = String(event);
      const list = this._events[key];
      if (!list) return this;
      this._events[key] = list.filter((item) => item !== fn && item.listener !== fn);
      return this;
    }
    removeAllListeners(event) {
      if (event === undefined) this._events = Object.create(null);
      else delete this._events[String(event)];
      return this;
    }
    listeners(event) {
      const list = this._events[String(event)];
      return list ? list.slice() : [];
    }
    listenerCount(event) {
      const list = this._events[String(event)];
      return list ? list.length : 0;
    }
    eventNames() { return Object.keys(this._events); }
    emit(event, ...args) {
      const list = this._events[String(event)];
      if (!list || list.length === 0) return false;
      for (const fn of list.slice()) {
        fn.apply(this, args);
      }
      return true;
    }
  }
  EventEmitter.EventEmitter = EventEmitter;

  // --- Timers ---
  const activeTimers = new Map();
  globalThis.__aitools_fire_timer__ = function(id) {
    const rec = activeTimers.get(id);
    if (!rec) return;
    if (!rec.repeat) activeTimers.delete(id);
    rec.fn(...rec.args);
  };
  function scheduleTimer(fn, delay, repeat, args) {
    if (typeof fn !== 'function') throw new TypeError('Timer callback must be a function');
    const delayMs = Math.max(0, Number(delay) || 0);
    const id = nativeCall('timer_schedule', { delayMs, repeat: Boolean(repeat) });
    activeTimers.set(id, { fn, args, repeat: Boolean(repeat) });
    return id;
  }
  function clearTimer(id) {
    if (id == null) return;
    activeTimers.delete(id);
    try { nativeCall('timer_clear', { id: Number(id) }); } catch (_) {}
  }
  globalThis.setTimeout = (fn, delay, ...args) => scheduleTimer(fn, delay, false, args);
  globalThis.clearTimeout = clearTimer;
  globalThis.setInterval = (fn, delay, ...args) => scheduleTimer(fn, delay, true, args);
  globalThis.clearInterval = clearTimer;
  globalThis.setImmediate = (fn, ...args) => scheduleTimer(fn, 0, false, args);
  globalThis.clearImmediate = clearTimer;
  globalThis.queueMicrotask = (fn) => Promise.resolve().then(fn);

  // --- OS & Process metadata ---
  const osInfo = nativeCall('os_info', {});
  let currentCwd = osInfo.cwd || '/';
  const processObj = new EventEmitter();
  Object.assign(processObj, {
    platform: osInfo.platform,
    arch: osInfo.arch,
    pid: osInfo.pid,
    ppid: osInfo.ppid,
    env: Object.assign({}, osInfo.env || {}),
    argv: ['node'],
    version: 'v20.18.0',
    versions: { node: '20.18.0', v8: '11.0.0', modules: '115' },
    cwd: () => currentCwd,
    chdir: (dir) => { currentCwd = String(dir); },
    nextTick: (fn, ...args) => Promise.resolve().then(() => fn(...args)),
    hrtime: Object.assign(
      (prev) => {
        const ms = Date.now();
        const sec = Math.floor(ms / 1000);
        const nsec = (ms % 1000) * 1e6;
        if (Array.isArray(prev)) {
          let ds = sec - prev[0];
          let dn = nsec - prev[1];
          if (dn < 0) { ds -= 1; dn += 1e9; }
          return [ds, dn];
        }
        return [sec, nsec];
      },
      { bigint: () => BigInt(Date.now()) * 1000000n }
    ),
    exit: () => {},
  });
  globalThis.process = processObj;

  // --- Path implementation (POSIX + Win32) ---
  function createPathImpl(isWin) {
    const sep = isWin ? '\\' : '/';
    const delimiter = isWin ? ';' : ':';
    const splitRe = isWin ? /[\\/]+/ : /\/+/;
    function normalizePath(p) {
      let raw = String(p || '');
      if (!raw) return '.';
      if (isWin) raw = raw.replace(/\//g, '\\');
      const isAbs = isWin ? (/^[A-Za-z]:\\|^\\/.test(raw)) : raw.startsWith('/');
      let drive = '';
      if (isWin && /^[A-Za-z]:/.test(raw)) {
        drive = raw.slice(0, 2);
        raw = raw.slice(2);
      }
      const trailing = raw.endsWith(sep) && raw.length > 1;
      const parts = raw.split(splitRe);
      const stack = [];
      for (const part of parts) {
        if (!part || part === '.') continue;
        if (part === '..') {
          if (stack.length && stack[stack.length - 1] !== '..') stack.pop();
          else if (!isAbs) stack.push('..');
        } else {
          stack.push(part);
        }
      }
      let res = (isAbs ? sep : '') + stack.join(sep);
      if (trailing && !res.endsWith(sep)) res += sep;
      if (!res) res = isAbs ? sep : '.';
      return drive + res;
    }
    function isAbsolute(p) {
      const s = String(p || '');
      return isWin ? (/^[A-Za-z]:[\\/]|^\\\\|^\//.test(s)) : s.startsWith('/');
    }
    function join(...args) {
      const filtered = args.filter((a) => a != null && String(a) !== '');
      if (!filtered.length) return '.';
      return normalizePath(filtered.join(sep));
    }
    function resolve(...args) {
      let resolved = '';
      for (let i = args.length - 1; i >= -1; i--) {
        const segment = i >= 0 ? String(args[i] || '') : currentCwd;
        if (!segment) continue;
        resolved = resolved ? (segment + sep + resolved) : segment;
        if (isAbsolute(segment)) break;
      }
      return normalizePath(resolved);
    }
    function dirname(p) {
      const s = String(p || '');
      if (!s) return '.';
      const idx = Math.max(s.lastIndexOf('/'), isWin ? s.lastIndexOf('\\') : -1);
      if (idx < 0) return '.';
      if (idx === 0) return sep;
      return s.slice(0, idx);
    }
    function basename(p, ext) {
      const s = String(p || '').replace(/[\\/]+$/, '');
      if (!s) return '';
      const idx = Math.max(s.lastIndexOf('/'), isWin ? s.lastIndexOf('\\') : -1);
      let base = idx >= 0 ? s.slice(idx + 1) : s;
      if (ext && base.endsWith(ext)) base = base.slice(0, base.length - ext.length);
      return base;
    }
    function extname(p) {
      const base = basename(p);
      const idx = base.lastIndexOf('.');
      if (idx <= 0) return '';
      return base.slice(idx);
    }
    function parse(p) {
      const raw = String(p || '');
      const root = isAbsolute(raw) ? (isWin && /^[A-Za-z]:/.test(raw) ? raw.slice(0, 3) : sep) : '';
      const dir = dirname(raw);
      const base = basename(raw);
      const ext = extname(base);
      const name = ext ? base.slice(0, base.length - ext.length) : base;
      return { root, dir: dir === '.' && root ? root : dir, base, ext, name };
    }
    function format(obj) {
      if (!obj || typeof obj !== 'object') return '';
      const dir = obj.dir || obj.root || '';
      const base = obj.base || ((obj.name || '') + (obj.ext || ''));
      if (!dir) return base;
      return dir.endsWith(sep) ? (dir + base) : (dir + sep + base);
    }
    function relative(from, to) {
      const a = resolve(from).split(splitRe).filter(Boolean);
      const b = resolve(to).split(splitRe).filter(Boolean);
      let i = 0;
      while (i < a.length && i < b.length && a[i] === b[i]) i++;
      const up = new Array(a.length - i).fill('..');
      return up.concat(b.slice(i)).join(sep) || '';
    }
    return { sep, delimiter, normalize: normalizePath, isAbsolute, join, resolve, dirname, basename, extname, parse, format, relative };
  }
  const posixPath = createPathImpl(false);
  const win32Path = createPathImpl(true);
  const platformPath = osInfo.platform === 'win32' ? win32Path : posixPath;
  const pathModule = Object.assign({}, platformPath, { posix: posixPath, win32: win32Path });

  // --- URL & URLSearchParams ---
  class URLSearchParamsPolyfill {
    constructor(init) {
      this._entries = [];
      if (typeof init === 'string') {
        const clean = init.startsWith('?') ? init.slice(1) : init;
        for (const pair of clean.split('&')) {
          if (!pair) continue;
          const eq = pair.indexOf('=');
          const k = eq >= 0 ? pair.slice(0, eq) : pair;
          const v = eq >= 0 ? pair.slice(eq + 1) : '';
          this._entries.push([decodeURIComponent(k.replace(/\+/g, ' ')), decodeURIComponent(v.replace(/\+/g, ' '))]);
        }
      } else if (Array.isArray(init)) {
        for (const pair of init) {
          if (Array.isArray(pair) && pair.length >= 2) this._entries.push([String(pair[0]), String(pair[1])]);
        }
      } else if (init && typeof init === 'object') {
        for (const [k, v] of Object.entries(init)) this._entries.push([String(k), String(v)]);
      }
    }
    append(k, v) { this._entries.push([String(k), String(v)]); }
    delete(k) { const key = String(k); this._entries = this._entries.filter(([ek]) => ek !== key); }
    get(k) { const found = this._entries.find(([ek]) => ek === String(k)); return found ? found[1] : null; }
    getAll(k) { return this._entries.filter(([ek]) => ek === String(k)).map(([, v]) => v); }
    has(k) { return this._entries.some(([ek]) => ek === String(k)); }
    set(k, v) {
      this.delete(k);
      this.append(k, v);
    }
    forEach(cb, thisArg) { for (const [k, v] of this._entries) cb.call(thisArg, v, k, this); }
    entries() { return this._entries[Symbol.iterator](); }
    keys() { return this._entries.map(([k]) => k)[Symbol.iterator](); }
    values() { return this._entries.map(([, v]) => v)[Symbol.iterator](); }
    [Symbol.iterator]() { return this.entries(); }
    toString() {
      return this._entries
        .map(([k, v]) => encodeURIComponent(k) + '=' + encodeURIComponent(v))
        .join('&');
    }
  }

  class URLPolyfill {
    constructor(input, base) {
      const parsed = nativeCall('url_op', { subOp: 'parse', input: String(input), base: base == null ? null : String(base) });
      this.href = parsed.href;
      this.origin = parsed.origin;
      this.protocol = parsed.protocol;
      this.username = parsed.username;
      this.password = parsed.password;
      this.host = parsed.host;
      this.hostname = parsed.hostname;
      this.port = parsed.port;
      this.pathname = parsed.pathname;
      this.search = parsed.search;
      this.hash = parsed.hash;
      this.searchParams = new URLSearchParamsPolyfill(parsed.search);
    }
    toString() {
      const qs = this.searchParams.toString();
      const searchPart = qs ? ('?' + qs) : '';
      return `${this.protocol}//${this.host}${this.pathname}${searchPart}${this.hash}`;
    }
    toJSON() { return this.toString(); }
  }
  globalThis.URL = globalThis.URL || URLPolyfill;
  globalThis.URLSearchParams = globalThis.URLSearchParams || URLSearchParamsPolyfill;

  // --- DOM & Window Event Listeners ---
  const winListeners = Object.create(null);
  globalThis.addEventListener = function(type, cb) {
    const k = String(type);
    if (!winListeners[k]) winListeners[k] = [];
    if (typeof cb === 'function') winListeners[k].push(cb);
  };
  globalThis.removeEventListener = function(type, cb) {
    const k = String(type);
    if (!winListeners[k]) return;
    winListeners[k] = winListeners[k].filter((fn) => fn !== cb);
  };
  globalThis.dispatchEvent = function(event) {
    if (!event || !event.type) return false;
    const list = winListeners[String(event.type)];
    if (list) {
      for (const fn of list.slice()) fn(event);
    }
    return true;
  };

  class StubEvent {
    constructor(type, init) {
      this.type = String(type || '');
      this.bubbles = Boolean(init && init.bubbles);
      this.cancelable = Boolean(init && init.cancelable);
      this.defaultPrevented = false;
    }
    preventDefault() { this.defaultPrevented = true; }
    stopPropagation() {}
  }
  class StubCustomEvent extends StubEvent {
    constructor(type, init) {
      super(type, init);
      this.detail = init && init.detail !== undefined ? init.detail : null;
    }
  }
  class StubNode {
    constructor() { this.childNodes = []; this.parentNode = null; this.textContent = ''; }
    appendChild(c) { if (c && typeof c === 'object') { c.parentNode = this; this.childNodes.push(c); } return c; }
    removeChild(c) { const i = this.childNodes.indexOf(c); if (i >= 0) this.childNodes.splice(i, 1); return c; }
    addEventListener() {}
    removeEventListener() {}
    dispatchEvent() { return true; }
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
        toggle: (x, f) => { const k = String(x); const on = f !== undefined ? Boolean(f) : !classes.has(k); if (on) classes.add(k); else classes.delete(k); return on; },
        contains: (x) => classes.has(String(x)),
      };
    }
    setAttribute(n, v) { this.attributes[String(n)] = String(v); }
    getAttribute(n) { return Object.prototype.hasOwnProperty.call(this.attributes, String(n)) ? this.attributes[String(n)] : null; }
    removeAttribute(n) { delete this.attributes[String(n)]; }
    remove() { if (this.parentNode) this.parentNode.removeChild(this); }
    querySelector() { return null; }
    querySelectorAll() { return []; }
  }
  class StubHTMLElement extends StubElement {}
  globalThis.Event = globalThis.Event || StubEvent;
  globalThis.CustomEvent = globalThis.CustomEvent || StubCustomEvent;
  globalThis.Node = globalThis.Node || StubNode;
  globalThis.Element = globalThis.Element || StubElement;
  globalThis.HTMLElement = globalThis.HTMLElement || StubHTMLElement;

  globalThis.document = {
    readyState: 'complete',
    title: '',
    cookie: '',
    documentElement: new StubHTMLElement('html'),
    head: new StubHTMLElement('head'),
    body: new StubHTMLElement('body'),
    createElement: (tag) => new StubHTMLElement(tag),
    createTextNode: (text) => { const n = new StubNode(); n.textContent = String(text == null ? '' : text); return n; },
    getElementById: () => null,
    querySelector: () => null,
    querySelectorAll: () => [],
    getElementsByTagName: () => [],
    getElementsByClassName: () => [],
    addEventListener: globalThis.addEventListener,
    removeEventListener: globalThis.removeEventListener,
    dispatchEvent: globalThis.dispatchEvent,
    execCommand: () => false,
  };
  globalThis.navigator = {
    userAgent: 'Mozilla/5.0 (aitools; QuickJsNodeCompat)',
    platform: osInfo.platform === 'darwin' ? 'MacIntel' : (osInfo.platform === 'win32' ? 'Win32' : 'Linux x86_64'),
    language: 'zh-CN',
    languages: ['zh-CN', 'en-US'],
  };
  const storageMap = new Map();
  globalThis.localStorage = {
    get length() { return storageMap.size; },
    key: (i) => Array.from(storageMap.keys())[i] || null,
    getItem: (k) => (storageMap.has(String(k)) ? storageMap.get(String(k)) : null),
    setItem: (k, v) => { storageMap.set(String(k), String(v)); },
    removeItem: (k) => { storageMap.delete(String(k)); },
    clear: () => { storageMap.clear(); },
  };
  globalThis.sessionStorage = globalThis.localStorage;
  globalThis.location = {
    href: 'aitools-plugin://unknown/index.html',
    protocol: 'aitools-plugin:',
    host: 'unknown',
    hostname: 'unknown',
    pathname: '/index.html',
    search: '',
    hash: '',
    origin: 'aitools-plugin://unknown',
  };
  globalThis.matchMedia = (q) => ({
    matches: String(q || '').includes('dark')
      ? (() => { try { return Boolean(hostBridgeCall('system/isDarkColors', {})); } catch (_) { return false; } })()
      : false,
    media: String(q || ''),
    addEventListener() {},
    removeEventListener() {},
  });

  // --- Built-in Node modules ---
  function makeStatObj(raw) {
    const mode = Number(raw.mode || 0);
    return {
      dev: 0,
      mode,
      nlink: 1,
      uid: 0,
      gid: 0,
      rdev: 0,
      blksize: 4096,
      ino: 0,
      size: Number(raw.size || 0),
      blocks: Math.ceil(Number(raw.size || 0) / 512),
      atimeMs: Number(raw.atimeMs || 0),
      mtimeMs: Number(raw.mtimeMs || 0),
      ctimeMs: Number(raw.ctimeMs || 0),
      birthtimeMs: Number(raw.birthtimeMs || 0),
      atime: new Date(Number(raw.atimeMs || 0)),
      mtime: new Date(Number(raw.mtimeMs || 0)),
      ctime: new Date(Number(raw.ctimeMs || 0)),
      birthtime: new Date(Number(raw.birthtimeMs || 0)),
      isFile: () => Boolean(raw.isFile),
      isDirectory: () => Boolean(raw.isDirectory),
      isSymbolicLink: () => Boolean(raw.isSymbolicLink),
      isFIFO: () => false,
      isSocket: () => false,
      isCharacterDevice: () => false,
      isBlockDevice: () => false,
    };
  }

  function makeFsModule() {
    const fsSync = {
      constants: { F_OK: 0, R_OK: 4, W_OK: 2, X_OK: 1 },
      existsSync: (p) => Boolean(nativeCall('fs_op', { subOp: 'existsSync', path: String(p) })),
      readFileSync(p, options) {
        const enc = typeof options === 'string' ? options : (options && options.encoding ? String(options.encoding) : null);
        const b64 = nativeCall('fs_op', { subOp: 'readFileSync', path: String(p) });
        const buf = Buffer.from(b64, 'base64');
        return enc ? buf.toString(enc) : buf;
      },
      writeFileSync(p, data, options) {
        const enc = typeof options === 'string' ? options : (options && options.encoding ? String(options.encoding) : 'utf8');
        const bytes = toRawBytes(data, enc);
        nativeCall('fs_op', { subOp: 'writeFileSync', path: String(p), dataBase64: bytesToBase64(bytes) });
      },
      appendFileSync(p, data, options) {
        const enc = typeof options === 'string' ? options : (options && options.encoding ? String(options.encoding) : 'utf8');
        const bytes = toRawBytes(data, enc);
        nativeCall('fs_op', { subOp: 'appendFileSync', path: String(p), dataBase64: bytesToBase64(bytes) });
      },
      mkdirSync(p, options) {
        const recursive = typeof options === 'boolean' ? options : Boolean(options && options.recursive);
        nativeCall('fs_op', { subOp: 'mkdirSync', path: String(p), recursive });
      },
      readdirSync(p, options) {
        const withFileTypes = Boolean(options && options.withFileTypes);
        const items = nativeCall('fs_op', { subOp: 'readdirSync', path: String(p), withFileTypes });
        if (!withFileTypes) return items;
        return items.map((d) => ({
          name: d.name,
          isFile: () => Boolean(d.isFile),
          isDirectory: () => Boolean(d.isDirectory),
          isSymbolicLink: () => Boolean(d.isSymbolicLink),
        }));
      },
      statSync: (p) => makeStatObj(nativeCall('fs_op', { subOp: 'statSync', path: String(p) })),
      lstatSync: (p) => makeStatObj(nativeCall('fs_op', { subOp: 'lstatSync', path: String(p) })),
      unlinkSync: (p) => { nativeCall('fs_op', { subOp: 'unlinkSync', path: String(p) }); },
      rmSync: (p, options) => { nativeCall('fs_op', { subOp: 'rmSync', path: String(p), recursive: Boolean(options && options.recursive), force: Boolean(options && options.force) }); },
      rmdirSync: (p, options) => { nativeCall('fs_op', { subOp: 'rmdirSync', path: String(p), recursive: Boolean(options && options.recursive) }); },
      renameSync: (a, b) => { nativeCall('fs_op', { subOp: 'renameSync', src: String(a), dst: String(b) }); },
      copyFileSync: (a, b) => { nativeCall('fs_op', { subOp: 'copyFileSync', src: String(a), dst: String(b) }); },
      realpathSync: (p) => nativeCall('fs_op', { subOp: 'realpathSync', path: String(p) }),
      accessSync: (p) => { nativeCall('fs_op', { subOp: 'accessSync', path: String(p) }); },
      chmodSync: (p, mode) => { nativeCall('fs_op', { subOp: 'chmodSync', path: String(p), mode: Number(mode) }); },
    };
    fsSync.realpathSync.native = fsSync.realpathSync;

    const fsPromises = {
      readFile: (p, opts) => Promise.resolve().then(() => fsSync.readFileSync(p, opts)),
      writeFile: (p, d, opts) => Promise.resolve().then(() => fsSync.writeFileSync(p, d, opts)),
      appendFile: (p, d, opts) => Promise.resolve().then(() => fsSync.appendFileSync(p, d, opts)),
      mkdir: (p, opts) => Promise.resolve().then(() => fsSync.mkdirSync(p, opts)),
      readdir: (p, opts) => Promise.resolve().then(() => fsSync.readdirSync(p, opts)),
      stat: (p) => Promise.resolve().then(() => fsSync.statSync(p)),
      lstat: (p) => Promise.resolve().then(() => fsSync.lstatSync(p)),
      unlink: (p) => Promise.resolve().then(() => fsSync.unlinkSync(p)),
      rm: (p, opts) => Promise.resolve().then(() => fsSync.rmSync(p, opts)),
      rmdir: (p, opts) => Promise.resolve().then(() => fsSync.rmdirSync(p, opts)),
      rename: (a, b) => Promise.resolve().then(() => fsSync.renameSync(a, b)),
      copyFile: (a, b) => Promise.resolve().then(() => fsSync.copyFileSync(a, b)),
      realpath: (p) => Promise.resolve().then(() => fsSync.realpathSync(p)),
      access: (p) => Promise.resolve().then(() => fsSync.accessSync(p)),
      chmod: (p, m) => Promise.resolve().then(() => fsSync.chmodSync(p, m)),
    };
    fsSync.promises = fsPromises;

    function wrapAsync(syncFn) {
      return function(...args) {
        const cb = args[args.length - 1];
        const callArgs = args.slice(0, -1);
        Promise.resolve().then(() => syncFn(...callArgs)).then(
          (val) => { if (typeof cb === 'function') cb(null, val); },
          (err) => { if (typeof cb === 'function') cb(err); }
        );
      };
    }
    fsSync.readFile = wrapAsync(fsSync.readFileSync);
    fsSync.writeFile = wrapAsync(fsSync.writeFileSync);
    fsSync.readdir = wrapAsync(fsSync.readdirSync);
    fsSync.stat = wrapAsync(fsSync.statSync);
    fsSync.lstat = wrapAsync(fsSync.lstatSync);
    fsSync.mkdir = wrapAsync(fsSync.mkdirSync);
    fsSync.unlink = wrapAsync(fsSync.unlinkSync);
    fsSync.rm = wrapAsync(fsSync.rmSync);
    fsSync.copyFile = wrapAsync(fsSync.copyFileSync);
    fsSync.realpath = wrapAsync(fsSync.realpathSync);
    fsSync.access = wrapAsync(fsSync.accessSync);
    return { fsSync, fsPromises };
  }
  const { fsSync, fsPromises } = makeFsModule();

  function makeOsModule() {
    return {
      EOL: osInfo.eol || '\n',
      platform: () => osInfo.platform,
      arch: () => osInfo.arch,
      homedir: () => osInfo.homedir,
      tmpdir: () => osInfo.tmpdir,
      hostname: () => osInfo.hostname,
      type: () => osInfo.osType,
      release: () => osInfo.release,
      endianness: () => osInfo.endianness || 'LE',
      cpus: () => osInfo.cpus || [],
      totalmem: () => Number(osInfo.totalmem || 0),
      freemem: () => Number(osInfo.freemem || 0),
      uptime: () => 0,
      networkInterfaces: () => nativeCall('os_network_interfaces', {}),
    };
  }

  function makeCryptoModule() {
    return {
      createHash(algorithm) {
        const chunks = [];
        return {
          update(data, inputEncoding) {
            chunks.push(toRawBytes(data, inputEncoding));
            return this;
          },
          digest(outputEncoding) {
            const combined = Buffer.concat(chunks);
            const b64 = nativeCall('crypto_hash', {
              algorithm: String(algorithm || 'sha256'),
              dataBase64: bytesToBase64(combined),
            });
            const out = Buffer.from(b64, 'base64');
            return outputEncoding ? out.toString(outputEncoding) : out;
          },
        };
      },
      createHmac(algorithm, key) {
        const chunks = [];
        const keyBytes = toRawBytes(key);
        return {
          update(data, inputEncoding) {
            chunks.push(toRawBytes(data, inputEncoding));
            return this;
          },
          digest(outputEncoding) {
            const combined = Buffer.concat(chunks);
            const b64 = nativeCall('crypto_hmac', {
              algorithm: String(algorithm || 'sha256'),
              keyBase64: bytesToBase64(keyBytes),
              dataBase64: bytesToBase64(combined),
            });
            const out = Buffer.from(b64, 'base64');
            return outputEncoding ? out.toString(outputEncoding) : out;
          },
        };
      },
      randomBytes(size, cb) {
        const b64 = nativeCall('crypto_random_bytes', { size: Number(size) });
        const buf = Buffer.from(b64, 'base64');
        if (typeof cb === 'function') {
          Promise.resolve().then(() => cb(null, buf));
          return;
        }
        return buf;
      },
      randomUUID: () => nativeCall('crypto_random_uuid', {}),
      randomInt(min, max) {
        const lo = max === undefined ? 0 : Number(min);
        const hi = max === undefined ? Number(min) : Number(max);
        if (hi <= lo) throw new RangeError('max must be greater than min');
        const buf = Buffer.from(nativeCall('crypto_random_bytes', { size: 4 }), 'base64');
        const n = ((buf[0] << 24) | (buf[1] << 16) | (buf[2] << 8) | buf[3]) >>> 0;
        return lo + (n % (hi - lo));
      },
      timingSafeEqual(a, b) {
        const ba = toRawBytes(a);
        const bb = toRawBytes(b);
        if (ba.length !== bb.length) throw new RangeError('Input buffers must have the same byte length');
        let diff = 0;
        for (let i = 0; i < ba.length; i++) diff |= ba[i] ^ bb[i];
        return diff === 0;
      },
    };
  }

  function makeElectronModule() {
    function parsePngDimensions(buf) {
      if (buf && buf.length >= 24 && buf[0] === 0x89 && buf[1] === 0x50 && buf[2] === 0x4e && buf[3] === 0x47) {
        const w = ((buf[16] << 24) | (buf[17] << 16) | (buf[18] << 8) | buf[19]) >>> 0;
        const h = ((buf[20] << 24) | (buf[21] << 16) | (buf[22] << 8) | buf[23]) >>> 0;
        return { width: w, height: h };
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
    const ipcListeners = new Map();
    return {
      clipboard: {
        readText: () => { try { return String(hostBridgeCall('clipboard/readText', {}) || ''); } catch (_) { return ''; } },
        writeText: (text) => { hostBridgeCall('clipboard/copyText', { text: String(text == null ? '' : text) }); },
        readHTML: () => { try { return String(hostBridgeCall('clipboard/readHTML', {}) || ''); } catch (_) { return ''; } },
        writeHTML: (html) => { try { hostBridgeCall('clipboard/writeHTML', { html: String(html == null ? '' : html) }); } catch (_) {} },
        readImage: () => {
          try {
            const raw = hostBridgeCall('clipboard/readImage', {});
            if (typeof raw === 'string' && raw.startsWith('data:')) {
              const idx = raw.indexOf(',');
              const mime = raw.slice(5, idx).split(';')[0] || 'image/png';
              return makeNativeImage(Buffer.from(raw.slice(idx + 1), 'base64'), mime);
            }
          } catch (_) {}
          return makeNativeImage(Buffer.alloc(0), 'image/png');
        },
        writeImage: (img) => {
          const dataUrl = img && typeof img.toDataURL === 'function' ? img.toDataURL() : String(img || '');
          hostBridgeCall('clipboard/copyImage', { image: dataUrl });
        },
        readBuffer: () => Buffer.alloc(0),
        writeBuffer: () => {},
        availableFormats: () => [],
        clear: () => { try { hostBridgeCall('clipboard/clear', {}); } catch (_) {} },
      },
      nativeImage: {
        createEmpty: () => makeNativeImage(Buffer.alloc(0), 'image/png'),
        createFromPath: (filePath) => {
          try {
            const data = fsSync.readFileSync(String(filePath || ''));
            const ext = pathModule.extname(String(filePath || '')).toLowerCase();
            const mime = ext === '.jpg' || ext === '.jpeg' ? 'image/jpeg'
              : (ext === '.svg' ? 'image/svg+xml' : (ext === '.webp' ? 'image/webp' : 'image/png'));
            return makeNativeImage(data, mime);
          } catch (_) {
            return makeNativeImage(Buffer.alloc(0), 'image/png');
          }
        },
        createFromDataURL: (dataUrl) => {
          const match = /^data:([^;,]+)?(?:;base64)?,(.*)$/i.exec(String(dataUrl || ''));
          if (!match) return makeNativeImage(Buffer.alloc(0), 'image/png');
          return makeNativeImage(Buffer.from(match[2] || '', 'base64'), match[1] || 'image/png');
        },
        createFromBuffer: (buf) => makeNativeImage(Buffer.isBuffer(buf) ? buf : Buffer.from(buf || []), 'image/png'),
      },
      shell: {
        openExternal: (url) => Promise.resolve().then(() => { hostBridgeCall('shell/openExternal', { url: String(url || '') }); }),
        openPath: (p) => Promise.resolve().then(() => { hostBridgeCall('shell/openPath', { path: String(p || '') }); return ''; }),
        showItemInFolder: (p) => { hostBridgeCall('shell/showItemInFolder', { path: String(p || '') }); },
        trashItem: (p) => Promise.resolve().then(() => { hostBridgeCall('shell/trashItem', { path: String(p || '') }); }),
        beep: () => { hostBridgeCall('shell/beep', {}); },
      },
      ipcRenderer: {
        on(ch, fn) { const k = String(ch); if (!ipcListeners.has(k)) ipcListeners.set(k, []); ipcListeners.get(k).push(fn); return this; },
        once(ch, fn) { const w = (...a) => { this.removeListener(ch, w); fn(...a); }; return this.on(ch, w); },
        off(ch, fn) { return this.removeListener(ch, fn); },
        removeListener(ch, fn) { const arr = ipcListeners.get(String(ch)); if (arr) ipcListeners.set(String(ch), arr.filter((f) => f !== fn)); return this; },
        removeAllListeners(ch) { if (ch === undefined) ipcListeners.clear(); else ipcListeners.delete(String(ch)); return this; },
        send(ch, ...args) { try { hostBridgeCall('window/ipcSend', { channel: String(ch), args }); } catch (_) {} },
        sendSync(ch, ...args) { try { return hostBridgeCall('window/ipcSendSync', { channel: String(ch), args }); } catch (_) { return null; } },
        invoke(ch, ...args) { return Promise.resolve().then(() => { try { return hostBridgeCall('window/ipcInvoke', { channel: String(ch), args }); } catch (_) { return null; } }); },
      },
      screen: {
        getPrimaryDisplay: () => hostBridgeCall('display/getPrimaryDisplay', {}),
        getAllDisplays: () => hostBridgeCall('display/getAllDisplays', {}),
        getCursorScreenPoint: () => hostBridgeCall('display/getCursorScreenPoint', {}),
        getDisplayNearestPoint: (pt) => hostBridgeCall('display/getDisplayNearestPoint', pt || { x: 0, y: 0 }),
        dipToScreenPoint: (pt) => hostBridgeCall('display/dipToScreenPoint', pt || { x: 0, y: 0 }),
        screenToDipPoint: (pt) => hostBridgeCall('display/screenToDipPoint', pt || { x: 0, y: 0 }),
      },
      dialog: {
        showOpenDialogSync: (a, b) => hostBridgeCall('dialog/showOpenDialog', (b && typeof b === 'object') ? b : (a && typeof a === 'object' && !a.webContents ? a : {})),
        showOpenDialog: (a, b) => Promise.resolve().then(() => {
          const opts = (b && typeof b === 'object') ? b : (a && typeof a === 'object' && !a.webContents ? a : {});
          const paths = hostBridgeCall('dialog/showOpenDialog', opts);
          const list = Array.isArray(paths) ? paths : [];
          return { canceled: list.length === 0, filePaths: list };
        }),
        showSaveDialogSync: (a, b) => hostBridgeCall('dialog/showSaveDialog', (b && typeof b === 'object') ? b : (a && typeof a === 'object' && !a.webContents ? a : {})),
        showSaveDialog: (a, b) => Promise.resolve().then(() => {
          const opts = (b && typeof b === 'object') ? b : (a && typeof a === 'object' && !a.webContents ? a : {});
          const filePath = hostBridgeCall('dialog/showSaveDialog', opts);
          return { canceled: !filePath, filePath: filePath || undefined };
        }),
      },
      app: {
        getPath: (name) => hostBridgeCall('system/getPath', { name: String(name || '') }),
        getName: () => hostBridgeCall('system/getAppName', {}),
        getVersion: () => hostBridgeCall('system/getAppVersion', {}),
      },
    };
  }

  class Stream extends EventEmitter {
    pipe(dest) {
      this.on('data', (chunk) => { dest.write(chunk); });
      this.on('end', () => { if (typeof dest.end === 'function') dest.end(); });
      return dest;
    }
  }
  class Readable extends Stream {
    static from(iterable) {
      const r = new Readable();
      Promise.resolve().then(() => {
        for (const chunk of iterable) r.emit('data', chunk);
        r.emit('end');
      });
      return r;
    }
    push(chunk) {
      if (chunk === null) this.emit('end');
      else this.emit('data', chunk);
      return true;
    }
    read() { return null; }
  }
  class Writable extends Stream {
    write(chunk) { this.emit('data', chunk); return true; }
    end(chunk) { if (chunk !== undefined) this.write(chunk); this.emit('finish'); }
  }
  class Duplex extends Stream {
    write(chunk) { this.emit('data', chunk); return true; }
    end(chunk) { if (chunk !== undefined) this.write(chunk); this.emit('finish'); this.emit('end'); }
  }
  class Transform extends Duplex {}
  class PassThrough extends Transform {}

  function makeHttpModule(defaultProto) {
    function request(urlOrOpts, maybeOpts, cb) {
      let opts = {};
      let callback = cb;
      if (typeof urlOrOpts === 'string' || urlOrOpts instanceof URLPolyfill) {
        const u = new URLPolyfill(String(urlOrOpts));
        opts = { protocol: u.protocol, hostname: u.hostname, port: u.port, path: u.pathname + u.search, href: u.href };
        if (typeof maybeOpts === 'function') callback = maybeOpts;
        else if (maybeOpts && typeof maybeOpts === 'object') Object.assign(opts, maybeOpts);
      } else if (urlOrOpts && typeof urlOrOpts === 'object') {
        opts = Object.assign({}, urlOrOpts);
        if (typeof maybeOpts === 'function') callback = maybeOpts;
      }
      const req = new EventEmitter();
      const bodyChunks = [];
      req.write = (chunk) => { bodyChunks.push(toRawBytes(chunk)); return true; };
      req.setHeader = (k, v) => { opts.headers = opts.headers || {}; opts.headers[k] = v; };
      req.end = (chunk) => {
        if (chunk !== undefined) req.write(chunk);
        const proto = opts.protocol || defaultProto;
        const targetUrl = opts.href || `${proto}//${opts.hostname || opts.host || 'localhost'}${opts.port ? ':' + opts.port : ''}${opts.path || '/'}`;
        const bodyBuf = Buffer.concat(bodyChunks);
        Promise.resolve().then(() => {
          const resp = hostBridgeCall('http/request', {
            url: targetUrl,
            method: String(opts.method || 'GET').toUpperCase(),
            headers: opts.headers || {},
            body: bodyBuf.length > 0 ? bodyBuf.toString('utf8') : null,
          });
          const res = new Readable();
          res.statusCode = Number(resp && resp.status || 200);
          res.statusMessage = String(resp && resp.statusText || 'OK');
          res.headers = (resp && resp.headers) || {};
          if (typeof callback === 'function') callback(res);
          req.emit('response', res);
          const bodyStr = resp && resp.body != null ? String(resp.body) : '';
          if (bodyStr.length > 0) res.emit('data', Buffer.from(bodyStr, 'utf8'));
          res.emit('end');
        }).catch((err) => {
          req.emit('error', err);
        });
      };
      return req;
    }
    return {
      request,
      get(urlOrOpts, maybeOpts, cb) {
        const req = request(urlOrOpts, maybeOpts, cb);
        req.end();
        return req;
      },
    };
  }

  globalThis.fetch = function(input, init) {
    const opts = init && typeof init === 'object' ? init : {};
    const targetUrl = typeof input === 'string' ? input : (input && input.url ? String(input.url) : String(input));
    return Promise.resolve().then(() => {
      const resp = hostBridgeCall('http/request', {
        url: targetUrl,
        method: String(opts.method || 'GET').toUpperCase(),
        headers: opts.headers || {},
        body: opts.body == null ? null : String(opts.body),
      });
      const bodyStr = resp && resp.body != null ? String(resp.body) : '';
      const status = Number(resp && resp.status || 200);
      return {
        ok: resp && typeof resp.ok === 'boolean' ? resp.ok : (status >= 200 && status < 300),
        status,
        statusText: String(resp && resp.statusText || ''),
        url: String(resp && resp.url || targetUrl),
        headers: (resp && resp.headers) || {},
        text: () => Promise.resolve(bodyStr),
        json: () => Promise.resolve(JSON.parse(bodyStr || 'null')),
        arrayBuffer: () => Promise.resolve(utf8Encode(bodyStr).buffer),
      };
    });
  };

  function makeZlibModule() {
    function wrapSync(subOp) {
      return function(data) {
        const b64 = nativeCall('zlib_op', { subOp, dataBase64: bytesToBase64(toRawBytes(data)) });
        return Buffer.from(b64, 'base64');
      };
    }
    function wrapCb(syncFn) {
      return function(data, optsOrCb, maybeCb) {
        const cb = typeof optsOrCb === 'function' ? optsOrCb : maybeCb;
        Promise.resolve().then(() => syncFn(data)).then(
          (val) => { if (typeof cb === 'function') cb(null, val); },
          (err) => { if (typeof cb === 'function') cb(err); }
        );
      };
    }
    const gzipSync = wrapSync('gzip');
    const gunzipSync = wrapSync('gunzip');
    const deflateSync = wrapSync('deflate');
    const inflateSync = wrapSync('inflate');
    const deflateRawSync = wrapSync('deflateRaw');
    const inflateRawSync = wrapSync('inflateRaw');
    return {
      constants: { Z_NO_COMPRESSION: 0, Z_BEST_SPEED: 1, Z_BEST_COMPRESSION: 9, Z_DEFAULT_COMPRESSION: -1 },
      gzipSync,
      gunzipSync,
      deflateSync,
      inflateSync,
      deflateRawSync,
      inflateRawSync,
      gzip: wrapCb(gzipSync),
      gunzip: wrapCb(gunzipSync),
      deflate: wrapCb(deflateSync),
      inflate: wrapCb(inflateSync),
    };
  }

  function makeChildProcessModule() {
    function runSync(subOp, cmd, args, options) {
      const opts = options && typeof options === 'object' ? options : {};
      const out = nativeCall('child_process_op', {
        subOp,
        command: String(cmd || ''),
        args: Array.isArray(args) ? args.map(String) : [],
        cwd: opts.cwd ? String(opts.cwd) : currentCwd,
        env: opts.env || null,
        shell: opts.shell !== undefined ? Boolean(opts.shell) : subOp === 'execSync',
        timeoutMs: opts.timeout ? Number(opts.timeout) : null,
        inputBase64: opts.input != null ? bytesToBase64(toRawBytes(opts.input)) : null,
      });
      const stdout = Buffer.from(out.stdoutBase64 || '', 'base64');
      const stderr = Buffer.from(out.stderrBase64 || '', 'base64');
      const enc = opts.encoding ? String(opts.encoding) : null;
      return {
        status: out.status,
        signal: null,
        stdout: enc ? stdout.toString(enc) : stdout,
        stderr: enc ? stderr.toString(enc) : stderr,
      };
    }
    return {
      execSync(cmd, options) {
        const res = runSync('execSync', cmd, [], options);
        if (res.status !== 0) {
          const err = new Error(`Command failed (${res.status}): ${cmd}`);
          err.status = res.status;
          err.stdout = res.stdout;
          err.stderr = res.stderr;
          throw err;
        }
        return res.stdout;
      },
      execFileSync(file, argsOrOpts, maybeOpts) {
        const args = Array.isArray(argsOrOpts) ? argsOrOpts : [];
        const opts = Array.isArray(argsOrOpts) ? maybeOpts : argsOrOpts;
        const res = runSync('execFileSync', file, args, opts);
        if (res.status !== 0) {
          const err = new Error(`Command failed (${res.status}): ${file}`);
          err.status = res.status;
          err.stdout = res.stdout;
          err.stderr = res.stderr;
          throw err;
        }
        return res.stdout;
      },
      spawnSync(cmd, argsOrOpts, maybeOpts) {
        const args = Array.isArray(argsOrOpts) ? argsOrOpts : [];
        const opts = Array.isArray(argsOrOpts) ? maybeOpts : argsOrOpts;
        return runSync('spawnSync', cmd, args, opts);
      },
      exec(cmd, optsOrCb, maybeCb) {
        const opts = typeof optsOrCb === 'function' ? {} : (optsOrCb || {});
        const cb = typeof optsOrCb === 'function' ? optsOrCb : maybeCb;
        const ee = new EventEmitter();
        Promise.resolve().then(() => {
          const res = runSync('execSync', cmd, [], Object.assign({ encoding: 'utf8' }, opts));
          if (typeof cb === 'function') cb(res.status === 0 ? null : new Error(`exit ${res.status}`), res.stdout, res.stderr);
          ee.emit('close', res.status);
          ee.emit('exit', res.status);
        }).catch((err) => {
          if (typeof cb === 'function') cb(err, '', '');
          ee.emit('error', err);
        });
        return ee;
      },
      spawn(cmd, argsOrOpts, maybeOpts) {
        const args = Array.isArray(argsOrOpts) ? argsOrOpts : [];
        const opts = Array.isArray(argsOrOpts) ? (maybeOpts || {}) : (argsOrOpts || {});
        const ee = new EventEmitter();
        ee.stdout = new Readable();
        ee.stderr = new Readable();
        ee.stdin = new Writable();
        Promise.resolve().then(() => {
          const res = runSync('spawnSync', cmd, args, opts);
          if (res.stdout && res.stdout.length) ee.stdout.emit('data', res.stdout);
          ee.stdout.emit('end');
          if (res.stderr && res.stderr.length) ee.stderr.emit('data', res.stderr);
          ee.stderr.emit('end');
          ee.emit('close', res.status);
          ee.emit('exit', res.status);
        }).catch((err) => ee.emit('error', err));
        return ee;
      },
    };
  }

  function deepEqual(a, b) {
    if (Object.is(a, b)) return true;
    if (a == null || b == null || typeof a !== 'object' || typeof b !== 'object') return false;
    if (Array.isArray(a) !== Array.isArray(b)) return false;
    const ka = Object.keys(a), kb = Object.keys(b);
    if (ka.length !== kb.length) return false;
    return ka.every((k) => Object.prototype.hasOwnProperty.call(b, k) && deepEqual(a[k], b[k]));
  }
  function makeAssertModule() {
    class AssertionError extends Error {
      constructor(message) { super(message); this.name = 'AssertionError'; }
    }
    const assert = (val, msg) => { if (!val) throw new AssertionError(msg || 'Assertion failed'); };
    assert.AssertionError = AssertionError;
    assert.ok = assert;
    assert.equal = (a, b, msg) => { if (a != b) throw new AssertionError(msg || `${a} == ${b}`); };
    assert.notEqual = (a, b, msg) => { if (a == b) throw new AssertionError(msg || `${a} != ${b}`); };
    assert.strictEqual = (a, b, msg) => { if (!Object.is(a, b)) throw new AssertionError(msg || `${a} === ${b}`); };
    assert.notStrictEqual = (a, b, msg) => { if (Object.is(a, b)) throw new AssertionError(msg || `${a} !== ${b}`); };
    assert.deepEqual = (a, b, msg) => { if (!deepEqual(a, b)) throw new AssertionError(msg || 'deepEqual failed'); };
    assert.deepStrictEqual = assert.deepEqual;
    assert.throws = (fn, errCheck, msg) => {
      let threw = false;
      try { fn(); } catch (e) {
        threw = true;
        if (errCheck instanceof RegExp && !errCheck.test(String(e && e.message || e))) throw e;
      }
      if (!threw) throw new AssertionError(msg || 'Missing expected exception');
    };
    assert.doesNotThrow = (fn) => { fn(); };
    assert.fail = (msg) => { throw new AssertionError(msg || 'Failed'); };
    return assert;
  }

  const querystringModule = {
    escape: (s) => encodeURIComponent(String(s == null ? '' : s)),
    unescape: (s) => decodeURIComponent(String(s == null ? '' : s).replace(/\+/g, ' ')),
    parse: (str) => {
      const p = new URLSearchParamsPolyfill(String(str || ''));
      const out = {};
      for (const [k, v] of p.entries()) {
        if (Object.prototype.hasOwnProperty.call(out, k)) {
          out[k] = Array.isArray(out[k]) ? out[k].concat(v) : [out[k], v];
        } else {
          out[k] = v;
        }
      }
      return out;
    },
    stringify: (obj) => {
      if (!obj || typeof obj !== 'object') return '';
      const parts = [];
      for (const [k, v] of Object.entries(obj)) {
        if (Array.isArray(v)) {
          for (const item of v) parts.push(encodeURIComponent(k) + '=' + encodeURIComponent(String(item)));
        } else if (v !== undefined) {
          parts.push(encodeURIComponent(k) + '=' + encodeURIComponent(String(v)));
        }
      }
      return parts.join('&');
    },
  };
  querystringModule.encode = querystringModule.stringify;
  querystringModule.decode = querystringModule.parse;

  const utilModule = {
    promisify(fn) {
      return function(...args) {
        return new Promise((resolve, reject) => {
          fn.call(this, ...args, (err, val) => (err ? reject(err) : resolve(val)));
        });
      };
    },
    callbackify(fn) {
      return function(...args) {
        const cb = args.pop();
        Promise.resolve(fn.apply(this, args)).then((v) => cb(null, v), (e) => cb(e));
      };
    },
    format(fmt, ...args) {
      if (typeof fmt !== 'string') return [fmt, ...args].map(formatLogArg).join(' ');
      let i = 0;
      const out = fmt.replace(/%[sdjifoO%]/g, (m) => {
        if (m === '%%') return '%';
        if (i >= args.length) return m;
        const val = args[i++];
        if (m === '%j') return JSON.stringify(val);
        if (m === '%d' || m === '%i') return String(parseInt(val, 10));
        if (m === '%f') return String(parseFloat(val));
        return String(val);
      });
      return i < args.length ? out + ' ' + args.slice(i).map(formatLogArg).join(' ') : out;
    },
    inspect: (v) => formatLogArg(v),
    inherits(ctor, superCtor) {
      ctor.super_ = superCtor;
      Object.setPrototypeOf(ctor.prototype, superCtor.prototype);
    },
    deprecate: (fn) => fn,
    types: {
      isArrayBuffer: (v) => v instanceof ArrayBuffer,
      isUint8Array: (v) => v instanceof Uint8Array,
      isDate: (v) => v instanceof Date,
      isRegExp: (v) => v instanceof RegExp,
      isPromise: (v) => v instanceof Promise,
      isMap: (v) => v instanceof Map,
      isSet: (v) => v instanceof Set,
    },
    TextEncoder: TextEncoderPolyfill,
    TextDecoder: TextDecoderPolyfill,
  };

  const urlModule = {
    URL: URLPolyfill,
    URLSearchParams: URLSearchParamsPolyfill,
    parse: (s) => {
      try {
        const u = new URLPolyfill(String(s || ''));
        return { href: u.href, protocol: u.protocol, host: u.host, hostname: u.hostname, port: u.port, pathname: u.pathname, path: u.pathname + u.search, search: u.search, query: u.search.replace(/^\?/, ''), hash: u.hash };
      } catch (_) {
        return { href: String(s || ''), pathname: String(s || ''), path: String(s || '') };
      }
    },
    format: (obj) => (typeof obj === 'string' ? obj : (obj && obj.href ? obj.href : String(obj || ''))),
    resolve: (from, to) => new URLPolyfill(String(to || ''), String(from || '')).href,
    fileURLToPath: (u) => nativeCall('url_op', { subOp: 'fileURLToPath', input: String(u instanceof URLPolyfill ? u.href : u) }),
    pathToFileURL: (p) => new URLPolyfill(nativeCall('url_op', { subOp: 'pathToFileURL', input: String(p) })),
  };

  const timersModule = {
    setTimeout: globalThis.setTimeout,
    clearTimeout: globalThis.clearTimeout,
    setInterval: globalThis.setInterval,
    clearInterval: globalThis.clearInterval,
    setImmediate: globalThis.setImmediate,
    clearImmediate: globalThis.clearImmediate,
  };
  const timersPromisesModule = {
    setTimeout: (delay, value) => new Promise((resolve) => globalThis.setTimeout(() => resolve(value), delay)),
    setImmediate: (value) => new Promise((resolve) => globalThis.setImmediate(() => resolve(value))),
  };

  const BUILTIN_MODULES = {
    electron: makeElectronModule(),
    fs: fsSync,
    'original-fs': fsSync,
    'fs/promises': fsPromises,
    path: pathModule,
    'path/posix': posixPath,
    'path/win32': win32Path,
    os: makeOsModule(),
    crypto: makeCryptoModule(),
    buffer: { Buffer, kMaxLength: 2147483647, INSPECT_MAX_BYTES: 50 },
    url: urlModule,
    querystring: querystringModule,
    util: utilModule,
    events: EventEmitter,
    stream: { Stream, Readable, Writable, Duplex, Transform, PassThrough, pipeline: () => {}, finished: () => {} },
    timers: timersModule,
    'timers/promises': timersPromisesModule,
    readline: {
      createInterface: () => Object.assign(new EventEmitter(), { close() { this.emit('close'); }, prompt() {} }),
    },
    zlib: makeZlibModule(),
    assert: makeAssertModule(),
    'assert/strict': makeAssertModule(),
    string_decoder: {
      StringDecoder: class StringDecoder {
        write(b) { return Buffer.isBuffer(b) ? b.toString('utf8') : String(b || ''); }
        end(b) { return b ? this.write(b) : ''; }
      },
    },
    process: processObj,
    console: globalThis.console,
    child_process: makeChildProcessModule(),
    http: makeHttpModule('http:'),
    https: makeHttpModule('https:'),
  };

  let activePluginDir = '';
  const moduleCache = Object.create(null);

  function createRequire(parentFilename) {
    function req(specifier) {
      const spec = String(specifier || '');
      const resolved = nativeCall('resolve_module', {
        request: spec,
        parentFilename: parentFilename || pathModule.join(activePluginDir || currentCwd, 'preload.js'),
        pluginDir: activePluginDir || currentCwd,
      });

      if (resolved.kind === 'builtin') {
        return BUILTIN_MODULES[resolved.name];
      }
      const filename = resolved.filename;
      if (Object.prototype.hasOwnProperty.call(moduleCache, filename)) {
        return moduleCache[filename].exports;
      }

      const rawSource = nativeCall('read_module_source', { filename });
      const mod = {
        id: filename,
        filename,
        loaded: false,
        exports: {},
        paths: [],
      };
      moduleCache[filename] = mod;

      try {
        if (resolved.kind === 'json') {
          mod.exports = JSON.parse(rawSource);
          mod.loaded = true;
          return mod.exports;
        }
        let body = rawSource;
        if (body.charCodeAt(0) === 0xfeff) body = body.slice(1);
        if (body.startsWith('#!')) {
          const nl = body.indexOf('\n');
          body = nl >= 0 ? body.slice(nl + 1) : '';
        }
        const localRequire = createRequire(filename);
        let compiled;
        try {
          compiled = new Function('exports', 'require', 'module', '__filename', '__dirname', body);
        } catch (syntaxErr) {
          if (/(?:^|\n)\s*(?:import|export)\b|import\.meta/.test(body)) {
            throw new Error('__AITOOLS_NODE_COMPAT_GAP__:esm:' + filename);
          }
          throw syntaxErr;
        }
        compiled.call(mod.exports, mod.exports, localRequire, mod, filename, pathModule.dirname(filename));
        mod.loaded = true;
        return mod.exports;
      } catch (err) {
        delete moduleCache[filename];
        throw err;
      }
    }
    req.cache = moduleCache;
    req.resolve = function(specifier) {
      const resolved = nativeCall('resolve_module', {
        request: String(specifier || ''),
        parentFilename: parentFilename || pathModule.join(activePluginDir || currentCwd, 'preload.js'),
        pluginDir: activePluginDir || currentCwd,
      });
      return resolved.kind === 'builtin' ? resolved.name : resolved.filename;
    };
    return req;
  }

  BUILTIN_MODULES.module = {
    builtinModules: Object.keys(BUILTIN_MODULES),
    createRequire,
    _load: (request, parent) => createRequire(parent && parent.filename ? parent.filename : '')(request),
  };

  // --- Introspection & Lifecycle Dispatch ---
  const FORBIDDEN_SEGMENTS = new Set(['__proto__', 'prototype', 'constructor']);
  const INTERNAL_WINDOW_KEYS = new Set([
    'aitools', 'ztools', 'utools', 'rubick', 'exports',
    'window', 'self', 'global', 'globalThis',
    'document', 'navigator', 'location', 'localStorage', 'sessionStorage',
    'Event', 'CustomEvent', 'Node', 'Element', 'HTMLElement',
    'TextEncoder', 'TextDecoder', 'Buffer', 'URL', 'URLSearchParams',
    'setTimeout', 'clearTimeout', 'setInterval', 'clearInterval', 'setImmediate', 'clearImmediate', 'queueMicrotask',
    'matchMedia', 'addEventListener', 'removeEventListener', 'dispatchEvent',
    'console', 'process', 'fetch',
  ]);

  let preRequireWindowKeys = new Set();
  let loadedModuleExports = null;

  function isPlainSerializable(val, seen, depth) {
    if (val === null || val === undefined) return true;
    const t = typeof val;
    if (t === 'string' || t === 'boolean') return true;
    if (t === 'number') return Number.isFinite(val);
    if (t !== 'object') return false;
    if (depth > 4) return false;
    if (seen.has(val)) return false;
    seen.add(val);
    if (Array.isArray(val)) return val.every((item) => isPlainSerializable(item, seen, depth + 1));
    const proto = Object.getPrototypeOf(val);
    if (proto !== Object.prototype && proto !== null) return false;
    return Object.keys(val).every((k) => isPlainSerializable(val[k], seen, depth + 1));
  }

  function describeProp(propPath, value, seen, depth) {
    if (typeof value === 'function') return { kind: 'function', path: propPath };
    if (value === null || value === undefined || typeof value !== 'object') {
      const safeVal = (typeof value === 'number' && !Number.isFinite(value)) || value === undefined ? null : value;
      return { kind: 'value', path: propPath, value: safeVal };
    }
    if (Array.isArray(value) || isPlainSerializable(value, new WeakSet(), 0)) {
      try { return { kind: 'value', path: propPath, value: JSON.parse(JSON.stringify(value)) }; }
      catch (_) { return { kind: 'value', path: propPath, value: null }; }
    }
    if (depth >= 4 || seen.has(value)) return null;
    seen.add(value);
    const entries = [];
    for (const key of Object.keys(value)) {
      if (FORBIDDEN_SEGMENTS.has(key)) continue;
      let child;
      try { child = value[key]; } catch (_) { continue; }
      if (child === undefined) continue;
      const desc = describeProp(propPath + '.' + key, child, seen, depth + 1);
      if (desc) entries.push(desc);
    }
    return { kind: 'object', path: propPath, entries };
  }

  function collectExportedProps() {
    const out = [];
    const seen = new WeakSet();
    for (const key of Object.keys(globalThis.window)) {
      if (INTERNAL_WINDOW_KEYS.has(key) || key.startsWith('__aitools_') || key.startsWith('__AITOOLS_')) continue;
      const isServicesWithItems = key === 'services' && globalThis.window.services && Object.keys(globalThis.window.services).length > 0;
      if (preRequireWindowKeys.has(key) && !isServicesWithItems) continue;
      let val;
      try { val = globalThis.window[key]; } catch (_) { continue; }
      if (val === undefined) continue;
      const desc = describeProp(key, val, seen, 0);
      if (desc) out.push(desc);
    }
    return out;
  }

  function getExportsRegistry() {
    const winExp = globalThis.window.exports && typeof globalThis.window.exports === 'object' ? globalThis.window.exports : {};
    const modExp = loadedModuleExports && typeof loadedModuleExports === 'object' ? loadedModuleExports : {};
    return Object.assign({}, modExp, winExp);
  }

  function findFeatureEntry(featureCode) {
    const reg = getExportsRegistry();
    return featureCode && Object.prototype.hasOwnProperty.call(reg, featureCode) ? reg[featureCode] : null;
  }

  function collectExportFeatures() {
    const out = [];
    for (const [code, entry] of Object.entries(getExportsRegistry())) {
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
    hostBridgeCall('callbackSetList', { items: normalized });
    return true;
  }

  const asyncSlots = new Map();
  let nextSlotId = 1;

  function errorPayload(err) {
    const msg = (err && err.message) ? String(err.message) : String(err);
    const prefix = '__AITOOLS_NODE_COMPAT_GAP__:';
    if (msg.includes(prefix)) {
      const idx = msg.indexOf(prefix);
      return { ok: false, compatGap: msg.slice(idx + prefix.length), error: msg };
    }
    return { ok: false, error: (err && err.stack) ? String(err.stack) : msg };
  }

  async function executeMethod(method, p) {
    switch (method) {
      case 'load': {
        activePluginDir = String(p.pluginDir || currentCwd);
        currentCwd = activePluginDir;
        const pluginId = String(p.pluginId || 'unknown');
        const enc = encodeURIComponent(pluginId);
        globalThis.location.href = `aitools-plugin://${enc}/index.html`;
        globalThis.location.host = enc;
        globalThis.location.hostname = enc;
        globalThis.location.origin = `aitools-plugin://${enc}`;
        globalThis.__AITOOLS_PLUGIN_ID__ = pluginId;
        globalThis.window.exports = {};
        globalThis.window.services = {};
        preRequireWindowKeys = new Set(Object.keys(globalThis.window));
        const req = createRequire(String(p.preloadPath));
        const modExports = req(String(p.preloadPath));
        loadedModuleExports = (modExports && typeof modExports === 'object') ? modExports : null;
        const lc = globalThis.__aitools_lifecycle__ || {};
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
        const lc = globalThis.__aitools_lifecycle__ || {};
        if (Array.isArray(lc.enter) && lc.enter.length > 0) {
          if (!lc.readyFired && Array.isArray(lc.ready)) {
            lc.readyFired = true;
            for (const rcb of lc.ready.slice()) await rcb(action);
          }
          for (const cb of lc.enter.slice()) await cb(action);
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
        const lc = globalThis.__aitools_lifecycle__ || {};
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
        const lc = globalThis.__aitools_lifecycle__ || {};
        if (lc.mainPush && typeof lc.mainPush.selectCb === 'function') {
          const res = await lc.mainPush.selectCb({ code: featureCode, option: itemData, action });
          return { handled: true, result: res === undefined ? null : res };
        }
        return { handled: false, result: null };
      }
      case 'main_push': {
        const featureCode = String(p.featureCode || '');
        const action = Object.assign({ code: featureCode }, p.action && typeof p.action === 'object' ? p.action : {});
        const lc = globalThis.__aitools_lifecycle__ || {};
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
        if (!segments.length) throw new Error('invoke_window_prop requires a non-empty propPath');
        let parent = globalThis.window;
        let target = globalThis.window;
        for (const seg of segments) {
          if (FORBIDDEN_SEGMENTS.has(seg)) throw new Error('forbidden property segment: ' + seg);
          if (target == null) throw new Error(`property path '${propPath}' is undefined at '${seg}'`);
          parent = target;
          target = target[seg];
        }
        if (typeof target === 'function') {
          const res = await Reflect.apply(target, parent, args);
          return res === undefined ? null : res;
        }
        return target === undefined ? null : target;
      }
      case 'get_exported_props':
        return collectExportedProps();
      case 'plugin_out': {
        const isKill = Boolean(p.isKill);
        const lc = globalThis.__aitools_lifecycle__ || {};
        if (Array.isArray(lc.out)) {
          for (const cb of lc.out.slice()) await cb(isKill);
        }
        return true;
      }
      default:
        throw new Error('unknown QuickJsNodeCompat method: ' + method);
    }
  }

  globalThis.__aitools_runtime_api__ = {
    start(method, paramsJson) {
      const slotId = nextSlotId++;
      const slot = { done: false, payload: null };
      asyncSlots.set(slotId, slot);
      let params;
      try {
        params = JSON.parse(paramsJson || '{}');
      } catch (e) {
        slot.done = true;
        slot.payload = JSON.stringify(errorPayload(e));
        return slotId;
      }
      Promise.resolve().then(() => executeMethod(method, params)).then(
        (val) => {
          slot.done = true;
          slot.payload = JSON.stringify({ ok: true, data: val === undefined ? null : val });
        },
        (err) => {
          slot.done = true;
          slot.payload = JSON.stringify(errorPayload(err));
        }
      );
      return slotId;
    },
    poll(slotId) {
      const slot = asyncSlots.get(slotId);
      if (!slot || !slot.done) return '';
      asyncSlots.delete(slotId);
      return slot.payload;
    },
  };
})();
