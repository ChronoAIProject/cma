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
