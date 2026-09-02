//! A Rig tool that calls an MCP server, carrying the warrant in `_meta.tenuo`.
//!
//! Why not Rig's `rmcp_tools()`? It has a first-class `_meta` channel: a
//! `rmcp::model::Meta` placed in `ToolContext` is forwarded on every call. But it
//! is read from the run's context, so it is fixed before the model chooses
//! arguments, and Rig's pre-tool hook can rewrite arguments but cannot write to
//! the context. Tenuo's envelope carries a proof of possession signed over the
//! exact arguments, which do not exist until the model picks them. So this tool
//! drives the rmcp client itself: guard first, and only an allowed call produces
//! an envelope. The server verifies it again regardless.

use std::sync::Arc;

use rig::tool::{Tool, ToolContext};
use rmcp::model::{CallToolRequestParams, RequestMetaObject};
use rmcp::service::{RoleClient, RunningService};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tenuo::sdk::transport::mcp_meta::encode_meta_from_authorized;

use crate::authority::{guarded, ToolError};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct IncidentArgs {
    /// Incident identifier, e.g. "INC-42".
    pub incident_id: String,
}

pub struct RemoteReadIncident {
    pub client: Arc<RunningService<RoleClient, ()>>,
}

impl Tool for RemoteReadIncident {
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
        // The closure only builds the envelope. No allow, no envelope.
        let envelope = guarded(ctx, Self::NAME, &args, |authorized| {
            encode_meta_from_authorized(authorized).map_err(|e| ToolError::Operation(format!("{e:?}")))
        })?;

        let mut meta = RequestMetaObject::new();
        meta.insert("tenuo".into(), envelope);
        let arguments = serde_json::to_value(&args)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        let mut params = CallToolRequestParams::new(Self::NAME).with_arguments(arguments);
        params.meta = Some(meta);

        let result = self.client.call_tool(params).await.map_err(|e| ToolError::Operation(e.to_string()))?;
        let text = result
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.clone()))
            .collect::<Vec<_>>()
            .join("\n");
        if result.is_error.unwrap_or(false) {
            return Err(ToolError::Denied { code: "server".into(), message: text });
        }
        Ok(text)
    }
}
