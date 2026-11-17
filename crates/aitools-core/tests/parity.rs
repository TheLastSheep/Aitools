use aitools_core::{Cmd, CmdTyped, FeatureEntry, FeatureSearchIndex, MediaFile, PluginManifest};
use serde::Deserialize;

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
