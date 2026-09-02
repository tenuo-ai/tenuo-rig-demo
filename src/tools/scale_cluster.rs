//! An ordinary Rig tool. The Tenuo check is the first thing `call()` does, so
//! every dispatch path Rig has (agent loop, streaming, `ToolSet::execute`)
//! goes through it.

use rig::tool::{Tool, ToolContext};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::authority::{guarded, ToolError};

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
            "properties": { "cluster": { "type": "string" }, "replicas": { "type": "integer" } },
            "required": ["cluster", "replicas"]
        })
    }

    async fn call(&self, ctx: &mut ToolContext, args: Self::Args) -> Result<Self::Output, Self::Error> {
        guarded(ctx, Self::NAME, &args, |_| {
            Ok(format!("scaled {} to {} replicas", args.cluster, args.replicas))
        })
    }
}
