use super::*;

#[tokio::test]
async fn recording_session_retains_assembly_capacity_and_pending_identities() {
    let root = IngressBudget::new(8192);
    let plan = |n| AllocationPlan::new("recording session", &[n]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(8192, plan(1024), plan(4096)).unwrap(),
    )
    .unwrap();
    let mut session = RecordingSession::new(Some(policy.clone()));
    let scope = IngressWorkScope::new(
        policy
            .reserve(IngressClass::Transformation, plan(4096))
            .unwrap(),
    );
    let assembly = scope
        .run(async {
            AgentMessageAssembly {
                content: "x".repeat(2048),
                message_id: Some("synthetic-message".into()),
                ..Default::default()
            }
        })
        .await;
    session.replace(assembly).unwrap();
    drop(scope);
    assert_eq!(root.available_bytes(), 4096);
    assert_eq!(session.assembly().content.len(), 2048);
    assert_eq!(
        session.assembly().message_id.as_deref(),
        Some("synthetic-message")
    );
    let invalid = IngressWorkScope::inherit_owned(async { AgentMessageAssembly::default() }).await;
    assert!(session.replace(invalid).is_err());
    assert_eq!(session.assembly().content.len(), 2048);
    drop(session);
    assert_eq!(root.available_bytes(), 8192);
}

#[tokio::test]
async fn disabled_session_has_no_implicit_policy_and_serializes_only_the_dto() {
    let mut session = RecordingSession::new(None);
    let assembly = IngressWorkScope::inherit_owned(async { AgentMessageAssembly::default() }).await;
    session.replace(assembly).unwrap();
    assert!(session.policy().is_none());
    let dto = serde_json::to_value(session.assembly()).unwrap();
    assert_eq!(
        dto,
        serde_json::to_value(AgentMessageAssembly::default()).unwrap()
    );
}

#[tokio::test]
async fn legacy_assembly_mutation_is_unavailable_to_enabled_or_installed_owners() {
    let mut legacy = RecordingSession::new(None);
    legacy.legacy_assembly_mut().unwrap().content = "legacy content".into();
    assert_eq!(legacy.assembly().content, "legacy content");
    let root = IngressBudget::new(8192);
    let plan = |bytes| AllocationPlan::checked("legacy mutation fixture", &[bytes]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), plan(1024), plan(4096)).unwrap(),
    )
    .unwrap();
    let mut enabled = RecordingSession::new(Some(policy.clone()));
    assert!(enabled.legacy_assembly_mut().is_none());
    let scope = IngressWorkScope::new(policy.admit(IngressClass::Data, plan(1024)).unwrap());
    legacy
        .replace(scope.run(async { AgentMessageAssembly::default() }).await)
        .unwrap();
    enabled
        .replace(scope.run(async { AgentMessageAssembly::default() }).await)
        .unwrap();
    assert!(legacy.legacy_assembly_mut().is_none());
    assert!(enabled.legacy_assembly_mut().is_none());
    drop((legacy, enabled, scope));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
