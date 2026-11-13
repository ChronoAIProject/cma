use super::*;

async fn runtime(store: &InMemorySandboxStore, agent: &Agent) {
    let mut sandbox = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let revision = sandbox.revision;
    sandbox.runtime = Some(RuntimeBinding {
        runtime_id: "history-runtime".into(),
        runtime_version: "fixture".into(),
        boot_id: None,
        provision_generation: None,
        bound_at: NOW.into(),
    });
    sandbox.revision += 1;
    store.save_sandbox(revision, sandbox).await.unwrap();
}

#[tokio::test]
async fn history_read_authority_expires_without_renewal_after_io() {
    let (store, _, access, _) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let agent = history(&store).await;
    runtime(&store, &agent).await;
    let reads = service(store).with_resource_access(Arc::new(access));
    let mut owner = context("owner");
    owner.credential_source = CredentialSource::RequestBoundRead {
        expires_at: SystemTime::now() + Duration::from_secs(30),
    };
    let reading = reads
        .begin_history_read_authorized(&owner, None, &agent.id)
        .await
        .unwrap();
    assert_eq!(reading.runtime_id().unwrap(), "history-runtime");
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(31)).await;
    assert!(reads.finish_history_read(reading).await.is_err());
}

#[tokio::test]
async fn history_without_resource_context_rechecks_runtime_and_owner() {
    for change in [false, true] {
        let (store, _, _, _) = setup(Reply::Role(OrganizationRole::Viewer)).await;
        let agent = history(&store).await;
        runtime(&store, &agent).await;
        let reads = service(store.clone());
        assert!(reads
            .begin_history_read(&context("foreign").caller, &agent.id)
            .await
            .is_err());
        let reading = reads
            .begin_history_read(&context("owner").caller, &agent.id)
            .await
            .unwrap();
        if change {
            let mut sandbox = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
            let revision = sandbox.revision;
            sandbox.runtime.as_mut().unwrap().runtime_id = "another-runtime".into();
            sandbox.revision += 1;
            store.save_sandbox(revision, sandbox).await.unwrap();
        }
        assert_eq!(reads.finish_history_read(reading).await.is_ok(), !change);
    }
}

#[tokio::test]
async fn concurrent_conversation_progress_does_not_invalidate_owner_history() {
    let (store, _, access, _) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let mut agent = history(&store).await;
    runtime(&store, &agent).await;
    let reads = service(store.clone()).with_resource_access(Arc::new(access));
    let reading = reads
        .begin_history_read_authorized(&context("owner"), None, &agent.id)
        .await
        .unwrap();
    let revision = agent.revision;
    agent.title = Some("Updated during export".into());
    agent.revision += 1;
    store.save_agent(revision, agent).await.unwrap();
    reads.finish_history_read(reading).await.unwrap();
}
