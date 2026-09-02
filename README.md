# Tenuo + Rig demo

An on-call orchestrator agent built with [Rig](https://rig.rs), a worker agent it delegates to, and an [MCP](https://modelcontextprotocol.io) server that verifies every call for itself. Authority comes from [Tenuo](https://tenuo.ai) warrants. No adapter crate.

## Run it

```bash
cargo build --bins && cargo run --bin demo
```

No API key needed. The default build uses scripted completion models, the same pattern Rig uses for its own credential-free examples, so the real agent loop runs deterministically: the model picks tools, Rig dispatches them, denials come back to the model as tool results, and the model reports them. To run the same agents on OpenAI:

```bash
cargo build --bins && OPENAI_API_KEY=... cargo run --features agent --bin demo
```

## Architecture

```text
 control plane (issuer)          agent process                     incident MCP server
 holds the ROOT key              holds its OWN key + a warrant     holds the root PUBLIC key
        |                                |                                  |
        |  mint warrant for orchestrator |                                  |
        +------------------------------->|                                  |
                                         |  orchestrator agent (Rig)        |
                                         |    scale_cluster  (local tool)   |
                                         |    delegate_incident ----+       |
                                         |                          v       |
                                         |  worker agent (Rig), child chain |
                                         |    read_incident --- _meta.tenuo ------> verify chain + PoP + constraints
                                         |                                  |        then run the handler
```

- **The issuer is separate from the agent.** The agent process receives a minted warrant and its own signing key. It never holds the root private key. `src/issuer.rs` stands in for that service so the demo runs in one command; the boundary is the same.
- **Tools are ordinary Rig tools** with the Tenuo check as the first thing `call()` does, so every dispatch path Rig has goes through it. Authority rides in Rig's `ToolContext`.
- **Delegation is a nested agent, not a context swap.** `delegate_incident` mints a narrower child chain through the guard (one incident, five minutes, no further delegation), builds a fresh `ToolContext` holding only that, and prompts the worker agent with it. Rig's `Agent::into_tool()` is not used because it propagates the parent's context, which would hand the worker the orchestrator's full authority.
- **The MCP server is a standard rmcp server** in its own process, configured with nothing but the root public key. It decodes `_meta.tenuo`, verifies the chain, the proof of possession, and the argument constraints, and only then runs the handler. A call with no `_meta.tenuo` is refused.
- **The client-side check is a convenience. The server is the boundary.** The last section plays an attacker holding a stolen worker chain and key, skipping the client guard, and sending a correctly signed proof over arguments the warrant forbids. The server denies it.

## Why the MCP tool is hand-written

Rig's `rmcp_tools()` has a first-class `_meta` channel: a `rmcp::model::Meta` placed in `ToolContext` is forwarded on every call, and Rig documents it as the idiomatic path for auth. It is read from the run's context, so it is fixed before the model chooses arguments, and Rig's pre-tool hook can rewrite arguments but cannot write to the context. Tenuo's envelope carries a proof of possession signed over the exact arguments, which do not exist until the model picks them. So `src/tools/incident_mcp.rs` drives the rmcp client itself: guard first, and only an allowed call produces an envelope. A per-call `Meta` from the pre-tool hook is the upstream change that would let this ride `rmcp_tools()` unchanged.

## Output

```text
== on-call orchestrator ==
   prompt: scale staging-web to 3, then production-web to 20, then investigate INC-42

      [tenuo] allow  orchestrator scale_cluster {"cluster":"staging-web","replicas":3}
      [tenuo] deny   orchestrator scale_cluster {"cluster":"production-web","replicas":20}  (constraint-violation)
      [tenuo] allow  orchestrator delegate_incident {"incident_id":"INC-42"}
      [tenuo] child  worker       read_incident incident_id=INC-42 ttl=300s terminal
      [tenuo] allow  worker       read_incident {"incident_id":"INC-42"}
      [mcp-server] verified read_incident INC-42 for holder 6f09abe4800dfb06 (chain depth 2)
      [tenuo] deny   worker       read_incident {"incident_id":"INC-99"}  (constraint-violation)

   orchestrator: Scaled staging-web to 3. Scaling production-web to 20 was denied by policy. The worker reports INC-42 is open and high severity, and could not read INC-99.

== attacker with a stolen worker chain ==
      [mcp-server] denied   read_incident INC-99 for holder 1e6c3f48d3c754e6: constraint-violation
   server: denied a validly signed call outside the warrant (Mcp error: -32602: denied (constraint-violation): Constraint not satisfied)
      [mcp-server] refused  read_incident INC-42: no _meta.tenuo
   server: refused a call with no warrant at all (Mcp error: -32602: missing _meta.tenuo)
```

`[tenuo]` lines are the client-side guard inside each tool. `[mcp-server]` lines are the server process verifying for itself. Note the worker's read is verified at chain depth 2: it presented its delegated chain, not the orchestrator's.

## Layout

- `src/issuer.rs`: the control plane stand-in. Root key, warrant minting.
- `src/authority.rs`: `RunAuthority` carried in `ToolContext`, and `guarded()`, the one helper every tool calls.
- `src/models.rs`: scripted `CompletionModel`s for the credential-free path.
- `src/tools/scale_cluster.rs`: a local Rig tool.
- `src/tools/delegate_incident.rs`: the agent-as-tool that mints a child chain and runs the worker.
- `src/tools/incident_mcp.rs`: the Rig tool that calls the MCP server with `_meta.tenuo`.
- `src/bin/incident_mcp_server.rs`: the MCP server.

Depends on `tenuo` from `main` (pinned in `Cargo.lock`) until 0.2.4 is published.
