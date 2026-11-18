//! Frozen ZTools/uTools/Rubick/aitools triple-global WebView & preload shim (`SPEC-008-R1`, `SPEC-015-R4..R6`, `INV-SEC-02`).

use crate::node_runtime::ExportedWindowProp;

/// Deterministic initialization script injected via `WebviewBuilder::initialization_script`
/// prior to navigation (`INV-SEC-02`).
pub const TRIPLE_GLOBAL_SHIM_JS: &str = r#"(function() {
    'use strict';

    if (typeof globalThis.window === 'undefined') {
        globalThis.window = globalThis;
    }

    function resolvePluginId() {
        if (typeof globalThis.__AITOOLS_PLUGIN_ID__ === 'string' && globalThis.__AITOOLS_PLUGIN_ID__.trim()) {
            return globalThis.__AITOOLS_PLUGIN_ID__.trim();
        }
        if (globalThis.window && typeof globalThis.window.__AITOOLS_PLUGIN_ID__ === 'string' && globalThis.window.__AITOOLS_PLUGIN_ID__.trim()) {
            return globalThis.window.__AITOOLS_PLUGIN_ID__.trim();
        }
        try {
            const loc = globalThis.location || (globalThis.window && globalThis.window.location);
            if (loc) {
                const host = String(loc.hostname || loc.host || '').trim();
                if (host && host !== 'localhost' && host !== 'aitools-plugin.localhost') {
                    return decodeURIComponent(host);
                }
                const href = String(loc.href || '');
                const match = href.match(/^aitools-plugin:\/\/([^/?#]+)/i);
                if (match && match[1]) {
                    return decodeURIComponent(match[1]);
                }
            }
        } catch (_) {}
        return 'unknown';
    }

    function defaultSyncCall(op, payload) {
        if (typeof XMLHttpRequest !== 'function') {
            throw new Error('aitools sync transport unavailable: XMLHttpRequest is not defined (op: ' + String(op || '') + ')');
        }
        const pid = resolvePluginId();
        const rawOp = String(op || '');
        const url = rawOp.startsWith('aitools-sync://')
            ? rawOp
            : 'aitools-sync://' + encodeURIComponent(pid) + '/' + rawOp.replace(/^\/+/, '');
        const xhr = new XMLHttpRequest();
        xhr.open('POST', url, false);
        try {
            xhr.setRequestHeader('Content-Type', 'application/json; charset=utf-8');
        } catch (_) {}
        xhr.send(payload === undefined ? '{}' : JSON.stringify(payload));
        if (xhr.status < 200 || xhr.status >= 300) {
            let msg = 'aitools-sync HTTP ' + xhr.status + ' for ' + rawOp;
            try {
                const errBody = xhr.responseText ? JSON.parse(xhr.responseText) : null;
                if (errBody && (errBody.message || errBody.error)) {
                    msg = String(errBody.message || errBody.error);
                }
            } catch (_) {}
            throw new Error(msg);
        }
        if (!xhr.responseText) {
            return null;
        }
        const parsed = JSON.parse(xhr.responseText);
        if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
            if (parsed.ok === false) {
                throw new Error(String(parsed.message || parsed.error || ('aitools-sync failed: ' + rawOp)));
            }
            if (parsed.ok === true && Object.prototype.hasOwnProperty.call(parsed, 'data')) {
                return parsed.data;
            }
        }
        return parsed;
    }

    if (typeof globalThis.__aitools_sync_call__ !== 'function') {
        globalThis.__aitools_sync_call__ = defaultSyncCall;
    }

    function syncCall(op, payload) {
        return globalThis.__aitools_sync_call__(op, payload);
    }

    function canSyncCall() {
        return typeof globalThis.__aitools_sync_call__ === 'function' &&
            (globalThis.__aitools_sync_call__ !== defaultSyncCall || typeof XMLHttpRequest === 'function');
    }

    function syncOrFallback(op, payload, fallbackFn) {
        if (!canSyncCall()) {
            return fallbackFn();
        }
        return syncCall(op, payload);
    }

    function utf8Encode(str) {
        const s = String(str);
        if (typeof TextEncoder === 'function') {
            return new TextEncoder().encode(s);
        }
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
            if (cp < 0x80) {
                out.push(cp);
            } else if (cp < 0x800) {
                out.push(0xc0 | (cp >> 6), 0x80 | (cp & 0x3f));
            } else if (cp < 0x10000) {
                out.push(0xe0 | (cp >> 12), 0x80 | ((cp >> 6) & 0x3f), 0x80 | (cp & 0x3f));
            } else {
                out.push(
                    0xf0 | (cp >> 18),
                    0x80 | ((cp >> 12) & 0x3f),
                    0x80 | ((cp >> 6) & 0x3f),
                    0x80 | (cp & 0x3f)
                );
            }
        }
        return new Uint8Array(out);
    }

    function utf8Decode(bytes) {
        if (typeof TextDecoder === 'function') {
            return new TextDecoder().decode(bytes);
        }
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
            }
        }
        return out;
    }

    function bytesToBase64(bytes) {
        if (typeof Buffer === 'function') {
            return Buffer.from(bytes).toString('base64');
        }
        const table = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
        let out = '';
        for (let i = 0; i < bytes.length; i += 3) {
            const a = bytes[i];
            const b = i + 1 < bytes.length ? bytes[i + 1] : 0;
            const c = i + 2 < bytes.length ? bytes[i + 2] : 0;
            const n = (a << 16) | (b << 8) | c;
            out += table[(n >> 18) & 63];
            out += table[(n >> 12) & 63];
            out += i + 1 < bytes.length ? table[(n >> 6) & 63] : '=';
            out += i + 2 < bytes.length ? table[n & 63] : '=';
        }
        return out;
    }

    function base64ToBytes(b64) {
        if (typeof Buffer === 'function') {
            return new Uint8Array(Buffer.from(String(b64), 'base64'));
        }
        const clean = String(b64).replace(/[^A-Za-z0-9+/=]/g, '');
        const table = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
        const out = [];
        for (let i = 0; i < clean.length; i += 4) {
            const c1 = table.indexOf(clean[i]);
            const c2 = table.indexOf(clean[i + 1]);
            const c3 = clean[i + 2] === '=' ? 0 : table.indexOf(clean[i + 2]);
            const c4 = clean[i + 3] === '=' ? 0 : table.indexOf(clean[i + 3]);
            if (c1 < 0 || c2 < 0) break;
            const n = (c1 << 18) | (c2 << 12) | (c3 << 6) | c4;
            out.push((n >> 16) & 255);
            if (clean[i + 2] !== '=') out.push((n >> 8) & 255);
            if (clean[i + 3] !== '=') out.push(n & 255);
        }
        return new Uint8Array(out);
    }

    function toAttachmentBytes(data) {
        if (data instanceof Uint8Array) return data;
        if (typeof ArrayBuffer !== 'undefined') {
            if (data instanceof ArrayBuffer) return new Uint8Array(data);
            if (ArrayBuffer.isView(data)) {
                return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
            }
        }
        if (Array.isArray(data)) return new Uint8Array(data);
        if (typeof data === 'string') return utf8Encode(data);
        throw new TypeError('attachment data must be a Uint8Array, ArrayBuffer, byte array, or string');
    }

    function decodeAttachmentResult(res) {
        if (res == null) return null;
        if (res instanceof Uint8Array) return res;
        if (Array.isArray(res)) return new Uint8Array(res);
        if (typeof res === 'string') return base64ToBytes(res);
        if (typeof res === 'object') {
            if (Array.isArray(res.data)) return new Uint8Array(res.data);
            if (typeof res.data === 'string') return base64ToBytes(res.data);
        }
        return null;
    }

    function normalizeAttachmentTarget(args) {
        if (args.length <= 2) {
            const id = args[0] == null ? '' : String(args[0]);
            const attName = args.length === 2 && args[1] != null ? String(args[1]) : '';
            return attName ? (id + ':' + attName) : id;
        }
        return String(args[0]);
    }

    const lifecycle = globalThis.__aitools_lifecycle__ = globalThis.__aitools_lifecycle__ || {
        enter: [],
        ready: [],
        out: [],
        detach: [],
        attach: [],
        mainPush: { cb: null, selectCb: null },
        subInput: { onChange: null },
        themeChange: [],
        pendingEnter: null,
        readyFired: false,
    };

    if (typeof globalThis.__aitools_dispatch_lifecycle__ !== 'function') {
        globalThis.__aitools_dispatch_lifecycle__ = function(eventName, arg1, arg2) {
            if (eventName === 'pluginReady') {
                lifecycle.readyFired = true;
                for (const cb of lifecycle.ready.slice()) {
                    cb(arg1);
                }
                return true;
            }
            if (eventName === 'pluginEnter') {
                if (!lifecycle.readyFired) {
                    lifecycle.readyFired = true;
                    for (const rcb of lifecycle.ready.slice()) {
                        rcb(arg1);
                    }
                }
                lifecycle.pendingEnter = arg1;
                for (const cb of lifecycle.enter.slice()) {
                    cb(arg1);
                }
                return true;
            }
            if (eventName === 'pluginOut') {
                for (const cb of lifecycle.out.slice()) {
                    cb(Boolean(arg1));
                }
                return true;
            }
            if (eventName === 'pluginDetach') {
                for (const cb of lifecycle.detach.slice()) {
                    cb(arg1);
                }
                return true;
            }
            if (eventName === 'pluginAttach') {
                for (const cb of lifecycle.attach.slice()) {
                    cb(arg1);
                }
                return true;
            }
            if (eventName === 'mainPush') {
                if (typeof lifecycle.mainPush.cb === 'function') {
                    return lifecycle.mainPush.cb(arg1);
                }
                return [];
            }
            if (eventName === 'mainPushSelect') {
                if (typeof lifecycle.mainPush.selectCb === 'function') {
                    return lifecycle.mainPush.selectCb(arg1, arg2);
                }
                return false;
            }
            if (eventName === 'subInput') {
                if (typeof lifecycle.subInput.onChange === 'function') {
                    const detail = (arg1 && typeof arg1 === 'object') ? arg1 : { text: String(arg1 == null ? '' : arg1) };
                    return lifecycle.subInput.onChange(detail);
                }
                return undefined;
            }
            if (eventName === 'themeChange') {
                for (const cb of lifecycle.themeChange.slice()) {
                    cb(arg1);
                }
                return true;
            }
            return false;
        };
    }

    globalThis.__aitools_registered_tools__ = globalThis.__aitools_registered_tools__ || Object.create(null);
    globalThis.__aitools_registered_providers__ = globalThis.__aitools_registered_providers__ || Object.create(null);

    function createApi(initialPluginId) {
        const currentPluginId = () => {
            const resolved = resolvePluginId();
            return resolved !== 'unknown' ? resolved : initialPluginId;
        };

        const dbPut = (doc) => {
            if (!doc || typeof doc !== 'object') {
                throw new TypeError('db.put requires a document object');
            }
            return syncCall('db/put', doc);
        };

        const dbGet = (id) => {
            return syncCall('db/get', { id: String(id) });
        };

        const dbRemove = (docOrId) => {
            if (typeof docOrId === 'string') {
                return syncCall('db/remove', { id: docOrId });
            }
            if (docOrId && typeof docOrId === 'object') {
                return syncCall('db/remove', { doc: docOrId });
            }
            throw new TypeError('db.remove requires a document id string or { _id, _rev } object');
        };

        const dbAllDocs = (keyOrKeys) => {
            if (Array.isArray(keyOrKeys)) {
                return syncCall('db/allDocs', { keys: keyOrKeys.map(String) });
            }
            if (typeof keyOrKeys === 'string') {
                return syncCall('db/allDocs', { key: keyOrKeys });
            }
            if (keyOrKeys && typeof keyOrKeys === 'object') {
                if (Array.isArray(keyOrKeys.keys)) {
                    return syncCall('db/allDocs', { keys: keyOrKeys.keys.map(String) });
                }
                if (typeof keyOrKeys.key === 'string') {
                    return syncCall('db/allDocs', { key: keyOrKeys.key });
                }
            }
            return syncCall('db/allDocs', {});
        };

        const dbBulkDocs = (docs) => {
            return syncCall('db/bulkDocs', { docs: Array.isArray(docs) ? docs : [] });
        };

        const dbPutAttachment = function(...args) {
            if (args.length < 2) {
                throw new TypeError('db.putAttachment requires (id, data, type)');
            }
            let attId;
            let rawData;
            let mimeType;
            if (args.length >= 5) {
                attId = String(args[0]) + ':' + String(args[1]);
                rawData = args[3];
                mimeType = args[4];
            } else if (args.length === 4) {
                attId = String(args[0]) + ':' + String(args[1]);
                rawData = args[2];
                mimeType = args[3];
            } else {
                attId = String(args[0]);
                rawData = args[1];
                mimeType = args[2];
            }
            const bytes = toAttachmentBytes(rawData);
            const contentType = String(mimeType || 'application/octet-stream');
            return syncCall('db/putAttachment', {
                id: attId,
                data: bytesToBase64(bytes),
                type: contentType,
                contentType,
            });
        };

        const dbGetAttachment = function(...args) {
            const attId = normalizeAttachmentTarget(args);
            const res = syncCall('db/getAttachment', { id: attId });
            return decodeAttachmentResult(res);
        };

        const dbGetAttachmentType = function(...args) {
            const attId = normalizeAttachmentTarget(args);
            const res = syncCall('db/getAttachmentType', { id: attId });
            if (res == null) return null;
            if (typeof res === 'string') return res;
            if (typeof res === 'object') return res.contentType || res.type || null;
            return null;
        };

        const dbRemoveAttachment = function(...args) {
            const attId = normalizeAttachmentTarget(args);
            return syncCall('db/removeAttachment', { id: attId });
        };

        const dbPromises = Object.freeze({
            put: (doc) => Promise.resolve().then(() => dbPut(doc)),
            get: (id) => Promise.resolve().then(() => dbGet(id)),
            remove: (docOrId) => Promise.resolve().then(() => dbRemove(docOrId)),
            allDocs: (keyOrKeys) => Promise.resolve().then(() => dbAllDocs(keyOrKeys)),
            bulkDocs: (docs) => Promise.resolve().then(() => dbBulkDocs(docs)),
            putAttachment: (...args) => Promise.resolve().then(() => dbPutAttachment(...args)),
            postAttachment: (...args) => Promise.resolve().then(() => dbPutAttachment(...args)),
            getAttachment: (...args) => Promise.resolve().then(() => dbGetAttachment(...args)),
            getAttachmentType: (...args) => Promise.resolve().then(() => dbGetAttachmentType(...args)),
            removeAttachment: (...args) => Promise.resolve().then(() => dbRemoveAttachment(...args)),
        });

        const db = Object.freeze({
            put: dbPut,
            get: dbGet,
            remove: dbRemove,
            allDocs: dbAllDocs,
            bulkDocs: dbBulkDocs,
            putAttachment: dbPutAttachment,
            postAttachment: dbPutAttachment,
            getAttachment: dbGetAttachment,
            getAttachmentType: dbGetAttachmentType,
            removeAttachment: dbRemoveAttachment,
            promises: dbPromises,
        });

        const dbStorage = Object.freeze({
            setItem(key, value) {
                const id = String(key);
                const existing = dbGet(id);
                const doc = existing && existing._rev
                    ? { _id: id, _rev: existing._rev, value }
                    : { _id: id, value };
                const res = dbPut(doc);
                if (res && res.error) {
                    throw new Error(String(res.message || 'dbStorage.setItem failed'));
                }
                return res;
            },
            getItem(key) {
                const doc = dbGet(String(key));
                return doc && Object.prototype.hasOwnProperty.call(doc, 'value') ? doc.value : null;
            },
            removeItem(key) {
                return dbRemove(String(key));
            },
        });

        const CRYPTO_PREFIX = '__aitools_crypto__:';
        function xorWithPluginSecret(bytes) {
            const secret = utf8Encode(currentPluginId() || 'aitools');
            const out = new Uint8Array(bytes.length);
            for (let i = 0; i < bytes.length; i++) {
                out[i] = bytes[i] ^ secret[i % secret.length] ^ ((i * 31) & 0xff);
            }
            return out;
        }

        const dbCryptoStorage = Object.freeze({
            setItem(key, value) {
                const id = CRYPTO_PREFIX + String(key);
                const payload = JSON.stringify({ v: value === undefined ? null : value });
                const cipher = bytesToBase64(xorWithPluginSecret(utf8Encode(payload)));
                const existing = dbGet(id);
                const doc = existing && existing._rev
                    ? { _id: id, _rev: existing._rev, cipher, __crypto: true }
                    : { _id: id, cipher, __crypto: true };
                const res = dbPut(doc);
                if (res && res.error) {
                    throw new Error(String(res.message || 'dbCryptoStorage.setItem failed'));
                }
                return res;
            },
            getItem(key) {
                const doc = dbGet(CRYPTO_PREFIX + String(key));
                if (!doc || typeof doc.cipher !== 'string') return null;
                const plainBytes = xorWithPluginSecret(base64ToBytes(doc.cipher));
                const parsed = JSON.parse(utf8Decode(plainBytes));
                return parsed && Object.prototype.hasOwnProperty.call(parsed, 'v') ? parsed.v : null;
            },
            removeItem(key) {
                return dbRemove(CRYPTO_PREFIX + String(key));
            },
        });

        const clipboard = Object.freeze({
            onChange() {
                return function() {};
            },
            getHistory(page, pageSize) {
                return Promise.resolve().then(() => syncCall('clipboard/getHistory', { page: page || 1, pageSize: pageSize || 50 }));
            },
            search(query) {
                return Promise.resolve().then(() => syncCall('clipboard/search', { query: String(query || '') }));
            },
            paste(id) {
                return Promise.resolve().then(() => syncCall('clipboard/paste', { id: String(id || '') }));
            },
            deleteItem(id) {
                return Promise.resolve().then(() => syncCall('clipboard/deleteItem', { id: String(id || '') }));
            },
            clearHistory() {
                return Promise.resolve().then(() => syncCall('clipboard/clearHistory', {}));
            },
            writeContent(entry) {
                if (!entry || typeof entry !== 'object') return false;
                if (entry.type === 'text') {
                    return syncCall('clipboard/copyText', { text: entry.content == null ? '' : String(entry.content) });
                }
                if (entry.type === 'image') {
                    return syncCall('clipboard/copyImage', { image: entry.content == null ? '' : String(entry.content) });
                }
                if (entry.type === 'file' || entry.type === 'files') {
                    const files = Array.isArray(entry.content) ? entry.content.map(String) : [String(entry.content || '')];
                    return syncCall('clipboard/copyFile', { files });
                }
                return false;
            },
        });

        function normalizeCopiedFileEntry(entry) {
            const record = entry && typeof entry === 'object' ? entry : {};
            const filePath = typeof entry === 'string'
                ? entry
                : (typeof record.path === 'string' ? record.path : (typeof record.name === 'string' ? record.name : ''));
            const isDiractory = typeof record.isDiractory === 'boolean'
                ? record.isDiractory
                : (typeof record.isDirectory === 'boolean' ? record.isDirectory : /[\\/]$/.test(filePath));
            const isFile = typeof record.isFile === 'boolean' ? record.isFile : !isDiractory;
            const trimmed = String(filePath || '').replace(/[\\/]+$/, '');
            const parts = trimmed.split(/[\\/]/).filter(Boolean);
            const fallbackName = parts.length ? parts[parts.length - 1] : filePath;
            return Object.assign({}, record, {
                path: filePath,
                name: typeof record.name === 'string' && record.name ? record.name : fallbackName,
                isFile,
                isDiractory,
            });
        }

        function platformName() {
            if (typeof process !== 'undefined' && process && typeof process.platform === 'string') {
                return process.platform;
            }
            if (typeof navigator !== 'undefined' && navigator && typeof navigator.platform === 'string') {
                const p = navigator.platform.toLowerCase();
                if (p.includes('mac')) return 'darwin';
                if (p.includes('win')) return 'win32';
                if (p.includes('linux')) return 'linux';
            }
            return 'darwin';
        }

        function setExpendHeightImpl(height) {
            return syncCall('window/setExpendHeight', { height: Number(height) });
        }

        function isDarkColorsImpl() {
            if (canSyncCall()) {
                return Boolean(syncCall('system/isDarkColors', {}));
            }
            if (typeof globalThis.matchMedia === 'function') {
                return Boolean(globalThis.matchMedia('(prefers-color-scheme: dark)').matches);
            }
            return false;
        }

        function getCopyedFilesImpl() {
            const res = syncCall('clipboard/getCopyedFiles', {});
            if (!Array.isArray(res)) return [];
            return res.map(normalizeCopiedFileEntry).filter((entry) => Boolean(entry.path));
        }

        function isMacOsImpl() {
            return Boolean(syncOrFallback('system/isMacOs', {}, () => platformName() === 'darwin'));
        }

        const shellNamespace = {
            openExternal(url) {
                return syncCall('shell/openExternal', { url: url == null ? '' : String(url) });
            },
            openPath(fullPath) {
                return syncCall('shell/openPath', { path: fullPath == null ? '' : String(fullPath) });
            },
            showItemInFolder(fullPath) {
                return syncCall('shell/showItemInFolder', { path: fullPath == null ? '' : String(fullPath) });
            },
            beep() {
                return syncCall('shell/beep', {});
            },
        };

        const api = {
            __pluginId: initialPluginId,
            db,
            dbStorage,
            dbCryptoStorage,
            clipboard,

            // Lifecycle hooks
            onPluginEnter(cb) {
                if (typeof cb === 'function') {
                    globalThis.__aitools_on_enter__ = cb;
                    lifecycle.enter.push(cb);
                    if (lifecycle.pendingEnter !== null) {
                        const pending = lifecycle.pendingEnter;
                        Promise.resolve().then(() => cb(pending));
                    }
                }
            },
            onPluginReady(cb) {
                if (typeof cb === 'function') {
                    lifecycle.ready.push(cb);
                }
            },
            onPluginOut(cb) {
                if (typeof cb === 'function') {
                    globalThis.__aitools_on_out__ = cb;
                    lifecycle.out.push(cb);
                }
            },
            onPluginDetach(cb) {
                if (typeof cb === 'function') {
                    lifecycle.detach.push(cb);
                }
            },
            onPluginAttach(cb) {
                if (typeof cb === 'function') {
                    lifecycle.attach.push(cb);
                }
            },
            onMainPush(cb, selectCb) {
                if (typeof cb === 'function') {
                    lifecycle.mainPush.cb = cb;
                }
                if (typeof selectCb === 'function') {
                    lifecycle.mainPush.selectCb = selectCb;
                }
                if (canSyncCall()) {
                    try {
                        syncCall('features/registerMainPush', {
                            hasCallback: typeof cb === 'function',
                            hasSelect: typeof selectCb === 'function',
                        });
                    } catch (_) {}
                }
            },
            onDbPull() {},

            // SubInput & window navigation
            setSubInput(onChangeOrOpts, placeholder, isFocus) {
                let onChange = onChangeOrOpts;
                let ph = placeholder;
                let focus = isFocus;
                if (onChangeOrOpts && typeof onChangeOrOpts === 'object' && typeof onChangeOrOpts !== 'function') {
                    onChange = onChangeOrOpts.onChange;
                    ph = onChangeOrOpts.placeholder;
                    focus = onChangeOrOpts.isFocus;
                }
                if (typeof onChange === 'function') {
                    lifecycle.subInput.onChange = onChange;
                }
                return syncCall('window/setSubInput', {
                    placeholder: ph == null ? '' : String(ph),
                    isFocus: focus !== false,
                });
            },
            setSubInputValue(value) {
                return syncCall('window/setSubInputValue', { value: value == null ? '' : String(value) });
            },
            removeSubInput() {
                lifecycle.subInput.onChange = null;
                return syncCall('window/removeSubInput', {});
            },
            subInputFocus() {
                return syncCall('window/subInputFocus', {});
            },
            subInputBlur() {
                return syncCall('window/subInputBlur', {});
            },
            subInputSelect() {
                return syncCall('window/subInputSelect', {});
            },
            redirect(label, payload) {
                return syncCall('window/redirect', {
                    label,
                    payload: payload === undefined ? '' : payload,
                });
            },
            outPlugin(isKill) {
                return syncCall('window/outPlugin', { isKill: Boolean(isKill) });
            },
            hideMainWindow(isRestorePreWindow) {
                try {
                    const res = syncCall('window/hideMainWindow', {
                        isRestorePreWindow: isRestorePreWindow !== false,
                    });
                    return Promise.resolve(res);
                } catch (err) {
                    return Promise.reject(err);
                }
            },
            hideMainWindowTypeString(text) {
                return syncCall('window/hideMainWindowTypeString', {
                    text: text == null ? '' : String(text),
                });
            },
            showMainWindow() {
                return syncCall('window/showMainWindow', {});
            },
            setExpendHeight: setExpendHeightImpl,
            setExpandHeight: setExpendHeightImpl,
            findInPage(text, options) {
                if (!canSyncCall()) return false;
                try {
                    return syncCall('window/findInPage', {
                        text: text == null ? '' : String(text),
                        options: options && typeof options === 'object' ? options : {},
                    });
                } catch (_) {
                    return false;
                }
            },
            stopFindInPage(action) {
                if (!canSyncCall()) return false;
                try {
                    return syncCall('window/stopFindInPage', {
                        action: action == null ? 'clearSelection' : String(action),
                    });
                } catch (_) {
                    return false;
                }
            },

            // Clipboard helpers
            copyText(text) {
                return syncCall('clipboard/copyText', { text: text == null ? '' : String(text) });
            },
            copyImage(img) {
                let imagePayload = img;
                if (img instanceof Uint8Array || (typeof ArrayBuffer !== 'undefined' && img instanceof ArrayBuffer)) {
                    imagePayload = 'data:image/png;base64,' + bytesToBase64(toAttachmentBytes(img));
                }
                return syncCall('clipboard/copyImage', { image: imagePayload == null ? '' : String(imagePayload) });
            },
            copyFile(fileOrFiles) {
                const files = Array.isArray(fileOrFiles)
                    ? fileOrFiles.map(String)
                    : (fileOrFiles == null ? [] : [String(fileOrFiles)]);
                return syncCall('clipboard/copyFile', { files });
            },
            getCopyedFiles: getCopyedFilesImpl,
            getCopiedFiles: getCopyedFilesImpl,
            readText() {
                return syncCall('clipboard/readText', {});
            },
            readImage() {
                return syncCall('clipboard/readImage', {});
            },

            // System & environment helpers
            getPath(name) {
                return syncCall('system/getPath', { name: name == null ? '' : String(name) });
            },
            getPathForFile(file) {
                if (file && typeof file === 'object' && typeof file.path === 'string' && file.path) {
                    return file.path;
                }
                if (canSyncCall()) {
                    return syncCall('system/getPathForFile', { file: file == null ? null : file });
                }
                return (file && typeof file === 'object' && typeof file.name === 'string') ? file.name : '';
            },
            getFileIcon(filePath) {
                return syncCall('system/getFileIcon', { path: filePath == null ? '' : String(filePath) });
            },
            getAppName() {
                return syncOrFallback('system/getAppName', {}, () => 'aitools');
            },
            getAppVersion() {
                return syncOrFallback('system/getAppVersion', {}, () => '1.0.0');
            },
            getNativeId() {
                return syncCall('system/getNativeId', {});
            },
            isDev() {
                return Boolean(syncOrFallback('system/isDev', {}, () => false));
            },
            isMacOs: isMacOsImpl,
            isMacOS: isMacOsImpl,
            isWindows() {
                return Boolean(syncOrFallback('system/isWindows', {}, () => platformName() === 'win32'));
            },
            isLinux() {
                return Boolean(syncOrFallback('system/isLinux', {}, () => platformName() === 'linux'));
            },
            isDarkColors: isDarkColorsImpl,
            getThemeInfo() {
                if (canSyncCall()) {
                    try {
                        const info = syncCall('system/getThemeInfo', {});
                        if (info && typeof info === 'object') return info;
                    } catch (_) {}
                }
                return { isDark: isDarkColorsImpl(), primaryColor: '#7c3aed' };
            },
            onThemeChange(cb) {
                if (typeof cb === 'function') {
                    lifecycle.themeChange.push(cb);
                }
            },

            // Shell & notifications
            shell: shellNamespace,
            shellOpenExternal(url) {
                return shellNamespace.openExternal(url);
            },
            shellOpenPath(fullPath) {
                return shellNamespace.openPath(fullPath);
            },
            shellShowItemInFolder(fullPath) {
                return shellNamespace.showItemInFolder(fullPath);
            },
            shellShowItem(fullPath) {
                return shellNamespace.showItemInFolder(fullPath);
            },
            shellBeep() {
                return shellNamespace.beep();
            },
            showNotification(body, clickFeatureCode) {
                return syncCall('notification/show', {
                    body: body == null ? '' : String(body),
                    clickFeatureCode: clickFeatureCode == null ? null : clickFeatureCode,
                });
            },

            // Screen, dialogs & input simulation
            screenCapture(callback) {
                const image = syncCall('system/screenCapture', {});
                if (typeof callback === 'function') {
                    callback(image);
                }
                return image;
            },
            showOpenDialog(options) {
                return syncCall('dialog/showOpenDialog', options && typeof options === 'object' ? options : {});
            },
            showSaveDialog(options) {
                return syncCall('dialog/showSaveDialog', options && typeof options === 'object' ? options : {});
            },
            simulateKeyboardTap(key, ...modifiers) {
                const flatMods = [];
                for (const mod of modifiers) {
                    if (Array.isArray(mod)) {
                        for (const inner of mod) {
                            if (inner != null && inner !== '') flatMods.push(String(inner));
                        }
                    } else if (mod != null && mod !== '') {
                        flatMods.push(String(mod));
                    }
                }
                return syncCall('system/simulateKeyboardTap', {
                    key: key == null ? '' : String(key),
                    modifiers: flatMods,
                });
            },
            simulateMouseMove(x, y) {
                return syncCall('system/simulateMouseMove', { x: Number(x), y: Number(y) });
            },
            simulateMouseClick(x, y) {
                return syncCall('system/simulateMouseClick', { x: Number(x), y: Number(y) });
            },

            // Display metrics & coordinate transforms
            getPrimaryDisplay() {
                return syncCall('display/getPrimaryDisplay', {});
            },
            getAllDisplays() {
                return syncCall('display/getAllDisplays', {});
            },
            getDisplayNearestPoint(point) {
                return syncCall('display/getDisplayNearestPoint', point && typeof point === 'object' ? point : { x: 0, y: 0 });
            },
            getCursorScreenPoint() {
                return syncCall('display/getCursorScreenPoint', {});
            },
            dipToScreenPoint(point) {
                return syncCall('display/dipToScreenPoint', point && typeof point === 'object' ? point : { x: 0, y: 0 });
            },
            screenToDipPoint(point) {
                return syncCall('display/screenToDipPoint', point && typeof point === 'object' ? point : { x: 0, y: 0 });
            },

            // Dynamic features
            getFeatures(codes) {
                const payload = codes === undefined
                    ? {}
                    : { codes: Array.isArray(codes) ? codes.map(String) : [String(codes)] };
                return syncCall('features/getFeatures', payload);
            },
            setFeature(feature) {
                return syncCall('features/setFeature', feature && typeof feature === 'object' ? feature : {});
            },
            removeFeature(code) {
                return syncCall('features/removeFeature', { code: code == null ? '' : String(code) });
            },

            // AI APIs
            ai(options, streamCallback) {
                let aborted = false;
                const reqId = 'ai_' + Date.now().toString(36) + '_' + Math.random().toString(36).slice(2, 8);
                const opts = typeof options === 'string'
                    ? { prompt: options }
                    : Object.assign({}, options && typeof options === 'object' ? options : {});
                if (!opts.requestId) {
                    opts.requestId = reqId;
                }
                const promise = Promise.resolve().then(() => {
                    if (aborted) {
                        const err = new Error('AI request aborted');
                        err.name = 'AbortError';
                        throw err;
                    }
                    const res = syncCall('ai/chat', opts);
                    if (aborted) {
                        const err = new Error('AI request aborted');
                        err.name = 'AbortError';
                        throw err;
                    }
                    if (typeof streamCallback === 'function') {
                        if (res && Array.isArray(res.chunks)) {
                            for (const chunk of res.chunks) {
                                if (aborted) break;
                                streamCallback(chunk);
                            }
                        } else if (typeof res === 'string') {
                            streamCallback({ content: res });
                        } else if (res && typeof res === 'object') {
                            streamCallback(res);
                        }
                    }
                    return res;
                });
                promise.abort = function() {
                    aborted = true;
                    try {
                        syncCall('ai/abort', { requestId: reqId });
                    } catch (_) {}
                };
                return promise;
            },
            allAiModels() {
                return Promise.resolve().then(() => {
                    const res = syncCall('ai/allModels', {});
                    return Array.isArray(res) ? res : [];
                });
            },

            // Context helpers
            readCurrentBrowserUrl() {
                return Promise.resolve().then(() => syncCall('system/readCurrentBrowserUrl', {}));
            },
            getCurrentBrowserUrl() {
                return Promise.resolve().then(() => syncCall('system/readCurrentBrowserUrl', {}));
            },
            readCurrentFolderPath() {
                return Promise.resolve().then(() => syncCall('system/readCurrentFolderPath', {}));
            },
            getCurrentFolderPath() {
                return Promise.resolve().then(() => syncCall('system/readCurrentFolderPath', {}));
            },

            // Plugin tool/provider registration hooks
            registerTool(name, definitionOrHandler, maybeHandler) {
                let toolName = name;
                let definition = definitionOrHandler;
                let handler = maybeHandler;
                if (typeof name === 'object' && name) {
                    handler = definitionOrHandler;
                    definition = name;
                    toolName = definition.name || definition.id || definition.title;
                } else if (typeof definitionOrHandler === 'function' && typeof maybeHandler !== 'function') {
                    handler = definitionOrHandler;
                    definition = {};
                }
                if (!toolName) {
                    throw new Error('registerTool requires a tool name');
                }
                globalThis.__aitools_registered_tools__[String(toolName)] = {
                    name: String(toolName),
                    definition: definition || {},
                    handler: typeof handler === 'function' ? handler : null,
                };
                return true;
            },
            registerProvider(key, handler) {
                const cleanKey = String(key || '').trim();
                if (!cleanKey || typeof handler !== 'function') {
                    throw new Error('registerProvider requires a key and handler');
                }
                globalThis.__aitools_registered_providers__[cleanKey] = handler;
                return true;
            },
            getUser() {
                return null;
            },
        };

        return Object.freeze(api);
    }

    const pluginId = resolvePluginId();
    const sharedShim = createApi(pluginId);

    for (const globalName of ['aitools', 'ztools', 'utools', 'rubick']) {
        const desc = Object.getOwnPropertyDescriptor(globalThis, globalName);
        if (!desc || desc.configurable) {
            Object.defineProperty(globalThis, globalName, {
                value: sharedShim,
                writable: false,
                enumerable: true,
                configurable: false,
            });
        }
        if (globalThis.window !== globalThis) {
            const winDesc = Object.getOwnPropertyDescriptor(globalThis.window, globalName);
            if (!winDesc || winDesc.configurable) {
                Object.defineProperty(globalThis.window, globalName, {
                    value: sharedShim,
                    writable: false,
                    enumerable: true,
                    configurable: false,
                });
            }
        }
    }

    function makePreloadFnProxy(propPath) {
        return function(...args) {
            return syncCall('preload/invoke', { propPath: String(propPath), args });
        };
    }

    function buildPreloadObjectProxy(basePath, entries) {
        const target = {};
        if (Array.isArray(entries)) {
            for (const entry of entries) {
                if (!entry || !entry.path) continue;
                const segments = String(entry.path).split('.');
                const leaf = segments[segments.length - 1];
                if (!leaf || leaf === '__proto__' || leaf === 'constructor' || leaf === 'prototype') continue;
                if (entry.kind === 'function') {
                    target[leaf] = makePreloadFnProxy(entry.path);
                } else if (entry.kind === 'value') {
                    target[leaf] = entry.value;
                } else if (entry.kind === 'object') {
                    target[leaf] = buildPreloadObjectProxy(entry.path, entry.entries);
                }
            }
        }
        if (typeof Proxy === 'function') {
            return new Proxy(target, {
                get(obj, prop, receiver) {
                    if (typeof prop === 'symbol' || Object.prototype.hasOwnProperty.call(obj, prop)) {
                        return Reflect.get(obj, prop, receiver);
                    }
                    if (prop === 'then' || prop === 'toJSON' || prop === 'valueOf' || prop === 'toString') {
                        return undefined;
                    }
                    return makePreloadFnProxy(basePath + '.' + String(prop));
                },
            });
        }
        return target;
    }

    const RESERVED_GLOBALS = new Set([
        'aitools', 'ztools', 'utools', 'rubick', 'window', 'globalThis', 'self',
        'document', 'navigator', 'location', 'localStorage', 'sessionStorage',
    ]);

    function bindPreloadProps(props) {
        if (!Array.isArray(props)) return;
        for (const entry of props) {
            if (!entry || !entry.path) continue;
            const rootName = String(entry.path).split('.')[0];
            if (!rootName || RESERVED_GLOBALS.has(rootName)) continue;
            let bound;
            if (entry.kind === 'function') {
                bound = makePreloadFnProxy(entry.path);
            } else if (entry.kind === 'value') {
                bound = entry.value;
            } else if (entry.kind === 'object') {
                bound = buildPreloadObjectProxy(entry.path, entry.entries);
            } else {
                continue;
            }
            globalThis[rootName] = bound;
            if (globalThis.window && globalThis.window !== globalThis) {
                globalThis.window[rootName] = bound;
            }
        }
    }

    globalThis.__aitools_create_preload_proxy__ = makePreloadFnProxy;
    globalThis.__aitools_bind_preload_props__ = bindPreloadProps;

    if (Array.isArray(globalThis.__AITOOLS_PRELOAD_PROPS__)) {
        bindPreloadProps(globalThis.__AITOOLS_PRELOAD_PROPS__);
    } else if (globalThis.__AITOOLS_HAS_PRELOAD__ === true && canSyncCall()) {
        try {
            const props = syncCall('preload/props', {});
            bindPreloadProps(props);
        } catch (_) {}
    }
})();"#;

/// Builds a plugin-scoped initialization script binding `__AITOOLS_PLUGIN_ID__` before `TRIPLE_GLOBAL_SHIM_JS`.
#[must_use]
pub fn build_triple_global_shim(plugin_id: &str) -> String {
    let escaped = serde_json::to_string(plugin_id).unwrap_or_else(|_| "\"unknown\"".to_string());
    format!("globalThis.__AITOOLS_PLUGIN_ID__ = {escaped};\n{TRIPLE_GLOBAL_SHIM_JS}")
}

/// Alias matching `WebviewBuilder::initialization_script` callers (`INV-SEC-02`).
#[must_use]
pub fn build_initialization_script(plugin_id: &str) -> String {
    build_triple_global_shim(plugin_id)
}

/// Builds a plugin-scoped initialization script that also binds Node preload `window.<prop>`
/// invocation proxies prior to page navigation (`SPEC-015-R4`, `INV-SEC-02`).
#[must_use]
pub fn build_initialization_script_with_preload_props(
    plugin_id: &str,
    exported_props: &[ExportedWindowProp],
) -> String {
    let escaped_id = serde_json::to_string(plugin_id).unwrap_or_else(|_| "\"unknown\"".to_string());
    let props_json = serde_json::to_string(exported_props).unwrap_or_else(|_| "[]".to_string());
    format!(
        "globalThis.__AITOOLS_PLUGIN_ID__ = {escaped_id};\n\
         globalThis.__AITOOLS_HAS_PRELOAD__ = true;\n\
         globalThis.__AITOOLS_PRELOAD_PROPS__ = {props_json};\n\
         {TRIPLE_GLOBAL_SHIM_JS}"
    )
}
