//! A Rig tool that calls an MCP server and carries the warrant in `_meta.tenuo`.
//!
//! Rig's built-in `rmcp_tools()` wrapper only forwards arguments, so this tool
//! drives the rmcp client itself. The guard runs first; only an allowed call
//! produces the `_meta` payload, and the server verifies it again.

use std::sync::Arc;

use rig::tool::{Tool, ToolContext};
use rmcp::model::{CallToolRequestParams, RequestMetaObject};
use rmcp::service::{RoleClient, RunningService};
use serde_json::json;

use crate::authority::{RunAuthority, ToolError};
use crate::tools::IncidentArgs;
use tenuo::sdk::prelude::*;
use tenuo::sdk::transport::mcp_meta::encode_meta_from_authorized;

pub struct RemoteReadIncident {
    pub client: Arc<RunningService<RoleClient, ()>>,
}

impl Tool for RemoteReadIncident {
    const NAME: &'static str = "read_incident";
    type Args = IncidentArgs;
    type Output = String;
    type Error = ToolError;

    fn description(&self) -> String {
        "Read an incident record from the incident MCP server.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": { "incident_id": { "type": "string" } },
            "required": ["incident_id"]
        })
    }

    async fn call(&self, ctx: &mut ToolContext, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let run = ctx
            .get::<RunAuthority>()
            .cloned()
            .ok_or(ToolError::NoAuthority)?;

        let value = serde_json::to_value(&args).map_err(|e| ToolError::Arguments(e.to_string()))?;
        let call = Call::try_from_json(Self::NAME, &value)
            .map_err(|e| ToolError::Arguments(format!("{e:?}")))?;

        // The closure only builds the envelope. No allow, no envelope.
        let guarded = run
            .guard
            .guard(&run.authority, &call, |authorized| {
                encode_meta_from_authorized(authorized)
                    .map_err(|e| ToolError::Operation(format!("{e:?}")))
            })
            .map_err(|e| match e {
                GuardError::Denied(d) => ToolError::Denied {
                    code: d.code().to_string(),
                    message: d.message().to_string(),
                },
                GuardError::Operation(e) => e,
            })?;
        ctx.insert_result(guarded.decision.metadata.clone());
        let tenuo_meta = guarded.into_inner();

        let mut meta = RequestMetaObject::new();
        meta.insert("tenuo".to_string(), tenuo_meta);
        let arguments = value.as_object().cloned().unwrap_or_default();
        let mut params = CallToolRequestParams::new(Self::NAME).with_arguments(arguments);
        params.meta = Some(meta);

        let result = self
            .client
            .call_tool(params)
            .await
            .map_err(|e| ToolError::Operation(e.to_string()))?;

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
