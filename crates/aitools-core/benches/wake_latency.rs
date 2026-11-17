use std::time::Instant;

use aitools_core::{Cmd, Document, FeatureEntry, FeatureSearchIndex, StorageEngine};
use serde_json::json;
use tempfile::TempDir;

fn main() {
    let mut features = Vec::with_capacity(1200);
    for i in 0..1200 {
        features.push(FeatureEntry {
            code: format!("feat-{i}"),
            plugin_id: format!("plugin-{}", i % 40),
            plugin_name: format!("Plugin {}", i % 40),
            label: format!("工具命令 {i} Tool"),
            icon: None,
            explain: "Benchmark feature".into(),
            cmds: vec![Cmd::Text(format!("剪贴板工具{i}"))],
            main_push: false,
            priority: i,
        });
    }

    let index = FeatureSearchIndex::build(&features);
    let mut search_us = Vec::with_capacity(200);
    for q in ["jtb", "gjml", "tool", "剪贴板", "feat-999"]
        .iter()
        .cycle()
        .take(200)
    {
        let start = Instant::now();
        let _ = index.search_checked(q).unwrap();
        search_us.push(start.elapsed().as_micros());
    }
    search_us.sort_unstable();
    let search_p50 = search_us[search_us.len() / 2];
    let search_p99 = search_us[(search_us.len() * 99) / 100];

    let dir = TempDir::new().unwrap();
    let db = StorageEngine::open(dir.path()).unwrap();
    db.put_doc(
        "bench-plugin",
        Document {
            id: "doc-1".into(),
            rev: None,
            data: json!({ "hello": "world" }),
        },
    )
    .unwrap();

    let mut lmdb_us = Vec::with_capacity(200);
    for _ in 0..200 {
        let start = Instant::now();
        let _ = db.get_doc("bench-plugin", "doc-1").unwrap();
        lmdb_us.push(start.elapsed().as_micros());
    }
    lmdb_us.sort_unstable();
    let lmdb_p50 = lmdb_us[lmdb_us.len() / 2];
    let lmdb_p99 = lmdb_us[(lmdb_us.len() * 99) / 100];

    println!("1200-feature search: P50={search_p50}us, P99={search_p99}us (budget: <= 5000us)");
    println!("LMDB get_doc:        P50={lmdb_p50}us, P99={lmdb_p99}us (SPEC-000 P50 <= 300us, P99 <= 5000us)");
}
