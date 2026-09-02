//! Tenuo + Rig demo.
//!
//! Scripted mode (default) drives the tools directly, so it needs no API key and
//! shows every allow, deny, and delegation outcome deterministically.
//! Set OPENAI_API_KEY to also run a real Rig agent against the same tools.

mod authority;
mod delegation;
mod mcp_tool;
mod tools;

use std::sync::Arc;
use std::time::Duration;

use rig::tool::Tool;
use tenuo::sdk::prelude::*;
use tenuo::{args, constraints, Exact, Range};
use tenuo::sdk::transport::mcp_meta::encode_meta;
use rmcp::model::{CallToolRequestParams, RequestMetaObject};

use authority::RunAuthority;
use tools::{IncidentArgs, ReadIncident, ScaleArgs, ScaleCluster};
use mcp_tool::RemoteReadIncident;
use rmcp::ServiceExt;
use rmcp::transport::TokioChildProcess;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // In production the root key lives in your control plane and the warrant
    // arrives already minted. Both are generated here so the demo runs anywhere.
    let root = SigningKey::generate();
    let orchestrator = SigningKey::generate();

    let warrant = Warrant::builder()
        .capability(
            "scale_cluster",
            constraints! {
                "cluster"  => Pattern::new("staging-*")?,
                "replicas" => Range::max(10.0)?,
            },
        )
        .capability(
            "read_incident",
            constraints! { "incident_id" => Pattern::new("INC-*")? },
        )
        .holder(orchestrator.public_key())
        .ttl(Duration::from_secs(600))
        .build(&root)?;

    let (guard, authority) = Tenuo::local()
        .trusted_root(root.public_key())
        .chain(vec![warrant])
        .signer(orchestrator)
        .revocation(RevocationMode::TtlOnly { max_lifetime: Duration::from_secs(3600) })
        .build()?;

    let run = RunAuthority {
        guard: Arc::new(guard),
        authority: Arc::new(authority),
        run_id: "run-1".into(),
    };

    println!("== 1. protected tools, orchestrator authority ==");
    let mut ctx = run.context();
    show(ScaleCluster.call(&mut ctx, ScaleArgs { cluster: "staging-web".into(), replicas: 3 }).await);
    show(ScaleCluster.call(&mut ctx, ScaleArgs { cluster: "production-web".into(), replicas: 3 }).await);
    show(ScaleCluster.call(&mut ctx, ScaleArgs { cluster: "staging-web".into(), replicas: 50 }).await);

    println!("\n== 2. delegation: a worker that may read one incident ==");
    let worker = delegation::child_for_incident(&run, "INC-42")?;
    let mut wctx = worker.context();
    show(ReadIncident.call(&mut wctx, IncidentArgs { incident_id: "INC-42".into() }).await);
    show(ReadIncident.call(&mut wctx, IncidentArgs { incident_id: "INC-99".into() }).await);
    show(ScaleCluster.call(&mut wctx, ScaleArgs { cluster: "staging-web".into(), replicas: 1 }).await);

    println!("\n== 3. delegation can only narrow ==");
    match delegation::child_for_incident(&worker, "INC-99") {
        Ok(_) => println!("  !! terminal worker delegated"),
        Err(e) => println!("  terminal worker cannot delegate -> {e:#}"),
    }
    match delegation::child_with_replicas(&run, 50.0) {
        Ok(_) => println!("  !! widening was allowed"),
        Err(e) => println!("  orchestrator cannot widen replicas -> {e:#}"),
    }

    println!("\n== 4. no authority in context ==");
    let mut empty = rig::tool::ToolContext::new();
    show(ReadIncident.call(&mut empty, IncidentArgs { incident_id: "INC-42".into() }).await);

    println!("\n== 5. MCP hop: server verifies the chain and proof itself ==");
    // The server gets only the root public key. It never sees a private key.
    let server_bin = std::env::current_exe()?
        .parent()
        .ok_or_else(|| anyhow::anyhow!("no exe dir"))?
        .join("incident-mcp-server");
    let mut cmd = tokio::process::Command::new(server_bin);
    cmd.env("TENUO_ROOT_PUBLIC_KEY", hex::encode(root.public_key().to_bytes()));
    let client = Arc::new(().serve(TokioChildProcess::new(cmd)?).await?);
    let remote = RemoteReadIncident { client: client.clone() };

    let mut ctx = run.context();
    show(remote.call(&mut ctx, IncidentArgs { incident_id: "INC-42".into() }).await);
    let mut wctx = worker.context();
    show(remote.call(&mut wctx, IncidentArgs { incident_id: "INC-42".into() }).await);
    show(remote.call(&mut wctx, IncidentArgs { incident_id: "INC-99".into() }).await);

    // Bypass the tool and call the server with no _meta at all. The server must refuse.
    let raw = client
        .call_tool(rmcp::model::CallToolRequestParams::new("read_incident").with_arguments(
            serde_json::json!({ "incident_id": "INC-42" }).as_object().cloned().unwrap(),
        ))
        .await;
    match raw {
        Ok(r) if r.is_error.unwrap_or(false) => println!("  deny   -> server refused call without _meta.tenuo"),
        Ok(_) => println!("  !! server accepted a call with no warrant"),
        Err(e) => println!("  deny   -> server refused call without _meta.tenuo ({})", e.to_string().lines().next().unwrap_or("")),
    }

    println!("\n== 6. compromised client: skips its own guard, signs a real proof anyway ==");
    // A worker holding a genuine chain and its own key bypasses the client-side
    // check and sends a correctly signed proof over arguments its warrant forbids.
    // Only the server stands between it and the record.
    let rogue = SigningKey::generate();
    let rogue_profile = DelegationProfile::new()
        .capability("read_incident", constraints! { "incident_id" => Exact::new("INC-42") })
        .ttl(Duration::from_secs(300))
        .terminal();
    let rogue_chain = run.guard.delegate_to(&run.authority, &rogue.public_key(), &rogue_profile)?;
    let forbidden = args! { "incident_id" => "INC-99" };
    let proof = rogue_chain.last().unwrap().sign(&rogue, "read_incident", &forbidden)?;
    let mut meta = RequestMetaObject::new();
    meta.insert("tenuo".into(), encode_meta(&rogue_chain, &proof, &[])?);
    let mut params = CallToolRequestParams::new("read_incident")
        .with_arguments(serde_json::json!({ "incident_id": "INC-99" }).as_object().cloned().unwrap());
    params.meta = Some(meta);
    match client.call_tool(params).await {
        Ok(r) if r.is_error.unwrap_or(false) => println!("  deny   -> server denied a validly signed but out-of-scope call"),
        Ok(_) => println!("  !! server allowed an out-of-scope call"),
        Err(e) => println!("  deny   -> server denied a validly signed but out-of-scope call ({})", e.to_string().lines().next().unwrap_or("")),
    }

    #[cfg(feature = "agent")]
    {
        println!("\n== 7. real Rig agent over the same tools ==");
        let openai = rig::providers::openai::Client::from_env();
        let agent = openai
            .agent("gpt-4o")
            .preamble("You operate infrastructure. Use tools; report what happened.")
            .tool(ScaleCluster)
            .tool(ReadIncident)
            .build();
        let answer = agent
            .runner("Scale staging-web to 3 replicas, then try production-web to 20, and tell me what was refused.")
            .tool_context(run.context())
            .run()
            .await?;
        println!("  agent -> {answer}");
    }

    drop(remote);
    if let Ok(service) = Arc::try_unwrap(client) {
        let _ = service.cancel().await;
    }
    Ok(())
}

fn show<T: std::fmt::Display, E: std::fmt::Display>(r: Result<T, E>) {
    match r {
        Ok(v) => println!("  allow  -> {v}"),
        Err(e) => println!("  deny   -> {e}"),
    }
}
