# Tenuo + Rig demo

A [Rig](https://rig.rs) agent whose tools are enforced by [Tenuo](https://tenuo.ai) warrants. No adapter crate: authority rides in Rig's `ToolContext`, the Tenuo SDK decides, and an MCP server verifies the chain and proof for itself.

## Run it

```bash
cargo run --bin demo
```

No API key needed. Scripted mode drives the tools directly so every allow, deny, and delegation outcome is deterministic. To also run a real Rig agent loop over the same tools:

```bash
OPENAI_API_KEY=... cargo run --features agent --bin demo
```

## What it shows

1. **Protected tools.** Ordinary Rig tools with the Tenuo check inside `call()`. Because the check is in the tool body, every dispatch path Rig has goes through it.
2. **Delegation that only narrows.** The orchestrator mints a worker that may read one incident and nothing else. A terminal worker cannot delegate further, and the orchestrator cannot hand out a higher replica cap than it holds.
3. **An MCP hop.** A Rig tool calls an MCP server with the warrant in `_meta.tenuo`. The server holds only the root public key and verifies the chain, the proof of possession, and the constraints before the handler runs. It refuses calls with no `_meta`.
4. **A compromised client.** A worker skips its own guard and sends a correctly signed proof over arguments its warrant forbids. The server denies it. Client-side checks are a convenience; the server is the boundary.

## Output

```text
== 1. protected tools, orchestrator authority ==
  allow  -> scaled staging-web to 3 replicas
  deny   -> denied (constraint-violation): Constraint not satisfied
  deny   -> denied (constraint-violation): Constraint not satisfied

== 2. delegation: a worker that may read one incident ==
  allow  -> INC-42: severity=high, status=open, owner=secops
  deny   -> denied (constraint-violation): Constraint not satisfied
  deny   -> denied (tool-not-authorized): Tool not authorized by warrant

== 3. delegation can only narrow ==
  terminal worker cannot delegate -> delegate to incident worker: attenuation would expand capabilities: max_depth 2 exceeds parent's max_depth 1
  orchestrator cannot widen replicas -> delegate scale_cluster: range expanded: child max (50) exceeds parent max (10)

== 4. no authority in context ==
  deny   -> no authority in tool context

== 5. MCP hop: server verifies the chain and proof itself ==
  allow  -> INC-42: severity=high, status=open, owner=secops (served by MCP)
  allow  -> INC-42: severity=high, status=open, owner=secops (served by MCP)
  deny   -> denied (constraint-violation): Constraint not satisfied
  deny   -> server refused call without _meta.tenuo (Mcp error: -32602: missing _meta.tenuo)

== 6. compromised client: skips its own guard, signs a real proof anyway ==
  deny   -> server denied a validly signed but out-of-scope call (Mcp error: -32602: denied (constraint-violation): Constraint not satisfied)
```

## Layout

- `src/authority.rs`: `RunAuthority` carried in `ToolContext`, and `guarded()`, the one helper every tool calls.
- `src/tools.rs`: `scale_cluster` and `read_incident` as Rig tools.
- `src/delegation.rs`: minting narrower children through `Guard::delegate`.
- `src/mcp_tool.rs`: a Rig tool that drives the rmcp client and sets `_meta.tenuo` from an allowed call.
- `src/bin/incident_mcp_server.rs`: the MCP server. Decodes `_meta.tenuo`, then `guard_received` before the handler.

Depends on `tenuo` from `main` until 0.2.4 is published.
