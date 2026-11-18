use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aitools_js_bridge::{
    build_initialization_script, build_initialization_script_with_preload_props,
    resolve_node_binary, ExportedWindowProp, HostBridgeHandler, JsBridgeError, ListItem,
    NodePreloadRuntime, NodeRuntimeConfig, NoopHostBridgeHandler, PreloadCapabilities,
    PreloadExecutionMode, PreloadRuntime, PreloadTier, TRIPLE_GLOBAL_SHIM_JS,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

#[derive(Default)]
struct RecordedRpcState {
    list_batches: Vec<Vec<ListItem>>,
    docs: HashMap<String, Value>,
    copied_texts: Vec<String>,
    copied_images: Vec<String>,
    copied_files: Vec<Vec<String>>,
    notifications: Vec<(String, Option<String>)>,
    window_ops: Vec<(String, Value)>,
    shell_ops: Vec<(String, Value)>,
    features: Vec<Value>,
    dialogs: Vec<(String, Value)>,
    input_ops: Vec<(String, Value)>,
    ai_ops: Vec<(String, Value)>,
    http_ops: Vec<(String, Value)>,
    rev_counter: u64,
}

#[derive(Default)]
struct RecordingHostBridge {
    state: Mutex<RecordedRpcState>,
}

impl RecordingHostBridge {
    fn snapshot_copied_texts(&self) -> Vec<String> {
        self.state.lock().unwrap().copied_texts.clone()
    }

    fn snapshot_list_batches(&self) -> Vec<Vec<ListItem>> {
        self.state.lock().unwrap().list_batches.clone()
    }

    fn snapshot_window_ops(&self) -> Vec<(String, Value)> {
        self.state.lock().unwrap().window_ops.clone()
    }

    fn snapshot_shell_ops(&self) -> Vec<(String, Value)> {
        self.state.lock().unwrap().shell_ops.clone()
    }

    fn snapshot_http_ops(&self) -> Vec<(String, Value)> {
        self.state.lock().unwrap().http_ops.clone()
    }

    fn snapshot_notifications(&self) -> Vec<(String, Option<String>)> {
        self.state.lock().unwrap().notifications.clone()
    }
}

impl HostBridgeHandler for RecordingHostBridge {
    fn callback_set_list(
        &self,
        _plugin_id: &str,
        items: Vec<ListItem>,
    ) -> aitools_js_bridge::Result<()> {
        self.state.lock().unwrap().list_batches.push(items);
        Ok(())
    }

    fn db_op(
        &self,
        _plugin_id: &str,
        op: &str,
        payload: Value,
    ) -> aitools_js_bridge::Result<Value> {
        let mut st = self.state.lock().unwrap();
        match op {
            "db/put" => {
                let id = payload["_id"].as_str().unwrap_or("").to_string();
                st.rev_counter += 1;
                let rev = format!("{}-node", st.rev_counter);
                let mut stored = payload.clone();
                stored["_rev"] = json!(rev);
                st.docs.insert(id.clone(), stored);
                Ok(json!({ "id": id, "rev": rev, "ok": true }))
            }
            "db/get" => {
                let id = payload["id"].as_str().unwrap_or("");
                Ok(st.docs.get(id).cloned().unwrap_or(Value::Null))
            }
            "db/remove" => {
                let id = payload["id"]
                    .as_str()
                    .or_else(|| payload["doc"]["_id"].as_str())
                    .unwrap_or("")
                    .to_string();
                st.docs.remove(&id);
                Ok(json!({ "id": id, "ok": true }))
            }
            "db/allDocs" => {
                let list: Vec<Value> = st.docs.values().cloned().collect();
                Ok(Value::Array(list))
            }
            _ => Ok(Value::Null),
        }
    }

    fn clipboard_op(
        &self,
        _plugin_id: &str,
        op: &str,
        payload: Value,
    ) -> aitools_js_bridge::Result<Value> {
        let mut st = self.state.lock().unwrap();
        match op {
            "clipboard/copyText" => {
                st.copied_texts
                    .push(payload["text"].as_str().unwrap_or("").to_string());
                Ok(Value::Bool(true))
            }
            "clipboard/readText" => Ok(Value::String(
                st.copied_texts.last().cloned().unwrap_or_default(),
            )),
            "clipboard/copyImage" => {
                st.copied_images
                    .push(payload["image"].as_str().unwrap_or("").to_string());
                Ok(Value::Bool(true))
            }
            "clipboard/copyFile" => {
                let files = payload["files"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                st.copied_files.push(files);
                Ok(Value::Bool(true))
            }
            "clipboard/getCopyedFiles" => Ok(json!([
                { "path": "/tmp/demo.txt", "name": "demo.txt", "isFile": true, "isDiractory": false }
            ])),
            _ => Ok(Value::Null),
        }
    }

    fn notification_op(
        &self,
        _plugin_id: &str,
        _op: &str,
        payload: Value,
    ) -> aitools_js_bridge::Result<Value> {
        let body = payload["body"].as_str().unwrap_or("").to_string();
        let click_feature_code = payload["clickFeatureCode"].as_str().map(str::to_string);
        self.state
            .lock()
            .unwrap()
            .notifications
            .push((body, click_feature_code));
        Ok(Value::Bool(true))
    }

    fn window_op(
        &self,
        _plugin_id: &str,
        op: &str,
        payload: Value,
    ) -> aitools_js_bridge::Result<Value> {
        self.state
            .lock()
            .unwrap()
            .window_ops
            .push((op.to_string(), payload));
        Ok(Value::Bool(true))
    }

    fn shell_op(
        &self,
        _plugin_id: &str,
        op: &str,
        payload: Value,
    ) -> aitools_js_bridge::Result<Value> {
        self.state
            .lock()
            .unwrap()
            .shell_ops
            .push((op.to_string(), payload));
        Ok(Value::Bool(true))
    }

    fn system_op(
        &self,
        plugin_id: &str,
        op: &str,
        payload: Value,
    ) -> aitools_js_bridge::Result<Value> {
        NoopHostBridgeHandler.system_op(plugin_id, op, payload)
    }

    fn features_op(
        &self,
        _plugin_id: &str,
        op: &str,
        payload: Value,
    ) -> aitools_js_bridge::Result<Value> {
        let mut st = self.state.lock().unwrap();
        match op {
            "features/setFeature" => {
                st.features.push(payload);
                Ok(Value::Bool(true))
            }
            "features/getFeatures" => Ok(Value::Array(st.features.clone())),
            "features/removeFeature" => {
                let code = payload["code"].as_str().unwrap_or("");
                st.features.retain(|f| f["code"].as_str() != Some(code));
                Ok(Value::Bool(true))
            }
            _ => Ok(Value::Bool(true)),
        }
    }

    fn dialog_op(
        &self,
        _plugin_id: &str,
        op: &str,
        payload: Value,
    ) -> aitools_js_bridge::Result<Value> {
        self.state
            .lock()
            .unwrap()
            .dialogs
            .push((op.to_string(), payload));
        match op {
            "dialog/showOpenDialog" => Ok(json!(["/tmp/selected-file.txt"])),
            "dialog/showSaveDialog" => Ok(json!("/tmp/saved-file.txt")),
            _ => Ok(Value::Null),
        }
    }

    fn ai_op(
        &self,
        _plugin_id: &str,
        op: &str,
        payload: Value,
    ) -> aitools_js_bridge::Result<Value> {
        self.state
            .lock()
            .unwrap()
            .ai_ops
            .push((op.to_string(), payload));
        match op {
            "ai/chat" => Ok(json!({ "role": "assistant", "content": "mocked-ai-ok" })),
            "ai/allModels" => Ok(json!([{ "id": "fast", "label": "Fast Model" }])),
            _ => Ok(Value::Null),
        }
    }

    fn input_op(
        &self,
        _plugin_id: &str,
        op: &str,
        payload: Value,
    ) -> aitools_js_bridge::Result<Value> {
        self.state
            .lock()
            .unwrap()
            .input_ops
            .push((op.to_string(), payload));
        Ok(Value::Bool(true))
    }

    fn http_op(
        &self,
        _plugin_id: &str,
        op: &str,
        payload: Value,
    ) -> aitools_js_bridge::Result<Value> {
        self.state
            .lock()
            .unwrap()
            .http_ops
            .push((op.to_string(), payload.clone()));
        let url = payload["url"]
            .as_str()
            .unwrap_or("https://example.invalid")
            .to_string();
        Ok(json!({
            "ok": true,
            "status": 200,
            "statusText": "OK",
            "url": url,
            "headers": {
                "content-type": "application/json",
                "x-aitools-http": "1"
            },
            "body": "{\"fromHost\":true}"
        }))
    }
}

fn hash_dir_tree(root: &Path) -> BTreeMap<PathBuf, [u8; 32]> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            let ft = entry.file_type().unwrap();
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                let bytes = std::fs::read(&path).unwrap();
                let mut hasher = Sha256::new();
                hasher.update(&bytes);
                let rel = path.strip_prefix(root).unwrap().to_path_buf();
                out.insert(rel, hasher.finalize().into());
            }
        }
    }
    out
}

fn run_node_script(script: &str) -> Value {
    let node_bin = resolve_node_binary().expect("node binary must be installed for spec_015 tests");
    let dir = TempDir::new().unwrap();
    let script_path = dir.path().join("test.cjs");
    std::fs::write(&script_path, script).unwrap();
    let output = Command::new(&node_bin)
        .arg(&script_path)
        .output()
        .expect("failed running node verification script");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "node script failed (status={}):\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("invalid JSON from node script ({e}):\nstdout:\n{stdout}\nstderr:\n{stderr}")
    })
}

fn legacy_backup_tarball() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let candidate = PathBuf::from(home).join(".aitools/atools-legacy-source-backup.tar.gz");
    candidate.is_file().then_some(candidate)
}

#[test]
fn spec_015_r1_zero_mod_upstream_sha256_unchanged() {
    // Part A: synthetic unmodified CommonJS + node_modules plugin fixture.
    let fixture = TempDir::new().unwrap();
    let plugin_dir = fixture.path().join("synthetic-ztools");
    std::fs::create_dir_all(plugin_dir.join("lib")).unwrap();
    std::fs::create_dir_all(plugin_dir.join("node_modules/mini-slug")).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.json"),
        r#"{"name":"synthetic-ztools","version":"1.0.0","main":"index.html","preload":"preload.js","features":[]}"#,
    )
    .unwrap();
    std::fs::write(
        plugin_dir.join("node_modules/mini-slug/package.json"),
        r#"{"name":"mini-slug","version":"1.0.0","main":"index.js"}"#,
    )
    .unwrap();
    std::fs::write(
        plugin_dir.join("node_modules/mini-slug/index.js"),
        "module.exports = (s) => String(s).trim().toLowerCase().replace(/\\s+/g, '-');\n",
    )
    .unwrap();
    std::fs::write(
        plugin_dir.join("lib/helper.cjs"),
        "exports.upper = (s) => String(s).toUpperCase();\n",
    )
    .unwrap();
    std::fs::write(
        plugin_dir.join("preload.js"),
        r#"
const slug = require('mini-slug');
const { upper } = require('./lib/helper.cjs');
window.syntheticApi = {
  slugify(text) { return slug(text); },
  shout(text) { return upper(text); },
};
window.exports = {
  demo: {
    mode: 'list',
    args: {
      enter(action, cb) { cb([{ title: slug(action.payload || 'ready'), description: 'ok' }]); },
      search(action, q, cb) { cb([{ title: upper(q), description: 'search' }]); },
      select(action, item) { return { picked: item.title }; }
    }
  }
};
"#,
    )
    .unwrap();

    let before_synthetic = hash_dir_tree(&plugin_dir);
    let handler = Arc::new(RecordingHostBridge::default());
    {
        let runtime = NodePreloadRuntime::new(Arc::clone(&handler) as Arc<dyn HostBridgeHandler>);
        let info = runtime
            .load("preload.js", "synthetic-ztools", &plugin_dir)
            .unwrap();
        assert_eq!(info.export_features.len(), 1);
        let slugged = runtime
            .invoke_window_prop("syntheticApi.slugify", &[json!("  Hello ZTools  ")])
            .unwrap();
        assert_eq!(slugged, json!("hello-ztools"));
        runtime
            .enter("demo", &json!({ "code": "demo", "payload": "Init Item" }))
            .unwrap();
        runtime
            .search("demo", &json!({ "code": "demo" }), "world")
            .unwrap();
        let selected = runtime
            .select(
                "demo",
                &json!({ "code": "demo" }),
                &json!({ "title": "WORLD" }),
            )
            .unwrap();
        assert_eq!(selected["result"]["picked"], json!("WORLD"));
        runtime.plugin_out(true).unwrap();
        runtime.shutdown().unwrap();
    }
    let after_synthetic = hash_dir_tree(&plugin_dir);
    assert_eq!(
        before_synthetic, after_synthetic,
        "synthetic ZTools plugin directory must remain byte-for-byte immutable"
    );

    // Part B: real unmodified ZTools plugins extracted from backup archive when available.
    if let Some(tarball) = legacy_backup_tarball() {
        let extract_dir = TempDir::new().unwrap();
        let status = Command::new("tar")
            .arg("-xzf")
            .arg(&tarball)
            .arg("--strip-components=3")
            .arg("-C")
            .arg(extract_dir.path())
            .arg("atools/ZTools-plugins/plugins/char-transform")
            .arg("atools/ZTools-plugins/plugins/uuid-kit")
            .status()
            .expect("failed running tar extraction");
        assert!(status.success(), "tar extraction of ZTools plugins failed");

        let char_dir = extract_dir.path().join("char-transform");
        let uuid_dir = extract_dir.path().join("uuid-kit");
        let char_before = hash_dir_tree(&char_dir);
        let uuid_before = hash_dir_tree(&uuid_dir);

        // Exercise real char-transform preload.js unmodified.
        {
            let rt = NodePreloadRuntime::new(Arc::new(NoopHostBridgeHandler));
            let load_info = rt.load("preload.js", "char-transform", &char_dir).unwrap();
            assert!(
                load_info.exported_props.iter().any(|p| matches!(
                    p,
                    ExportedWindowProp::Object { path, .. } if path == "charTransformApi"
                )),
                "charTransformApi should be discovered on window: {:?}",
                load_info.exported_props
            );
            let camel = rt
                .invoke_window_prop(
                    "charTransformApi.transform",
                    &[json!("hello ztools world"), json!("camelCase")],
                )
                .unwrap();
            assert_eq!(camel, json!("helloZtoolsWorld"));
            let md5 = rt
                .invoke_window_prop(
                    "charTransformApi.transform",
                    &[json!("hello"), json!("md5")],
                )
                .unwrap();
            assert_eq!(md5, json!("5d41402abc4b2a76b9719d911017c592"));
            let modes = rt
                .invoke_window_prop("charTransformApi.modes", &[])
                .unwrap();
            assert!(modes
                .as_array()
                .is_some_and(|a| a.contains(&json!("camelCase"))));
            rt.enter(
                "char-transform",
                &json!({ "code": "char-transform", "type": "text", "payload": "hello" }),
            )
            .unwrap();
            rt.plugin_out(true).unwrap();
            rt.shutdown().unwrap();
        }
        assert_eq!(
            char_before,
            hash_dir_tree(&char_dir),
            "char-transform files must have identical SHA-256 after execution"
        );

        // Exercise real uuid-kit preload.js unmodified (uses require('electron').clipboard).
        {
            let rec = Arc::new(RecordingHostBridge::default());
            let rt = NodePreloadRuntime::new(Arc::clone(&rec) as Arc<dyn HostBridgeHandler>);
            let load_info = rt.load("preload.js", "uuid-kit", &uuid_dir).unwrap();
            assert!(
                load_info.exported_props.iter().any(|p| matches!(
                    p,
                    ExportedWindowProp::Function { path } if path == "__nativeCopy"
                )),
                "uuid-kit __nativeCopy should be exported: {:?}",
                load_info.exported_props
            );
            let ok = rt
                .invoke_window_prop("__nativeCopy", &[json!("uuid-test-1234")])
                .unwrap();
            assert_eq!(ok, json!(true));
            assert_eq!(rec.snapshot_copied_texts(), vec!["uuid-test-1234"]);
            rt.shutdown().unwrap();
        }
        assert_eq!(
            uuid_before,
            hash_dir_tree(&uuid_dir),
            "uuid-kit files must have identical SHA-256 after execution"
        );
    }
}

#[test]
fn spec_015_r2_node_preload_spawns_commonjs_with_electron_and_globals() {
    let dir = TempDir::new().unwrap();
    let plugin_dir = dir.path();
    std::fs::create_dir_all(plugin_dir.join("node_modules/pkg-in-node-modules")).unwrap();
    std::fs::write(
        plugin_dir.join("node_modules/pkg-in-node-modules/index.js"),
        "module.exports = { answer: 42 };\n",
    )
    .unwrap();
    let png_stub: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x08,
    ];
    std::fs::write(plugin_dir.join("sample.png"), png_stub).unwrap();

    std::fs::write(
        plugin_dir.join("preload.js"),
        r#"
const crypto = require('node:crypto');
const pkg = require('pkg-in-node-modules');
const electron = require('electron');

// Ensure noisy console output does not break NDJSON stdio protocol.
console.log('plugin preload noisy log', { ok: true });

window.envCheck = {
  async inspectAll() {
    electron.clipboard.writeText('from-electron');
    const readBack = electron.clipboard.readText();
    const img = electron.nativeImage.createFromPath('sample.png');
    const primary = electron.screen.getPrimaryDisplay();
    const opened = await electron.dialog.showOpenDialog({});
    await electron.shell.openExternal('https://example.com');
    const el = document.createElement('div');
    el.classList.add('active');
    localStorage.setItem('k', 'v');
    return {
      winSelf: window === global && globalThis.window === globalThis,
      isHtmlEl: el instanceof HTMLElement && el.classList.contains('active'),
      hasNavigator: typeof navigator.platform === 'string' && navigator.platform.length > 0,
      locationProto: location.protocol,
      storageVal: localStorage.getItem('k'),
      globalsIdentical: ztools === utools && utools === aitools && aitools === rubick,
      globalsFrozen: Object.isFrozen(ztools) && Object.isFrozen(ztools.db),
      pkgAnswer: pkg.answer,
      sha: crypto.createHash('sha256').update('abc').digest('hex'),
      electronReadBack: readBack,
      imgSize: img.getSize(),
      imgEmpty: img.isEmpty(),
      primaryWidth: primary.bounds.width,
      dialogPaths: opened.filePaths,
    };
  },
  infiniteLoop() {
    while (true) {}
  }
};
"#,
    )
    .unwrap();

    let handler = Arc::new(RecordingHostBridge::default());
    let cfg = NodeRuntimeConfig {
        node_binary: None,
        startup_timeout: Duration::from_secs(10),
        call_timeout: Duration::from_millis(500),
    };
    let runtime =
        NodePreloadRuntime::with_config(cfg, Arc::clone(&handler) as Arc<dyn HostBridgeHandler>);
    runtime
        .load("preload.js", "env-plugin", plugin_dir)
        .unwrap();
    assert!(runtime.is_alive());

    let res = runtime
        .invoke_window_prop("envCheck.inspectAll", &[])
        .unwrap();
    assert_eq!(res["winSelf"], json!(true));
    assert_eq!(res["isHtmlEl"], json!(true));
    assert_eq!(res["hasNavigator"], json!(true));
    assert_eq!(res["locationProto"], json!("aitools-plugin:"));
    assert_eq!(res["storageVal"], json!("v"));
    assert_eq!(res["globalsIdentical"], json!(true));
    assert_eq!(res["globalsFrozen"], json!(true));
    assert_eq!(res["pkgAnswer"], json!(42));
    assert_eq!(
        res["sha"],
        json!("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    );
    assert_eq!(res["electronReadBack"], json!("from-electron"));
    assert_eq!(res["imgSize"], json!({ "width": 16, "height": 8 }));
    assert_eq!(res["imgEmpty"], json!(false));
    assert_eq!(res["primaryWidth"], json!(1920));
    assert_eq!(res["dialogPaths"], json!(["/tmp/selected-file.txt"]));
    assert_eq!(handler.snapshot_shell_ops()[0].0, "shell/openExternal");

    // Verify bounded timeout kills runaway loops cleanly without hanging Drop.
    let err = runtime
        .invoke_window_prop("envCheck.infiniteLoop", &[])
        .unwrap_err();
    assert!(
        matches!(err, JsBridgeError::Timeout(_)),
        "expected Timeout error, got: {err:?}"
    );
    assert!(!runtime.is_alive());
}

#[test]
fn spec_015_r3_node_preload_list_mode_enter_search_select_and_invoke() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("preload.js"),
        r#"
let lastOutKill = null;

window.ztools.onMainPush((action) => {
  return [{ text: 'Push:' + (action.payload || ''), title: 'MainPush' }];
});

window.ztools.onPluginOut((isKill) => {
  lastOutKill = isKill;
});

window.services = {
  async asyncGreet(name) {
    return 'hello ' + name;
  },
  getLastOutKill() {
    return lastOutKill;
  }
};

window.exports = {
  bookmarks: {
    mode: 'list',
    args: {
      enter(action, callbackSetList) {
        window.ztools.db.put({ _id: 'bm-1', url: 'https://rust-lang.org' });
        const downloads = window.ztools.getPath('downloads');
        callbackSetList([{ title: 'Rust Home', description: downloads, url: 'https://rust-lang.org' }]);
      },
      search(action, query, callbackSetList) {
        const doc = window.ztools.db.get('bm-1');
        callbackSetList([
          {
            title: 'Match ' + query,
            description: doc ? doc.url : 'missing',
            url: doc ? doc.url : '',
          }
        ]);
      },
      select(action, itemData) {
        window.ztools.copyText(itemData.url);
        window.ztools.outPlugin(false);
        return { openedUrl: itemData.url };
      }
    }
  }
};
"#,
    )
    .unwrap();

    let handler = Arc::new(RecordingHostBridge::default());
    let runtime = NodePreloadRuntime::new(Arc::clone(&handler) as Arc<dyn HostBridgeHandler>);
    let info = runtime
        .load("preload.js", "list-plugin", dir.path())
        .unwrap();
    assert!(info.has_on_main_push);
    assert!(info.has_on_plugin_out);
    assert_eq!(info.export_features.len(), 1);
    assert_eq!(info.export_features[0].code, "bookmarks");

    runtime
        .enter(
            "bookmarks",
            &json!({ "code": "bookmarks", "type": "text", "payload": "" }),
        )
        .unwrap();
    runtime
        .search("bookmarks", &json!({ "code": "bookmarks" }), "rust")
        .unwrap();

    let batches = handler.snapshot_list_batches();
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0][0].title, "Rust Home");
    assert_eq!(batches[1][0].title, "Match rust");
    assert_eq!(batches[1][0].description, "https://rust-lang.org");

    let item_json = serde_json::to_value(&batches[1][0]).unwrap();
    let sel = runtime
        .select("bookmarks", &json!({ "code": "bookmarks" }), &item_json)
        .unwrap();
    assert_eq!(sel["result"]["openedUrl"], json!("https://rust-lang.org"));
    assert_eq!(
        handler.snapshot_copied_texts(),
        vec!["https://rust-lang.org"]
    );
    assert!(handler
        .snapshot_window_ops()
        .iter()
        .any(|(op, _)| op == "window/outPlugin"));

    let push_res = runtime
        .main_push("bookmarks", &json!({ "payload": "query" }))
        .unwrap();
    assert_eq!(push_res[0]["text"], json!("Push:query"));

    let greeted = runtime
        .invoke_window_prop("services.asyncGreet", &[json!("aitools")])
        .unwrap();
    assert_eq!(greeted, json!("hello aitools"));

    runtime.plugin_out(true).unwrap();
    let killed = runtime
        .invoke_window_prop("services.getLastOutKill", &[])
        .unwrap();
    assert_eq!(killed, json!(true));
    runtime.shutdown().unwrap();
}

#[test]
fn spec_015_r4_webview_shim_sync_call_and_preload_proxy() {
    let props = vec![
        ExportedWindowProp::Object {
            path: "charTransformApi".to_string(),
            entries: vec![
                ExportedWindowProp::Value {
                    path: "charTransformApi.modes".to_string(),
                    value: json!(["camelCase", "md5"]),
                },
                ExportedWindowProp::Function {
                    path: "charTransformApi.transform".to_string(),
                },
            ],
        },
        ExportedWindowProp::Function {
            path: "__nativeCopy".to_string(),
        },
    ];
    let init_script = build_initialization_script_with_preload_props("char-transform", &props);
    let plain_init = build_initialization_script("plain-id");
    assert!(plain_init.contains("\"plain-id\""));

    let test_js = format!(
        r#"
const calls = [];
globalThis.window = globalThis;
globalThis.location = {{ href: 'aitools-plugin://fallback-id/index.html', hostname: 'fallback-id' }};
globalThis.XMLHttpRequest = class MockXHR {{
  open(method, url, asyncFlag) {{
    this.method = method;
    this.url = url;
    this.asyncFlag = asyncFlag;
    this.headers = {{}};
  }}
  setRequestHeader(k, v) {{ this.headers[k] = v; }}
  send(body) {{
    const parsedBody = body ? JSON.parse(body) : null;
    calls.push({{ method: this.method, url: this.url, asyncFlag: this.asyncFlag, body: parsedBody }});
    if (this.url.endsWith('/err/envelope')) {{
      this.status = 200;
      this.responseText = JSON.stringify({{ ok: false, error: 'envelope failure' }});
      return;
    }}
    if (this.url.endsWith('/err/http')) {{
      this.status = 403;
      this.responseText = JSON.stringify({{ error: 'forbidden caller' }});
      return;
    }}
    if (this.url.endsWith('/preload/invoke')) {{
      this.status = 200;
      this.responseText = JSON.stringify({{ ok: true, data: {{ invoked: parsedBody.propPath, args: parsedBody.args }} }});
      return;
    }}
    this.status = 200;
    this.responseText = JSON.stringify({{ ok: true, data: {{ pong: parsedBody }} }});
  }}
}};

{init_script}

const syncRes = globalThis.__aitools_sync_call__('custom/ping', {{ n: 7 }});
let envErr = '';
try {{ globalThis.__aitools_sync_call__('err/envelope', {{}}); }} catch (e) {{ envErr = e.message; }}
let httpErr = '';
try {{ globalThis.__aitools_sync_call__('err/http', {{}}); }} catch (e) {{ httpErr = e.message; }}

const transformRes = window.charTransformApi.transform('hello world', 'camelCase');
const dynamicProxyRes = window.charTransformApi.dynamicExtra('arg1');
const nativeCopyRes = window.__nativeCopy('copied');

// Also verify location fallback when __AITOOLS_PLUGIN_ID__ is omitted.
delete globalThis.__AITOOLS_PLUGIN_ID__;
delete globalThis.__aitools_sync_call__;
{TRIPLE_GLOBAL_SHIM_JS}
const fallbackRes = globalThis.__aitools_sync_call__('custom/loc', {{}});

console.log(JSON.stringify({{
  sameGlobals: window.aitools === window.ztools && window.ztools === window.utools && window.utools === window.rubick,
  modes: window.charTransformApi.modes,
  syncRes,
  envErr,
  httpErr,
  transformRes,
  dynamicProxyRes,
  nativeCopyRes,
  calls,
  fallbackUrl: calls[calls.length - 1].url,
  fallbackRes,
}}));
"#
    );

    let out = run_node_script(&test_js);
    assert_eq!(out["sameGlobals"], json!(true));
    assert_eq!(out["modes"], json!(["camelCase", "md5"]));
    assert_eq!(out["syncRes"], json!({ "pong": { "n": 7 } }));
    assert_eq!(out["envErr"], json!("envelope failure"));
    assert_eq!(out["httpErr"], json!("forbidden caller"));
    assert_eq!(
        out["transformRes"],
        json!({ "invoked": "charTransformApi.transform", "args": ["hello world", "camelCase"] })
    );
    assert_eq!(
        out["dynamicProxyRes"],
        json!({ "invoked": "charTransformApi.dynamicExtra", "args": ["arg1"] })
    );
    assert_eq!(
        out["nativeCopyRes"],
        json!({ "invoked": "__nativeCopy", "args": ["copied"] })
    );
    assert_eq!(
        out["calls"][0]["url"],
        json!("aitools-sync://char-transform/custom/ping")
    );
    assert_eq!(out["calls"][0]["asyncFlag"], json!(false));
    assert_eq!(
        out["fallbackUrl"],
        json!("aitools-sync://fallback-id/custom/loc")
    );
}

#[test]
fn spec_015_r5_sync_protocol_db_all_ops_attachments_and_promises() {
    let init_script = build_initialization_script("storage-plugin");
    let test_js = format!(
        r#"
const docs = new Map();
const atts = new Map();
const ops = [];
let revSeq = 0;

globalThis.window = globalThis;
globalThis.XMLHttpRequest = class MockDbXHR {{
  open(method, url) {{ this.url = url; }}
  setRequestHeader() {{}}
  send(body) {{
    const op = this.url.replace(/^aitools-sync:\/\/[^/]+\//, '');
    const payload = body ? JSON.parse(body) : {{}};
    ops.push({{ op, payload }});
    this.status = 200;

    if (op === 'db/put') {{
      const existing = docs.get(payload._id);
      if (existing && payload._rev !== existing._rev) {{
        this.responseText = JSON.stringify({{ id: payload._id, error: true, name: 'conflict', message: 'conflict' }});
        return;
      }}
      const rev = `${{++revSeq}}-hash`;
      const stored = Object.assign({{}}, payload, {{ _rev: rev }});
      docs.set(payload._id, stored);
      this.responseText = JSON.stringify({{ id: payload._id, rev, ok: true }});
      return;
    }}
    if (op === 'db/get') {{
      this.responseText = JSON.stringify(docs.get(payload.id) || null);
      return;
    }}
    if (op === 'db/remove') {{
      const id = payload.doc ? payload.doc._id : payload.id;
      const existing = docs.get(id);
      if (!existing) {{
        this.responseText = JSON.stringify({{ id, error: true, name: 'not_found' }});
        return;
      }}
      if (payload.doc && payload.doc._rev && payload.doc._rev !== existing._rev) {{
        this.responseText = JSON.stringify({{ id, error: true, name: 'conflict' }});
        return;
      }}
      docs.delete(id);
      this.responseText = JSON.stringify({{ id, rev: existing._rev, ok: true }});
      return;
    }}
    if (op === 'db/allDocs') {{
      let out = Array.from(docs.values());
      if (Array.isArray(payload.keys)) {{
        const set = new Set(payload.keys);
        out = out.filter((d) => set.has(d._id));
      }} else if (typeof payload.key === 'string') {{
        out = out.filter((d) => d._id.startsWith(payload.key));
      }}
      this.responseText = JSON.stringify(out);
      return;
    }}
    if (op === 'db/bulkDocs') {{
      const results = (payload.docs || []).map((d) => {{
        const rev = `${{++revSeq}}-bulk`;
        docs.set(d._id, Object.assign({{}}, d, {{ _rev: rev }}));
        return {{ id: d._id, rev, ok: true }};
      }});
      this.responseText = JSON.stringify(results);
      return;
    }}
    if (op === 'db/putAttachment') {{
      atts.set(payload.id, {{ data: payload.data, contentType: payload.contentType || payload.type }});
      this.responseText = JSON.stringify({{ id: payload.id, ok: true }});
      return;
    }}
    if (op === 'db/getAttachment') {{
      const entry = atts.get(payload.id);
      this.responseText = JSON.stringify(entry ? {{ data: entry.data, contentType: entry.contentType }} : null);
      return;
    }}
    if (op === 'db/getAttachmentType') {{
      const entry = atts.get(payload.id);
      this.responseText = JSON.stringify(entry ? entry.contentType : null);
      return;
    }}
    if (op === 'db/removeAttachment') {{
      atts.delete(payload.id);
      this.responseText = JSON.stringify({{ id: payload.id, ok: true }});
      return;
    }}
    this.status = 404;
    this.responseText = JSON.stringify({{ error: 'unknown op ' + op }});
  }}
}};

{init_script}

(async function main() {{
  const r1 = ztools.db.put({{ _id: 'todo:1', title: 'first' }});
  const r2 = ztools.db.put({{ _id: 'todo:2', title: 'second' }});
  const r3 = ztools.db.put({{ _id: 'note:1', title: 'note' }});

  const allAll = ztools.db.allDocs();
  const allPrefix = ztools.db.allDocs('todo:');
  const allKeys = ztools.db.allDocs(['todo:1', 'note:1']);

  const remById = ztools.db.remove('todo:1');
  const remByDoc = ztools.db.remove({{ _id: 'todo:2', _rev: r2.rev }});
  const afterRemove = ztools.db.allDocs('todo:');

  const bulkRes = ztools.db.bulkDocs([{{ _id: 'b1', v: 1 }}, {{ _id: 'b2', v: 2 }}]);

  // Attachments roundtrip
  const bin = new Uint8Array([0, 42, 128, 255]);
  ztools.db.putAttachment('att-bin', bin, 'application/octet-stream');
  const gotBin = ztools.db.getAttachment('att-bin');
  const gotType = ztools.db.getAttachmentType('att-bin');
  ztools.db.removeAttachment('att-bin');
  const gotAfterRemove = ztools.db.getAttachment('att-bin');

  // db.promises.*
  const pPut = await ztools.db.promises.put({{ _id: 'p1', ok: 1 }});
  const pGet = await ztools.db.promises.get('p1');
  const pAll = await ztools.db.promises.allDocs(['p1']);
  const pRem = await ztools.db.promises.remove({{ _id: 'p1', _rev: pPut.rev }});

  // dbStorage update with rev preservation + dbCryptoStorage
  ztools.dbStorage.setItem('cfg', {{ count: 1 }});
  ztools.dbStorage.setItem('cfg', {{ count: 2 }});
  const cfgVal = ztools.dbStorage.getItem('cfg');
  ztools.dbStorage.removeItem('cfg');
  const cfgGone = ztools.dbStorage.getItem('cfg');

  ztools.dbCryptoStorage.setItem('token', 'super-secret-token');
  const rawCryptoDoc = ztools.db.get('__aitools_crypto__:token');
  const decrypted = ztools.dbCryptoStorage.getItem('token');
  ztools.dbCryptoStorage.removeItem('token');
  const cryptoGone = ztools.dbCryptoStorage.getItem('token');

  console.log(JSON.stringify({{
    r1Ok: r1.ok,
    r3Ok: r3.ok,
    allAllLen: allAll.length,
    allPrefixLen: allPrefix.length,
    allKeysLen: allKeys.length,
    remByIdOk: remById.ok,
    remByDocOk: remByDoc.ok,
    afterRemoveLen: afterRemove.length,
    bulkLen: bulkRes.length,
    gotBinArr: Array.from(gotBin),
    gotBinIsU8: gotBin instanceof Uint8Array,
    gotType,
    gotAfterRemove,
    pGetOk: pGet && pGet.ok,
    pAllLen: pAll.length,
    pRemOk: pRem.ok,
    cfgVal,
    cfgGone,
    rawHasCipher: Boolean(rawCryptoDoc && rawCryptoDoc.cipher && !JSON.stringify(rawCryptoDoc).includes('super-secret-token')),
    decrypted,
    cryptoGone,
  }}));
}})().catch((e) => {{
  console.error(e);
  process.exit(1);
}});
"#
    );

    let out = run_node_script(&test_js);
    assert_eq!(out["r1Ok"], json!(true));
    assert_eq!(out["r3Ok"], json!(true));
    assert_eq!(out["allAllLen"], json!(3));
    assert_eq!(out["allPrefixLen"], json!(2));
    assert_eq!(out["allKeysLen"], json!(2));
    assert_eq!(out["remByIdOk"], json!(true));
    assert_eq!(out["remByDocOk"], json!(true));
    assert_eq!(out["afterRemoveLen"], json!(0));
    assert_eq!(out["bulkLen"], json!(2));
    assert_eq!(out["gotBinArr"], json!([0, 42, 128, 255]));
    assert_eq!(out["gotBinIsU8"], json!(true));
    assert_eq!(out["gotType"], json!("application/octet-stream"));
    assert_eq!(out["gotAfterRemove"], Value::Null);
    assert_eq!(out["pGetOk"], json!(1));
    assert_eq!(out["pAllLen"], json!(1));
    assert_eq!(out["pRemOk"], json!(true));
    assert_eq!(out["cfgVal"], json!({ "count": 2 }));
    assert_eq!(out["cfgGone"], Value::Null);
    assert_eq!(out["rawHasCipher"], json!(true));
    assert_eq!(out["decrypted"], json!("super-secret-token"));
    assert_eq!(out["cryptoGone"], Value::Null);
}

#[test]
fn spec_015_r6_ztools_lifecycle_os_clipboard_shell_window_and_ai_apis() {
    let init_script = build_initialization_script("full-api-plugin");
    let test_js = format!(
        r#"
const ops = [];
globalThis.window = globalThis;
globalThis.XMLHttpRequest = class MockFullXHR {{
  open(method, url) {{ this.url = url; }}
  setRequestHeader() {{}}
  send(body) {{
    const op = this.url.replace(/^aitools-sync:\/\/[^/]+\//, '');
    const payload = body ? JSON.parse(body) : {{}};
    ops.push({{ op, payload }});
    this.status = 200;
    let data = true;
    switch (op) {{
      case 'clipboard/getCopyedFiles':
        data = ['/tmp/a.txt', {{ path: '/tmp/folder/', name: 'folder', isDirectory: true }}];
        break;
      case 'system/getPath': data = '/tmp/' + payload.name; break;
      case 'system/getAppName': data = 'aitools'; break;
      case 'system/getAppVersion': data = '1.0.0'; break;
      case 'system/getNativeId': data = 'native-xyz'; break;
      case 'system/isMacOs': data = true; break;
      case 'system/isWindows': data = false; break;
      case 'system/isLinux': data = false; break;
      case 'system/isDarkColors': data = true; break;
      case 'system/screenCapture': data = 'data:image/png;base64,AAAA'; break;
      case 'dialog/showOpenDialog': data = ['/tmp/one.txt']; break;
      case 'dialog/showSaveDialog': data = '/tmp/save.txt'; break;
      case 'display/getPrimaryDisplay': data = {{ id: 1, scaleFactor: 2 }}; break;
      case 'display/getAllDisplays': data = [{{ id: 1, scaleFactor: 2 }}]; break;
      case 'display/getDisplayNearestPoint': data = {{ id: 1 }}; break;
      case 'display/getCursorScreenPoint': data = {{ x: 100, y: 200 }}; break;
      case 'display/dipToScreenPoint': data = {{ x: payload.x * 2, y: payload.y * 2 }}; break;
      case 'display/screenToDipPoint': data = {{ x: payload.x / 2, y: payload.y / 2 }}; break;
      case 'features/getFeatures': data = [{{ code: 'f1' }}]; break;
      case 'ai/chat': data = {{ role: 'assistant', content: 'AI reply', chunks: [{{ content: 'AI ' }}, {{ content: 'reply' }}] }}; break;
      case 'ai/allModels': data = [{{ id: 'balanced', label: 'Balanced' }}]; break;
      case 'system/readCurrentBrowserUrl': data = 'https://example.org'; break;
      case 'system/readCurrentFolderPath': data = '/Users/demo/Projects'; break;
      default: data = true;
    }}
    this.responseText = JSON.stringify({{ ok: true, data }});
  }}
}};

{init_script}

(async function main() {{
  const events = [];
  ztools.onPluginReady((a) => events.push(['ready', a.code]));
  ztools.onPluginEnter((a) => events.push(['enter', a.code]));
  ztools.onPluginOut((k) => events.push(['out', k]));
  ztools.onPluginDetach(() => events.push(['detach']));
  ztools.onPluginAttach(() => events.push(['attach']));
  ztools.onMainPush((a) => [{{ text: a.payload }}], () => true);

  let subText = '';
  ztools.setSubInput((d) => {{ subText = d.text; }}, 'Search...', true);
  globalThis.__aitools_dispatch_lifecycle__('pluginEnter', {{ code: 'feat-1' }});
  globalThis.__aitools_dispatch_lifecycle__('subInput', 'hello sub');
  globalThis.__aitools_dispatch_lifecycle__('pluginOut', true);
  globalThis.__aitools_dispatch_lifecycle__('pluginDetach', {{}});
  globalThis.__aitools_dispatch_lifecycle__('pluginAttach', {{}});

  ztools.setSubInputValue('next');
  ztools.removeSubInput();
  ztools.redirect('other-plugin', 'payload');
  ztools.outPlugin(false);
  const hideRes = await ztools.hideMainWindow(true);
  const typeRes = ztools.hideMainWindowTypeString('typed-text');
  ztools.showMainWindow();
  ztools.setExpendHeight(480);

  ztools.copyText('txt');
  ztools.copyImage('data:image/png;base64,AAAA');
  ztools.copyFile('/tmp/a.txt');
  const copiedFiles = ztools.getCopyedFiles();

  const p = ztools.getPath('downloads');
  const appName = ztools.getAppName();
  const appVer = ztools.getAppVersion();
  const nativeId = ztools.getNativeId();
  const mac = ztools.isMacOs() && ztools.isMacOS();
  const win = ztools.isWindows();
  const lin = ztools.isLinux();
  const dark = ztools.isDarkColors();

  ztools.shellOpenExternal('https://example.com');
  ztools.shellOpenPath('/tmp');
  ztools.shellShowItemInFolder('/tmp/a.txt');
  ztools.shell.openExternal('https://shell.example.com');
  ztools.shell.openPath('/tmp/shell');
  ztools.shell.showItemInFolder('/tmp/shell.txt');
  ztools.shell.beep();
  ztools.showNotification('Done', 'feat-1');

  let capturedCb = '';
  const captured = ztools.screenCapture((img) => {{ capturedCb = img; }});
  const opened = ztools.showOpenDialog({{ title: 'Open' }});
  const saved = ztools.showSaveDialog({{ title: 'Save' }});
  ztools.simulateKeyboardTap('v', 'command', 'shift');
  ztools.simulateMouseMove(15, 25);
  ztools.simulateMouseClick(15, 25);

  const d1 = ztools.getPrimaryDisplay();
  const dAll = ztools.getAllDisplays();
  const dNear = ztools.getDisplayNearestPoint({{ x: 1, y: 2 }});
  const cursor = ztools.getCursorScreenPoint();
  const scrPt = ztools.dipToScreenPoint({{ x: 10, y: 20 }});
  const dipPt = ztools.screenToDipPoint({{ x: 20, y: 40 }});

  ztools.setFeature({{ code: 'dyn-1', cmds: ['dyn'] }});
  const feats = ztools.getFeatures();
  ztools.removeFeature('dyn-1');

  const chunks = [];
  const aiReq = ztools.ai({{ prompt: 'hi' }}, (c) => chunks.push(c.content));
  const aiRes = await aiReq;
  const abortedReq = ztools.ai({{ prompt: 'cancel' }});
  abortedReq.abort();
  let abortErr = '';
  try {{ await abortedReq; }} catch (e) {{ abortErr = e.name; }}

  const models = await ztools.allAiModels();
  const browserUrl = await ztools.readCurrentBrowserUrl();
  const folderPath = await ztools.getCurrentFolderPath();

  console.log(JSON.stringify({{
    events,
    subText,
    copiedFiles,
    p,
    appName,
    appVer,
    nativeId,
    mac,
    win,
    lin,
    dark,
    captured,
    capturedCb,
    opened,
    saved,
    d1,
    dAllLen: dAll.length,
    dNear,
    cursor,
    scrPt,
    dipPt,
    feats,
    chunks,
    aiResContent: aiRes.content,
    abortErr,
    models,
    browserUrl,
    folderPath,
    hideRes,
    typeRes,
    recordedOps: ops.map((o) => o.op),
  }}));
}})().catch((e) => {{
  console.error(e);
  process.exit(1);
}});
"#
    );

    let out = run_node_script(&test_js);
    assert_eq!(
        out["events"],
        json!([
            ["ready", "feat-1"],
            ["enter", "feat-1"],
            ["out", true],
            ["detach"],
            ["attach"]
        ])
    );
    assert_eq!(out["hideRes"], json!(true));
    assert_eq!(out["typeRes"], json!(true));
    assert_eq!(out["subText"], json!("hello sub"));
    assert_eq!(out["copiedFiles"][0]["isFile"], json!(true));
    assert_eq!(out["copiedFiles"][1]["isDiractory"], json!(true));
    assert_eq!(out["p"], json!("/tmp/downloads"));
    assert_eq!(out["appName"], json!("aitools"));
    assert_eq!(out["appVer"], json!("1.0.0"));
    assert_eq!(out["nativeId"], json!("native-xyz"));
    assert_eq!(out["mac"], json!(true));
    assert_eq!(out["win"], json!(false));
    assert_eq!(out["lin"], json!(false));
    assert_eq!(out["dark"], json!(true));
    assert_eq!(out["captured"], json!("data:image/png;base64,AAAA"));
    assert_eq!(out["capturedCb"], json!("data:image/png;base64,AAAA"));
    assert_eq!(out["opened"], json!(["/tmp/one.txt"]));
    assert_eq!(out["saved"], json!("/tmp/save.txt"));
    assert_eq!(out["scrPt"], json!({ "x": 20, "y": 40 }));
    assert_eq!(out["dipPt"], json!({ "x": 10, "y": 20 }));
    assert_eq!(out["feats"], json!([{ "code": "f1" }]));
    assert_eq!(out["chunks"], json!(["AI ", "reply"]));
    assert_eq!(out["aiResContent"], json!("AI reply"));
    assert_eq!(out["abortErr"], json!("AbortError"));
    assert_eq!(
        out["models"],
        json!([{ "id": "balanced", "label": "Balanced" }])
    );
    assert_eq!(out["browserUrl"], json!("https://example.org"));
    assert_eq!(out["folderPath"], json!("/Users/demo/Projects"));
}

#[test]
fn spec_015_r2_pure_rust_quickjs_node_compat_runs_real_ztools_without_node() {
    // Configure an intentionally invalid node_binary so ANY unintended Tier-3 spawn
    // would fail immediately — proving PureRustOnly stays strictly in QuickJsNodeCompat.
    let impossible_node_cfg = NodeRuntimeConfig {
        node_binary: Some(PathBuf::from("/nonexistent/node-binary-must-not-run")),
        startup_timeout: Duration::from_secs(2),
        call_timeout: Duration::from_secs(2),
    };

    let scratch = TempDir::new().unwrap();
    let sample_png_path = scratch.path().join("sample.png");
    // Tiny valid base64 PNG header bytes (non-empty for fs + crypto MD5 checks).
    let png_bytes = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01";
    std::fs::write(&sample_png_path, png_bytes).unwrap();
    let ascii_out_path = scratch.path().join("tree.txt");

    let mut plugins_root = TempDir::new().unwrap();
    let plugin_names = [
        "char-transform",
        "uuid-kit",
        "ascii-tree",
        "local-ip",
        "markdown",
        "memo-quick-paste",
    ];
    let has_tarball = legacy_backup_tarball();
    if let Some(ref tarball) = has_tarball {
        let mut cmd = Command::new("tar");
        cmd.arg("-xzf")
            .arg(tarball)
            .arg("--strip-components=3")
            .arg("-C")
            .arg(plugins_root.path());
        for name in &plugin_names {
            cmd.arg(format!("atools/ZTools-plugins/plugins/{name}"));
        }
        let status = cmd.status().expect("tar extract failed");
        assert!(status.success(), "tar extraction of ZTools plugins failed");
    } else {
        // Minimal self-contained mirrors when running on CI without the local backup tarball.
        plugins_root = TempDir::new().unwrap();
        let ct = plugins_root.path().join("char-transform/lib");
        std::fs::create_dir_all(&ct).unwrap();
        std::fs::write(ct.join("char-transform.cjs"), "exports.modes = () => ['camelCase', 'md5']; exports.transform = (s, m) => m === 'md5' ? require('node:crypto').createHash('md5').update(s).digest('hex') : 'helloZtoolsWorld';").unwrap();
        std::fs::write(plugins_root.path().join("char-transform/preload.js"), "const t = require('./lib/char-transform.cjs'); window.charTransformApi = t; window.exports = { 'char-transform': { mode: 'none', args: { enter() {} } } };").unwrap();

        let uk = plugins_root.path().join("uuid-kit");
        std::fs::create_dir_all(&uk).unwrap();
        std::fs::write(uk.join("preload.js"), "const { clipboard } = require('electron'); window.__nativeCopy = (s) => { clipboard.writeText(String(s)); return true; };").unwrap();

        let at = plugins_root.path().join("ascii-tree");
        std::fs::create_dir_all(&at).unwrap();
        std::fs::write(at.join("preload.js"), "const fs = require('node:fs'); window.asciiTreeServices = { writeTextFile(p, c) { fs.writeFileSync(p, c, 'utf8'); return true; } }; window.ztools.onPluginEnter(({ code, type, payload }) => window.dispatchEvent(new CustomEvent('ascii-tree:plugin-enter', { detail: { code, type, payload } })));").unwrap();

        let li = plugins_root.path().join("local-ip");
        std::fs::create_dir_all(&li).unwrap();
        std::fs::write(li.join("preload.js"), "const os = require('os'); window.exports = { ip: { mode: 'none', args: { enter() { os.networkInterfaces(); window.ztools.copyText('127.0.0.1'); setTimeout(() => window.ztools.outPlugin(true), 0); } } } };").unwrap();

        let md = plugins_root.path().join("markdown/public");
        std::fs::create_dir_all(&md).unwrap();
        std::fs::write(md.join("preload.js"), "const fs = require('fs'), path = require('path'), { fileURLToPath } = require('url'); window.ztools.readMarkdownLocalImage = async (u) => { const p = u.startsWith('file://') ? fileURLToPath(u) : u; return { success: fs.existsSync(p), fileName: path.basename(p), contentType: 'image/png', buffer: fs.readFileSync(p) }; };").unwrap();

        let mqp = plugins_root.path().join("memo-quick-paste");
        std::fs::create_dir_all(&mqp).unwrap();
        std::fs::write(mqp.join("preload.js"), "const crypto = require('crypto'), { pathToFileURL, fileURLToPath } = require('url'); window.services = { md5Format: (s) => crypto.createHash('md5').update(s).digest('hex'), pathToFileURL: (p) => pathToFileURL(p).href, getImageData: (u) => { const p = fileURLToPath(u); return { digest: crypto.createHash('md5').update(require('fs').readFileSync(p)).digest('hex'), contentType: 'image/png' }; } };").unwrap();
    }

    let mut before_hashes = BTreeMap::new();
    for name in &plugin_names {
        before_hashes.insert(*name, hash_dir_tree(&plugins_root.path().join(name)));
    }

    let mut cold_starts_ms = Vec::new();

    // 1. char-transform
    {
        let rt = PreloadRuntime::with_node_config(
            PreloadExecutionMode::PureRustOnly,
            PreloadCapabilities::all(),
            impossible_node_cfg.clone(),
            Arc::new(NoopHostBridgeHandler),
        );
        let t0 = std::time::Instant::now();
        let info = rt
            .load(
                "preload.js",
                "char-transform",
                plugins_root.path().join("char-transform"),
            )
            .unwrap();
        cold_starts_ms.push(("char-transform", t0.elapsed()));
        assert_eq!(rt.active_tier(), PreloadTier::QuickJsNodeCompat);
        assert!(info.exported_props.iter().any(|p| matches!(
            p,
            ExportedWindowProp::Object { path, .. } if path == "charTransformApi"
        )));
        let camel = rt
            .invoke_window_prop(
                "charTransformApi.transform",
                &[json!("hello ztools world"), json!("camelCase")],
            )
            .unwrap();
        assert_eq!(camel, json!("helloZtoolsWorld"));
        let md5 = rt
            .invoke_window_prop(
                "charTransformApi.transform",
                &[json!("hello"), json!("md5")],
            )
            .unwrap();
        assert_eq!(md5, json!("5d41402abc4b2a76b9719d911017c592"));
        rt.shutdown().unwrap();
    }

    // 2. uuid-kit
    {
        let rec = Arc::new(RecordingHostBridge::default());
        let rt = PreloadRuntime::with_node_config(
            PreloadExecutionMode::PureRustOnly,
            PreloadCapabilities::all(),
            impossible_node_cfg.clone(),
            Arc::clone(&rec) as Arc<dyn HostBridgeHandler>,
        );
        let t0 = std::time::Instant::now();
        rt.load(
            "preload.js",
            "uuid-kit",
            plugins_root.path().join("uuid-kit"),
        )
        .unwrap();
        cold_starts_ms.push(("uuid-kit", t0.elapsed()));
        assert_eq!(rt.active_tier(), PreloadTier::QuickJsNodeCompat);
        let copied = rt
            .invoke_window_prop("__nativeCopy", &[json!("quickjs-uuid-ok")])
            .unwrap();
        assert_eq!(copied, json!(true));
        assert_eq!(rec.snapshot_copied_texts(), vec!["quickjs-uuid-ok"]);
        rt.shutdown().unwrap();
    }

    // 3. ascii-tree
    {
        let rt = PreloadRuntime::with_node_config(
            PreloadExecutionMode::PureRustOnly,
            PreloadCapabilities::all(),
            impossible_node_cfg.clone(),
            Arc::new(NoopHostBridgeHandler),
        );
        let t0 = std::time::Instant::now();
        rt.load(
            "preload.js",
            "ascii-tree",
            plugins_root.path().join("ascii-tree"),
        )
        .unwrap();
        cold_starts_ms.push(("ascii-tree", t0.elapsed()));
        assert_eq!(rt.active_tier(), PreloadTier::QuickJsNodeCompat);
        let ok = rt
            .invoke_window_prop(
                "asciiTreeServices.writeTextFile",
                &[
                    json!(ascii_out_path.to_string_lossy()),
                    json!("├── a\n└── b\n"),
                ],
            )
            .unwrap();
        assert_eq!(ok, json!(true));
        assert_eq!(
            std::fs::read_to_string(&ascii_out_path).unwrap(),
            "├── a\n└── b\n"
        );
        rt.enter(
            "ascii-tree",
            &json!({ "code": "ascii-tree", "type": "text", "payload": "" }),
        )
        .unwrap();
        rt.shutdown().unwrap();
    }

    // 4. local-ip
    {
        let rec = Arc::new(RecordingHostBridge::default());
        let rt = PreloadRuntime::with_node_config(
            PreloadExecutionMode::PureRustOnly,
            PreloadCapabilities::all(),
            impossible_node_cfg.clone(),
            Arc::clone(&rec) as Arc<dyn HostBridgeHandler>,
        );
        let t0 = std::time::Instant::now();
        rt.load(
            "preload.js",
            "local-ip",
            plugins_root.path().join("local-ip"),
        )
        .unwrap();
        cold_starts_ms.push(("local-ip", t0.elapsed()));
        assert_eq!(rt.active_tier(), PreloadTier::QuickJsNodeCompat);
        rt.enter(
            "ip",
            &json!({ "code": "ip", "type": "text", "payload": "" }),
        )
        .unwrap();
        // Give async setTimeout(() => outPlugin(true), 0) a brief turn via shutdown/host poll.
        std::thread::sleep(Duration::from_millis(25));
        assert!(
            rec.snapshot_window_ops()
                .iter()
                .any(|(op, _)| op == "window/outPlugin"),
            "expected local-ip setTimeout outPlugin call, got: {:?}",
            rec.snapshot_window_ops()
        );
        assert!(
            !rec.snapshot_notifications().is_empty(),
            "expected local-ip showNotification call"
        );
        rt.shutdown().unwrap();
    }

    // 5. markdown
    {
        let rt = PreloadRuntime::with_node_config(
            PreloadExecutionMode::PureRustOnly,
            PreloadCapabilities::all(),
            impossible_node_cfg.clone(),
            Arc::new(NoopHostBridgeHandler),
        );
        let t0 = std::time::Instant::now();
        let info = rt
            .load(
                "public/preload.js",
                "markdown",
                plugins_root.path().join("markdown"),
            )
            .unwrap();
        cold_starts_ms.push(("markdown", t0.elapsed()));
        assert_eq!(rt.active_tier(), PreloadTier::QuickJsNodeCompat);
        assert!(
            info.has_on_plugin_enter,
            "markdown preload should register onPluginEnter"
        );
        let enter_res = rt
            .enter(
                "markdown",
                &json!({ "code": "markdown", "type": "text", "payload": "" }),
            )
            .unwrap();
        assert_eq!(enter_res["handled"], json!(true));
        rt.shutdown().unwrap();
    }

    // 6. memo-quick-paste
    {
        let rt = PreloadRuntime::with_node_config(
            PreloadExecutionMode::PureRustOnly,
            PreloadCapabilities::all(),
            impossible_node_cfg,
            Arc::new(NoopHostBridgeHandler),
        );
        let t0 = std::time::Instant::now();
        rt.load(
            "preload.js",
            "memo-quick-paste",
            plugins_root.path().join("memo-quick-paste"),
        )
        .unwrap();
        cold_starts_ms.push(("memo-quick-paste", t0.elapsed()));
        assert_eq!(rt.active_tier(), PreloadTier::QuickJsNodeCompat);
        let md5 = rt
            .invoke_window_prop("services.md5Format", &[json!("abc")])
            .unwrap();
        assert_eq!(md5, json!("900150983cd24fb0d6963f7d28e17f72"));
        let href = rt
            .invoke_window_prop(
                "services.pathToFileURL",
                &[json!(sample_png_path.to_string_lossy())],
            )
            .unwrap();
        let href_str = href.as_str().unwrap();
        assert!(href_str.starts_with("file://"));
        let img = rt
            .invoke_window_prop("services.getImageData", &[json!(href_str)])
            .unwrap();
        assert_eq!(img["contentType"], json!("image/png"));
        assert!(img["digest"].as_str().is_some_and(|s| s.len() == 32));
        rt.shutdown().unwrap();
    }

    for (name, elapsed) in &cold_starts_ms {
        eprintln!(
            "[SPEC-015 QuickJsNodeCompat cold start] {name}: {:.2} ms",
            elapsed.as_secs_f64() * 1000.0
        );
    }

    // Verify byte-for-byte immutability for every plugin directory after PureRustOnly execution.
    for name in &plugin_names {
        assert_eq!(
            before_hashes[*name],
            hash_dir_tree(&plugins_root.path().join(name)),
            "plugin '{name}' SHA-256 changed after QuickJsNodeCompat run"
        );
    }
}

#[test]
fn spec_015_r2_node_compat_gap_fallback_and_pure_rust_only_mode() {
    let dir = TempDir::new().unwrap();
    let plugin_dir = dir.path();

    // Fixture 1: native .node addon + worker_threads gap.
    std::fs::write(plugin_dir.join("native.node"), b"not-a-real-elf-or-macho").unwrap();
    std::fs::write(
        plugin_dir.join("gap_worker.js"),
        r#"
const { Worker } = require('worker_threads');
window.gapApi = { kind: typeof Worker };
"#,
    )
    .unwrap();
    std::fs::write(
        plugin_dir.join("gap_native.js"),
        r#"
require('./native.node');
"#,
    )
    .unwrap();

    // 1. Under PureRustOnly, unsupported built-in (`worker_threads`) MUST return NodeCompatGap
    // and NEVER invoke SystemNodeFallback even if node_binary points nowhere.
    let no_node_cfg = NodeRuntimeConfig {
        node_binary: Some(PathBuf::from("/nonexistent/node-must-not-spawn")),
        startup_timeout: Duration::from_secs(2),
        call_timeout: Duration::from_secs(2),
    };
    {
        let rt = PreloadRuntime::with_node_config(
            PreloadExecutionMode::PureRustOnly,
            PreloadCapabilities::all(),
            no_node_cfg.clone(),
            Arc::new(NoopHostBridgeHandler),
        );
        let err = rt
            .load("gap_worker.js", "gap-worker-plugin", plugin_dir)
            .unwrap_err();
        assert!(
            matches!(
                err,
                JsBridgeError::NodeCompatGap { ref plugin_id, ref capability }
                if plugin_id == "gap-worker-plugin" && capability.contains("worker_threads")
            ),
            "expected NodeCompatGap for worker_threads, got: {err:?}"
        );
        assert!(!rt.is_alive());
        assert_eq!(rt.active_tier(), PreloadTier::WebViewOnly);
    }

    // 2. Native `.node` addon MUST also return NodeCompatGap under PureRustOnly.
    {
        let rt = PreloadRuntime::with_node_config(
            PreloadExecutionMode::PureRustOnly,
            PreloadCapabilities::all(),
            no_node_cfg,
            Arc::new(NoopHostBridgeHandler),
        );
        let err = rt
            .load("gap_native.js", "gap-native-plugin", plugin_dir)
            .unwrap_err();
        assert!(
            matches!(
                err,
                JsBridgeError::NodeCompatGap { ref plugin_id, ref capability }
                if plugin_id == "gap-native-plugin" && capability.ends_with(".node")
            ),
            "expected NodeCompatGap for .node native addon, got: {err:?}"
        );
    }

    // 3. Under RustFirstWithFallback, hitting NodeCompatGap seamlessly transitions to SystemNodeFallback.
    if resolve_node_binary().is_ok() {
        let rt = PreloadRuntime::new(
            PreloadExecutionMode::RustFirstWithFallback,
            PreloadCapabilities::all(),
            Arc::new(NoopHostBridgeHandler),
        );
        let info = rt
            .load("gap_worker.js", "gap-worker-fallback", plugin_dir)
            .unwrap();
        assert_eq!(rt.active_tier(), PreloadTier::SystemNodeFallback);
        assert!(info.exported_props.iter().any(|p| matches!(
            p,
            ExportedWindowProp::Value { path, .. } | ExportedWindowProp::Object { path, .. } if path == "gapApi"
        )));
        let kind = rt.invoke_window_prop("gapApi.kind", &[]).unwrap();
        assert_eq!(kind, json!("function"));
        rt.shutdown().unwrap();
    }

    // 4. Capabilities matrix: restricted() denies fs/net/child_process; all() bridges http/fetch to HostBridgeHandler::http_op.
    std::fs::write(
        plugin_dir.join("caps_check.js"),
        r#"
const fs = require('fs');
const cp = require('child_process');
window.capsApi = {
  tryFs() {
    try { fs.existsSync('/'); return 'allowed'; } catch (e) { return e.code || e.message; }
  },
  tryExec() {
    try { cp.execSync('echo hi'); return 'allowed'; } catch (e) { return e.code || e.message; }
  },
  async tryFetch() {
    try {
      const res = await fetch('https://example.invalid/api');
      const data = await res.json();
      return { ok: true, status: res.status, data };
    } catch (e) {
      return { ok: false, code: e.code || '', message: e.message };
    }
  }
};
"#,
    )
    .unwrap();

    // Restricted capabilities block fs, child_process, and fetch.
    {
        let rt = PreloadRuntime::new(
            PreloadExecutionMode::PureRustOnly,
            PreloadCapabilities::restricted(),
            Arc::new(NoopHostBridgeHandler),
        );
        rt.load("caps_check.js", "caps-restricted", plugin_dir)
            .unwrap();
        assert_eq!(
            rt.invoke_window_prop("capsApi.tryFs", &[]).unwrap(),
            json!("EACCES")
        );
        assert_eq!(
            rt.invoke_window_prop("capsApi.tryExec", &[]).unwrap(),
            json!("EACCES")
        );
        let fetch_res = rt.invoke_window_prop("capsApi.tryFetch", &[]).unwrap();
        assert_eq!(fetch_res["ok"], json!(false));
        assert_eq!(fetch_res["code"], json!("EACCES"));
        rt.shutdown().unwrap();
    }

    // Allowed capabilities route fetch through HostBridgeHandler::http_op without direct OS socket bypass.
    {
        let rec = Arc::new(RecordingHostBridge::default());
        let rt = PreloadRuntime::new(
            PreloadExecutionMode::PureRustOnly,
            PreloadCapabilities::all(),
            Arc::clone(&rec) as Arc<dyn HostBridgeHandler>,
        );
        rt.load("caps_check.js", "caps-allowed", plugin_dir)
            .unwrap();
        let fetch_res = rt.invoke_window_prop("capsApi.tryFetch", &[]).unwrap();
        assert_eq!(fetch_res["ok"], json!(true));
        assert_eq!(fetch_res["status"], json!(200));
        assert_eq!(fetch_res["data"], json!({ "fromHost": true }));
        let http_ops = rec.snapshot_http_ops();
        assert_eq!(http_ops.len(), 1);
        assert_eq!(http_ops[0].0, "http/request");
        assert_eq!(http_ops[0].1["url"], json!("https://example.invalid/api"));
        rt.shutdown().unwrap();
    }
}

#[test]
fn spec_015_r2_and_r6_regressions_module_paths_64mb_heap_error_payload_nics_and_shim() {
    let dir = TempDir::new().unwrap();
    let plugin_dir = dir.path();

    // 1. Module._initPaths() + _nodeModulePaths() + globalPaths + NODE_PATH resolution (`imgbatch` pattern)
    let runtime_pkgs = plugin_dir
        .join("runtime-packages")
        .join("custom-img-helper");
    std::fs::create_dir_all(&runtime_pkgs).unwrap();
    std::fs::write(
        runtime_pkgs.join("package.json"),
        r#"{ "name": "custom-img-helper", "main": "index.js" }"#,
    )
    .unwrap();
    std::fs::write(
        runtime_pkgs.join("index.js"),
        "exports.resizeFactor = (n) => n * 2;",
    )
    .unwrap();

    // 2. Large >16 MiB heap allocation & bundle stress test (`sqlmate` / `xlsx.js` OOM fix)
    let large_filler = " ".repeat(1_500_000);
    std::fs::write(
        plugin_dir.join("large_bundle.js"),
        format!(
            "{large_filler}\nexports.allocateLarge = () => {{ const buf = Buffer.alloc(24 * 1024 * 1024, 0x41); return buf.length; }};\n"
        ),
    )
    .unwrap();

    std::fs::write(
        plugin_dir.join("preload.js"),
        r#"
const Module = require('module');
const path = require('path');
const os = require('os');

const runtimePackagesPath = path.join(__dirname, 'runtime-packages');
const sep = path.delimiter;
process.env.NODE_PATH = runtimePackagesPath + (process.env.NODE_PATH ? sep + process.env.NODE_PATH : '');
Module._initPaths();
const helper = require('custom-img-helper');
const largeMod = require('./large_bundle.js');

// `port-use` pattern: ztools is frozen, ztools.shell exists with openExternal/openPath/showItemInFolder/beep
// and remains extensible so preload can attach `ztools.shell.exec`.
window.ztools = window.ztools || {};
window.ztools.shell = window.ztools.shell || {};
window.ztools.shell.exec = (cmd) => 'exec:' + cmd;

let attachFired = false;
window.utools.onPluginAttach(() => { attachFired = true; });

window.regressionApi = {
  checkModule() {
    return {
      hasInitPaths: typeof Module._initPaths === 'function',
      globalPaths: Module.globalPaths,
      nodeModulePaths: Module._nodeModulePaths(__dirname),
      helperResult: helper.resizeFactor(21),
    };
  },
  checkLargeHeap() {
    return largeMod.allocateLarge();
  },
  throwTypedError() {
    throw new TypeError('missing required option xyz');
  },
  checkNetworkInterfaces() {
    const nets = os.networkInterfaces();
    const names = Object.keys(nets);
    let totalEntries = 0;
    let firstValid = false;
    for (const name of names) {
      for (const entry of nets[name]) {
        totalEntries++;
        if (entry.address && entry.netmask && entry.family && entry.cidr) {
          firstValid = true;
        }
      }
    }
    return { names, totalEntries, firstValid };
  },
  async checkShimApis() {
    const frozenTop = Object.isFrozen(window.ztools);
    const sameShell = window.ztools.shell === window.utools.shell && window.ztools.shell === window.aitools.shell;
    const shellTypes = [
      typeof window.ztools.shell.openExternal,
      typeof window.ztools.shell.openPath,
      typeof window.ztools.shell.showItemInFolder,
      typeof window.ztools.shell.beep,
      typeof window.ztools.shell.exec,
    ];
    let thenCalled = false;
    const hidePromise = window.ztools.hideMainWindow(true);
    const isThenable = Boolean(hidePromise && typeof hidePromise.then === 'function');
    await hidePromise.then(() => { thenCalled = true; });
    const typedOk = window.ztools.hideMainWindowTypeString('hello-random-data');
    window.ztools.shell.openExternal('https://example.org/shell');
    window.ztools.shell.openPath('/tmp/demo');
    window.ztools.shell.showItemInFolder('/tmp/demo/file.txt');
    window.ztools.shell.beep();
    globalThis.__aitools_dispatch_lifecycle__('pluginAttach', {});
    return {
      frozenTop,
      sameShell,
      shellTypes,
      isThenable,
      thenCalled,
      typedOk,
      execRes: window.ztools.shell.exec('lsof -i'),
      attachFired,
    };
  },
};
"#,
    )
    .unwrap();

    let rec = Arc::new(RecordingHostBridge::default());
    let rt = PreloadRuntime::with_node_config(
        PreloadExecutionMode::PureRustOnly,
        PreloadCapabilities::all(),
        NodeRuntimeConfig {
            node_binary: Some(PathBuf::from("/nonexistent/node-must-not-spawn")),
            startup_timeout: Duration::from_secs(2),
            call_timeout: Duration::from_secs(5),
        },
        Arc::clone(&rec) as Arc<dyn HostBridgeHandler>,
    );
    rt.load("preload.js", "regression-plugin", plugin_dir)
        .unwrap();
    assert_eq!(rt.active_tier(), PreloadTier::QuickJsNodeCompat);

    // 1. Module._initPaths / _nodeModulePaths / globalPaths / NODE_PATH resolve
    let mod_res = rt
        .invoke_window_prop("regressionApi.checkModule", &[])
        .unwrap();
    assert_eq!(mod_res["hasInitPaths"], json!(true));
    assert_eq!(mod_res["helperResult"], json!(42));
    assert!(mod_res["globalPaths"]
        .as_array()
        .is_some_and(|a| !a.is_empty()));
    assert!(mod_res["nodeModulePaths"].as_array().is_some_and(|a| a
        .iter()
        .any(|p| p.as_str().is_some_and(|s| s.ends_with("node_modules")))));

    // 2. >16 MiB QuickJS heap allocation succeeds cleanly (64 MiB ceiling)
    let heap_len = rt
        .invoke_window_prop("regressionApi.checkLargeHeap", &[])
        .unwrap();
    assert_eq!(heap_len, json!(24 * 1024 * 1024));

    // 3. errorPayload preserves err.name AND err.message alongside stack
    let err = rt
        .invoke_window_prop("regressionApi.throwTypedError", &[])
        .unwrap_err();
    let err_str = err.to_string();
    assert!(
        err_str.contains("TypeError") && err_str.contains("missing required option xyz"),
        "expected TypeError + message in error string, got: {err_str}"
    );

    // 4. os.networkInterfaces() returns valid interface structures
    let net_res = rt
        .invoke_window_prop("regressionApi.checkNetworkInterfaces", &[])
        .unwrap();
    assert!(net_res["totalEntries"].as_u64().unwrap_or(0) >= 1);
    assert_eq!(net_res["firstValid"], json!(true));

    // 5. hideMainWindow().then, hideMainWindowTypeString, onPluginAttach, and ztools.shell
    let shim_res = rt
        .invoke_window_prop("regressionApi.checkShimApis", &[])
        .unwrap();
    assert_eq!(shim_res["frozenTop"], json!(true));
    assert_eq!(shim_res["sameShell"], json!(true));
    assert_eq!(
        shim_res["shellTypes"],
        json!(["function", "function", "function", "function", "function"])
    );
    assert_eq!(shim_res["isThenable"], json!(true));
    assert_eq!(shim_res["thenCalled"], json!(true));
    assert_eq!(shim_res["typedOk"], json!(true));
    assert_eq!(shim_res["execRes"], json!("exec:lsof -i"));
    assert_eq!(shim_res["attachFired"], json!(true));

    let win_ops = rec.snapshot_window_ops();
    assert!(win_ops.iter().any(|(op, _)| op == "window/hideMainWindow"));
    assert!(win_ops.iter().any(|(op, p)| {
        op == "window/hideMainWindowTypeString" && p["text"] == json!("hello-random-data")
    }));
    let shell_ops = rec.snapshot_shell_ops();
    assert!(shell_ops.iter().any(|(op, _)| op == "shell/openExternal"));
    assert!(shell_ops.iter().any(|(op, _)| op == "shell/openPath"));
    assert!(shell_ops
        .iter()
        .any(|(op, _)| op == "shell/showItemInFolder"));
    assert!(shell_ops.iter().any(|(op, _)| op == "shell/beep"));

    rt.shutdown().unwrap();
}
