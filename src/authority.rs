//! What a run carries into every tool call, and the one helper every tool uses.
//!
//! Rig clones `ToolContext` into each tool invocation. We put a `RunAuthority`
//! in it once per run; tools pull it back out by type. Nothing here is
//! Rig-specific beyond `ToolContext::insert` / `require` / `insert_result`.

use std::fmt;
use std::sync::Arc;

use rig::tool::ToolContext;
use serde::Serialize;
use tenuo::sdk::prelude::*;

/// Authority for one run: the guard that decides and the chain the caller holds.
#[derive(Clone)]
pub struct RunAuthority {
    pub guard: Arc<Guard>,
    pub authority: Arc<PresentedAuthority>,
    pub run_id: String,
}

impl RunAuthority {
    /// A fresh `ToolContext` carrying this authority.
    pub fn context(&self) -> ToolContext {
        let mut ctx = ToolContext::new();
        ctx.insert(self.clone());
        ctx
    }
}

/// Error a guarded tool returns. Denials carry only the sanitized code and message.
#[derive(Debug)]
pub enum ToolError {
    NoAuthority,
    Denied { code: String, message: String },
    Arguments(String),
    Operation(String),
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAuthority => write!(f, "no authority in tool context"),
            Self::Denied { code, message } => write!(f, "denied ({code}): {message}"),
            Self::Arguments(m) => write!(f, "invalid arguments: {m}"),
            Self::Operation(m) => write!(f, "operation failed: {m}"),
        }
    }
}

impl std::error::Error for ToolError {}

/// Run `op` only if the warrant in `ctx` allows `capability` with `args`.
///
/// On allow, the decision record goes into the context's host-only result slot,
/// so the agent host can log or forward it without the model ever seeing it.
pub fn guarded<A, T>(
    ctx: &mut ToolContext,
    capability: &'static str,
    args: &A,
    op: impl FnOnce() -> Result<T, ToolError>,
) -> Result<T, ToolError>
where
    A: Serialize,
{
    let run = ctx
        .get::<RunAuthority>()
        .cloned()
        .ok_or(ToolError::NoAuthority)?;

    let value = serde_json::to_value(args).map_err(|e| ToolError::Arguments(e.to_string()))?;
    let call = Call::try_from_json(capability, &value)
        .map_err(|e| ToolError::Arguments(format!("{e:?}")))?;

    let guarded = run
        .guard
        .guard(&run.authority, &call, |_authorized| op())
        .map_err(|e| match e {
            GuardError::Denied(d) => ToolError::Denied {
                code: d.code().to_string(),
                message: d.message().to_string(),
            },
            GuardError::Operation(e) => e,
        })?;

    ctx.insert_result(guarded.decision.metadata.clone());
    Ok(guarded.into_inner())
}
