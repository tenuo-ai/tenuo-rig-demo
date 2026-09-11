# Tenuo authorization for Rig agents and MCP tools

This example shows how to add [Tenuo](https://tenuo.ai) authorization to a Rust agent system built with [Rig](https://rig.rs) and [MCP](https://modelcontextprotocol.io).

An on-call orchestrator delegates work to two parallel agents, and one worker delegates a narrower task to a reader agent. Each agent receives its own key and scoped warrant. Delegated authority can only shrink. Incident reads cross an MCP process boundary, where the server verifies the warrant chain, holder proof, and exact call arguments before running the handler.

The integration uses standard Rig and MCP APIs: ordinary `Tool` implementations, `ToolContext`, `agent.prompt()`, Rig concurrency, the `rmcp` client, and an `rmcp` server over a child-process transport.

## What the example includes

| Component | Demonstrated behavior | Implementation |
|---|---|---|
| Rig orchestration | Agent loop, tool dispatch, parallel workers, and nested agent-as-tool delegation | Working integration |
| Tenuo authorization | Argument constraints, holder-bound warrants, narrowing delegation, depth limits, and expiry | Working integration |
| MCP enforcement | Per-call authorization in `_meta.tenuo`, verified before the operation | Separate child process |
| Issuance | Root authority mints the orchestrator warrant | In-process demo issuer |
| Operations | Cluster scaling and incident reads | Example handlers with no external side effects |
| Model | Deterministic scripted completion by default, with optional OpenAI completions | Selectable at build time |

The repository is an executable integration example rather than a reusable adapter crate. The [production considerations](#production-considerations) describe how the demo setup maps to a deployed system.

## What is Tenuo

Tenuo gives each task only the authority it needs. That authority is a signed **warrant**: which tools may be called, which argument values are allowed, for how long, and which key may use it. The warrant travels with the request across agents, tools, and processes. When one agent delegates to another, the new warrant can only narrow. Whoever executes the action verifies the warrant locally, with nothing but the issuer's public key. Tenuo sits alongside the identity and policy systems you already run; it answers what this task may do right now.

## Run it

Prerequisites: a stable Rust toolchain and Linux or macOS. No API key is needed for the default path.

```bash
cargo build --locked --bins && cargo run --locked --bin demo
```

The default build uses scripted completion models, the same pattern Rig uses for its own credential-free examples. The normal agent loop still runs: the model selects tools, Rig dispatches them, denials return as tool results, and the model reports them. The scripted path keeps the example deterministic and requires no external credentials.

To run the same agents with OpenAI-backed completions:

```bash
cargo build --locked --bins && OPENAI_API_KEY=... cargo run --locked --features agent --bin demo
```

Depends on `tenuo` 0.2.4 from crates.io and `rig` 0.42.

To run the end-to-end security checks:

```bash
cargo test --locked --all-targets
```

The tests execute the complete scripted flow and assert the Rig argument denials, peer isolation, non-widening delegation, independent MCP verification, and server-side rejection of bypass attempts.

## Code walkthrough

1. [`src/main.rs`](src/main.rs) assembles the Rig agent graph, creates the orchestrator authority, and starts the MCP server process.
2. [`src/tools/delegate_incident.rs`](src/tools/delegate_incident.rs) gives each worker authority for one incident.
3. [`src/tools/delegate_subtask.rs`](src/tools/delegate_subtask.rs) creates the terminal reader delegation and demonstrates that a worker cannot widen its scope.
4. [`src/tools/incident_mcp.rs`](src/tools/incident_mcp.rs) constructs per-call MCP authorization after Rig has selected the tool arguments.
5. [`src/bin/incident_mcp_server.rs`](src/bin/incident_mcp_server.rs) reconstructs the received call and verifies it before handling the incident read.
6. [`tests/demo_boundaries.rs`](tests/demo_boundaries.rs) runs the complete flow and asserts the authorization boundaries shown in the transcript.

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

`[tenuo]` lines come from the check inside each tool. `[mcp-server]` lines come from the server process verifying for itself. The two workers' lines interleave because they ran concurrently. Every `holder=` value is a different key. The argument values are printed to make the demo legible; production logging should emit decision metadata and redacted summaries instead of raw tool arguments.

## Tenuo in Rig terms

Four concepts connect Tenuo to Rig in this example.

- **A warrant** is a signed grant: which tools may be called, what argument values are allowed (a `Pattern` on `cluster`, a `Range` on `replicas`), and when it expires. It is bound to a public key. Whoever calls with it must sign each call with the matching private key, so copying the warrant alone is insufficient.
- **A guard** checks one call against a warrant. In this repo the guard runs as the first thing inside `Tool::call()`, through one helper, `guarded()` in `src/authority.rs`. The warrant and the key travel in Rig's `ToolContext`, inserted once per run, so every dispatch path Rig has goes through the check.
- **Delegation** mints a narrower warrant for another agent, signed by the current one. The new warrant can drop tools, tighten constraints, and shorten expiry. It cannot add anything, and the core library refuses the mint if it tries. The list of warrants from the root down is the **chain**; a verifier walks all of it.
- **The MCP server verifies independently.** The client sends its chain and its per-call signature in the request's `_meta`. The server holds only the root public key and checks the chain, the signature, and the argument constraints before the handler runs.

## Architecture

Authority enters at the top and can only shrink on the way down. Every box holds its own key. Every `read_incident` call, from any level, goes to the MCP server at the bottom, which verifies it with nothing but the root public key.

```text
  DEMO ISSUER                             holds the ROOT private key
  (same demo process; separate service in production)
  mints one warrant for the orchestrator
      │
      │  warrant: scale_cluster      cluster=staging-*  replicas<=10
      │           read_incident      incident_id=INC-*
      │           delegate_incident  incident_id=INC-*
      │           delegate_subtask   incident_id=INC-*    ttl 10 min
      ▼
  ORCHESTRATOR AGENT (Rig)                own key · chain depth 1
      │
      │  delegate_incident(INC-42) and delegate_incident(INC-43)
      │  in the same turn, run in parallel
      ├───────────────────────────────────────┐
      ▼                                       ▼
  WORKER AGENT A                          WORKER AGENT B
  own key · chain depth 2                 own key · chain depth 2
  warrant: read_incident  INC-42 only     warrant: read_incident  INC-43 only
           delegate_subtask                        delegate_subtask
           ttl 5 min · may delegate once           ttl 5 min · may delegate once
      │
      │  delegate_subtask(INC-42)
      ▼
  READER AGENT
  own key · chain depth 3 · terminal, cannot delegate
  warrant: read_incident  INC-42 only     ttl 2 min
      │
      │  read_incident(INC-42)
      │  _meta.tenuo = chain + signature over these exact arguments
      ▼
  INCIDENT MCP SERVER (separate process)  holds only the ROOT PUBLIC key
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

The design has three trust positions. The issuer holds the root private key and mints warrants. An agent process holds its own key and warrant. An enforcement point needs only the trusted root public key.

The MCP server runs as a separate child process and receives only the root public key. To keep the example runnable with one command, the demo issuer and agents share the parent process. A production deployment should run issuance as a separate control-plane service and protect its signing keys with an appropriate secrets manager, KMS, or HSM.

## What each scene shows

1. **A constrained tool.** The orchestrator scales `staging-web` to 3 and is denied `production-web`: the warrant allows `staging-*` up to 10 replicas. The denial returns to the model as a tool result, and the model reports it.
2. **Two workers, one turn, disjoint scope.** The orchestrator delegates INC-42 and INC-43 in the same turn under `tool_concurrency(2)`. `delegate_incident` mints each worker its own chain and key, builds a fresh `ToolContext` holding only that, and prompts a new worker agent. Each worker reads its own incident and is denied the other's. Peers with the same role cannot reach each other's data.
3. **A second hop that cannot widen.** The INC-42 worker hands a sub-step to a reader agent through `delegate_subtask`. The reader gets a terminal chain and its own key, and the server verifies it at chain depth 3. The worker then asks for `all-incidents` scope, more than it holds, and the mint is refused before any agent runs: `Exact("INC-42")` cannot become `Pattern("INC-*")`.
4. **The server is the boundary.** The last section constructs a fresh terminal credential equivalent to a compromised worker credential: the attacker controls both its warrant chain and matching holder key. It then skips the client guard and signs a call for an incident the warrant forbids. The server denies it. This models full holder compromise, not theft of a warrant without its key. A call with no `_meta` at all is also refused.

The nested agents use the agent-as-tool pattern with one deliberate difference from `Agent::into_tool()`: that method forwards the parent's `ToolContext`, which would hand a worker the orchestrator's full authority. `delegate_incident` and `delegate_subtask` build a fresh context holding only the narrower chain.

## Carrying authorization to the MCP server

`src/tools/incident_mcp.rs` is a Rig `Tool` that drives the `rmcp` client directly. It runs the guard first; only an allowed call produces the `_meta.tenuo` envelope, which it sets on `CallToolRequestParams`. The server side is a standard `rmcp` server (`src/bin/incident_mcp_server.rs`) that reads `_meta`, verifies the chain, proof, and arguments, and only then runs the operation.

`Guard::guard` accepts a synchronous closure, so this demo creates the signed envelope inside the guard and performs the asynchronous network request afterward. That client check prevents denied calls from producing credentials or reaching the wire, but the independently configured MCP server is the authoritative enforcement point for the remote action. It rejects missing, invalid, stale, expired, or out-of-scope authority even when a caller bypasses the client helper.

Rig 0.42 already forwards an `rmcp::model::Meta` from `ToolContext` as `_meta`, which covers bearer tokens and session ids. That value is read from the run's context before the model chooses arguments. Tenuo's signature covers the exact arguments, so it can only be produced after the model chooses them. Support for per-call `_meta` derived from the chosen call is tracked upstream in [rig#2442](https://github.com/0xPlaygrounds/rig/issues/2442).

## Production considerations

The example prioritizes a small, one-command setup. A production integration should account for the following:

- **Separate issuance from agents.** Keep the root signing key out of the agent process. The in-process `ControlPlane` is present only to make the example self-contained.
- **Enforce at the operation boundary.** The MCP process receives the root public key, reconstructs the call from received arguments, and verifies independently before executing the handler.
- **Do not rely on the client guard as the remote trust boundary.** Client-side denial saves a network call and avoids releasing an envelope. Server verification remains mandatory because callers can bypass or compromise the client.
- **Holder compromise remains bounded, not harmless.** An attacker with both a warrant chain and its holder key can make valid calls within that warrant. Narrow constraints, short lifetimes, delegation depth limits, revocation, and server enforcement bound the damage.
- **Configure revocation for the deployment.** This example uses TTL-only revocation. Systems that require invalidation before expiry should configure a fail-closed revocation source and define its freshness and outage behavior.
- **Add application-level deduplication where exact replay matters.** The current proof scheme is stateless: a captured valid request can be replayed unchanged within its short proof window. Non-idempotent handlers should atomically claim `Warrant::dedup_key(tool, args)` in a shared store before performing the action and retain it for at least `Warrant::dedup_ttl_secs()`.
- **Logs and errors need a disclosure policy.** The transcript prints full arguments and some internal error details for teaching value. Production code should redact arguments by default, expose stable denial codes to callers, and keep diagnostic causes in protected logs.
- **Use one canonical argument view.** The values checked, signed, transmitted, and reconstructed by the verifier must be semantically identical. A production adapter should construct the outbound arguments and authorization metadata together and fail closed on serialization errors.
- **Manage holder-key lifecycle explicitly.** Generated in-memory keys are appropriate for this short-lived run. Long-running services should define holder-key storage, tenant isolation, rotation, and destruction.

Tenuo authorizes actions; it does not sandbox agent code, isolate processes, authenticate users, or replace the surrounding IAM system.

## Adapting the example

Adapting the example to another system requires a few application and deployment choices:

- where root issuance and holder keys should live;
- whether MCP calls are local child processes, remote services, or both;
- which agent-to-agent handoffs need independently constrained authority;
- which tool arguments identify tenant, environment, resource, or operation scope;
- required warrant lifetime, revocation freshness, replay handling, and audit evidence;
- how the integration should run across processes, containers, or Kubernetes workloads.

These choices determine the integration surface without changing the delegation and verification model shown here.

## Layout

- [`src/issuer.rs`](src/issuer.rs): the in-process demo issuer. Root key and warrant minting.
- [`src/authority.rs`](src/authority.rs): `RunAuthority`, carried in `ToolContext`, and `guarded()`, the helper every tool calls.
- [`src/models.rs`](src/models.rs): scripted `CompletionModel`s for the credential-free path.
- [`src/tools/scale_cluster.rs`](src/tools/scale_cluster.rs): a local Rig tool.
- [`src/tools/delegate_incident.rs`](src/tools/delegate_incident.rs): the agent-as-tool that mints a worker's chain and runs a fresh worker agent.
- [`src/tools/delegate_subtask.rs`](src/tools/delegate_subtask.rs): the second hop. A worker mints a terminal reader chain, or is refused when it asks for more than it holds.
- [`src/tools/incident_mcp.rs`](src/tools/incident_mcp.rs): the Rig tool that calls the MCP server with `_meta.tenuo`.
- [`src/bin/incident_mcp_server.rs`](src/bin/incident_mcp_server.rs): the MCP server.
- [`tests/demo_boundaries.rs`](tests/demo_boundaries.rs): end-to-end assertions over the scripted Rig and MCP flow.
- [`.github/workflows/ci.yml`](.github/workflows/ci.yml): formatting, locked builds, tests, optional agent compilation, and a scripted demo smoke run.
