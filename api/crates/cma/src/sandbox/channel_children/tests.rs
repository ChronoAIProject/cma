use super::*;

#[test]
fn setup_receipts_require_exact_request_and_every_successful_command_in_order() {
    let request =
        WorkspaceSetupRequest::new("original-op", "workspace", "checkout", vec!["true".into()]);
    let valid = SetupReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        workspace_id: request.workspace_id.clone(),
        commands: vec![layer3_adapter::CommandReceipt {
            index: 0,
            exit_code: 0,
            stdout_sha256: "a".repeat(64),
            stderr_sha256: "b".repeat(64),
        }],
    };
    assert!(valid_setup(&request, &valid));
    for field in [
        "operation",
        "request",
        "workspace",
        "missing",
        "index",
        "exit",
        "digest",
    ] {
        let mut wrong = valid.clone();
        match field {
            "operation" => wrong.operation_id = "replacement-op".into(),
            "request" => wrong.request_sha256 = "c".repeat(64),
            "workspace" => wrong.workspace_id = "foreign".into(),
            "missing" => wrong.commands.clear(),
            "index" => wrong.commands[0].index = 1,
            "exit" => wrong.commands[0].exit_code = 1,
            _ => wrong.commands[0].stdout_sha256 = "invalid".into(),
        }
        assert!(!valid_setup(&request, &wrong), "{field}");
    }
}
