//! Hand one incident to a worker agent with less authority than the orchestrator.
//!
//! This is the agent-as-tool pattern. Rig's `Agent::into_tool()` would work
//! mechanically, but it propagates the parent's `ToolContext`, so the worker
//! would run with the orchestrator's full authority. Instead this tool mints a
//! narrower child through the guard, builds a fresh context holding only that,
//! and prompts a worker with it. A new worker agent is built per delegation so
//! two incidents handled in the same turn never share state.

use std::sync::Arc;
use std::time::Duration;

use rig::prelude::*;
use rig::tool::{Tool, ToolContext};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tenuo::sdk::prelude::*;
use tenuo::{constraints, Exact, Wildcard};

use crate::authority::{guarded, RunAuthority, ToolError};

pub type WorkerFactory = Arc<dyn Fn(&str) -> Agent + Send + Sync>;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct DelegateArgs {
    /// Incident the worker may investigate, e.g. "INC-42".
    pub incident_id: String,
}

pub struct DelegateIncident {
    pub worker_factory: WorkerFactory,
}

impl Tool for DelegateIncident {
    const NAME: &'static str = "delegate_incident";
    type Args = DelegateArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Ask an incident worker agent to investigate one incident.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": { "incident_id": { "type": "string" } },
            "required": ["incident_id"]
        })
    }

    async fn call(&self, ctx: &mut ToolContext, args: Self::Args) -> Result<Self::Output, Self::Error> {
        // 1. Delegating is itself a guarded action on the orchestrator's warrant.
        let parent = ctx.get::<RunAuthority>().cloned().ok_or(ToolError::NoAuthority)?;
        guarded(ctx, Self::NAME, &args, |_| Ok(()))?;

        // 2. The worker may read this one incident and delegate one sub-step for
        //    it. Five minutes. It may go one hop deeper, no further.
        let id = args.incident_id.as_str();
        let profile = DelegationProfile::new()
            .capability("read_incident", constraints! { "incident_id" => Exact::new(id) })
            .capability(
                "delegate_subtask",
                constraints! { "incident_id" => Exact::new(id), "scope" => Wildcard::new() },
            )
            .ttl(Duration::from_secs(300))
            .max_depth(2);
        let child = parent.guard.delegate(&parent.authority, &profile).map_err(|e| {
            println!("      [tenuo] refuse {:<14} mint worker for {id}: {e}", parent.agent);
            ToolError::Operation(format!("delegate: {e}"))
        })?;
        let label = format!("worker[{id}]");
        println!(
            "      [tenuo] child  {:<14} holder={} depth={} ttl=300s may_delegate_to_depth=2",
            label,
            child.holder().fingerprint(),
            child.chain().len()
        );
        let worker_authority = RunAuthority { guard: parent.guard.clone(), authority: Arc::new(child), agent: label };

        // 3. Run a fresh worker agent with a fresh context. Only the child authority is in it.
        let worker = (self.worker_factory)(id);
        let report = worker
            .prompt(format!("Investigate {id} and report."))
            .tool_context(worker_authority.context())
            .max_turns(8)
            .await
            .map_err(|e| ToolError::Operation(e.to_string()))?;
        Ok(report)
    }
}
