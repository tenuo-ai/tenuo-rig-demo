//! A Rig tool that calls an MCP server, carrying the warrant in
//! `_meta["ai.tenuo/authorization"]`.
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
use std::time::Duration;

use rig::message::ToolResultContent;
use rig::tool::{Tool, ToolContext, ToolExecutionError, ToolOutput};
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResult, ClientRequest, ContentBlock,
    RequestMetaObject, ServerResult,
};
use rmcp::service::{PeerRequestOptions, RoleClient, RunningService, ServiceError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tenuo::sdk::transport::mcp_meta::encode_meta_from_authorized;

use crate::authority::{guarded_value, ToolError};

const TENUO_META_KEY: &str = "ai.tenuo/authorization";
const MCP_CALL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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
    type Output = ToolOutput;
    type Error = ToolError;

    fn description(&self) -> String {
        "Read an incident record from the security system of record.".into()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": { "incident_id": { "type": "string" } },
            "required": ["incident_id"],
            "additionalProperties": false
        })
    }

    fn map_error(&self, error: Self::Error) -> ToolExecutionError {
        error.into_execution_error()
    }

    async fn call(
        &self,
        ctx: &mut ToolContext,
        args: Self::Args,
    ) -> Result<Self::Output, Self::Error> {
        // Serialize once: this exact object is authorized and sent over MCP.
        let value = serde_json::to_value(&args).map_err(|e| ToolError::Arguments(e.to_string()))?;
        let arguments = value
            .as_object()
            .cloned()
            .ok_or_else(|| ToolError::Arguments("tool arguments must be an object".into()))?;

        // The closure only builds the envelope. No allow, no envelope.
        let envelope = guarded_value(ctx, Self::NAME, &value, |authorized| {
            encode_meta_from_authorized(authorized)
                .map_err(|e| ToolError::Operation(format!("{e:?}")))
        })?;

        let mut meta = RequestMetaObject::new();
        meta.insert(TENUO_META_KEY.into(), envelope);
        let mut params = CallToolRequestParams::new(Self::NAME).with_arguments(arguments);
        params.meta = Some(meta);

        let request = ClientRequest::CallToolRequest(CallToolRequest::new(params));
        let handle = self
            .client
            .peer()
            .send_cancellable_request(request, PeerRequestOptions::with_timeout(MCP_CALL_TIMEOUT))
            .await
            .map_err(map_service_error)?;
        let response = handle.await_response().await.map_err(map_service_error)?;
        let result = match response {
            ServerResult::CallToolResult(result) => result,
            _ => return Err(ToolError::Operation("unexpected MCP response".into())),
        };

        // Preserve the untouched protocol result for host-side hooks and telemetry.
        ctx.insert_result(result.clone());
        if let Some(structured) = result.structured_content.clone() {
            ctx.insert_result(structured);
        }
        if let Some(meta) = result.meta.clone() {
            ctx.insert_result(meta);
        }

        let output = mcp_result_output(&result)?;
        if result.is_error.unwrap_or(false) {
            let message = result
                .content
                .iter()
                .filter_map(|c| c.as_text().map(|t| t.text.clone()))
                .collect::<Vec<_>>()
                .join("\n");
            return Err(ToolError::McpDenied { message, output });
        }
        Ok(output)
    }
}

fn map_service_error(error: ServiceError) -> ToolError {
    match error {
        ServiceError::Timeout { timeout } => {
            ToolError::Timeout(format!("MCP call exceeded {timeout:?}"))
        }
        other => ToolError::Operation(format!("MCP request failed: {other}")),
    }
}

/// Keep structured and non-text MCP content model-visible without flattening it.
/// The exact `CallToolResult` is also retained in `ToolContext` above.
fn mcp_result_output(result: &CallToolResult) -> Result<ToolOutput, ToolError> {
    let mut content = Vec::with_capacity(result.content.len() + 1);
    if let Some(structured) = result.structured_content.clone() {
        content.push(ToolResultContent::json(structured));
    }
    for block in &result.content {
        match block {
            ContentBlock::Text(text) => content.push(ToolResultContent::text(text.text.clone())),
            _ => content.push(ToolResultContent::json(
                serde_json::to_value(block)
                    .map_err(|e| ToolError::Operation(format!("serialize MCP content: {e}")))?,
            )),
        }
    }
    if content.is_empty() {
        Ok(ToolOutput::text(""))
    } else {
        ToolOutput::content(content).map_err(|e| ToolError::Operation(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::IncidentArgs;

    #[test]
    fn incident_arguments_reject_unknown_fields() {
        let result = serde_json::from_value::<IncidentArgs>(serde_json::json!({
            "incident_id": "INC-42",
            "unapproved_extension": true
        }));
        assert!(result.is_err());
    }
}
