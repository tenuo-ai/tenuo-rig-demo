# Taking the Rig demo to production

The [README](README.md) shows the integration. This page lists what changes when the same design runs as a deployed system.

## Production considerations

The example prioritizes a small, one-command setup. A production integration should account for the following:

- **Separate issuance from agents.** Keep the root signing key out of the agent process. The in-process `ControlPlane` is present only to make the example self-contained.
- **Enforce at the operation boundary.** The MCP process receives the root public key, reconstructs the call from received arguments, and verifies independently before executing the handler.
- **Do not rely on the client guard as the remote trust boundary.** Client-side denial saves a network call and avoids releasing an envelope. Server verification remains mandatory because callers can bypass or compromise the client.
- **Holder compromise remains bounded, not harmless.** An attacker with both a warrant chain and its holder key can make valid calls within that warrant. Narrow constraints, short lifetimes, delegation depth limits, revocation, and server enforcement bound the damage.
- **Configure revocation for the deployment.** This example uses TTL-only revocation. Systems that require invalidation before expiry should configure a fail-closed revocation source and define its freshness and outage behavior.
- **Add application-level deduplication where exact replay matters.** The attacker scene deliberately shows that this stateless, read-only handler accepts a captured valid request again within its short proof window. Non-idempotent handlers should atomically claim `Warrant::dedup_key(tool, args)` in a shared store before performing the action and retain it for at least `Warrant::dedup_ttl_secs()`.
- **Logs and errors need a disclosure policy.** The transcript prints full arguments and some internal error details for teaching value. Production code should redact arguments by default, expose stable denial codes to callers, and keep diagnostic causes in protected logs.
- **Use one canonical argument view.** The values checked, signed, transmitted, and reconstructed by the verifier must be semantically identical. The demo serializes once, rejects unknown fields, constructs the outbound arguments and authorization metadata together, and fails closed on serialization errors.
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
