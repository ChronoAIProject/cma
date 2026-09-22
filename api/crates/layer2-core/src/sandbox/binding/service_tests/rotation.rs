use super::*;

#[tokio::test]
async fn rotation_keeps_environment_and_conversation_generation_and_supports_paused_delivery() {
    let f = fixture();
    let binding = f.ready().await;
    let sandbox = f
        .store
        .get_sandbox(&binding.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let new_key =
        ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
    let next = binding
        .rotate_credential(new_key.sha256(), crate::now_rfc3339())
        .unwrap();
    assert_eq!(next.generation, binding.generation);
    assert_eq!(next.sandbox_id, binding.sandbox_id);
    assert_eq!(next.workspace_id, binding.workspace_id);
    let mut authority = sandbox.credential.current.clone().unwrap();
    authority.generation += 1;
    authority.provider_key_id = format!("provided-{}", new_key.sha256());
    let active = next
        .admit_credential_delivery(&sandbox, authority.clone())
        .unwrap();
    assert_eq!(active.status, SandboxStatus::CredentialRotating);
    assert_eq!(active.credential.current, sandbox.credential.current);
    assert!(!next.credential_delivery_complete(&active));
    let mut paused = sandbox.clone();
    paused.status = SandboxStatus::Paused;
    paused.runtime = None;
    paused.lifecycle.checkpoints.latest = Some(crate::sandbox::lifecycle_test_support::checkpoint(
        1,
        &crate::now_rfc3339(),
    ));
    let rotated = next
        .admit_credential_delivery(&paused, authority.clone())
        .unwrap();
    assert_eq!(rotated.status, SandboxStatus::Paused);
    assert_eq!(rotated.work.step, SandboxWorkStep::None);
    assert_eq!(rotated.credential.current, Some(authority.clone()));
    assert!(next.credential_delivery_complete(&rotated));
    for case in 0..4 {
        let mut invalid = sandbox.clone();
        match case {
            0 => invalid.owner = SubjectId::new("another-owner"),
            1 => invalid.credential.source = SandboxCredentialSource::Managed,
            2 => invalid.credential.pending = Some(authority.clone()),
            _ => invalid
                .work
                .schedule(SandboxWorkStep::Activate, crate::now_rfc3339()),
        }
        assert!(next
            .admit_credential_delivery(&invalid, authority.clone())
            .is_err());
    }
}
