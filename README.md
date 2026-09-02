# Tenuo + Rig demo

A [Rig](https://rig.rs) agent system where every tool call is authorized by a [Tenuo](https://tenuo.ai) warrant: an on-call orchestrator, two worker agents it delegates to in parallel, a reader agent one worker delegates to in turn, and an [MCP](https://modelcontextprotocol.io) server that verifies each call for itself. Everything is standard Rig: ordinary `Tool` impls, `ToolContext`, `agent.prompt()`, and the `rmcp` client.

## What is Tenuo

Tenuo gives each task only the authority it needs. That authority is a signed **warrant**: which tools may be called, which argument values are allowed, for how long, and which key may use it. The warrant travels with the request across agents, tools, and processes. When one agent delegates to another, the new warrant can only narrow. Whoever executes the action verifies the warrant locally, with nothing but the issuer's public key. Tenuo sits alongside the identity and policy systems you already run; it answers what this task may do right now.

## Run it

```bash
cargo build --bins && cargo run --bin demo
```

No API key needed. The default build uses scripted completion models, the same pattern Rig uses for its own credential-free examples. The real agent loop runs: the model picks tools, Rig dispatches them, denials come back to the model as tool results, and the model reports them. To run the same agents on OpenAI:

```bash
cargo build --bins && OPENAI_API_KEY=... cargo run --features agent --bin demo
```

Depends on `tenuo` 0.2.4 from crates.io and `rig` 0.42.

## What you'll see

```text
== on-call orchestrator ==
   prompt: scale staging-web to 3, then production-web to 20, then investigate INC-42 and INC-43

      [tenuo] allow  orchestrator   scale_cluster {"cluster":"staging-web","replicas":3}
      [tenuo] deny   orchestrator   scale_cluster {"cluster":"production-web","replicas":20}  (constraint-violation)
      [tenuo] allow  orchestrator   delegate_incident {"incident_id":"INC-42"}
      [tenuo] child  worker[INC-42] holder=69fba3eb1369ede9 depth=2 ttl=300s may_delegate_to_depth=2
      [tenuo] allow  worker[INC-42] read_incident {"incident_id":"INC-42"}
      [tenuo] allow  orchestrator   delegate_incident {"incident_id":"INC-43"}
      [tenuo] child  worker[INC-43] holder=6e7f3ec036210980 depth=2 ttl=300s may_delegate_to_depth=2
      [tenuo] allow  worker[INC-43] read_incident {"incident_id":"INC-43"}
      [mcp-server] verified read_incident INC-42 for holder 69fba3eb1369ede9 (chain depth 2)
      [tenuo] deny   worker[INC-42] read_incident {"incident_id":"INC-43"}  (constraint-violation)
      [mcp-server] verified read_incident INC-43 for holder 6e7f3ec036210980 (chain depth 2)
      [tenuo] allow  worker[INC-42] delegate_subtask {"incident_id":"INC-42","scope":"incident"}
      [tenuo] child  reader[INC-42] holder=ff77f521325ddf45 depth=3 ttl=120s terminal
      [tenuo] allow  reader[INC-42] read_incident {"incident_id":"INC-42"}
      [tenuo] deny   worker[INC-43] read_incident {"incident_id":"INC-42"}  (constraint-violation)
      [mcp-server] verified read_incident INC-42 for holder ff77f521325ddf45 (chain depth 3)
      [tenuo] allow  worker[INC-42] delegate_subtask {"incident_id":"INC-42","scope":"all-incidents"}
      [tenuo] refuse worker[INC-42] mint for scope=all-incidents: incompatible constraint types: cannot attenuate Exact to Pattern

   orchestrator: Scaled staging-web to 3. Scaling production-web to 20 was denied by policy. Two workers investigated INC-42 and INC-43 in parallel; each could read only its own incident.

== attacker with a stolen worker chain ==
      [mcp-server] denied   read_incident INC-99 for holder 8c7e4fa9433ac335: constraint-violation
   server: denied a validly signed call outside the warrant (Mcp error: -32602: denied (constraint-violation): Constraint not satisfied)
      [mcp-server] refused  read_incident INC-42: no _meta.tenuo
   server: refused a call with no warrant at all (Mcp error: -32602: missing _meta.tenuo)
```

`[tenuo]` lines come from the check inside each tool. `[mcp-server]` lines come from the server process verifying for itself. The two workers' lines interleave because they ran concurrently. Every `holder=` value is a different key.

## Tenuo in Rig terms

If you know Rig, four ideas cover everything in this repo.

- **A warrant** is a signed grant: which tools may be called, what argument values are allowed (a `Pattern` on `cluster`, a `Range` on `replicas`), and when it expires. It is bound to a public key. Whoever calls with it must sign each call with the matching private key, so a copied warrant is useless.
- **A guard** checks one call against a warrant. In this repo the guard runs as the first thing inside `Tool::call()`, through one helper, `guarded()` in `src/authority.rs`. The warrant and the key travel in Rig's `ToolContext`, inserted once per run, so every dispatch path Rig has goes through the check.
- **Delegation** mints a narrower warrant for another agent, signed by the current one. The new warrant can drop tools, tighten constraints, and shorten expiry. It cannot add anything, and the core library refuses the mint if it tries. The list of warrants from the root down is the **chain**; a verifier walks all of it.
- **The MCP server verifies independently.** The client sends its chain and its per-call signature in the request's `_meta`. The server holds only the root public key and checks the chain, the signature, and the argument constraints before the handler runs.

## Architecture

Authority enters at the top and can only shrink on the way down. Every box holds its own key. Every `read_incident` call, from any level, goes to the MCP server at the bottom, which verifies it with nothing but the root public key.

```text
  CONTROL PLANE (issuer)                     holds the ROOT private key
  mints one warrant for the orchestrator
      │
      │  warrant: scale_cluster  cluster=staging-*  replicas<=10
      │           read_incident  incident_id=INC-*
      │           delegate_*     incident_id=INC-*          ttl 10 min
      ▼
  ORCHESTRATOR AGENT (Rig)                   own key · chain depth 1
      │
      ├──── delegate_incident(INC-42) ─────────────────┐   same turn,
      │                                                │   in parallel
      │  warrant: read_incident  incident_id=INC-42    │
      │           delegate_subtask                      │
      │           ttl 5 min · may delegate once more    │
      ▼                                                ▼
  WORKER AGENT A                                  WORKER AGENT B
  own key · chain depth 2                         own key · chain depth 2
  reads INC-42 only                               reads INC-43 only
      │
      │  delegate_subtask(INC-42)
      │  warrant: read_incident  incident_id=INC-42   ttl 2 min · terminal
      ▼
  READER AGENT
  own key · chain depth 3 · cannot delegate
      │
      │  read_incident(INC-42)  +  chain  +  signature over these arguments   (in _meta.tenuo)
      ▼
  INCIDENT MCP SERVER (separate process)     holds only the ROOT PUBLIC key
  verify chain → verify signature → check constraints → run the handler
```

How authority shrinks at each hop:

| | issuer | orchestrator | worker | reader |
|---|---|---|---|---|
| capabilities | mints any | 4 | 2 | 1 |
| incidents | any | `INC-*` | one, exact | one, exact |
| replicas | any | `staging-*`, max 10 | none | none |
| lifetime | root key | 10 min | 5 min | 2 min |
| may delegate | yes | yes | once more | no |
| chain depth | 0 | 1 | 2 | 3 |

Three trust positions hold three different secrets. The issuer holds the root key and mints warrants. The agent process holds its own key and a warrant; it never sees the root key. The MCP server holds only the root public key. `src/issuer.rs` stands in for the control-plane service so the demo runs in one command; the boundary is the same.

## What each scene shows

1. **A constrained tool.** The orchestrator scales `staging-web` to 3 and is denied `production-web`: the warrant allows `staging-*` up to 10 replicas. The denial returns to the model as a tool result, and the model reports it.
2. **Two workers, one turn, disjoint scope.** The orchestrator delegates INC-42 and INC-43 in the same turn under `tool_concurrency(2)`. `delegate_incident` mints each worker its own chain and key, builds a fresh `ToolContext` holding only that, and prompts a new worker agent. Each worker reads its own incident and is denied the other's. Peers with the same role cannot reach each other's data.
3. **A second hop that cannot widen.** The INC-42 worker hands a sub-step to a reader agent through `delegate_subtask`. The reader gets a terminal chain and its own key, and the server verifies it at chain depth 3. The worker then asks for `all-incidents` scope, more than it holds, and the mint is refused before any agent runs: `Exact("INC-42")` cannot become `Pattern("INC-*")`.
4. **The server is the boundary.** The last section plays an attacker holding a stolen worker chain and key. It skips the client guard and sends a correctly signed call for an incident the warrant forbids. The server denies it. A call with no `_meta` at all is refused.

The nested agents use the agent-as-tool pattern with one deliberate difference from `Agent::into_tool()`: that method forwards the parent's `ToolContext`, which would hand a worker the orchestrator's full authority. `delegate_incident` and `delegate_subtask` build a fresh context holding only the narrower chain.

## Carrying the proof to the MCP server

`src/tools/incident_mcp.rs` is a Rig `Tool` that drives the `rmcp` client directly. It runs the guard first; only an allowed call produces the `_meta.tenuo` envelope, which it sets on `CallToolRequestParams`. The server side is a standard `rmcp` server (`src/bin/incident_mcp_server.rs`) that reads `_meta`, verifies, and then runs the handler.

Rig 0.42 already forwards an `rmcp::model::Meta` from `ToolContext` as `_meta`, which covers bearer tokens and session ids. That value is read from the run's context before the model chooses arguments. Tenuo's signature covers the exact arguments, so it can only be produced after the model chooses them. Support for per-call `_meta` derived from the chosen call is tracked upstream in [rig#2442](https://github.com/0xPlaygrounds/rig/issues/2442).

## Layout

- `src/issuer.rs`: the control-plane stand-in. Root key, warrant minting.
- `src/authority.rs`: `RunAuthority`, carried in `ToolContext`, and `guarded()`, the helper every tool calls.
- `src/models.rs`: scripted `CompletionModel`s for the credential-free path.
- `src/tools/scale_cluster.rs`: a local Rig tool.
- `src/tools/delegate_incident.rs`: the agent-as-tool that mints a worker's chain and runs a fresh worker agent.
- `src/tools/delegate_subtask.rs`: the second hop. A worker mints a terminal reader chain, or is refused when it asks for more than it holds.
- `src/tools/incident_mcp.rs`: the Rig tool that calls the MCP server with `_meta.tenuo`.
- `src/bin/incident_mcp_server.rs`: the MCP server.
