//! Inbound `mcp_servers.json` client host configuration & registration (`SPEC-009-R2`, `SPEC-013-R1`).

use std::collections::BTreeMap;
use std::path::Path;

use aitools_core::RiskTier;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::registry::ToolRegistry;
use crate::Result;

/// Inbound MCP server transport configuration (`stdio` or `http`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum McpServerConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    Http {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

/// Root configuration structure parsed from `mcp_servers.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpServersFile {
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
}

impl McpServersFile {
    /// Parses `<app_data_dir>/mcp_servers.json` if present, returning an empty config if absent.
    ///
    /// # Errors
    ///
    /// Returns [`crate::AiError::Core`] wrapping I/O or JSON deserialization failures if reading or
    /// parsing the configuration file fails.
    pub fn load_or_empty(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.is_file() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path).map_err(aitools_core::AitoolsError::Io)?;
        let parsed: Self = serde_json::from_str(&raw).map_err(aitools_core::AitoolsError::Json)?;
        Ok(parsed)
    }
}

/// Discovered external MCP tool descriptor ready to register into [`ToolRegistry`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredMcpTool {
    pub server_name: String,
    pub tool_name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Registers discovered external MCP tools with default `Tier2DestructiveOrExternal` safety (`SPEC-013-R1`).
pub fn register_mcp_tools(
    registry: &mut ToolRegistry,
    discovered: &[DiscoveredMcpTool],
    tier_override: Option<RiskTier>,
) {
    for item in discovered {
        registry.register_mcp_tool(
            &item.server_name,
            &item.tool_name,
            &item.description,
            item.input_schema.clone(),
            tier_override,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn spec_013_r1_parses_mcp_servers_json_and_registers_tier2_tools() {
        let dir = TempDir::new().unwrap();
        let cfg_path = dir.path().join("mcp_servers.json");
        std::fs::write(
            &cfg_path,
            r#"{
                "mcpServers": {
                    "filesystem": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem"] },
                    "remote": { "url": "http://127.0.0.1:8080/mcp" }
                }
            }"#,
        )
        .unwrap();

        let parsed = McpServersFile::load_or_empty(&cfg_path).unwrap();
        assert_eq!(parsed.mcp_servers.len(), 2);

        let mut registry = ToolRegistry::new();
        register_mcp_tools(
            &mut registry,
            &[DiscoveredMcpTool {
                server_name: "filesystem".into(),
                tool_name: "write_file".into(),
                description: "Write file".into(),
                input_schema: json!({}),
            }],
            None,
        );
        assert_eq!(
            registry.get("mcp.filesystem.write_file").unwrap().risk_tier,
            RiskTier::Tier2DestructiveOrExternal
        );
    }
}
