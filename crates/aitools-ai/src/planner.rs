//! Streaming multi-tool DAG planner & coroutine HITL suspension (`SPEC-011-R2`).

use std::collections::{HashMap, HashSet};
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

use crate::hitl::{GateDecision, PolicyGuard};
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
}
