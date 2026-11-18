//! Streaming multi-tool DAG planner & coroutine HITL suspension (`SPEC-011-R2`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aitools_core::RiskTier;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use tracing::{info_span, Instrument};

use crate::broker::{ApiKeyProvider, ModelBroker, ModelProfileKind};
use crate::harness::{prune_tool_output, PromptSanitizer};
use crate::hitl::{GateDecision, PolicyGuard};
use crate::registry::{RegisteredTool, ToolRegistry};
use crate::stream::{SseMessage, StreamEvent};
use crate::{AiError, Result};

/// Maximum DAG nodes allowed per turn (`SPEC-011-R3`).
pub const MAX_DAG_NODES: usize = 8;
/// Maximum topological depth allowed per DAG (`SPEC-011-R3`).
pub const MAX_DAG_DEPTH: usize = 4;
/// Maximum wait for user HITL confirmation (`SPEC-011-R5`).
pub const HITL_TIMEOUT: Duration = Duration::from_secs(120);

/// Single step in a multi-tool execution DAG (`SPEC-011-R2`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagNode {
    pub id: String,
    pub tool_name: String,
    pub risk_tier: RiskTier,
    pub arguments: Value,
    #[serde(default)]
    pub depends_on: Vec<String>,
}

/// Ordered or parallelizable tool execution plan (`SPEC-011-R2`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DagPlan {
    pub trace_id: String,
    pub nodes: Vec<DagNode>,
}

/// Real-time execution event streamed to the frontend/DSH channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DagEvent {
    NodeStarted {
        node_id: String,
        tool_name: String,
    },
    HitlSuspended {
        node_id: String,
        tool_name: String,
        risk_tier: RiskTier,
        arguments: Value,
    },
    NodeCompleted {
        node_id: String,
        output: Value,
    },
}

/// Decision passed back into a suspended HITL coroutine (`SPEC-011-R2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HitlResolution {
    ApproveOnce,
    ApproveSession,
    Deny,
}

/// Coordinates `tokio::sync::oneshot` channels for suspended HITL tool nodes.
#[derive(Default)]
pub struct HitlBroker {
    pending: Mutex<HashMap<String, oneshot::Sender<HitlResolution>>>,
}

impl HitlBroker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a waiter for `node_id` and returns the receiver half.
    pub fn register(&self, node_id: &str) -> oneshot::Receiver<HitlResolution> {
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .expect("hitl broker mutex poisoned")
            .insert(node_id.to_string(), tx);
        rx
    }

    /// Resolves a suspended `node_id`, waking the waiting async task immediately.
    pub fn resolve(&self, node_id: &str, resolution: HitlResolution) -> bool {
        let tx = self
            .pending
            .lock()
            .expect("hitl broker mutex poisoned")
            .remove(node_id);
        tx.is_some_and(|sender| sender.send(resolution).is_ok())
    }
}

pub type ToolExecFuture = Pin<Box<dyn Future<Output = Result<Value>> + Send>>;

/// Executes a [`DagPlan`] using `JoinSet` for independent nodes and `oneshot` HITL suspension.
///
/// # Errors
///
/// Returns [`AiError::Provider`] if the DAG has cyclic/unresolvable dependencies or a task
/// panics, [`AiError::HitlDenied`] if a suspended HITL confirmation is denied or dropped, or
/// propagates errors from `executor`.
pub async fn execute_dag<F, E>(
    plan: &DagPlan,
    guard: Arc<PolicyGuard>,
    hitl_broker: Arc<HitlBroker>,
    mut on_event: E,
    executor: Arc<F>,
) -> Result<HashMap<String, Value>>
where
    F: Fn(String, Value) -> ToolExecFuture + Send + Sync + 'static,
    E: FnMut(DagEvent) + Send,
{
    let span = info_span!("agent_dag", trace_id = %plan.trace_id);
    async move {
        if plan.nodes.len() > MAX_DAG_NODES {
            return Err(AiError::Provider(format!(
                "DAG exceeds max {MAX_DAG_NODES} nodes per turn"
            )));
        }

        let mut outputs: HashMap<String, Value> = HashMap::with_capacity(plan.nodes.len());
        let mut completed: HashSet<String> = HashSet::with_capacity(plan.nodes.len());
        let remaining: Vec<DagNode> = plan.nodes.clone();
        let mut depth = 0usize;

        while completed.len() < remaining.len() {
            depth += 1;
            if depth > MAX_DAG_DEPTH {
                return Err(AiError::Provider(format!(
                    "DAG exceeds max depth {MAX_DAG_DEPTH}"
                )));
            }

            let ready: Vec<DagNode> = remaining
                .iter()
                .filter(|n| !completed.contains(&n.id))
                .filter(|n| n.depends_on.iter().all(|dep| completed.contains(dep)))
                .cloned()
                .collect();

            if ready.is_empty() {
                return Err(AiError::Provider(
                    "DAG contains cyclic or unresolvable dependencies".into(),
                ));
            }

            let mut join_set = JoinSet::new();
            for mut node in ready {
                interpolate_variables(&mut node.arguments, &outputs);
                let upstream = collect_upstream(&node.depends_on, &outputs);
                if !upstream.is_null() {
                    inject_upstream(&mut node.arguments, upstream);
                }

                on_event(DagEvent::NodeStarted {
                    node_id: node.id.clone(),
                    tool_name: node.tool_name.clone(),
                });

                if let GateDecision::RequireSessionApproval { .. }
                | GateDecision::RequireExplicitApproval { .. } = guard.evaluate(
                    &node.tool_name,
                    node.risk_tier,
                    &node.id,
                    &node.arguments,
                    false,
                ) {
                    let rx = hitl_broker.register(&node.id);
                    on_event(DagEvent::HitlSuspended {
                        node_id: node.id.clone(),
                        tool_name: node.tool_name.clone(),
                        risk_tier: node.risk_tier,
                        arguments: node.arguments.clone(),
                    });
                    let resolution = tokio::time::timeout(HITL_TIMEOUT, rx)
                        .await
                        .map_err(|_| AiError::HitlDenied {
                            tool: node.tool_name.clone(),
                            reason: "HITL confirmation timed out after 120s".into(),
                        })?
                        .map_err(|_| AiError::HitlDenied {
                            tool: node.tool_name.clone(),
                            reason: "HITL channel closed before user decision".into(),
                        })?;
                    match resolution {
                        HitlResolution::ApproveOnce => {}
                        HitlResolution::ApproveSession => {
                            if node.risk_tier == RiskTier::Tier2DestructiveOrExternal {
                                return Err(AiError::HitlDenied {
                                    tool: node.tool_name,
                                    reason: "Tier-2 tools strictly require per-call ApproveOnce"
                                        .into(),
                                });
                            }
                            guard.approve_for_session(&node.tool_name);
                        }
                        HitlResolution::Deny => {
                            return Err(AiError::HitlDenied {
                                tool: node.tool_name,
                                reason: "denied by user".into(),
                            });
                        }
                    }
                }

                let exec = Arc::clone(&executor);
                let node_id = node.id.clone();
                let tool_name = node.tool_name.clone();
                let args = node.arguments.clone();
                join_set.spawn(async move {
                    let value = exec(tool_name, args).await?;
                    Ok::<(String, Value), AiError>((node_id, value))
                });
            }

            while let Some(res) = join_set.join_next().await {
                let (node_id, out) = res.map_err(|e| AiError::Provider(e.to_string()))??;
                on_event(DagEvent::NodeCompleted {
                    node_id: node_id.clone(),
                    output: out.clone(),
                });
                completed.insert(node_id.clone());
                outputs.insert(node_id, out);
            }
        }

        Ok(outputs)
    }
    .instrument(span)
    .await
}

fn collect_upstream(deps: &[String], outputs: &HashMap<String, Value>) -> Value {
    if deps.is_empty() {
        return Value::Null;
    }
    let mut map = serde_json::Map::new();
    for dep in deps {
        if let Some(val) = outputs.get(dep) {
            map.insert(dep.clone(), val.clone());
        }
    }
    Value::Object(map)
}

fn inject_upstream(args: &mut Value, upstream: Value) {
    if let Value::Object(obj) = args {
        obj.insert("_upstream".to_string(), upstream);
    } else if args.is_null() {
        *args = json!({ "_upstream": upstream });
    }
}

/// Interpolates `$node.<step_id>.output.<field>` expressions from upstream outputs (`SPEC-011-R4`).
fn interpolate_variables(args: &mut Value, outputs: &HashMap<String, Value>) {
    match args {
        Value::String(s) => {
            if let Some(resolved) = resolve_node_var(s, outputs) {
                *args = resolved;
            }
        }
        Value::Array(items) => {
            for item in items {
                interpolate_variables(item, outputs);
            }
        }
        Value::Object(map) => {
            for val in map.values_mut() {
                interpolate_variables(val, outputs);
            }
        }
        _ => {}
    }
}

fn resolve_node_var(expr: &str, outputs: &HashMap<String, Value>) -> Option<Value> {
    let rest = expr.strip_prefix("$node.")?;
    let (step_id, after_step) = rest.split_once(".output")?;
    let mut current = outputs.get(step_id)?;
    let trimmed = after_step.strip_prefix('.').unwrap_or(after_step);
    if trimmed.is_empty() {
        return Some(current.clone());
    }
    for segment in trimmed.split('.') {
        current = current.get(segment)?;
    }
    Some(current.clone())
}

/// Context chip passed from the launcher frontend into `submit_agent_prompt` (`SPEC-016-R6`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPromptChip {
    /// Chip category (`"selection"`, `"clipboard"`, `"active_app"`, `"active_path"`).
    pub kind: String,
    /// Human-readable label.
    pub label: String,
    /// Raw captured payload text.
    pub value: String,
}

/// Streaming event emitted during `submit_agent_prompt` execution (`SPEC-016-R6`, `SPEC-017-R3`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStreamEvent {
    /// Incremental assistant response token.
    Token {
        /// Text delta chunk.
        text: String,
    },
    /// Optional planner/thinking status message.
    Thinking {
        /// Status summary.
        text: String,
    },
    /// A DAG tool node began execution.
    NodeStarted {
        /// Unique node identifier within the DAG.
        node_id: String,
        /// Registered tool name.
        tool_name: String,
    },
    /// Execution suspended awaiting user HITL confirmation.
    HitlSuspended {
        /// Unique node identifier awaiting decision.
        node_id: String,
        /// Registered tool name.
        tool_name: String,
        /// Policy risk tier (`Tier1StateMutating` or `Tier2DestructiveOrExternal`).
        risk_tier: RiskTier,
        /// Tool call arguments shown in the HITL prompt.
        arguments: Value,
    },
    /// A DAG tool node finished successfully.
    NodeCompleted {
        /// Completed node identifier.
        node_id: String,
        /// Structured tool output.
        output: Value,
    },
    /// A DAG tool node failed or was denied.
    NodeFailed {
        /// Failed node identifier.
        node_id: String,
        /// Error explanation.
        error: String,
    },
    /// Final stream completion marker.
    Completed {
        /// Complete assistant text for the turn.
        final_text: String,
        /// Correlation trace ID for telemetry lookup.
        trace_id: String,
    },
    /// Fatal error emitted before stream termination.
    Error {
        /// Error description.
        message: String,
    },
}

/// Final return payload of `submit_agent_prompt` (`SPEC-016-R6`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentTurnResult {
    /// Correlation trace identifier.
    pub trace_id: String,
    /// Complete response text produced for the prompt.
    pub final_text: String,
    /// Outputs produced by executed DAG tool nodes keyed by `node_id`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub completed_nodes: HashMap<String, Value>,
}

/// Parameters controlling [`execute_agent_prompt`].
pub struct AgentPromptRequest<'a> {
    /// User prompt entered in the launcher.
    pub prompt: &'a str,
    /// Correlation trace identifier.
    pub trace_id: &'a str,
    /// Active context chips attached from pre-focus capture.
    pub chips: &'a [AgentPromptChip],
    /// Requested model routing tier.
    pub profile: ModelProfileKind,
}

/// Plans and executes an agent prompt turn with streaming tokens, DAG tool execution, and
/// strict `PolicyGuard` HITL enforcement (`SPEC-016-R6`).
///
/// # Errors
///
/// Returns [`AiError`] if quota checks fail, a live provider fails, or a HITL-gated tool is denied.
#[expect(
    clippy::too_many_arguments,
    reason = "Coordinates broker, keychain lookup, tool registry, HITL guards, and async executor"
)]
pub async fn execute_agent_prompt<F, E>(
    req: AgentPromptRequest<'_>,
    client: &reqwest::Client,
    broker: &ModelBroker,
    keys: &dyn ApiKeyProvider,
    registry: &ToolRegistry,
    guard: Arc<PolicyGuard>,
    hitl_broker: Arc<HitlBroker>,
    on_event: E,
    executor: Arc<F>,
) -> Result<AgentTurnResult>
where
    F: Fn(String, Value) -> ToolExecFuture + Send + Sync + 'static,
    E: FnMut(AgentStreamEvent) + Send,
{
    let span = info_span!("agent_prompt", trace_id = %req.trace_id);
    async move {
        let on_event = Arc::new(Mutex::new(on_event));
        let emit = |ev: AgentStreamEvent| {
            if let Ok(mut cb) = on_event.lock() {
                cb(ev);
            }
        };

        let (sanitized_prompt, prompt_injected) = PromptSanitizer::sanitize_observation(req.prompt);
        let mut ctx_parts = Vec::with_capacity(req.chips.len());
        let mut any_injection = prompt_injected;
        for chip in req.chips {
            let (clean_val, flagged) = PromptSanitizer::sanitize_observation(&chip.value);
            any_injection |= flagged;
            ctx_parts.push(format!("[{}: {}] {}", chip.kind, chip.label, clean_val));
        }
        let ctx_summary = ctx_parts.join("\n");
        if any_injection {
            emit(AgentStreamEvent::Thinking {
                text: "Untrusted prompt-injection markers neutralized before planning.".into(),
            });
        }

        let selected = registry.select_for_prompt(req.prompt, &ctx_summary);
        let tool_schemas: Vec<Value> = selected
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    }
                })
            })
            .collect();

        let mut skill_sections = Vec::new();
        for t in &selected {
            if let Some(ref skill) = t.skill_markdown {
                if !skill_sections.contains(skill) {
                    skill_sections.push(skill.clone());
                }
            }
        }
        let system_prompt = build_agent_system_prompt(&ctx_summary, &skill_sections);

        let user_msg = if ctx_summary.is_empty() {
            sanitized_prompt.clone()
        } else {
            format!("Context:\n{ctx_summary}\n\nRequest:\n{sanitized_prompt}")
        };

        // Probe whether live provider credentials exist; if not, run deterministic local DAG + response.
        let messages = [SseMessage {
            role: "user".to_string(),
            content: user_msg,
        }];

        let mut tool_accum: BTreeMap<usize, (Option<String>, String, String)> = BTreeMap::new();
        let mut live_tokens = String::new();

        // Only stream tokens directly during pass 1 if live provider runs; offline path runs DAG first.
        let has_live = broker
            .resolve_profile_with_fallback(req.profile)
            .is_some_and(|p| match p.provider {
                crate::broker::ProviderKind::OfflineMock => false,
                crate::broker::ProviderKind::Ollama => true,
                _ => keys
                    .get_api_key(&p.keychain_account)
                    .ok()
                    .flatten()
                    .is_some_and(|k| !k.trim().is_empty()),
            });

        if has_live {
            emit(AgentStreamEvent::Thinking {
                text: format!("Planning with {} profile...", req.profile.as_str()),
            });
            let outcome = broker
                .stream_chat(
                    client,
                    keys,
                    "host-agent",
                    req.profile,
                    Some(&system_prompt),
                    &messages,
                    &tool_schemas,
                    |ev| match ev {
                        StreamEvent::Token(delta) => {
                            live_tokens.push_str(&delta);
                            emit(AgentStreamEvent::Token { text: delta });
                        }
                        StreamEvent::ToolCallDelta {
                            index,
                            id,
                            name,
                            arguments_delta,
                        } => {
                            let entry = tool_accum
                                .entry(index)
                                .or_insert_with(|| (None, String::new(), String::new()));
                            if id.is_some() {
                                entry.0 = id;
                            }
                            if let Some(n) = name {
                                entry.1.push_str(&n);
                            }
                            entry.2.push_str(&arguments_delta);
                        }
                        StreamEvent::Done { .. } => {}
                    },
                )
                .await?;

            if tool_accum.is_empty() {
                let final_text = if live_tokens.is_empty() {
                    outcome.content
                } else {
                    live_tokens
                };
                emit(AgentStreamEvent::Completed {
                    final_text: final_text.clone(),
                    trace_id: req.trace_id.to_string(),
                });
                return Ok(AgentTurnResult {
                    trace_id: req.trace_id.to_string(),
                    final_text,
                    completed_nodes: HashMap::new(),
                });
            }

            let mut nodes = Vec::new();
            for (idx, (_, tool_name, raw_args)) in tool_accum {
                let parsed_args: Value =
                    serde_json::from_str(&raw_args).unwrap_or_else(|_| json!({}));
                let risk_tier = registry
                    .get(&tool_name)
                    .map_or(RiskTier::Tier2DestructiveOrExternal, |t| t.risk_tier);
                nodes.push(DagNode {
                    id: format!("step_{}", idx + 1),
                    tool_name,
                    risk_tier,
                    arguments: parsed_args,
                    depends_on: Vec::new(),
                });
                if nodes.len() >= MAX_DAG_NODES {
                    break;
                }
            }
            let plan = DagPlan {
                trace_id: req.trace_id.to_string(),
                nodes,
            };
            let dag_cb = Arc::clone(&on_event);
            let completed_nodes = execute_dag(
                &plan,
                guard,
                hitl_broker,
                move |dag_ev| {
                    if let Ok(mut cb) = dag_cb.lock() {
                        cb(map_dag_event(dag_ev));
                    }
                },
                executor,
            )
            .await?;

            let summary_prompt = format!(
                "User request: {sanitized_prompt}\nTool outputs JSON: {}",
                prune_tool_output(&json!(completed_nodes).to_string(), 6000)
            );
            let mut synthesis = String::new();
            let _ = broker
                .stream_chat(
                    client,
                    keys,
                    "host-agent",
                    req.profile,
                    Some("Summarize the completed tool results clearly and concisely."),
                    &[SseMessage {
                        role: "user".to_string(),
                        content: summary_prompt,
                    }],
                    &[],
                    |ev| {
                        if let StreamEvent::Token(delta) = ev {
                            synthesis.push_str(&delta);
                            emit(AgentStreamEvent::Token { text: delta });
                        }
                    },
                )
                .await?;

            emit(AgentStreamEvent::Completed {
                final_text: synthesis.clone(),
                trace_id: req.trace_id.to_string(),
            });
            return Ok(AgentTurnResult {
                trace_id: req.trace_id.to_string(),
                final_text: synthesis,
                completed_nodes,
            });
        }

        // OfflineMock fallback: execute deterministic context-aware DAG + stream clear synthesis.
        emit(AgentStreamEvent::Thinking {
            text:
                "No live API key configured in OS Keychain; running deterministic offline planner."
                    .into(),
        });
        let plan = plan_deterministic_offline_dag(req.trace_id, req.prompt, req.chips, &selected);
        let completed_nodes = if plan.nodes.is_empty() {
            HashMap::new()
        } else {
            let dag_cb = Arc::clone(&on_event);
            match execute_dag(
                &plan,
                guard,
                hitl_broker,
                move |dag_ev| {
                    if let Ok(mut cb) = dag_cb.lock() {
                        cb(map_dag_event(dag_ev));
                    }
                },
                executor,
            )
            .await
            {
                Ok(out) => out,
                Err(err) => {
                    emit(AgentStreamEvent::Error {
                        message: err.to_string(),
                    });
                    return Err(err);
                }
            }
        };

        // Consume quota cleanly even on offline mode via broker check.
        broker.check_and_consume_quota("host-agent", 16, std::time::Instant::now())?;

        let final_text = format_offline_summary(req.prompt, req.chips, &completed_nodes);
        for chunk in split_stream_chunks(&final_text) {
            emit(AgentStreamEvent::Token { text: chunk });
        }
        emit(AgentStreamEvent::Completed {
            final_text: final_text.clone(),
            trace_id: req.trace_id.to_string(),
        });

        Ok(AgentTurnResult {
            trace_id: req.trace_id.to_string(),
            final_text,
            completed_nodes,
        })
    }
    .instrument(span)
    .await
}

fn map_dag_event(ev: DagEvent) -> AgentStreamEvent {
    match ev {
        DagEvent::NodeStarted { node_id, tool_name } => {
            AgentStreamEvent::NodeStarted { node_id, tool_name }
        }
        DagEvent::HitlSuspended {
            node_id,
            tool_name,
            risk_tier,
            arguments,
        } => AgentStreamEvent::HitlSuspended {
            node_id,
            tool_name,
            risk_tier,
            arguments,
        },
        DagEvent::NodeCompleted { node_id, output } => {
            AgentStreamEvent::NodeCompleted { node_id, output }
        }
    }
}

fn build_agent_system_prompt(ctx_summary: &str, skills: &[String]) -> String {
    let mut out = String::from(
        "You are aitools, a desktop assistant. Use registered tools safely and keep answers concise.",
    );
    if !ctx_summary.is_empty() {
        out.push_str("\n\nActive desktop context:\n");
        out.push_str(ctx_summary);
    }
    for skill in skills {
        out.push_str("\n\n");
        out.push_str(skill);
    }
    out
}

fn plan_deterministic_offline_dag(
    trace_id: &str,
    prompt: &str,
    chips: &[AgentPromptChip],
    selected_tools: &[&RegisteredTool],
) -> DagPlan {
    let lower = prompt.to_ascii_lowercase();
    let mut nodes: Vec<DagNode> = Vec::new();
    let mut seen_tools = HashSet::new();

    let mut push_node = |tool: &RegisteredTool, arguments: Value| {
        if nodes.len() < MAX_DAG_NODES && seen_tools.insert(tool.name.clone()) {
            let idx = nodes.len() + 1;
            nodes.push(DagNode {
                id: format!("step_{idx}"),
                tool_name: tool.name.clone(),
                risk_tier: tool.risk_tier,
                arguments,
                depends_on: Vec::new(),
            });
        }
    };

    // 1. Explicit @plugin or @tool mentions select matching tools directly.
    for word in prompt.split_whitespace() {
        if let Some(mention) = word.strip_prefix('@') {
            let needle = mention
                .trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_' && c != '.')
                .to_ascii_lowercase();
            if needle.is_empty() {
                continue;
            }
            for tool in selected_tools {
                let matches_plugin = tool
                    .plugin_id
                    .as_deref()
                    .is_some_and(|pid| pid.to_ascii_lowercase() == needle);
                let matches_name = tool.name.to_ascii_lowercase().contains(&needle);
                if matches_plugin || matches_name {
                    push_node(tool, json!({ "query": prompt }));
                }
            }
        }
    }

    // 2. Semantic intent matching over built-in OS tools when no explicit mention matched.
    if (lower.contains("clipboard") || lower.contains("剪贴板"))
        && (lower.contains("copy") || lower.contains("write") || lower.contains("写入"))
    {
        if let Some(t) = selected_tools
            .iter()
            .find(|t| t.name == "os.clipboard.write")
        {
            let text_val = chips
                .iter()
                .find(|c| c.kind == "selection" || c.kind == "clipboard")
                .map_or_else(|| prompt.to_string(), |c| c.value.clone());
            push_node(t, json!({ "text": text_val }));
        }
    } else if lower.contains("clipboard") || lower.contains("剪贴板") || lower.contains("paste")
    {
        if let Some(t) = selected_tools
            .iter()
            .find(|t| t.name == "os.clipboard.read")
        {
            push_node(t, json!({}));
        }
    }

    if lower.contains("file") || lower.contains("find") || lower.contains("文件") {
        if let Some(t) = selected_tools.iter().find(|t| t.name == "os.files.find") {
            push_node(t, json!({ "query": prompt }));
        }
    }

    if lower.contains("launch ") || lower.contains("open app") || lower.contains("打开应用") {
        if let Some(t) = selected_tools.iter().find(|t| t.name == "os.apps.launch") {
            let target = prompt
                .split_whitespace()
                .last()
                .unwrap_or("Finder")
                .to_string();
            push_node(t, json!({ "app": target }));
        }
    }

    DagPlan {
        trace_id: trace_id.to_string(),
        nodes,
    }
}

fn format_offline_summary(
    prompt: &str,
    chips: &[AgentPromptChip],
    completed: &HashMap<String, Value>,
) -> String {
    if completed.is_empty() {
        return format!(
            "[offline-mock] No live LLM API key is stored in OS Keychain. Received \"{}\" with {} context chip(s). Add a provider key in Keychain to enable live model streaming.",
            prompt.trim(),
            chips.len()
        );
    }
    let mut keys: Vec<&String> = completed.keys().collect();
    keys.sort();
    let mut lines = vec![format!(
        "[offline-mock] Executed {} tool step(s) for \"{}\":",
        completed.len(),
        prompt.trim()
    )];
    for k in keys {
        if let Some(val) = completed.get(k) {
            lines.push(format!(
                "- {k}: {}",
                prune_tool_output(&val.to_string(), 400)
            ));
        }
    }
    lines.join("\n")
}

fn split_stream_chunks(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut start = 0usize;
    for (idx, ch) in text.char_indices() {
        if ch.is_whitespace() && idx > start {
            chunks.push(text[start..=idx].to_string());
            start = idx + ch.len_utf8();
        }
    }
    if start < text.len() {
        chunks.push(text[start..].to_string());
    }
    if chunks.is_empty() && !text.is_empty() {
        chunks.push(text.to_string());
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spec_011_r2_executes_parallel_dag_and_suspends_on_hitl() {
        let plan = DagPlan {
            trace_id: "agent-dag-1".into(),
            nodes: vec![
                DagNode {
                    id: "read_a".into(),
                    tool_name: "os.clipboard.read".into(),
                    risk_tier: RiskTier::Tier0ReadOnly,
                    arguments: json!({}),
                    depends_on: vec![],
                },
                DagNode {
                    id: "read_b".into(),
                    tool_name: "os.files.find".into(),
                    risk_tier: RiskTier::Tier0ReadOnly,
                    arguments: json!({ "query": "spec" }),
                    depends_on: vec![],
                },
                DagNode {
                    id: "write_out".into(),
                    tool_name: "os.clipboard.write".into(),
                    risk_tier: RiskTier::Tier1StateMutating,
                    arguments: json!({ "text": "done" }),
                    depends_on: vec!["read_a".into(), "read_b".into()],
                },
            ],
        };

        let guard = Arc::new(PolicyGuard::new());
        let broker = Arc::new(HitlBroker::new());
        let broker_trigger = Arc::clone(&broker);
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_cb = Arc::clone(&events);

        let exec = Arc::new(|tool: String, args: Value| -> ToolExecFuture {
            Box::pin(async move { Ok(json!({ "tool": tool, "args": args })) })
        });

        let results = execute_dag(
            &plan,
            guard,
            broker,
            move |ev| {
                if let DagEvent::HitlSuspended { ref node_id, .. } = ev {
                    assert!(broker_trigger.resolve(node_id, HitlResolution::ApproveOnce));
                }
                events_cb.lock().expect("events").push(ev);
            },
            exec,
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 3);
        let write_args = &results["write_out"]["args"];
        assert!(write_args["_upstream"]["read_a"].is_object());
        assert!(write_args["_upstream"]["read_b"].is_object());
        assert!(events.lock().expect("events").iter().any(
            |e| matches!(e, DagEvent::HitlSuspended { node_id, .. } if node_id == "write_out")
        ));
    }

    #[tokio::test]
    async fn spec_011_r3_and_r4_enforces_bounds_and_interpolates_jsonpath() {
        let too_many = DagPlan {
            trace_id: "too-many".into(),
            nodes: (0..=MAX_DAG_NODES)
                .map(|i| DagNode {
                    id: format!("n{i}"),
                    tool_name: "os.clipboard.read".into(),
                    risk_tier: RiskTier::Tier0ReadOnly,
                    arguments: json!({}),
                    depends_on: vec![],
                })
                .collect(),
        };
        let err = execute_dag(
            &too_many,
            Arc::new(PolicyGuard::new()),
            Arc::new(HitlBroker::new()),
            |_| {},
            Arc::new(|_, _| Box::pin(async { Ok(json!({})) }) as ToolExecFuture),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("nodes"));

        let pipe_plan = DagPlan {
            trace_id: "pipe".into(),
            nodes: vec![
                DagNode {
                    id: "step1".into(),
                    tool_name: "os.clipboard.read".into(),
                    risk_tier: RiskTier::Tier0ReadOnly,
                    arguments: json!({}),
                    depends_on: vec![],
                },
                DagNode {
                    id: "step2".into(),
                    tool_name: "os.files.find".into(),
                    risk_tier: RiskTier::Tier0ReadOnly,
                    arguments: json!({ "query": "$node.step1.output.payload.keyword" }),
                    depends_on: vec!["step1".into()],
                },
            ],
        };
        let out = execute_dag(
            &pipe_plan,
            Arc::new(PolicyGuard::new()),
            Arc::new(HitlBroker::new()),
            |_| {},
            Arc::new(|tool, args| {
                Box::pin(async move {
                    if tool == "os.clipboard.read" {
                        Ok(json!({ "payload": { "keyword": "readme.md" } }))
                    } else {
                        Ok(args)
                    }
                }) as ToolExecFuture
            }),
        )
        .await
        .unwrap();
        assert_eq!(out["step2"]["query"], "readme.md");
    }

    #[tokio::test]
    async fn spec_016_r6_agent_prompt_enforces_tier2_per_call_and_streams_events() {
        use crate::broker::NoopApiKeyProvider;

        let mut registry = ToolRegistry::new();
        registry.register_builtin_os_defaults();
        registry.register_mcp_tool(
            "shell",
            "exec",
            "Execute destructive shell script",
            json!({ "type": "object", "properties": { "cmd": { "type": "string" } } }),
            Some(RiskTier::Tier2DestructiveOrExternal),
        );

        let guard = Arc::new(PolicyGuard::new());
        let hitl = Arc::new(HitlBroker::new());
        let broker = ModelBroker::new();
        let client = reqwest::Client::new();
        let keys = NoopApiKeyProvider;

        // Tier-2 ApproveSession must be rejected; ApproveOnce must NOT grant future calls.
        let plan_tier2 = DagPlan {
            trace_id: "t2-check".into(),
            nodes: vec![DagNode {
                id: "danger_1".into(),
                tool_name: "mcp.shell.exec".into(),
                risk_tier: RiskTier::Tier2DestructiveOrExternal,
                arguments: json!({ "cmd": "rm -rf /tmp/x" }),
                depends_on: vec![],
            }],
        };

        let hitl_session_try = Arc::clone(&hitl);
        let denied = execute_dag(
            &plan_tier2,
            Arc::clone(&guard),
            Arc::clone(&hitl),
            move |ev| {
                if let DagEvent::HitlSuspended { node_id, .. } = ev {
                    hitl_session_try.resolve(&node_id, HitlResolution::ApproveSession);
                }
            },
            Arc::new(|_, _| Box::pin(async { Ok(json!({ "ok": true })) }) as ToolExecFuture),
        )
        .await
        .unwrap_err();
        assert!(matches!(denied, AiError::HitlDenied { .. }));

        // Now execute submit_agent_prompt offline against @shell tool with per-call ApproveOnce.
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_cb = Arc::clone(&events);
        let hitl_once = Arc::clone(&hitl);
        let res = execute_agent_prompt(
            AgentPromptRequest {
                prompt: "Run @shell command",
                trace_id: "agent-trace-1",
                chips: &[AgentPromptChip {
                    kind: "clipboard".into(),
                    label: "Clipboard".into(),
                    value: "echo hi".into(),
                }],
                profile: ModelProfileKind::Balanced,
            },
            &client,
            &broker,
            &keys,
            &registry,
            Arc::clone(&guard),
            hitl,
            move |ev| {
                if let AgentStreamEvent::HitlSuspended { ref node_id, .. } = ev {
                    assert!(hitl_once.resolve(node_id, HitlResolution::ApproveOnce));
                }
                events_cb.lock().expect("events").push(ev);
            },
            Arc::new(|tool, args| {
                Box::pin(async move { Ok(json!({ "tool": tool, "args": args })) }) as ToolExecFuture
            }),
        )
        .await
        .unwrap();

        assert_eq!(res.trace_id, "agent-trace-1");
        assert!(!res.completed_nodes.is_empty());
        let recorded = events.lock().expect("events");
        assert!(recorded
            .iter()
            .any(|e| matches!(e, AgentStreamEvent::HitlSuspended { .. })));
        assert!(recorded
            .iter()
            .any(|e| matches!(e, AgentStreamEvent::Token { .. })));
        assert!(recorded
            .iter()
            .any(|e| matches!(e, AgentStreamEvent::Completed { .. })));
    }
}
