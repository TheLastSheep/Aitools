use aitools_core::{
    load_plugin_dir, sanitize_plugin_id, scan_plugins_dir, Cmd, CmdTyped, FeatureEntry,
    FeatureSearchIndex, MediaFile, PluginManifest, PluginRuntimeTier,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

#[derive(Deserialize)]
struct TypedTextCasesFile {
    cases: Vec<TypedTextCase>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TypedTextCase {
    name: String,
    text: String,
    command: CmdTyped,
    ordinary: bool,
    fixed_text: bool,
}

#[derive(Deserialize)]
struct MediaCasesFile {
    cases: Vec<MediaCase>,
}

#[derive(Deserialize)]
struct MediaCase {
    name: String,
    files: Vec<MediaFile>,
    command: CmdTyped,
    expected: bool,
}

fn feature_with_cmd(cmd: CmdTyped) -> FeatureEntry {
    FeatureEntry {
        code: "target".to_string(),
        plugin_id: "parity-plugin".to_string(),
        plugin_name: "Parity Plugin".to_string(),
        label: "Target".to_string(),
        icon: None,
        explain: "Parity test feature".to_string(),
        cmds: vec![Cmd::Typed(cmd)],
        main_push: false,
        priority: 1,
    }
}

#[test]
fn spec_005_r3_ztools_typed_text_contract_parity() {
    let raw = include_str!("fixtures/launcher-typed-text-contract-cases.json");
    let fixture: TypedTextCasesFile = serde_json::from_str(raw).expect("valid typed text fixture");

    for case in fixture.cases {
        let index = FeatureSearchIndex::build(&[feature_with_cmd(case.command)]);
        let ordinary_matched = !index.search(&case.text).is_empty();
        let fixed_matched = !index
            .search_text_content(&case.text)
            .expect("text content search")
            .is_empty();

        assert_eq!(
            ordinary_matched, case.ordinary,
            "ordinary mismatch on case '{}'",
            case.name
        );
        assert_eq!(
            fixed_matched, case.fixed_text,
            "fixedText mismatch on case '{}'",
            case.name
        );
    }
}

#[test]
fn spec_005_r4_ztools_media_contract_parity() {
    let raw = include_str!("fixtures/launcher-media-contract-cases.json");
    let fixture: MediaCasesFile = serde_json::from_str(raw).expect("valid media fixture");

    for case in fixture.cases {
        let index = FeatureSearchIndex::build(&[feature_with_cmd(case.command)]);
        let matched = !index
            .search_file_content(&case.files)
            .expect("file content search")
            .is_empty();
        assert_eq!(
            matched, case.expected,
            "media mismatch on case '{}'",
            case.name
        );
    }
}

#[test]
fn spec_003_r1_ztools_manifest_quirks_parse_cleanly() {
    let json = r#"{
        "name": "Quirky Plugin",
        "version": "1.2.3",
        "main": "index.html",
        "pluginSetting": { "single": "ture", "height": 600 },
        "features": [
            {
                "code": "window-match",
                "explain": "Matches Finder",
                "mainPush": "ture",
                "mainHide": "0",
                "cmds": [
                    { "type": "window", "match": { "app": ["Finder.app"] } },
                    { "type": "over", "minLength": "3", "maxLength": 20 },
                    { "type": "regex", "regex": "/^abc$/i" }
                ]
            }
        ]
    }"#;

    let manifest: PluginManifest = serde_json::from_str(json).expect("quirky manifest parses");
    assert!(manifest.plugin_setting.as_ref().unwrap().single);
    assert!(!manifest.requires_quickjs_preload());
    let feature = &manifest.features[0];
    assert!(feature.main_push);
    assert!(!feature.main_hide);
    assert_eq!(feature.cmds.len(), 3);
}

#[test]
fn spec_015_r1_loads_unmodified_ztools_plugin_dir() {
    let dir = TempDir::new().unwrap();
    let plugin_a = dir.path().join("alpha-ztools");
    std::fs::create_dir_all(&plugin_a).unwrap();

    // UTF-8 BOM + pluginName + string platform + minNum/maxNum + relative PNG icon + http main
    let mut manifest_bytes = vec![0xEF, 0xBB, 0xBF];
    manifest_bytes.extend_from_slice(
        br#"{
            "name": "AlphaZTools",
            "pluginName": "\u963f\u5c14\u6cd5\u5de5\u5177",
            "main": "https://tools.example.com/app",
            "logo": "logo.png",
            "preload": "./preload.js",
            "platform": "darwin",
            "height": 620,
            "features": [
                {
                    "code": "batch-files",
                    "explain": "Batch process files",
                    "cmds": [
                        "alpha",
                        { "type": "files", "label": "Rename files", "fileType": "file", "minNum": 1, "maxNum": 20 }
                    ]
                }
            ]
        }"#,
    );
    std::fs::write(plugin_a.join("plugin.json"), &manifest_bytes).unwrap();
    std::fs::write(
        plugin_a.join("logo.png"),
        [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A],
    )
    .unwrap();
    std::fs::write(
        plugin_a.join("preload.js"),
        "window.exports = { 'batch-files': { mode: 'none', args: { enter() {} } } };",
    )
    .unwrap();

    // Non-ASCII plugin names must not collide onto "unnamed-plugin".
    let id_zh_1 = sanitize_plugin_id("待办清单");
    let id_zh_2 = sanitize_plugin_id("翻译助手");
    assert_ne!(id_zh_1, "unnamed-plugin");
    assert_ne!(id_zh_1, id_zh_2);

    // Record SHA-256 of every file before scanning.
    let hash_file =
        |p: &std::path::Path| format!("{:x}", Sha256::digest(std::fs::read(p).unwrap()));
    let before = [
        hash_file(&plugin_a.join("plugin.json")),
        hash_file(&plugin_a.join("logo.png")),
        hash_file(&plugin_a.join("preload.js")),
    ];

    let loaded = load_plugin_dir(&plugin_a).unwrap();
    assert_eq!(loaded.id, "alphaztools");
    assert_eq!(loaded.name, "阿尔法工具");
    assert_eq!(loaded.tier, PluginRuntimeTier::LegacyCompat);
    assert_eq!(
        loaded.main.as_deref(),
        Some("https://tools.example.com/app")
    );
    assert_eq!(loaded.window_height, 620);
    assert!(loaded
        .icon
        .as_deref()
        .is_some_and(|i| i.starts_with("data:image/png;base64,")));

    let entries = loaded.to_feature_entries_for_platform(Some("darwin"));
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].plugin_name, "阿尔法工具");
    let index = FeatureSearchIndex::build(&entries);
    assert_eq!(index.search("alpha").len(), 1);

    let scanned = scan_plugins_dir(dir.path()).unwrap();
    assert_eq!(scanned.len(), 1);

    // INV-ZTOOLS-IMMUTABLE: zero file mutations.
    let after = [
        hash_file(&plugin_a.join("plugin.json")),
        hash_file(&plugin_a.join("logo.png")),
        hash_file(&plugin_a.join("preload.js")),
    ];
    assert_eq!(before, after);

    // If sibling ZTools-plugins repository exists on this host, verify all manifests parse read-only.
    let ztools_repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../ZTools-plugins/plugins");
    if ztools_repo.is_dir() {
        let real_plugins = scan_plugins_dir(&ztools_repo).unwrap();
        assert!(
            real_plugins.len() >= 100,
            "expected >= 100 unmodified ZTools plugins to load cleanly, got {}",
            real_plugins.len()
        );
    }
}
