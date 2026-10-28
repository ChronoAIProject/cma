use super::*;

async fn failed(f: &Fixture) -> ProviderBinding {
    let mut binding = f.ready().await;
    let mut conversation = BindingConversation::new(
        &binding,
        "retired_conversation".into(),
        crate::now_rfc3339(),
    )
    .unwrap();
    f.store
        .save_binding_conversation(binding.revision, None, conversation.clone())
        .await
        .unwrap();
    for state in [
        BindingConversationState::Retiring,
        BindingConversationState::Retired,
    ] {
        let revision = conversation.revision;
        conversation.state = state;
        conversation.revision += 1;
        f.store
            .save_binding_conversation(binding.revision, Some(revision), conversation.clone())
            .await
            .unwrap();
    }
    let mut sandbox = f
        .store
        .get_sandbox(&binding.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let revision = sandbox.revision;
    sandbox.runtime = Some(RuntimeBinding {
        boot_id: None,
        runtime_id: "original-runtime".into(),
        runtime_version: "runtime-v1".into(),
        provision_generation: Some(3),
        bound_at: crate::now_rfc3339(),
    });
    sandbox.lifecycle.runtime_generation_high_water = 3;
    sandbox.revision += 1;
    f.store.save_sandbox(revision, sandbox).await.unwrap();
    let revision = binding.revision;
    binding.state = BindingState::Failed {
        reason: "sandbox provisioning failed".into(),
    };
    binding.advance_revision(crate::now_rfc3339()).unwrap();
    f.store
        .save_binding(Some(revision), binding.clone())
        .await
        .unwrap();
    binding
}

#[tokio::test]
async fn identical_put_recovers_failed_binding_without_changing_hierarchy_or_terminal_conversation()
{
    let f = fixture();
    let binding = failed(&f).await;
    let retired = f
        .store
        .binding_conversation(
            &binding.binding_id,
            binding.generation,
            "retired_conversation",
        )
        .await
        .unwrap();
    let sandbox = f.store.get_sandbox(&binding.sandbox_id).await.unwrap();
    let workspace = f.store.get_workspace(&binding.workspace_id).await.unwrap();
    let keys = f.custody.keys.lock().unwrap().clone();
    let recovered = f.put("binding-one", "Original instruction").await;
    let mut expected = binding.clone();
    expected.state = BindingState::Ready;
    expected.revision = recovered.revision;
    expected.updated_at = recovered.updated_at.clone();
    assert_eq!(recovered, expected);
    assert_eq!(recovered.revision, binding.revision + 1);
    assert_eq!(
        f.put("binding-one", "Original instruction").await,
        recovered
    );
    assert_eq!(
        f.store.get_sandbox(&binding.sandbox_id).await.unwrap(),
        sandbox
    );
    assert_eq!(
        f.store.get_workspace(&binding.workspace_id).await.unwrap(),
        workspace
    );
    assert_eq!(*f.custody.keys.lock().unwrap(), keys);
    assert_eq!(
        f.store
            .binding_conversations(&binding.binding_id, binding.generation, None, 100)
            .await
            .unwrap(),
        vec![retired.unwrap()]
    );
}

#[tokio::test]
async fn recovered_binding_retry_refuses_unsettled_or_changed_authority() {
    let f = fixture();
    let binding = failed(&f).await;
    let sandbox = f
        .store
        .get_sandbox(&binding.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let workspace = f
        .store
        .get_workspace(&binding.workspace_id)
        .await
        .unwrap()
        .unwrap();
    for case in 0..13 {
        let mut b = binding.clone();
        let mut s = sandbox.clone();
        let mut w = workspace.clone();
        match case {
            0 => b.work = BindingWork::ProvisionSandbox,
            1 => b.state = BindingState::Retired,
            2 => {
                b.previous = Some(PreviousBindingEnvironment {
                    generation: 1,
                    sandbox_id: b.sandbox_id.clone(),
                    workspace_id: b.workspace_id.clone(),
                    retire_sandbox: false,
                })
            }
            3 => s.status = SandboxStatus::Failed,
            4 => s
                .work
                .schedule(SandboxWorkStep::Activate, crate::now_rfc3339()),
            5 => s.owner = SubjectId::new("different-owner"),
            6 => s.credential.source = SandboxCredentialSource::Managed,
            7 => s.credential.key_generation += 1,
            8 => {
                s.credential.current.as_mut().unwrap().provider_key_id =
                    format!("provided-{}", "b".repeat(64))
            }
            9 => s.credential.pending = s.credential.current.clone(),
            10 => w.status = WorkspaceStatus::Removed,
            11 => w
                .work
                .schedule(WorkspaceWorkStep::Remove, crate::now_rfc3339()),
            _ => w.sandbox_id = SandboxId::generate(),
        }
        assert!(
            b.retry_recovered_hierarchy(&s, &w, &f.key, crate::now_rfc3339())
                .is_err(),
            "case {case}"
        );
    }
    let mut changed = profile("Changed instruction");
    let digest = format!("sha256:{}", content_digest(&changed).unwrap());
    assert!(f
        .service
        .put(&f.caller, "binding-one", changed.clone(), &digest, &f.key)
        .await
        .is_err());
    changed = profile("Original instruction");
    let digest = format!("sha256:{}", content_digest(&changed).unwrap());
    let key = ProvidedCredential::new("nyx_different".into()).unwrap();
    assert!(f
        .service
        .put(&f.caller, "binding-one", changed.clone(), &digest, &key)
        .await
        .is_err());
    assert!(f
        .service
        .put_with_tool_endpoint(
            &f.caller,
            "binding-one",
            changed.clone(),
            &digest,
            &f.key,
            Some("https://other.example/tools".into())
        )
        .await
        .is_err());
    f.custody.keys.lock().unwrap().clear();
    assert!(f
        .service
        .put(&f.caller, "binding-one", changed, &digest, &f.key)
        .await
        .is_err());
    assert_eq!(f.binding().await, binding);
}

#[tokio::test]
async fn reconciliation_recovers_with_retained_custody_and_pages_past_unrecoverable_bindings() {
    let f = fixture();
    let binding = failed(&f).await;
    let mut stale = binding.clone();
    stale.binding_id = "a-unrecoverable".into();
    stale.sandbox_id = SandboxId::generate();
    stale.workspace_id = crate::WorkspaceId::generate();
    stale.revision = 1;
    stale.generation = 1;
    stale.key_sha256 = "b".repeat(64);
    f.store.save_binding(None, stale.clone()).await.unwrap();
    assert_eq!(
        f.store.pending_bindings(None, 1).await.unwrap(),
        vec![stale.clone()]
    );
    assert_eq!(
        f.store
            .pending_bindings(Some(&stale.binding_id), 1)
            .await
            .unwrap(),
        vec![binding.clone()]
    );
    let terminal = f
        .store
        .binding_conversations(&binding.binding_id, binding.generation, None, 100)
        .await
        .unwrap();
    f.service.reconcile(1).await.unwrap();
    let ready = f.binding().await;
    assert_eq!(ready.state, BindingState::Ready);
    assert_eq!(ready.generation, binding.generation);
    assert_eq!(ready.sandbox_id, binding.sandbox_id);
    assert_eq!(ready.workspace_id, binding.workspace_id);
    assert_eq!(
        f.store.binding(&stale.binding_id).await.unwrap(),
        Some(stale)
    );
    assert_eq!(
        f.store
            .binding_conversations(&binding.binding_id, binding.generation, None, 100)
            .await
            .unwrap(),
        terminal
    );
    f.service.reconcile(1).await.unwrap();
    assert_eq!(f.binding().await, ready);
    f.service
        .retire(&f.caller, &binding.binding_id)
        .await
        .unwrap();
    let retired = f.store.binding(&binding.binding_id).await.unwrap().unwrap();
    assert!(!super::super::store::binding_is_recovery_candidate(
        &retired
    ));
    assert!(f.custody.recover_binding(&retired).await.is_err());
}
