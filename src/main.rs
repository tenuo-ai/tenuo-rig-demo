//! Tenuo + Rig: an on-call orchestrator agent, a worker agent it delegates to,
//! and an MCP server that verifies every call for itself.
//!
//! Default mode uses scripted models (Rig's own credential-free pattern), so the
//! real agent loop runs deterministically with no API key. `--features agent`
//! swaps in OpenAI over the same tools and prompt.

mod authority;
mod issuer;
#[cfg(not(feature = "agent"))]
mod models;
mod tools;

use std::sync::Arc;
use std::time::Duration;

use rig::prelude::*;
#[cfg(not(feature = "agent"))]
use rig::AgentBuilder;
use rmcp::transport::TokioChildProcess;
use rmcp::ServiceExt;
use tenuo::sdk::prelude::*;
use tenuo::sdk::transport::mcp_meta::encode_meta;
use tenuo::{args, constraints, Exact};

use authority::RunAuthority;
use tools::{DelegateIncident, DelegateSubtask, RemoteReadIncident, ScaleCluster, WorkerFactory};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ---- Control plane (a separate service in production) ------------------
    let control_plane = issuer::ControlPlane::start();
    let root_public_key = control_plane.root_public_key();

    // ---- Agent process: gets its own key and a minted warrant, never the root --
    let orchestrator_key = SigningKey::generate();
    let warrant = control_plane.mint_orchestrator_warrant(&orchestrator_key.public_key())?;

    let (guard, authority) = Tenuo::local()
        .trusted_root(root_public_key.clone())
        .chain(vec![warrant])
        .signer(orchestrator_key)
        .revocation(RevocationMode::TtlOnly { max_lifetime: Duration::from_secs(3600) })
        .build()?;
    let run = RunAuthority { guard: Arc::new(guard), authority: Arc::new(authority), agent: "orchestrator".into() };

    // ---- MCP server: separate process, configured with the root public key only --
    let server_bin = std::env::current_exe()?
        .parent()
        .ok_or_else(|| anyhow::anyhow!("no exe dir"))?
        .join("incident-mcp-server");
    if !server_bin.exists() {
        anyhow::bail!("{} not found. Run `cargo build --bins` first.", server_bin.display());
    }
    let mut cmd = tokio::process::Command::new(server_bin);
    cmd.env("TENUO_ROOT_PUBLIC_KEY", hex::encode(root_public_key.to_bytes()));
    let mcp = Arc::new(().serve(TokioChildProcess::new(cmd)?).await?);

    // ---- Agents ---------------------------------------------------------------
    let reader_factory = build_reader_factory(mcp.clone());
    let worker_factory = build_worker_factory(mcp.clone(), reader_factory);
    let orchestrator = build_orchestrator(worker_factory);

    println!("== on-call orchestrator ==");
    println!("   prompt: scale staging-web to 3, then production-web to 20, then investigate INC-42 and INC-43\n");
    let answer = orchestrator
        .prompt("Scale staging-web to 3 replicas, then scale production-web to 20, then investigate INC-42 and INC-43 in parallel and summarize.")
        .tool_context(run.context())
        .tool_concurrency(2)
        .max_turns(10)
        .await?;
    println!("\n   orchestrator: {answer}");

    // ---- Attacker: outside any agent. Stolen chain and key, bypasses the client guard.
    println!("\n== attacker with a stolen worker chain ==");
    let stolen_key = SigningKey::generate();
    let profile = DelegationProfile::new()
        .capability("read_incident", constraints! { "incident_id" => Exact::new("INC-42") })
        .ttl(Duration::from_secs(300))
        .terminal();
    let stolen_chain = run.guard.delegate_to(&run.authority, &stolen_key.public_key(), &profile)?;
    let forbidden = args! { "incident_id" => "INC-99" };
    let proof = stolen_chain.last().unwrap().sign(&stolen_key, "read_incident", &forbidden)?;
    let mut meta = rmcp::model::RequestMetaObject::new();
    meta.insert("tenuo".into(), encode_meta(&stolen_chain, &proof, &[])?);
    let mut params = rmcp::model::CallToolRequestParams::new("read_incident")
        .with_arguments(serde_json::json!({ "incident_id": "INC-99" }).as_object().cloned().unwrap());
    params.meta = Some(meta);
    match mcp.call_tool(params).await {
        Ok(r) if r.is_error.unwrap_or(false) => println!("   server: denied a validly signed call outside the warrant"),
        Ok(_) => println!("   !! server allowed an out-of-scope call"),
        Err(e) => println!("   server: denied a validly signed call outside the warrant ({})", e.to_string().lines().next().unwrap_or("")),
    }
    match mcp.call_tool(rmcp::model::CallToolRequestParams::new("read_incident")
        .with_arguments(serde_json::json!({ "incident_id": "INC-42" }).as_object().cloned().unwrap())).await {
        Ok(r) if r.is_error.unwrap_or(false) => println!("   server: refused a call with no warrant at all"),
        Ok(_) => println!("   !! server accepted a call with no warrant"),
        Err(e) => println!("   server: refused a call with no warrant at all ({})", e.to_string().lines().next().unwrap_or("")),
    }

    drop(orchestrator);
    if let Ok(service) = Arc::try_unwrap(mcp) {
        let _ = service.cancel().await;
    }
    Ok(())
}

type Mcp = Arc<rmcp::service::RunningService<rmcp::service::RoleClient, ()>>;

#[cfg(not(feature = "agent"))]
fn build_reader_factory(mcp: Mcp) -> WorkerFactory {
    use models::{ScriptedModel, Step};
    Arc::new(move |id: &str| {
        let id: &'static str = Box::leak(id.to_owned().into_boxed_str());
        let model = ScriptedModel::new("reader", vec![
            Step::Call { tool: "read_incident", args: serde_json::json!({ "incident_id": id }) },
            Step::Say("Timeline collected."),
        ]);
        AgentBuilder::new(model)
            .preamble("You collect one incident's timeline with read_incident and report.")
            .tool(RemoteReadIncident { client: mcp.clone() })
            .build()
    })
}

#[cfg(not(feature = "agent"))]
fn build_worker_factory(mcp: Mcp, reader_factory: WorkerFactory) -> WorkerFactory {
    use models::{ScriptedModel, Step};
    Arc::new(move |id: &str| {
        let peer = if id == "INC-42" { "INC-43" } else { "INC-42" };
        let (id, peer): (&'static str, &'static str) =
            (Box::leak(id.to_owned().into_boxed_str()), Box::leak(peer.to_owned().into_boxed_str()));
        // Both workers read their own incident, then try the peer's. INC-42's
        // worker also delegates one sub-step, then asks for more than it holds.
        let mut steps = vec![
            Step::Call { tool: "read_incident", args: serde_json::json!({ "incident_id": id }) },
            Step::Call { tool: "read_incident", args: serde_json::json!({ "incident_id": peer }) },
        ];
        if id == "INC-42" {
            steps.push(Step::Call { tool: "delegate_subtask", args: serde_json::json!({ "incident_id": id, "scope": "incident" }) });
            steps.push(Step::Call { tool: "delegate_subtask", args: serde_json::json!({ "incident_id": id, "scope": "all-incidents" }) });
        }
        steps.push(Step::Say("Reported: my incident is open and high severity. The peer incident is outside my authority."));
        let model = ScriptedModel::new("worker", steps);
        AgentBuilder::new(model)
            .preamble("You investigate one incident. Use read_incident, and delegate_subtask for sub-steps. Report what you found and what you could not do.")
            .tool(RemoteReadIncident { client: mcp.clone() })
            .tool(DelegateSubtask { reader_factory: reader_factory.clone() })
            .build()
    })
}

#[cfg(not(feature = "agent"))]
fn build_orchestrator(worker_factory: WorkerFactory) -> Agent {
    use models::{ScriptedModel, Step};
    let model = ScriptedModel::new("orchestrator", vec![
        Step::Call { tool: "scale_cluster", args: serde_json::json!({ "cluster": "staging-web", "replicas": 3 }) },
        Step::Call { tool: "scale_cluster", args: serde_json::json!({ "cluster": "production-web", "replicas": 20 }) },
        Step::Calls(vec![
            ("delegate_incident", serde_json::json!({ "incident_id": "INC-42" })),
            ("delegate_incident", serde_json::json!({ "incident_id": "INC-43" })),
        ]),
        Step::Say("Scaled staging-web to 3. Scaling production-web to 20 was denied by policy. Two workers investigated INC-42 and INC-43 in parallel; each could read only its own incident."),
    ]);
    AgentBuilder::new(model)
        .preamble("You are the on-call orchestrator. Use tools. Never claim an action succeeded if the tool denied it.")
        .tool(ScaleCluster)
        .tool(DelegateIncident { worker_factory })
        .build()
}

#[cfg(feature = "agent")]
fn openai() -> rig::providers::openai::Client {
    let key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY is required for --features agent");
    rig::providers::openai::Client::new(key).expect("openai client")
}

#[cfg(feature = "agent")]
fn build_reader_factory(mcp: Mcp) -> WorkerFactory {
    Arc::new(move |_id: &str| {
        openai()
            .agent(rig::providers::openai::GPT_4O)
            .preamble("You collect one incident's timeline with read_incident and report.")
            .tool(RemoteReadIncident { client: mcp.clone() })
            .build()
    })
}

#[cfg(feature = "agent")]
fn build_worker_factory(mcp: Mcp, reader_factory: WorkerFactory) -> WorkerFactory {
    Arc::new(move |_id: &str| {
        openai()
            .agent(rig::providers::openai::GPT_4O)
            .preamble("You investigate one incident. Use read_incident, and delegate_subtask for sub-steps. Report what you found and what you could not do.")
            .tool(RemoteReadIncident { client: mcp.clone() })
            .tool(DelegateSubtask { reader_factory: reader_factory.clone() })
            .build()
    })
}

#[cfg(feature = "agent")]
fn build_orchestrator(worker_factory: WorkerFactory) -> Agent {
    openai()
        .agent(rig::providers::openai::GPT_4O)
        .preamble("You are the on-call orchestrator. Use tools. Never claim an action succeeded if the tool denied it.")
        .tool(ScaleCluster)
        .tool(DelegateIncident { worker_factory })
        .build()
}
