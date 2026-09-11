//! What a run carries into every tool call, and the one helper every tool uses.
//!
//! Rig clones `ToolContext` into each tool invocation. A `RunAuthority` goes in
//! once per run; tools pull it back out by type. Nothing here is Rig-specific
//! beyond `ToolContext::insert` / `get` / `insert_result`.

use std::fmt;
use std::sync::Arc;

use rig::tool::{ToolContext, ToolExecutionError, ToolOutput};
use serde::Serialize;
use tenuo::sdk::prelude::*;

/// Authority for one run: the guard that decides and the chain the caller holds.
#[derive(Clone)]
pub struct RunAuthority {
    pub guard: Arc<Guard>,
    pub authority: Arc<PresentedAuthority>,
    pub agent: String,
}

impl RunAuthority {
    /// A fresh `ToolContext` carrying this authority and nothing else.
    pub fn context(&self) -> ToolContext {
        let mut ctx = ToolContext::new();
        ctx.insert(self.clone());
        ctx
    }
}

/// Error a guarded tool returns. Denials carry only a sanitized code and message.
/// Tool implementations pass these through [`ToolError::into_execution_error`]
/// so Rig can safely expose them to the model.
#[derive(Debug)]
pub enum ToolError {
    NoAuthority,
    Denied { code: String, message: String },
    Arguments(String),
    Timeout(String),
    Operation(String),
    McpDenied { message: String, output: ToolOutput },
}

impl ToolError {
    /// Normalize domain errors for Rig's runtime, telemetry, and model feedback.
    pub fn into_execution_error(self) -> ToolExecutionError {
        match self {
            Self::NoAuthority => {
                ToolExecutionError::refused("denied (missing-authority): no authority was provided")
                    .with_code("missing-authority")
            }
            Self::Denied { code, message } => {
                ToolExecutionError::refused(format!("denied ({code}): {message}")).with_code(code)
            }
            Self::Arguments(message) => ToolExecutionError::invalid_args(message),
            Self::Timeout(message) => ToolExecutionError::timeout(message),
            Self::Operation(message) => ToolExecutionError::provider(message)
                .with_model_feedback("the tool's upstream operation failed"),
            Self::McpDenied { message, output } => ToolExecutionError::refused(message)
                .with_code("mcp-tool-error")
                .with_model_output(output),
        }
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAuthority => write!(f, "no authority in tool context"),
            Self::Denied { code, message } => write!(f, "denied ({code}): {message}"),
            Self::Arguments(m) => write!(f, "invalid arguments: {m}"),
            Self::Timeout(m) => write!(f, "operation timed out: {m}"),
            Self::Operation(m) => write!(f, "operation failed: {m}"),
            Self::McpDenied { message, .. } => write!(f, "MCP tool denied the call: {message}"),
        }
    }
}

impl std::error::Error for ToolError {}

/// Preserve policy denials and canonical Tenuo codes while keeping signer and
/// authority-construction failures out of model-visible diagnostics.
pub fn map_delegation_error(error: DelegationError) -> ToolError {
    match error {
        DelegationError::Denied(denial) => ToolError::Denied {
            code: denial.code().into(),
            message: denial.message().into(),
        },
        DelegationError::Core(error) => ToolError::Denied {
            code: error.code().name().into(),
            message: error.to_string(),
        },
        other => ToolError::Operation(format!("delegate: {other}")),
    }
}

/// Run `op` only if the warrant in `ctx` allows `capability` with `args`.
///
/// On allow, the decision record goes into the context's host-only result slot,
/// so the host can log or forward it without the model ever seeing it.
pub fn guarded<A, T>(
    ctx: &mut ToolContext,
    capability: &'static str,
    args: &A,
    op: impl FnOnce(&AuthorizedCall<'_>) -> Result<T, ToolError>,
) -> Result<T, ToolError>
where
    A: Serialize,
{
    let value = serde_json::to_value(args).map_err(|e| ToolError::Arguments(e.to_string()))?;
    guarded_value(ctx, capability, &value, op)
}

/// `guarded`, for callers that must reuse the exact serialized argument value
/// across authorization and a downstream protocol request.
pub fn guarded_value<T>(
    ctx: &mut ToolContext,
    capability: &'static str,
    value: &serde_json::Value,
    op: impl FnOnce(&AuthorizedCall<'_>) -> Result<T, ToolError>,
) -> Result<T, ToolError> {
    let run = ctx
        .get::<RunAuthority>()
        .cloned()
        .ok_or(ToolError::NoAuthority)?;
    let call = Call::try_from_json(capability, value)
        .map_err(|e| ToolError::Arguments(format!("{e:?}")))?;

    let summary = value.to_string();
    let result = run.guard.guard(&run.authority, &call, op);
    match &result {
        Ok(_) => println!(
            "      [tenuo] allow  {:<14} {capability} {summary}",
            run.agent
        ),
        Err(GuardError::Denied(d)) => {
            println!(
                "      [tenuo] deny   {:<14} {capability} {summary}  ({})",
                run.agent,
                d.code()
            )
        }
        Err(GuardError::Operation(e)) => {
            println!("      [tenuo] error  {:<14} {capability}: {e}", run.agent)
        }
    }

    let guarded = result.map_err(|e| match e {
        GuardError::Denied(d) => ToolError::Denied {
            code: d.code().to_string(),
            message: d.message().to_string(),
        },
        GuardError::Operation(e) => e,
    })?;
    ctx.insert_result(guarded.decision.metadata.clone());
    Ok(guarded.into_inner())
}

#[cfg(test)]
mod tests {
    use super::ToolError;

    #[test]
    fn denial_is_a_model_visible_rig_refusal() {
        let error = ToolError::Denied {
            code: "constraint-violation".into(),
            message: "Constraint not satisfied".into(),
        }
        .into_execution_error();

        assert!(error.is_refusal());
        assert_eq!(error.code(), Some("constraint-violation"));
        assert_eq!(
            error.model_feedback(),
            Some("denied (constraint-violation): Constraint not satisfied")
        );
    }

    #[test]
    fn operation_diagnostics_are_redacted_from_the_model() {
        let error = ToolError::Operation("upstream response contained a secret".into())
            .into_execution_error();

        assert_eq!(
            error.model_feedback(),
            Some("the tool's upstream operation failed")
        );
        assert!(!error.model_feedback().unwrap().contains("secret"));
    }
}
