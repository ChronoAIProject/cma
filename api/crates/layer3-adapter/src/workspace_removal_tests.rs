use super::*;

fn request() -> WorkspaceRemovalRequest {
    WorkspaceRemovalRequest {
        runtime: RuntimeStatus {
            agent_filesystem: false,
            agent_attachment_upload: false,
            agent_skill_install: false,
            agent_skill_upgrade: false,
            agent_model_catalog: false,
            agent_browser: false,
            agent_resource_preparation: false,
            agent_process_restart: false,
            exact_github_credential_route: false,
            github_repository_scope: false,
            sandbox_id: "sandbox".into(),
            generation: 2,
            home_identity: "a".repeat(43),
            boot_id: "b".repeat(32),
            installation_ready: true,
            credential_ready: true,
            restore_pending: false,
            agent_processes: 2,
        },
        request: WorkspaceRemoveRequest::new(
            "workspace:wks_target:remove:v1",
            "wks_target",
            "target",
        ),
    }
}

#[test]
fn current_proof_is_correlated_to_writer_and_canonical_request_but_not_sibling_count() {
    let request = request();
    let observation = WorkspaceRemovalObservation {
        runtime: request.runtime.clone(),
        receipt: WorkspaceRemoveReceipt {
            operation_id: request.request.operation_id.clone(),
            request_sha256: request.request.request_sha256.clone(),
            workspace_id: request.request.workspace_id.clone(),
        },
        outcome: WorkspaceRemovalOutcome::CurrentlyAbsent,
    };
    observation.validate_for(&request).unwrap();
    let encoded = serde_json::to_vec(&observation).unwrap();
    assert_eq!(
        serde_json::from_slice::<WorkspaceRemovalObservation>(&encoded).unwrap(),
        observation
    );
    for field in [
        "sandbox",
        "generation",
        "home",
        "boot",
        "credential",
        "restore",
        "operation",
        "digest",
        "workspace",
    ] {
        let mut changed = observation.clone();
        match field {
            "sandbox" => changed.runtime.sandbox_id = "foreign".into(),
            "generation" => changed.runtime.generation += 1,
            "home" => changed.runtime.home_identity = "c".repeat(43),
            "boot" => changed.runtime.boot_id = "d".repeat(32),
            "credential" => changed.runtime.credential_ready = false,
            "restore" => changed.runtime.restore_pending = true,
            "operation" => changed.receipt.operation_id.push_str("other"),
            "digest" => changed.receipt.request_sha256 = "e".repeat(64),
            "workspace" => changed.receipt.workspace_id = "successor".into(),
            _ => unreachable!(),
        }
        assert!(changed.validate_for(&request).is_err(), "{field}");
    }
    let mut sibling = observation;
    sibling.runtime.agent_processes = 1;
    sibling.validate_for(&request).unwrap();
    sibling.outcome = WorkspaceRemovalOutcome::SuccessorPresent;
    sibling.validate_for(&request).unwrap();
    assert_ne!(sibling.outcome, WorkspaceRemovalOutcome::CurrentlyAbsent);
}

#[test]
fn protocol_or_job_specific_removal_ids_cannot_claim_support_or_current_proof() {
    let mut request = request();
    request.request.operation_id = "job:workspace:remove:retry1".into();
    assert!(request.request.has_valid_sha256());
    assert!(request.validate().is_err());
    let mut support = WorkspaceRemovalSupport {
        runtime: request.runtime.clone(),
        protocol_version: WORKSPACE_REMOVAL_PROTOCOL,
    };
    support.validate_for(&request.runtime_request()).unwrap();
    support.protocol_version = 0;
    assert!(support.validate_for(&request.runtime_request()).is_err());
}
