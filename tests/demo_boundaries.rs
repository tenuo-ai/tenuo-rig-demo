use std::process::{Command, Output};
use std::sync::OnceLock;

struct DemoOutput {
    stdout: String,
    stderr: String,
}

fn demo_output() -> &'static DemoOutput {
    static OUTPUT: OnceLock<DemoOutput> = OnceLock::new();
    OUTPUT.get_or_init(|| {
        let output = Command::new(env!("CARGO_BIN_EXE_demo"))
            .output()
            .expect("run the scripted demo");
        assert_success(&output);
        DemoOutput {
            stdout: String::from_utf8(output.stdout).expect("demo stdout is UTF-8"),
            stderr: String::from_utf8(output.stderr).expect("demo stderr is UTF-8"),
        }
    })
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "demo failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn rig_dispatch_enforces_argument_constraints() {
    let output = demo_output();
    assert!(output.stdout.contains(
        "[tenuo] allow  orchestrator   scale_cluster {\"cluster\":\"staging-web\",\"replicas\":3}"
    ));
    assert!(output.stdout.contains(
        "[tenuo] deny   orchestrator   scale_cluster {\"cluster\":\"production-web\",\"replicas\":20}  (constraint-violation)"
    ));
}

#[test]
fn scripted_model_reports_the_actual_model_visible_denial() {
    let output = demo_output();
    assert!(output.stdout.contains(
        "orchestrator: Staging scale succeeded. Production scale was denied: denied (constraint-violation): Constraint not satisfied."
    ));
}

#[test]
fn delegated_workers_are_isolated_and_cannot_widen_authority() {
    let output = demo_output();
    assert!(output.stdout.contains(
        "worker[INC-42] read_incident {\"incident_id\":\"INC-43\"}  (constraint-violation)"
    ));
    assert!(output.stdout.contains(
        "worker[INC-43] read_incident {\"incident_id\":\"INC-42\"}  (constraint-violation)"
    ));
    assert!(output.stdout.contains("reader[INC-42] holder="));
    assert!(output.stdout.contains("depth=3 ttl=120s terminal"));
    assert!(output.stdout.contains("denied (invalid-attenuation)"));
    assert!(output.stdout.contains("cannot attenuate Exact to Pattern"));
}

#[test]
fn mcp_server_verifies_delegated_calls_independently() {
    let output = demo_output();
    assert!(output
        .stderr
        .contains("[mcp-server] verified read_incident INC-42"));
    assert!(output
        .stderr
        .contains("[mcp-server] verified read_incident INC-43"));
    assert!(output.stderr.contains("chain depth 2"));
    assert!(output.stderr.contains("chain depth 3"));
}

#[test]
fn mcp_server_enforces_holder_binding_scope_and_argument_integrity() {
    let output = demo_output();
    assert!(output
        .stdout
        .contains("server: rejected a copied warrant signed by a different key"));
    assert!(output
        .stderr
        .contains("[mcp-server] denied   read_incident INC-99"));
    assert!(output
        .stdout
        .contains("server: bounded a compromised holder to its warrant"));
    assert!(output
        .stdout
        .contains("server: rejected a proof signed for different arguments"));
    assert!(output
        .stderr
        .contains("[mcp-server] refused  read_incident INC-42: no authorization metadata"));
    assert!(output
        .stdout
        .contains("server: refused a call with no warrant at all"));
}

#[test]
fn demo_discloses_that_identical_replay_is_accepted() {
    let output = demo_output();
    assert!(output
        .stdout
        .contains("server: accepted a valid call from the compromised holder"));
    assert!(output
        .stdout
        .contains("server: accepted an identical replay (demo has no deduplication)"));
}

#[test]
fn demo_does_not_emit_duplicate_sdk_denial_lines() {
    let output = demo_output();
    assert!(!output.stderr.contains("tenuo deny ["));
}
