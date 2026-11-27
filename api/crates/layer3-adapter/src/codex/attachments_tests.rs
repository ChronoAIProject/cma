use super::*;
use sha2::{Digest as _, Sha256};

#[tokio::test]
async fn attachment_upload_reaches_the_hosted_authority_checks() {
    let (adapter, _) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let request = crate::AgentAttachmentRequest {
        sandbox_id: "sbx_fixture".into(),
        runtime_generation: 1,
        boot_id: "a".repeat(32),
        agent_id: AgentSlot::new("agt_fixture"),
        attachment_id: format!("att_{}", "b".repeat(64)),
        name: "file.txt".into(),
        media_type: "text/plain".into(),
        size_bytes: 0,
        sha256: format!("{:x}", Sha256::digest([])),
    };
    let error = adapter
        .upload_agent_attachment(&RuntimeHandle::new("unadopted"), &request, vec![])
        .await
        .unwrap_err();
    // Provider wrappers must not silently inherit the trait's Unsupported default.
    assert!(matches!(
        error,
        crate::AdapterError::Unavailable(_) | crate::AdapterError::Rejected(_)
    ));
}

#[tokio::test]
async fn filesystem_operations_reach_hosted_authority_checks() {
    let (adapter, _) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let handle = RuntimeHandle::new("unadopted");
    let errors = [
        adapter
            .list_agent_files(
                &handle,
                &crate::AgentFileListRequest {
                    owner_access: true,
                    protected_paths: Vec::new(),
                    sandbox_id: "sandbox".into(),
                    runtime_generation: 1,
                    boot_id: "a".repeat(32),
                    path: String::new(),
                },
            )
            .await
            .unwrap_err(),
        adapter
            .read_agent_file(
                &handle,
                &crate::AgentFileReadRequest {
                    owner_access: true,
                    protected_paths: Vec::new(),
                    sandbox_id: "sandbox".into(),
                    runtime_generation: 1,
                    boot_id: "a".repeat(32),
                    home_identity: None,
                    path: "file".into(),
                    max_bytes: 1024,
                },
            )
            .await
            .unwrap_err(),
        adapter
            .write_agent_file(
                &handle,
                &crate::AgentFileWriteRequest {
                    owner_access: true,
                    protected_paths: Vec::new(),
                    sandbox_id: "sandbox".into(),
                    runtime_generation: 1,
                    boot_id: "a".repeat(32),
                    path: "file".into(),
                    expected_sha256: None,
                    content_base64: String::new(),
                    directory: false,
                },
            )
            .await
            .unwrap_err(),
        adapter
            .delete_agent_file(
                &handle,
                &crate::AgentFileDeleteRequest {
                    owner_access: true,
                    protected_paths: Vec::new(),
                    sandbox_id: "sandbox".into(),
                    runtime_generation: 1,
                    boot_id: "a".repeat(32),
                    path: "file".into(),
                    recursive: false,
                },
            )
            .await
            .unwrap_err(),
    ];
    for error in errors {
        assert!(
            matches!(
                error,
                crate::AdapterError::Unavailable(_) | crate::AdapterError::Rejected(_)
            ),
            "{error:?}"
        );
    }
}
