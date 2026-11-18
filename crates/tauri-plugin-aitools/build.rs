const COMMANDS: &[&str] = &[
    "search_features",
    "zero_query_suggestions",
    "launch_item",
    "search_list_mode",
    "select_list_item",
    "sync_ztools_dir",
    "submit_agent_prompt",
    "run_agent_dag",
    "resolve_hitl_step",
    "invoke_legacy_list_feature",
    "dsh_recent_logs",
    "dsh_ingest_frontend_log",
    "dsh_set_fault_injection",
    "dsh_preload_status",
    "db_get",
    "db_put",
    "db_remove",
    "db_all_docs",
    "ai_chat",
    "ai_embed",
    "telemetry_log",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).build();
}
