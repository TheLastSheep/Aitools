//! Embedded LMDB (`heed`) storage engine & synchronous plugin sync contract (`SPEC-002`).

use std::collections::HashSet;
use std::path::Path;

use heed::types::{Bytes, Str};
use heed::{Database as HeedDb, Env, EnvOpenOptions};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::info_span;

use crate::models::{Document, PluginRecord};
use crate::{AitoolsError, Result};

pub const MAX_DOCUMENT_BYTES: usize = 256 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 512 * 1024;
pub const MAX_QUERY_DOCUMENTS: usize = 1000;
pub const MAX_KEYS_SELECTOR: usize = 128;

const DEFAULT_MAP_SIZE: usize = 256 * 1024 * 1024; // 256 MiB LMDB virtual map

/// Query selector for `all_docs`.
#[derive(Debug, Clone)]
pub enum PluginDataQuery {
    All,
    Prefix(String),
    Keys(Vec<String>),
}

/// Remove selector for `remove_doc`.
#[derive(Debug, Clone)]
pub enum PluginDataRemove {
    Id(String),
    Document(Document),
}

/// PouchDB-compatible write outcome (`SPEC-002-R1`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginDataWriteResult {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl PluginDataWriteResult {
    fn success(id: &str, rev: Option<String>) -> Self {
        Self {
            id: id.to_string(),
            rev,
            ok: Some(true),
            error: None,
            name: None,
            message: None,
        }
    }

    fn failure(id: &str, name: &str, message: &str) -> Self {
        Self {
            id: id.to_string(),
            rev: None,
            ok: None,
            error: Some(true),
            name: Some(name.to_string()),
            message: Some(message.to_string()),
        }
    }
}

/// Metadata + payload for a plugin attachment (`SPEC-002-R3`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttachmentRecord {
    pub id: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

/// Short-term agent session scratchpad entry (`SPEC-011-R3`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionScratchpad {
    pub session_id: String,
    pub summary: String,
    pub artifacts: Vec<String>,
    pub context_snapshot: serde_json::Value,
    pub updated_at: String,
}

/// Deterministic step inside a saved Workflow Macro (`SPEC-011-R3`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MacroStep {
    pub tool_name: String,
    pub arguments: serde_json::Value,
}

/// User-saved Workflow Macro exportable as `SKILL.md` or Pinyin-searchable command (`SPEC-011-R3`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowMacro {
    pub id: String,
    pub name: String,
    pub description: String,
    pub triggers: Vec<String>,
    pub steps: Vec<MacroStep>,
    pub created_at: String,
}

impl WorkflowMacro {
    /// Renders this macro into a `SKILL.md` document for `aitools-ai` (`SPEC-011-R3`).
    pub fn to_skill_markdown(&self) -> String {
        let mut out = format!(
            "---\nname: {}\ndescription: {}\n---\n\n# {}\n\n{}\n\n## Steps\n",
            self.id, self.description, self.name, self.description
        );
        for (idx, step) in self.steps.iter().enumerate() {
            out.push_str(&format!(
                "{}. `{}` with `{}`\n",
                idx + 1,
                step.tool_name,
                step.arguments
            ));
        }
        out
    }

    /// Converts this macro into a Pinyin-searchable [`crate::models::FeatureEntry`].
    pub fn to_feature_entry(&self) -> crate::models::FeatureEntry {
        crate::models::FeatureEntry {
            code: format!("macro:{}", self.id),
            plugin_id: "aitools-macros".to_string(),
            plugin_name: "Saved Macros".to_string(),
            label: self.name.clone(),
            icon: None,
            explain: self.description.clone(),
            cmds: self
                .triggers
                .iter()
                .cloned()
                .map(crate::models::Cmd::Text)
                .collect(),
            main_push: false,
            priority: 50,
        }
    }
}

/// Validates that a custom protocol caller (`aitools-sync://`, `aitools-plugin://`)
/// belongs strictly to `plugin-<target_plugin_id>` (`INV-SEC-01`, `SPEC-002-R4`).
///
/// # Errors
///
/// Returns [`AitoolsError::Forbidden`] if `target_plugin_id` is empty or `webview_label`
/// does not match `plugin-<target_plugin_id>`.
pub fn validate_sync_caller(webview_label: &str, target_plugin_id: &str) -> Result<()> {
    let expected = format!("plugin-{target_plugin_id}");
    if target_plugin_id.is_empty() || webview_label != expected {
        return Err(AitoolsError::Forbidden(format!(
            "webview '{webview_label}' is not authorized to access plugin '{target_plugin_id}'"
        )));
    }
    Ok(())
}

/// Embedded LMDB storage engine (`SPEC-002`, `SPEC-011-R3`).
pub struct StorageEngine {
    env: Env,
    docs: HeedDb<Str, Bytes>,
    attachments: HeedDb<Str, Bytes>,
    plugins: HeedDb<Str, Bytes>,
    settings: HeedDb<Str, Bytes>,
    agent_memory: HeedDb<Str, Bytes>,
}

impl StorageEngine {
    /// Opens (or initializes) an LMDB environment at `dir`.
    ///
    /// # Errors
    ///
    /// Returns [`AitoolsError::Io`] if the directory cannot be created or [`AitoolsError::Lmdb`]
    /// if the LMDB environment or databases fail to open.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)?;
        // SAFETY: `EnvOpenOptions::open` is unsafe in heed because multiple independent processes
        // opening the same LMDB file with different options or uncoordinated locks is UB. Within
        // `aitools`, `StorageEngine` owns this directory exclusively for the process lifetime.
        let env = unsafe {
            EnvOpenOptions::new()
                .map_size(DEFAULT_MAP_SIZE)
                .max_dbs(8)
                .open(dir)?
        };

        let mut wtxn = env.write_txn()?;
        let docs = env.create_database(&mut wtxn, Some("plugin_docs"))?;
        let attachments = env.create_database(&mut wtxn, Some("plugin_attachments"))?;
        let plugins = env.create_database(&mut wtxn, Some("plugins"))?;
        let settings = env.create_database(&mut wtxn, Some("settings"))?;
        let agent_memory = env.create_database(&mut wtxn, Some("agent_memory"))?;
        wtxn.commit()?;

        Ok(Self {
            env,
            docs,
            attachments,
            plugins,
            settings,
            agent_memory,
        })
    }

    /// Inserts or updates a plugin document using optimistic revision matching.
    ///
    /// # Errors
    ///
    /// Returns [`AitoolsError::Storage`] for invalid IDs, [`AitoolsError::BudgetExceeded`] if
    /// document or response size limits are exceeded, [`AitoolsError::Json`] on serialization
    /// errors, or [`AitoolsError::Lmdb`] on LMDB failures.
    pub fn put_doc(&self, plugin_id: &str, doc: Document) -> Result<PluginDataWriteResult> {
        let _span = info_span!("db_sync", plugin_id = %plugin_id, op = "put").entered();
        validate_id(plugin_id)?;
        validate_document(&doc)?;

        let key = composite_key(plugin_id, &doc.id);
        let mut wtxn = self.env.write_txn()?;
        let existing = self.read_doc_tx(&wtxn, &key)?;

        let result = match existing {
            Some(curr) => {
                if doc.rev.as_deref() != curr.rev.as_deref() {
                    return Ok(PluginDataWriteResult::failure(
                        &doc.id,
                        "conflict",
                        "Document update conflict",
                    ));
                }
                let next_rev = next_revision(curr.rev.as_deref(), &doc);
                let mut stored = doc.clone();
                stored.rev = Some(next_rev.clone());
                let bytes = serde_json::to_vec(&stored)?;
                validate_doc_bytes(bytes.len())?;
                self.docs.put(&mut wtxn, &key, &bytes)?;
                PluginDataWriteResult::success(&doc.id, Some(next_rev))
            }
            None => {
                if doc.rev.is_some() {
                    return Ok(PluginDataWriteResult::failure(
                        &doc.id,
                        "conflict",
                        "Document update conflict on missing document",
                    ));
                }
                let next_rev = next_revision(None, &doc);
                let mut stored = doc.clone();
                stored.rev = Some(next_rev.clone());
                let bytes = serde_json::to_vec(&stored)?;
                validate_doc_bytes(bytes.len())?;
                self.docs.put(&mut wtxn, &key, &bytes)?;
                PluginDataWriteResult::success(&doc.id, Some(next_rev))
            }
        };

        validate_response_budget(&result)?;
        wtxn.commit()?;
        Ok(result)
    }

    /// Fetches a plugin document by ID.
    ///
    /// # Errors
    ///
    /// Returns [`AitoolsError::Storage`] for invalid IDs, [`AitoolsError::Json`] if stored data
    /// cannot be deserialized, or [`AitoolsError::Lmdb`] on LMDB failures.
    pub fn get_doc(&self, plugin_id: &str, doc_id: &str) -> Result<Option<Document>> {
        let _span = info_span!("db_sync", plugin_id = %plugin_id, op = "get").entered();
        validate_id(plugin_id)?;
        validate_id(doc_id)?;

        let rtxn = self.env.read_txn()?;
        let key = composite_key(plugin_id, doc_id);
        self.read_doc_tx(&rtxn, &key)
    }

    /// Removes a plugin document by ID or revision-checked document payload.
    ///
    /// # Errors
    ///
    /// Returns [`AitoolsError::Storage`] for invalid IDs, [`AitoolsError::BudgetExceeded`] if the
    /// response exceeds budget, [`AitoolsError::Json`] on serialization errors, or
    /// [`AitoolsError::Lmdb`] on LMDB failures.
    pub fn remove_doc(
        &self,
        plugin_id: &str,
        target: PluginDataRemove,
    ) -> Result<PluginDataWriteResult> {
        let _span = info_span!("db_sync", plugin_id = %plugin_id, op = "remove").entered();
        validate_id(plugin_id)?;

        let (doc_id, expected_rev) = match target {
            PluginDataRemove::Id(id) => (id, None),
            PluginDataRemove::Document(doc) => (doc.id, doc.rev),
        };
        validate_id(&doc_id)?;

        let key = composite_key(plugin_id, &doc_id);
        let mut wtxn = self.env.write_txn()?;
        let Some(curr) = self.read_doc_tx(&wtxn, &key)? else {
            return Ok(PluginDataWriteResult::failure(
                &doc_id,
                "not_found",
                "missing",
            ));
        };

        if let Some(expected) = expected_rev {
            if curr.rev.as_deref() != Some(expected.as_str()) {
                return Ok(PluginDataWriteResult::failure(
                    &doc_id,
                    "conflict",
                    "Document update conflict",
                ));
            }
        }

        self.docs.delete(&mut wtxn, &key)?;
        let res = PluginDataWriteResult::success(&doc_id, curr.rev);
        validate_response_budget(&res)?;
        wtxn.commit()?;
        Ok(res)
    }

    /// Atomically applies a batch of document writes for `plugin_id`.
    ///
    /// # Errors
    ///
    /// Returns [`AitoolsError::Storage`] for invalid or duplicate IDs,
    /// [`AitoolsError::BudgetExceeded`] if batch or byte limits are exceeded,
    /// [`AitoolsError::Json`] on serialization errors, or [`AitoolsError::Lmdb`] on LMDB failures.
    pub fn bulk_docs(
        &self,
        plugin_id: &str,
        docs: Vec<Document>,
    ) -> Result<Vec<PluginDataWriteResult>> {
        let _span = info_span!("db_sync", plugin_id = %plugin_id, op = "bulk_docs").entered();
        validate_id(plugin_id)?;
        if docs.len() > MAX_QUERY_DOCUMENTS {
            return Err(AitoolsError::BudgetExceeded(format!(
                "bulkDocs exceeds {MAX_QUERY_DOCUMENTS} document limit"
            )));
        }

        // SPEC-002-R2: reject duplicate IDs before committing anything.
        let mut seen = HashSet::with_capacity(docs.len());
        for doc in &docs {
            validate_document(doc)?;
            if !seen.insert(doc.id.as_str()) {
                return Err(AitoolsError::Storage(format!(
                    "duplicate document id '{}' in bulkDocs",
                    doc.id
                )));
            }
        }

        let mut wtxn = self.env.write_txn()?;
        let mut results = Vec::with_capacity(docs.len());

        for doc in docs {
            let key = composite_key(plugin_id, &doc.id);
            let existing = self.read_doc_tx(&wtxn, &key)?;
            match existing {
                Some(curr) => {
                    if doc.rev.as_deref() != curr.rev.as_deref() {
                        results.push(PluginDataWriteResult::failure(
                            &doc.id,
                            "conflict",
                            "Document update conflict",
                        ));
                        continue;
                    }
                    let next_rev = next_revision(curr.rev.as_deref(), &doc);
                    let mut stored = doc.clone();
                    stored.rev = Some(next_rev.clone());
                    let bytes = serde_json::to_vec(&stored)?;
                    validate_doc_bytes(bytes.len())?;
                    self.docs.put(&mut wtxn, &key, &bytes)?;
                    results.push(PluginDataWriteResult::success(&doc.id, Some(next_rev)));
                }
                None => {
                    if doc.rev.is_some() {
                        results.push(PluginDataWriteResult::failure(
                            &doc.id,
                            "conflict",
                            "Document update conflict on missing document",
                        ));
                        continue;
                    }
                    let next_rev = next_revision(None, &doc);
                    let mut stored = doc.clone();
                    stored.rev = Some(next_rev.clone());
                    let bytes = serde_json::to_vec(&stored)?;
                    validate_doc_bytes(bytes.len())?;
                    self.docs.put(&mut wtxn, &key, &bytes)?;
                    results.push(PluginDataWriteResult::success(&doc.id, Some(next_rev)));
                }
            }
        }

        validate_response_budget(&results)?;
        wtxn.commit()?;
        Ok(results)
    }

    /// Queries plugin documents by key list, prefix, or full scan.
    ///
    /// # Errors
    ///
    /// Returns [`AitoolsError::Storage`] for invalid IDs, [`AitoolsError::BudgetExceeded`] if
    /// selector or response budgets are exceeded, [`AitoolsError::Json`] on deserialization
    /// errors, or [`AitoolsError::Lmdb`] on LMDB failures.
    pub fn all_docs(&self, plugin_id: &str, query: PluginDataQuery) -> Result<Vec<Document>> {
        let _span = info_span!("db_sync", plugin_id = %plugin_id, op = "all_docs").entered();
        validate_id(plugin_id)?;

        let rtxn = self.env.read_txn()?;
        let mut out = Vec::new();

        match query {
            PluginDataQuery::Keys(keys) => {
                if keys.len() > MAX_KEYS_SELECTOR {
                    return Err(AitoolsError::BudgetExceeded(format!(
                        "allDocs keys selector exceeds {MAX_KEYS_SELECTOR} limit"
                    )));
                }
                for id in keys {
                    validate_id(&id)?;
                    let key = composite_key(plugin_id, &id);
                    if let Some(doc) = self.read_doc_tx(&rtxn, &key)? {
                        out.push(doc);
                    }
                }
            }
            PluginDataQuery::All => {
                let prefix = format!("{plugin_id}\0");
                for item in self.docs.prefix_iter(&rtxn, &prefix)? {
                    let (_, bytes) = item?;
                    if out.len() >= MAX_QUERY_DOCUMENTS {
                        return Err(AitoolsError::BudgetExceeded(format!(
                            "allDocs response exceeds {MAX_QUERY_DOCUMENTS} documents"
                        )));
                    }
                    out.push(serde_json::from_slice(bytes)?);
                }
            }
            PluginDataQuery::Prefix(doc_prefix) => {
                let prefix = format!("{plugin_id}\0{doc_prefix}");
                for item in self.docs.prefix_iter(&rtxn, &prefix)? {
                    let (_, bytes) = item?;
                    if out.len() >= MAX_QUERY_DOCUMENTS {
                        return Err(AitoolsError::BudgetExceeded(format!(
                            "allDocs response exceeds {MAX_QUERY_DOCUMENTS} documents"
                        )));
                    }
                    out.push(serde_json::from_slice(bytes)?);
                }
            }
        }

        validate_response_budget(&out)?;
        Ok(out)
    }

    /// Stores a binary attachment for `plugin_id`.
    ///
    /// # Errors
    ///
    /// Returns [`AitoolsError::Storage`] for invalid IDs, [`AitoolsError::BudgetExceeded`] if
    /// the payload exceeds [`MAX_DOCUMENT_BYTES`], [`AitoolsError::Json`] on serialization errors,
    /// or [`AitoolsError::Lmdb`] on LMDB failures.
    pub fn put_attachment(&self, plugin_id: &str, attachment: AttachmentRecord) -> Result<()> {
        let _span = info_span!("db_sync", plugin_id = %plugin_id, op = "put_attachment").entered();
        validate_id(plugin_id)?;
        validate_id(&attachment.id)?;
        if attachment.data.len() > MAX_DOCUMENT_BYTES {
            return Err(AitoolsError::BudgetExceeded(format!(
                "attachment exceeds {MAX_DOCUMENT_BYTES} byte limit"
            )));
        }
        let key = composite_key(plugin_id, &attachment.id);
        let bytes = serde_json::to_vec(&attachment)?;
        let mut wtxn = self.env.write_txn()?;
        self.attachments.put(&mut wtxn, &key, &bytes)?;
        wtxn.commit()?;
        Ok(())
    }

    /// Retrieves a stored binary attachment for `plugin_id`.
    ///
    /// # Errors
    ///
    /// Returns [`AitoolsError::Storage`] for invalid IDs, [`AitoolsError::Json`] if stored data
    /// cannot be deserialized, or [`AitoolsError::Lmdb`] on LMDB failures.
    pub fn get_attachment(
        &self,
        plugin_id: &str,
        attachment_id: &str,
    ) -> Result<Option<AttachmentRecord>> {
        let _span = info_span!("db_sync", plugin_id = %plugin_id, op = "get_attachment").entered();
        validate_id(plugin_id)?;
        validate_id(attachment_id)?;
        let key = composite_key(plugin_id, attachment_id);
        let rtxn = self.env.read_txn()?;
        match self.attachments.get(&rtxn, &key)? {
            Some(bytes) => Ok(Some(serde_json::from_slice(bytes)?)),
            None => Ok(None),
        }
    }

    /// Persists an installed plugin metadata record.
    ///
    /// # Errors
    ///
    /// Returns [`AitoolsError::Storage`] for an invalid plugin ID, [`AitoolsError::Json`] on
    /// serialization errors, or [`AitoolsError::Lmdb`] on LMDB failures.
    pub fn save_plugin(&self, plugin: &PluginRecord) -> Result<()> {
        validate_id(&plugin.id)?;
        let bytes = serde_json::to_vec(plugin)?;
        let mut wtxn = self.env.write_txn()?;
        self.plugins.put(&mut wtxn, &plugin.id, &bytes)?;
        wtxn.commit()?;
        Ok(())
    }

    /// Lists all persisted plugin metadata records.
    ///
    /// # Errors
    ///
    /// Returns [`AitoolsError::Json`] if a stored record cannot be deserialized or
    /// [`AitoolsError::Lmdb`] on LMDB failures.
    pub fn list_plugins(&self) -> Result<Vec<PluginRecord>> {
        let rtxn = self.env.read_txn()?;
        let mut out = Vec::new();
        for item in self.plugins.iter(&rtxn)? {
            let (_, bytes) = item?;
            out.push(serde_json::from_slice(bytes)?);
        }
        Ok(out)
    }

    /// Removes a plugin and cascades deletion to all its documents and attachments (`SPEC-002-R3`).
    ///
    /// # Errors
    ///
    /// Returns [`AitoolsError::Storage`] for an invalid plugin ID or [`AitoolsError::Lmdb`] on
    /// LMDB failures.
    pub fn uninstall_plugin_cascade(&self, plugin_id: &str) -> Result<usize> {
        let _span =
            info_span!("db_sync", plugin_id = %plugin_id, op = "uninstall_cascade").entered();
        validate_id(plugin_id)?;
        let mut wtxn = self.env.write_txn()?;
        self.plugins.delete(&mut wtxn, plugin_id)?;

        let prefix = format!("{plugin_id}\0");
        let doc_keys: Vec<String> = self
            .docs
            .prefix_iter(&wtxn, &prefix)?
            .filter_map(|item| item.ok().map(|(k, _)| k.to_string()))
            .collect();
        let att_keys: Vec<String> = self
            .attachments
            .prefix_iter(&wtxn, &prefix)?
            .filter_map(|item| item.ok().map(|(k, _)| k.to_string()))
            .collect();

        let removed = doc_keys.len() + att_keys.len();
        for key in doc_keys {
            self.docs.delete(&mut wtxn, &key)?;
        }
        for key in att_keys {
            self.attachments.delete(&mut wtxn, &key)?;
        }

        wtxn.commit()?;
        Ok(removed)
    }

    /// Persists a global host setting by key.
    ///
    /// # Errors
    ///
    /// Returns [`AitoolsError::Storage`] for an invalid key, [`AitoolsError::Json`] on
    /// serialization errors, or [`AitoolsError::Lmdb`] on LMDB failures.
    pub fn set_setting(&self, key: &str, value: &serde_json::Value) -> Result<()> {
        validate_id(key)?;
        let bytes = serde_json::to_vec(value)?;
        let mut wtxn = self.env.write_txn()?;
        self.settings.put(&mut wtxn, key, &bytes)?;
        wtxn.commit()?;
        Ok(())
    }

    /// Reads a global host setting by key.
    ///
    /// # Errors
    ///
    /// Returns [`AitoolsError::Storage`] for an invalid key, [`AitoolsError::Json`] if stored data
    /// cannot be deserialized, or [`AitoolsError::Lmdb`] on LMDB failures.
    pub fn get_setting(&self, key: &str) -> Result<Option<serde_json::Value>> {
        validate_id(key)?;
        let rtxn = self.env.read_txn()?;
        match self.settings.get(&rtxn, key)? {
            Some(bytes) => Ok(Some(serde_json::from_slice(bytes)?)),
            None => Ok(None),
        }
    }

    /// Persists an agent session scratchpad (`SPEC-011-R3`).
    ///
    /// # Errors
    ///
    /// Returns [`AitoolsError::Storage`] for an invalid `session_id`,
    /// [`AitoolsError::BudgetExceeded`] if the serialized payload exceeds limits,
    /// [`AitoolsError::Json`] on serialization errors, or [`AitoolsError::Lmdb`] on LMDB failures.
    pub fn put_scratchpad(&self, scratchpad: &SessionScratchpad) -> Result<()> {
        validate_id(&scratchpad.session_id)?;
        let key = format!("scratch:{}", scratchpad.session_id);
        let bytes = serde_json::to_vec(scratchpad)?;
        validate_doc_bytes(bytes.len())?;
        let mut wtxn = self.env.write_txn()?;
        self.agent_memory.put(&mut wtxn, &key, &bytes)?;
        wtxn.commit()?;
        Ok(())
    }

    /// Reads an agent session scratchpad by `session_id`.
    ///
    /// # Errors
    ///
    /// Returns [`AitoolsError::Storage`] for an invalid `session_id`, [`AitoolsError::Json`] if
    /// stored data cannot be deserialized, or [`AitoolsError::Lmdb`] on LMDB failures.
    pub fn get_scratchpad(&self, session_id: &str) -> Result<Option<SessionScratchpad>> {
        validate_id(session_id)?;
        let key = format!("scratch:{session_id}");
        let rtxn = self.env.read_txn()?;
        match self.agent_memory.get(&rtxn, &key)? {
            Some(bytes) => Ok(Some(serde_json::from_slice(bytes)?)),
            None => Ok(None),
        }
    }

    /// Saves a reusable workflow macro (`SPEC-011-R3`).
    ///
    /// # Errors
    ///
    /// Returns [`AitoolsError::Storage`] for an invalid macro ID,
    /// [`AitoolsError::BudgetExceeded`] if the serialized payload exceeds limits,
    /// [`AitoolsError::Json`] on serialization errors, or [`AitoolsError::Lmdb`] on LMDB failures.
    pub fn save_workflow_macro(&self, wf: &WorkflowMacro) -> Result<()> {
        validate_id(&wf.id)?;
        let key = format!("macro:{}", wf.id);
        let bytes = serde_json::to_vec(wf)?;
        validate_doc_bytes(bytes.len())?;
        let mut wtxn = self.env.write_txn()?;
        self.agent_memory.put(&mut wtxn, &key, &bytes)?;
        wtxn.commit()?;
        Ok(())
    }

    /// Lists all saved workflow macros.
    ///
    /// # Errors
    ///
    /// Returns [`AitoolsError::Json`] if a stored macro cannot be deserialized or
    /// [`AitoolsError::Lmdb`] on LMDB failures.
    pub fn list_workflow_macros(&self) -> Result<Vec<WorkflowMacro>> {
        let rtxn = self.env.read_txn()?;
        let mut out = Vec::new();
        for item in self.agent_memory.prefix_iter(&rtxn, "macro:")? {
            let (_, bytes) = item?;
            out.push(serde_json::from_slice(bytes)?);
        }
        Ok(out)
    }

    fn read_doc_tx(&self, txn: &heed::RoTxn<'_>, key: &str) -> Result<Option<Document>> {
        match self.docs.get(txn, key)? {
            Some(bytes) => Ok(Some(serde_json::from_slice(bytes)?)),
            None => Ok(None),
        }
    }
}

fn composite_key(plugin_id: &str, item_id: &str) -> String {
    format!("{plugin_id}\0{item_id}")
}

fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() || id.contains('\0') {
        return Err(AitoolsError::Storage(
            "non-empty ID without NUL bytes required".into(),
        ));
    }
    if id.len() > MAX_DOCUMENT_BYTES {
        return Err(AitoolsError::BudgetExceeded(
            "ID exceeds maximum document byte budget".into(),
        ));
    }
    Ok(())
}

fn validate_document(doc: &Document) -> Result<()> {
    validate_id(&doc.id)?;
    let Some(map) = doc.data.as_object() else {
        return Err(AitoolsError::Storage(
            "document fields must be a JSON object".into(),
        ));
    };
    if map.contains_key("_id") || map.contains_key("_rev") {
        return Err(AitoolsError::Storage(
            "document fields cannot shadow _id or _rev".into(),
        ));
    }
    Ok(())
}

fn validate_doc_bytes(len: usize) -> Result<()> {
    if len > MAX_DOCUMENT_BYTES {
        return Err(AitoolsError::BudgetExceeded(format!(
            "document exceeds {MAX_DOCUMENT_BYTES} byte limit"
        )));
    }
    Ok(())
}

fn validate_response_budget<T: Serialize>(value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_RESPONSE_BYTES - 64 {
        return Err(AitoolsError::BudgetExceeded(format!(
            "response exceeds {MAX_RESPONSE_BYTES} byte limit"
        )));
    }
    Ok(())
}

fn next_revision(current_rev: Option<&str>, doc: &Document) -> String {
    let seq = current_rev
        .and_then(|rev| rev.split('-').next())
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(0)
        + 1;
    let mut hasher = Sha256::new();
    hasher.update(doc.id.as_bytes());
    if let Ok(payload) = serde_json::to_vec(&doc.data) {
        hasher.update(&payload);
    }
    hasher.update(seq.to_le_bytes());
    let digest = hasher.finalize();
    let prefix: [u8; 8] = digest[..8].try_into().expect("SHA-256 digest is 32 bytes");
    format!("{seq}-{:016x}", u64::from_le_bytes(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn sample_doc(id: &str, rev: Option<&str>, val: i64) -> Document {
        Document {
            id: id.to_string(),
            rev: rev.map(str::to_string),
            data: json!({ "value": val }),
        }
    }

    #[test]
    fn spec_002_r1_atomic_revision_conflict_and_cascade() {
        let dir = TempDir::new().unwrap();
        let db = StorageEngine::open(dir.path()).unwrap();

        let res1 = db.put_doc("demo", sample_doc("k1", None, 1)).unwrap();
        assert_eq!(res1.ok, Some(true));
        let rev1 = res1.rev.clone().unwrap();

        // Stale write without rev must conflict.
        let conflict = db.put_doc("demo", sample_doc("k1", None, 2)).unwrap();
        assert_eq!(conflict.error, Some(true));
        assert_eq!(conflict.name.as_deref(), Some("conflict"));

        // Valid update with rev1 succeeds.
        let res2 = db
            .put_doc("demo", sample_doc("k1", Some(&rev1), 2))
            .unwrap();
        assert_eq!(res2.ok, Some(true));
        assert_ne!(res2.rev.as_deref(), Some(rev1.as_str()));

        let fetched = db.get_doc("demo", "k1").unwrap().unwrap();
        assert_eq!(fetched.data["value"], 2);

        // Cascade uninstall deletes docs & attachments.
        db.put_attachment(
            "demo",
            AttachmentRecord {
                id: "att1".into(),
                content_type: "text/plain".into(),
                data: b"hello".to_vec(),
            },
        )
        .unwrap();
        let removed = db.uninstall_plugin_cascade("demo").unwrap();
        assert_eq!(removed, 2);
        assert!(db.get_doc("demo", "k1").unwrap().is_none());
        assert!(db.get_attachment("demo", "att1").unwrap().is_none());
    }

    #[test]
    fn spec_002_r2_enforces_budgets_and_rejects_bulk_duplicates() {
        let dir = TempDir::new().unwrap();
        let db = StorageEngine::open(dir.path()).unwrap();

        let dup_err = db
            .bulk_docs(
                "demo",
                vec![sample_doc("same", None, 1), sample_doc("same", None, 2)],
            )
            .unwrap_err();
        assert!(matches!(dup_err, AitoolsError::Storage(_)));
        assert!(db.get_doc("demo", "same").unwrap().is_none());

        let huge = "x".repeat(MAX_DOCUMENT_BYTES + 1);
        let size_err = db
            .put_doc(
                "demo",
                Document {
                    id: "big".into(),
                    rev: None,
                    data: json!({ "blob": huge }),
                },
            )
            .unwrap_err();
        assert!(matches!(size_err, AitoolsError::BudgetExceeded(_)));
    }

    #[test]
    fn spec_002_r4_validates_sync_caller_webview_label() {
        assert!(validate_sync_caller("plugin-color-helper", "color-helper").is_ok());
        let err = validate_sync_caller("plugin-evil", "color-helper").unwrap_err();
        assert!(matches!(err, AitoolsError::Forbidden(_)));
        let main_err = validate_sync_caller("main", "color-helper").unwrap_err();
        assert!(matches!(main_err, AitoolsError::Forbidden(_)));
    }

    #[test]
    fn spec_011_r3_persists_agent_memory_and_workflow_macros() {
        let dir = TempDir::new().unwrap();
        let db = StorageEngine::open(dir.path()).unwrap();

        let scratch = SessionScratchpad {
            session_id: "sess-1".into(),
            summary: "Generated Rust struct".into(),
            artifacts: vec!["model.rs".into()],
            context_snapshot: json!({ "app": "Terminal" }),
            updated_at: "2026-11-16T19:00:00Z".into(),
        };
        db.put_scratchpad(&scratch).unwrap();
        assert_eq!(db.get_scratchpad("sess-1").unwrap(), Some(scratch));

        let wf = WorkflowMacro {
            id: "json-to-rust".into(),
            name: "JSON 转 Rust 结构体".into(),
            description: "Convert clipboard JSON to Rust structs".into(),
            triggers: vec!["json2rust".into(), "转结构体".into()],
            steps: vec![MacroStep {
                tool_name: "os.clipboard.read".into(),
                arguments: json!({}),
            }],
            created_at: "2026-11-16T19:00:00Z".into(),
        };
        db.save_workflow_macro(&wf).unwrap();
        let macros = db.list_workflow_macros().unwrap();
        assert_eq!(macros.len(), 1);
        assert!(macros[0].to_skill_markdown().contains("name: json-to-rust"));

        let index = crate::FeatureSearchIndex::build(&[macros[0].to_feature_entry()]);
        assert_eq!(index.search("zjg").len(), 1);
    }
}
