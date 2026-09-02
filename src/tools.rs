//! Ordinary Rig tools with the Tenuo check inside `call()`.
//!
//! Because the check lives in the tool body, every dispatch path Rig has
//! (agent runs, streaming, direct `ToolSet::execute`) goes through it.

use rig::tool::{Tool, ToolContext};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::authority::{guarded, ToolError};

// ---------------------------------------------------------------- scale_cluster

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ScaleArgs {
    /// Cluster name, e.g. "staging-web".
    pub cluster: String,
    /// Desired replica count.
    pub replicas: i64,
}

pub struct ScaleCluster;

impl Tool for ScaleCluster {
    const NAME: &'static str = "scale_cluster";
    type Args = ScaleArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Scale a Kubernetes cluster to a replica count.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "cluster": { "type": "string" },
                "replicas": { "type": "integer" }
            },
            "required": ["cluster", "replicas"]
        })
    }

    async fn call(&self, ctx: &mut ToolContext, args: Self::Args) -> Result<Self::Output, Self::Error> {
        guarded(ctx, Self::NAME, &args, || {
            Ok(format!("scaled {} to {} replicas", args.cluster, args.replicas))
        })
    }
}

// ---------------------------------------------------------------- read_incident

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct IncidentArgs {
    /// Incident identifier, e.g. "INC-42".
    pub incident_id: String,
}

pub struct ReadIncident;

impl Tool for ReadIncident {
    const NAME: &'static str = "read_incident";
    type Args = IncidentArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Read an incident record from the security system of record.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": { "incident_id": { "type": "string" } },
            "required": ["incident_id"]
        })
    }

    async fn call(&self, ctx: &mut ToolContext, args: Self::Args) -> Result<Self::Output, Self::Error> {
        guarded(ctx, Self::NAME, &args, || {
            Ok(format!(
                "{}: severity=high, status=open, owner=secops",
                args.incident_id
            ))
        })
    }
}
