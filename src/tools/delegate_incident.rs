//! Hand one incident to a worker agent with less authority than the orchestrator.
//!
//! This is the agent-as-tool pattern. Rig's `Agent::into_tool()` would work
//! mechanically, but it propagates the parent's `ToolContext`, so the worker
//! would run with the orchestrator's full authority. Instead this tool mints a
//! narrower child through the guard, builds a fresh context holding only that,
//! and prompts the worker with it. The worker cannot reach the parent's key.

use std::sync::Arc;
use std::time::Duration;

use rig::prelude::*;
use rig::tool::{Tool, ToolContext};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tenuo::sdk::prelude::*;
use tenuo::{constraints, Exact};

use crate::authority::{guarded, RunAuthority, ToolError};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct DelegateArgs {
    /// Incident the worker may investigate, e.g. "INC-42".
    pub incident_id: String,
}

pub struct DelegateIncident {
    pub worker: Agent,
}

impl Tool for DelegateIncident {
    const NAME: &'static str = "delegate_incident";
    type Args = DelegateArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Ask the incident worker agent to investigate one incident.".into()
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

        // 2. Mint the worker's authority: read this one incident, nothing else,
        //    five minutes, no further delegation. Checked against current policy.
        let profile = DelegationProfile::new()
            .capability("read_incident", constraints! { "incident_id" => Exact::new(args.incident_id.as_str()) })
            .ttl(Duration::from_secs(300))
            .terminal();
        let child = parent
            .guard
            .delegate(&parent.authority, &profile)
            .map_err(|e| ToolError::Operation(format!("delegate: {e}")))?;
        let worker_authority = RunAuthority {
            guard: parent.guard.clone(),
            authority: Arc::new(child),
            agent: "worker",
        };
        println!("      [tenuo] child  worker       read_incident incident_id={} ttl=300s terminal", args.incident_id);

        // 3. Run the worker agent with a fresh context. Only the child authority is in it.
        let report = self
            .worker
            .prompt(format!("Investigate {} and report.", args.incident_id))
            .tool_context(worker_authority.context())
            .max_turns(6)
            .await
            .map_err(|e| ToolError::Operation(e.to_string()))?;
        Ok(report)
    }
}
