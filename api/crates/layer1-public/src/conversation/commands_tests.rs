#[path = "commands_tests/approvals.rs"]
mod approvals;
#[path = "commands_tests/binding.rs"]
mod binding;
#[path = "commands_tests/execution.rs"]
mod execution;
#[path = "commands_tests/faults.rs"]
mod faults;
#[path = "commands_tests/fixtures.rs"]
mod fixtures;
#[path = "commands_tests/observation.rs"]
mod observation;
#[path = "commands_tests/refusals.rs"]
mod refusals;
use fixtures::*;
use layer2_core::sandbox::agent_io::{AgentCommand, AgentInput, AgentInputOrder};
use serde_json::json;
use tower::ServiceExt;

#[tokio::test]
async fn agui_working_input_reserves_a_message_before_any_runtime_call() {
    let f = fixture(true).await;
    let request = request(
        AGUI,
        Some("queued-input"),
        json!({"threadId":ID,"runId":"correlation",
        "messages":[{"role":"user","content":"Next turn"}]}),
    );
    let task = tokio::spawn(f.router.clone().oneshot(request));
    let commands = admitted(&f).await;
    task.abort();
    assert_eq!(
        commands.len(),
        1,
        "the protocol must reserve common work before dispatch"
    );
    assert_eq!(
        commands[0].receipt.envelope.command,
        AgentCommand::message("Next turn")
    );
    assert!(
        f.runtime.0.lock().unwrap().is_empty(),
        "the HTTP handler must not steer or send"
    );
    assert!(commands[0].work.dispatch.is_none());
}

#[tokio::test]
async fn responses_first_input_reserves_its_linear_condition_before_any_runtime_call() {
    let f = fixture(false).await;
    let request = request(
        RESPONSES,
        Some("first-input"),
        json!({"conversation":ID,"stream":true,
        "input":"First turn","model":"accepted-but-ignored","metadata":{"tag":"kept"}}),
    );
    let task = tokio::spawn(f.router.clone().oneshot(request));
    let commands = admitted(&f).await;
    task.abort();
    assert_eq!(
        commands.len(),
        1,
        "the protocol must reserve common work before dispatch"
    );
    assert_eq!(
        commands[0].receipt.envelope.command,
        AgentCommand::Message {
            input: AgentInput::text("First turn"),
            order: AgentInputOrder::AfterTurn {
                turn_sequence: None
            },
            metadata: [("tag".into(), "kept".into())].into(),
        }
    );
    assert!(
        f.runtime.0.lock().unwrap().is_empty(),
        "only the common worker may send"
    );
    assert!(commands[0].work.dispatch.is_none());
}
