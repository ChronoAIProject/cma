use super::*;
use crate::resource_access::{
    AccessPurpose, ResourceAccessService, ResourceCapability, ResourceTarget,
};

async fn ready_agent(f: &Fixture, owner_kind: &str) -> (ProviderBinding, BindingConversation) {
    let mut value = profile("Keep the existing Agent instruction");
    value["metadata"]["owner"]["kind"] = owner_kind.into();
    let binding = f
        .ready_with_profile(value, Some("https://gateway.example/tools".into()))
        .await;
    let conversation = resource_access::ready_conversation(f, &binding).await;
    let mut worktree = f
        .store
        .get_worktree(&conversation.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let revision = worktree.revision;
    worktree.status = WorktreeStatus::Ready;
    worktree.revision += 1;
    f.store.save_worktree(revision, worktree).await.unwrap();
    let mut agent = f
        .store
        .get_agent(&conversation.agent_id)
        .await
        .unwrap()
        .unwrap();
    let revision = agent.revision;
    agent.status = AgentStatus::Ready;
    agent.ready_at = Some(crate::now_rfc3339());
    agent.conversation.conversation_id = Some("existing-runtime-conversation".into());
    agent.conversation.runtime_generation = Some(1);
    agent.work = AgentWorkState::new(AgentWorkStep::None, crate::now_rfc3339());
    agent.revision += 1;
    f.store.save_agent(revision, agent).await.unwrap();
    (binding, conversation)
}

fn key() -> ProvidedCredential {
    ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap()
}

async fn attach(
    f: &Fixture,
    id: &str,
    agent: &crate::AgentId,
    key: &ProvidedCredential,
    owner_kind: &str,
) -> ProviderBinding {
    let mut value = profile("This must never replace the existing Agent instruction");
    value["metadata"]["owner"]["kind"] = owner_kind.into();
    let digest = format!("sha256:{}", content_digest(&value).unwrap());
    f.service
        .put_existing_agent(&f.caller, id, value, &digest, key, agent.clone())
        .await
        .unwrap()
}

#[tokio::test]
async fn existing_agent_shares_one_mapping_for_all_senders_and_preserves_its_resources() {
    for kind in ["human", "organization"] {
        let f = fixture();
        let (original, original_conversation) = ready_agent(&f, kind).await;
        let sandbox = f.store.get_sandbox(&original.sandbox_id).await.unwrap();
        let workspace = f.store.get_workspace(&original.workspace_id).await.unwrap();
        let worktree = f
            .store
            .get_worktree(&original_conversation.worktree_id)
            .await
            .unwrap();
        let agent = f
            .store
            .get_agent(&original_conversation.agent_id)
            .await
            .unwrap();
        let credentials = f.custody.keys.lock().unwrap().clone();
        let credential = key();
        let binding = attach(
            &f,
            "borrowed-one",
            &original_conversation.agent_id,
            &credential,
            kind,
        )
        .await;
        assert_eq!(binding.level, 1);
        assert_eq!(binding.sandbox_id, original.sandbox_id);
        assert_eq!(binding.workspace_id, original.workspace_id);
        assert!(binding.tool_endpoint.is_none());
        let (first, created) = f
            .service
            .ensure_conversation(&f.caller, "borrowed-one", "sender-one-conversation")
            .await
            .unwrap();
        let (second, replay) = f
            .service
            .ensure_conversation(&f.caller, "borrowed-one", "sender-two-conversation")
            .await
            .unwrap();
        assert!(created);
        assert!(!replay);
        assert_eq!(first, second);
        assert!(first.borrowed);
        assert_eq!(first.state, BindingConversationState::Ready);
        assert_eq!(first.conversation_id, EXISTING_AGENT_CONVERSATION);
        assert_eq!(first.agent_id, original_conversation.agent_id);
        assert_eq!(first.worktree_id, original_conversation.worktree_id);
        let inspected = f
            .service
            .inspect_conversation(&f.caller, "borrowed-one", "unseen-transport-partition")
            .await
            .unwrap();
        assert_eq!(inspected.conversation, first);
        assert!(inspected.agent_status.is_some());
        // A second attached channel can borrow the same Agent without taking
        // over the original channel's native-tool association.
        let other = attach(&f, "borrowed-two", &first.agent_id, &key(), kind).await;
        let (third, _) = f
            .service
            .ensure_conversation(&f.caller, &other.binding_id, "sender-three-conversation")
            .await
            .unwrap();
        assert_eq!(third.agent_id, first.agent_id);
        let access = BindingAccessService::new(f.store.clone());
        let principal = access
            .authenticate(&credential.sha256())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            access
                .resolve(&principal, first.agent_id.as_str())
                .await
                .unwrap()
                .conversation,
            first
        );
        for alias in ["sender-one-conversation", "sender-two-conversation"] {
            assert_eq!(
                access
                    .resolve(&principal, alias)
                    .await
                    .unwrap()
                    .conversation,
                first
            );
        }
        for alias in ["short", "sender/invalid", "agt_foreign"] {
            assert!(matches!(
                access.resolve(&principal, alias).await,
                Err(BindingError::NotFound)
            ));
        }
        assert!(matches!(
            access
                .retained_principal(&binding.binding_id, binding.generation + 1)
                .await,
            Err(BindingError::NotFound)
        ));
        let resources = ResourceAccessService::new(
            f.store.clone(),
            Arc::new(command_tests::LocalAuthority(access.clone())),
        )
        .with_bindings(access.clone());
        let caller = resource_access::context(&principal);
        let target = ResourceTarget::Agent {
            agent_id: first.agent_id.clone(),
        };
        resources
            .authorize(
                &caller,
                None,
                &target,
                ResourceCapability::Interact,
                AccessPurpose::Read,
            )
            .await
            .unwrap();
        assert!(resources
            .authorize(
                &caller,
                None,
                &ResourceTarget::Sandbox {
                    sandbox_id: binding.sandbox_id.clone()
                },
                ResourceCapability::ManageSandbox,
                AccessPurpose::Read
            )
            .await
            .is_err());
        // Replays are idempotent and credential rotation never reaches custody.
        assert_eq!(
            attach(&f, "borrowed-one", &first.agent_id, &credential, kind).await,
            binding
        );
        let rotated = attach(&f, "borrowed-one", &first.agent_id, &key(), kind).await;
        assert_eq!(rotated.work, BindingWork::Idle);
        assert_eq!(rotated.state, BindingState::Ready);
        assert_eq!(rotated.generation, binding.generation);
        assert!(access
            .authenticate(&credential.sha256())
            .await
            .unwrap()
            .is_none());
        assert!(matches!(
            access.resolve(&principal, "sender-one-conversation").await,
            Err(BindingError::NotFound)
        ));
        let rotated_principal = access
            .authenticate(&rotated.key_sha256)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            access
                .resolve(&rotated_principal, "sender-one-conversation")
                .await
                .unwrap()
                .conversation,
            first
        );
        f.service.retire(&f.caller, "borrowed-one").await.unwrap();
        for _ in 0..4 {
            let current = f.service.read(&f.caller, "borrowed-one").await.unwrap();
            f.service.reconcile_binding(&current).await.unwrap();
        }
        let retired = f.service.read(&f.caller, "borrowed-one").await.unwrap();
        assert_eq!(retired.work, BindingWork::Idle);
        assert_eq!(retired.state, BindingState::Retired);
        assert!(matches!(
            access
                .resolve(&rotated_principal, "sender-one-conversation")
                .await,
            Err(BindingError::NotFound)
        ));
        assert_eq!(
            f.store
                .binding_conversation(
                    "borrowed-one",
                    binding.generation,
                    EXISTING_AGENT_CONVERSATION
                )
                .await
                .unwrap()
                .unwrap()
                .state,
            BindingConversationState::Retired
        );
        assert_eq!(f.store.get_agent(&first.agent_id).await.unwrap(), agent);
        assert_eq!(
            f.store.get_worktree(&first.worktree_id).await.unwrap(),
            worktree
        );
        assert_eq!(
            f.store.get_workspace(&binding.workspace_id).await.unwrap(),
            workspace
        );
        assert_eq!(
            f.store.get_sandbox(&binding.sandbox_id).await.unwrap(),
            sandbox
        );
        assert_eq!(*f.custody.keys.lock().unwrap(), credentials);
        assert_eq!(
            f.store
                .binding_conversation(
                    &original.binding_id,
                    original.generation,
                    &original_conversation.conversation_id
                )
                .await
                .unwrap(),
            Some(original_conversation)
        );
    }
}

#[tokio::test]
async fn existing_agent_rejects_wrong_owner_kind_or_subject_and_retargeting() {
    let f = fixture();
    let (_, conversation) = ready_agent(&f, "human").await;
    let credential = key();
    for (kind, subject) in [("organization", "owner"), ("human", "another-owner")] {
        let mut value = profile("Borrow");
        value["metadata"]["owner"] = json!({"kind":kind,"subject":subject});
        let digest = format!("sha256:{}", content_digest(&value).unwrap());
        assert!(matches!(
            f.service
                .put_existing_agent(
                    &f.caller,
                    "borrowed-denied",
                    value,
                    &digest,
                    &credential,
                    conversation.agent_id.clone()
                )
                .await,
            Err(BindingError::NotFound)
        ));
        assert!(f.store.binding("borrowed-denied").await.unwrap().is_none());
    }
    let borrowed = attach(
        &f,
        "borrowed-one",
        &conversation.agent_id,
        &credential,
        "human",
    )
    .await;
    assert!(borrowed
        .reconfigure(
            borrowed.profile.clone(),
            credential.sha256(),
            crate::now_rfc3339()
        )
        .is_err());
    let value = profile("Changed settings");
    let digest = format!("sha256:{}", content_digest(&value).unwrap());
    assert!(matches!(
        f.service
            .put(
                &f.caller,
                "borrowed-one",
                value.clone(),
                &digest,
                &credential
            )
            .await,
        Err(BindingError::Conflict)
    ));
    assert!(matches!(
        f.service
            .put_existing_agent(
                &f.caller,
                "borrowed-one",
                value,
                &digest,
                &credential,
                conversation.agent_id
            )
            .await,
        Err(BindingError::Conflict)
    ));
}

#[tokio::test]
async fn retiring_borrowed_paused_agent_preserves_the_resumable_parent_and_agent() {
    let f = fixture();
    let (original, conversation) = ready_agent(&f, "human").await;
    let borrowed = attach(
        &f,
        "borrowed-paused",
        &conversation.agent_id,
        &key(),
        "human",
    )
    .await;
    f.service
        .ensure_conversation(&f.caller, &borrowed.binding_id, "sender-conversation")
        .await
        .unwrap();
    let mut agent = f
        .store
        .get_agent(&conversation.agent_id)
        .await
        .unwrap()
        .unwrap();
    let revision = agent.revision;
    agent
        .transition(AgentStatus::Paused, crate::now_rfc3339(), None)
        .unwrap();
    f.store.save_agent(revision, agent.clone()).await.unwrap();
    let mut sandbox = f
        .store
        .get_sandbox(&original.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let revision = sandbox.revision;
    sandbox.status = SandboxStatus::Paused;
    sandbox.lifecycle.checkpoints.latest = Some(
        crate::sandbox::lifecycle_test_support::checkpoint(1, &crate::now_rfc3339()),
    );
    sandbox.revision += 1;
    f.store
        .save_sandbox(revision, sandbox.clone())
        .await
        .unwrap();
    f.service
        .retire(&f.caller, &borrowed.binding_id)
        .await
        .unwrap();
    for _ in 0..4 {
        let current = f
            .service
            .read(&f.caller, &borrowed.binding_id)
            .await
            .unwrap();
        f.service.reconcile_binding(&current).await.unwrap();
    }
    assert_eq!(
        f.service
            .read(&f.caller, &borrowed.binding_id)
            .await
            .unwrap()
            .work,
        BindingWork::Idle
    );
    assert_eq!(f.store.get_agent(&agent.id).await.unwrap().unwrap(), agent);
    assert_eq!(
        f.store.get_sandbox(&sandbox.id).await.unwrap().unwrap(),
        sandbox
    );
    assert!(sandbox.revive_eligibility().is_ok());
}
