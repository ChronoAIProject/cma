use super::*;
use layer2_core::agent_ownership::{
    test_support::MemoryOwnershipStore, Authority, Member, OwnershipCoordinator, PROTOCOL_VERSION,
};

async fn coordinator(id: &str) -> Arc<OwnershipCoordinator> {
    OwnershipCoordinator::register(
        MemoryOwnershipStore::default().client(),
        Member {
            instance_id: id.into(),
            protocol: PROTOCOL_VERSION,
            relay_address: format!("http://{id}:8080"),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 16,
        },
    )
    .await
    .unwrap()
}

async fn install(f: &Fixture) -> (Arc<OwnershipCoordinator>, Arc<Authority>) {
    let owner = coordinator("media-target-owner").await;
    let authority = owner.acquire(&f.agent.id).await.unwrap().unwrap();
    f.service.install_ownership(owner.clone()).unwrap();
    (owner, authority)
}

async fn unchanged(f: &Fixture) {
    assert_eq!(
        f.store
            .get_command(&f.parent.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(f.parent.clone())
    );
    assert_eq!(
        f.store.get_agent(&f.agent.id).await.unwrap(),
        Some(f.agent.clone())
    );
    assert_eq!(
        f.store
            .agent_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert!(f
        .store
        .get_media_upload(&MediaUploadRecord::id(
            &f.parent.receipt.envelope.command_id,
            0,
        ))
        .await
        .unwrap()
        .is_none());
    assert_eq!(f.files.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn media_owner_target_requires_exact_binding_path_and_custody_without_writes() {
    let f = Fixture::new("media_target_paths", false).await;
    let (_owner, _authority) = install(&f).await;
    assert_eq!(
        f.service.owner_target(&f.caller, &f.path).await.unwrap(),
        f.agent.id
    );
    for part in ["binding", "conversation", "event", "attachment"] {
        let mut path = f.path.clone();
        match part {
            "binding" => path.binding_id.push_str("-other"),
            "conversation" => path.conversation_id.push_str("-other"),
            "event" => path.event_id.push_str("-other"),
            _ => path.attachment_index = 32,
        }
        assert!(
            matches!(
                f.service.owner_target(&f.caller, &path).await,
                Err(BindingMediaError::Forbidden)
            ),
            "mismatched {part} must not reveal a routing target"
        );
    }
    let mut missing = f.caller.clone();
    missing.credential = None;
    let mut retained = f.caller.clone();
    retained.context.credential_source = CredentialSource::Retained;
    let mut foreign = f.caller.clone();
    let credential =
        ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
    // Binding authorization consumes the authenticated credential digest from
    // context, so keep the presented credential and that digest consistent.
    foreign.context.credential.credential_id = credential.sha256();
    foreign.credential = Some(DelegatedCredential::new(credential.expose().into()));
    for caller in [missing, retained, foreign] {
        assert!(matches!(
            f.service.owner_target(&caller, &f.path).await,
            Err(BindingMediaError::Forbidden)
        ));
    }
    unchanged(&f).await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn media_owner_target_rechecks_retained_binding_revocation() {
    let f = Fixture::new("media_target_revoke", false).await;
    let (_owner, _authority) = install(&f).await;
    assert_eq!(
        f.service.owner_target(&f.caller, &f.path).await.unwrap(),
        f.agent.id
    );
    let mut binding = f.store.binding(&f.path.binding_id).await.unwrap().unwrap();
    let revision = binding.revision;
    binding.state = BindingState::Retired;
    binding.work = BindingWork::RetireCurrentConversations;
    binding
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    f.store.save_binding(Some(revision), binding).await.unwrap();
    assert!(matches!(
        f.service.owner_target(&f.caller, &f.path).await,
        Err(BindingMediaError::Forbidden)
    ));
    unchanged(&f).await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn media_upload_refuses_wrong_agent_foreign_instance_and_draining_member() {
    let f = Fixture::new("media_target_scope", false).await;
    let (owner, authority) = install(&f).await;
    let wrong_agent = owner
        .acquire(&AgentId("unrelated-media-target".into()))
        .await
        .unwrap()
        .unwrap();
    let foreign = coordinator("foreign-media-owner").await;
    let foreign_authority = foreign.acquire(&f.agent.id).await.unwrap().unwrap();
    for denied in [wrong_agent, foreign_authority] {
        assert!(matches!(
            denied.run(f.upload()).await.unwrap(),
            Err(BindingMediaError::Unavailable)
        ));
        unchanged(&f).await;
    }
    owner.stop_admission();
    assert!(authority.is_current());
    assert!(matches!(
        authority.run(f.upload()).await.unwrap(),
        Err(BindingMediaError::Unavailable)
    ));
    unchanged(&f).await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn media_same_owner_replay_preserves_attachment_and_continuation_identity() {
    let f = Fixture::new("media_owner_replay", false).await;
    let (_owner, authority) = install(&f).await;
    let receipt = authority.run(f.upload()).await.unwrap().unwrap();
    let replay = authority.run(f.upload()).await.unwrap().unwrap();
    assert_eq!(receipt, replay);
    assert_eq!(receipt.agent_id, f.agent.id);
    assert_eq!(f.files.writes.load(Ordering::SeqCst), 1);
    let body = f.supplement(&receipt, "same owner transcript");
    let first = authority
        .run(
            f.service
                .supplement(f.caller.clone(), f.path.clone(), body.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    let replay = authority
        .run(f.service.supplement(f.caller.clone(), f.path.clone(), body))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.command, replay.command);
    assert_eq!(first.metadata, replay.metadata);
    assert_ne!(
        first.command.receipt.response_id,
        f.parent.receipt.response_id
    );
    assert_eq!(
        f.store
            .agent_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        f.store
            .get_command(&f.parent.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(f.parent.clone())
    );
    assert_eq!(f.files.writes.load(Ordering::SeqCst), 1);
    f.close().await;
}
