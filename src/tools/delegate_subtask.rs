//! The second hop: a worker hands one sub-step to a reader agent.
//!
//! Same pattern as `delegate_incident`, one level down. The grandchild is
//! terminal and scoped to the same incident. A worker that asks for broader
//! scope than it holds is refused at mint time by core attenuation, before any
//! agent runs: `Exact("INC-42")` cannot become `Pattern("INC-*")`.

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
use crate::tools::delegate_incident::WorkerFactory;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SubtaskArgs {
    /// Incident the sub-step concerns.
    pub incident_id: String,
    /// "incident" (default) or "all-incidents". The latter asks for more than the worker holds.
    #[serde(default = "default_scope")]
    pub scope: String,
}

fn default_scope() -> String {
    "incident".into()
}

pub struct DelegateSubtask {
    pub reader_factory: WorkerFactory,
}

impl Tool for DelegateSubtask {
    const NAME: &'static str = "delegate_subtask";
    type Args = SubtaskArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Hand one sub-step of the investigation to a reader agent.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "incident_id": { "type": "string" },
                "scope": { "type": "string", "enum": ["incident", "all-incidents"] }
            },
            "required": ["incident_id"]
        })
    }

    async fn call(&self, ctx: &mut ToolContext, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let parent = ctx.get::<RunAuthority>().cloned().ok_or(ToolError::NoAuthority)?;
        guarded(ctx, Self::NAME, &args, |_| Ok(()))?;

        let id = args.incident_id.as_str();
        let wanted = if args.scope == "all-incidents" {
            constraints! { "incident_id" => Pattern::new("INC-*").map_err(|e| ToolError::Operation(e.to_string()))? }
        } else {
            constraints! { "incident_id" => Exact::new(id) }
        };
        let profile = DelegationProfile::new()
            .capability("read_incident", wanted)
            .ttl(Duration::from_secs(120))
            .terminal();

        let grandchild = match parent.guard.delegate(&parent.authority, &profile) {
            Ok(g) => g,
            Err(e) => {
                println!("      [tenuo] refuse {:<14} mint for scope={}: {e}", parent.agent, args.scope);
                return Err(ToolError::Denied { code: "attenuation".into(), message: e.to_string() });
            }
        };
        let label = format!("reader[{id}]");
        println!(
            "      [tenuo] child  {:<14} holder={} depth={} ttl=120s terminal",
            label,
            grandchild.holder().fingerprint(),
            grandchild.chain().len()
        );
        let authority = RunAuthority { guard: parent.guard.clone(), authority: Arc::new(grandchild), agent: label };

        let reader = (self.reader_factory)(id);
        reader
            .prompt(format!("Collect the timeline for {id}."))
            .tool_context(authority.context())
            .max_turns(4)
            .await
            .map_err(|e| ToolError::Operation(e.to_string()))
    }
}
