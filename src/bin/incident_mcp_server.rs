//! MCP server for `read_incident`. Verifies
//! `_meta["ai.tenuo/authorization"]` before the handler runs.
//!
//! The server trusts one root public key, passed as TENUO_ROOT_PUBLIC_KEY (hex).
//! It never sees a private key. Every call must carry a warrant chain rooted
//! there plus a proof of possession over these exact arguments.

use std::time::Duration;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerInfo};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler, ServiceExt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tenuo::sdk::prelude::*;
use tenuo::sdk::transport::mcp_meta::decode_meta;
use tenuo::PublicKey;

const TENUO_META_KEY: &str = "ai.tenuo/authorization";

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IncidentArgs {
    pub incident_id: String,
}

#[derive(Clone)]
pub struct IncidentServer {
    guard: std::sync::Arc<Guard>,
}

#[tool_router]
impl IncidentServer {
    pub fn new(guard: Guard) -> Self {
        Self {
            guard: std::sync::Arc::new(guard),
        }
    }

    #[tool(description = "Read an incident record from the security system of record.")]
    async fn read_incident(
        &self,
        ctx: RequestContext<RoleServer>,
        Parameters(args): Parameters<IncidentArgs>,
    ) -> Result<CallToolResult, McpError> {
        // 1. Pull the Tenuo envelope out of _meta. Missing means deny.
        let tenuo = ctx.meta.get(TENUO_META_KEY).cloned().ok_or_else(|| {
            eprintln!(
                "      [mcp-server] refused  read_incident {}: no authorization metadata",
                args.incident_id
            );
            McpError::invalid_params("missing Tenuo authorization metadata", None)
        })?;
        let received = decode_meta(&tenuo)
            .map_err(|e| McpError::invalid_params(format!("bad Tenuo metadata: {e:?}"), None))?;
        let received = received
            .as_received()
            .map_err(|e| McpError::invalid_params(format!("bad authority: {e}"), None))?;

        // 2. Same argument view the client signed over.
        let value = serde_json::to_value(&args)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let call = Call::try_from_json("read_incident", &value)
            .map_err(|e| McpError::invalid_params(format!("{e:?}"), None))?;

        // 3. Verify chain + PoP + constraints, then run the handler once.
        let holder = received.leaf().authorized_holder().fingerprint();
        let depth = received.chain().len();
        let out = match self
            .guard
            .guard_received(&received, &call, |_| {
                eprintln!("      [mcp-server] verified read_incident {} for holder {holder} (chain depth {depth})", args.incident_id);
                Ok::<_, std::convert::Infallible>(format!(
                    "{}: severity=high, status=open, owner=secops (served by MCP)",
                    args.incident_id
                ))
            }) {
            Ok(out) => out,
            Err(e) => match e {
                GuardError::Denied(d) => {
                    eprintln!("      [mcp-server] denied   read_incident {} for holder {holder}: {}", args.incident_id, d.code());
                    return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                        "denied ({}): {}",
                        d.code(),
                        d.message()
                    ))]));
                }
                GuardError::Operation(never) => match never {},
                other => {
                    return Err(McpError::internal_error(other.to_string(), None));
                }
            },
        };

        Ok(CallToolResult::success(vec![ContentBlock::text(
            out.into_inner(),
        )]))
    }
}

#[tool_handler]
impl ServerHandler for IncidentServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.instructions =
            Some("Incident records. Every call must carry a Tenuo warrant in namespaced MCP request metadata.".into());
        info
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let root_hex = std::env::var("TENUO_ROOT_PUBLIC_KEY")
        .map_err(|_| anyhow::anyhow!("TENUO_ROOT_PUBLIC_KEY (hex) is required"))?;
    let bytes = hex::decode(&root_hex)?;
    let arr: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("root key must be 32 bytes"))?;
    let root = PublicKey::from_bytes(&arr)?;

    let guard = Tenuo::enforcement()
        .trusted_root(root)
        .revocation(RevocationMode::TtlOnly {
            max_lifetime: Duration::from_secs(3600),
        })
        // The handler emits the customer-facing MCP verification transcript.
        .denial_reporting(DenialReporting::Debug)
        .build()?;

    let service = IncidentServer::new(guard)
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}
