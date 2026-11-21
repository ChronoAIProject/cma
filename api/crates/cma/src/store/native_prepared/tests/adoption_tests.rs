use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn native_prepared_atomic_lifecycle_claim_is_exclusive_replayable_and_preserves_authority() {
    let (client, name, store) = database().await;
    let plan = plan();
    store
        .reconcile_native_prepared_target(Some(&plan), 1, NOW)
        .await
        .unwrap();
    let record = publish_ready(&store).await;
    let request = record
        .payload
        .home
        .as_ref()
        .unwrap()
        .native_pool
        .as_ref()
        .unwrap()
        .request
        .clone();
    assert!(exact_claim_authority(&store, &request).await);
    let mut foreign = request.clone();
    foreign.pool.uid = "replacement".into();
    assert!(!exact_claim_authority(&store, &foreign).await);
    let a = admission(&store, &plan, "owner-a", "a").await;
    let b = admission(&store, &plan, "owner-b", "b").await;
    let (ra, rb) = tokio::join!(
        admit_at(&store, a.clone(), NOW),
        admit_at(&store, b.clone(), NOW)
    );
    let (created_a, created_b) = match (ra.unwrap(), rb.unwrap()) {
        (SandboxAdmissionResult::Created(a), SandboxAdmissionResult::Created(b)) => (a, b),
        _ => panic!("both fresh"),
    };
    assert_ne!(created_a.id, created_b.id);
    let (warm, request) = if created_a.runtime.is_some() {
        (created_a, a)
    } else {
        (created_b, b)
    };
    assert_eq!(warm.id, record.payload.id);
    assert_eq!(warm.owner, request.sandbox.owner);
    assert_eq!(warm.profile, request.sandbox.profile);
    assert!(!warm.credential.delivered);
    assert!(warm.credential.current.is_none());
    assert!(
        !exact_claim_authority(
            &store,
            &record
                .payload
                .home
                .as_ref()
                .unwrap()
                .native_pool
                .as_ref()
                .unwrap()
                .request
        )
        .await
    );

    assert_eq!(
        store.get_sandbox(&warm.id).await.unwrap().unwrap(),
        warm,
        "fresh owned authority follows Claimed read"
    );
    let SandboxAdmissionResult::Existing(replay) = admit_at(&store, request, NOW).await.unwrap()
    else {
        panic!("same operation replays")
    };
    assert_eq!(replay, warm);
    assert_eq!(
        records(&store)
            .await
            .iter()
            .filter(|r| r.ownership.is_some())
            .count(),
        1
    );
    assert!(store
        .consumed_native_prepared_for_sandbox(&warm.id)
        .await
        .unwrap()
        .is_some());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn native_prepared_adoption_waits_for_parent_and_tool_marker_requires_exact_receipt() {
    use base64::Engine as _;
    use layer3_adapter::hosted::HostedAccessToolSource;
    let (client, name, store) = database().await;
    let plan = plan();
    store
        .reconcile_native_prepared_target(Some(&plan), 1, NOW)
        .await
        .unwrap();
    let record = publish_ready(&store).await;
    let request = admission(&store, &plan, "owner", "adoption").await;
    let SandboxOrigin::Lifecycle { operation_id } = request.sandbox.origin.clone() else {
        unreachable!()
    };
    let SandboxAdmissionResult::Created(mut parent) = admit_at(&store, request, NOW).await.unwrap()
    else {
        panic!("fresh")
    };
    let (_, profile) = actual_profiles(&plan);
    let mut candidate = crate::store::agent_access_fixtures::workspace(&parent, "prepared");
    candidate.profile = profile;
    candidate.origin = WorkspaceOrigin::SandboxProfile { index: 0 };
    candidate.name = "workspace".into();
    candidate.path = "/home/cma/workspace".into();
    candidate.status = WorkspaceStatus::Preparing;
    candidate.resolved_commit = None;
    candidate.ready_at = None;
    candidate.work = WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, NOW.into());
    let WorkspaceAdmissionResult::Created(mut workspace) = store
        .admit_workspace(WorkspaceAdmission {
            binding: None,
            workspace: candidate,
            kind: WorkspaceAdmissionKind::SandboxProfile,
            event_at: NOW.into(),
        })
        .await
        .unwrap()
    else {
        panic!("fresh workspace")
    };
    assert_eq!(workspace.id, record.payload.workspaces[0].id);
    let mut input = crate::store::agent_access_fixtures::admission(&workspace, "prepared");
    input.profile.content = plan.warm.workspaces[0].agents[0].content.clone();
    input.profile.digest = content_digest(&input.profile.content).unwrap();
    input.origin = AgentOrigin::Lifecycle { operation_id };
    input.creation_key = format!("lifecycle:{}", content_digest(&input.origin).unwrap());
    assert!(matches!(
        store.admit_agent(input.clone()).await,
        Err(SandboxStoreError::SandboxNotReady)
    ));
    let previous = parent.revision;
    parent.status = SandboxStatus::Installing;
    parent.revision += 1;
    store.save_sandbox(previous, parent.clone()).await.unwrap();
    workspace.status = WorkspaceStatus::Ready;
    workspace.resolved_commit = record.payload.workspaces[0].resolved_commit.clone();
    workspace.ready_at = Some(NOW.into());
    workspace.work.step = WorkspaceWorkStep::None;
    workspace.revision += 1;
    store
        .save_workspace(workspace.revision - 1, workspace.clone())
        .await
        .unwrap();
    parent = store.get_sandbox(&parent.id).await.unwrap().unwrap();
    let previous = parent.revision;
    parent.status = SandboxStatus::Active;
    parent.credential = crate::store::agent_access_fixtures::sandbox("prepared").credential;
    parent.work.step = SandboxWorkStep::WaitForCredentialRotation;
    parent.revision += 1;
    store.save_sandbox(previous, parent.clone()).await.unwrap();
    let admitted = store.admit_agent(input.clone()).await.unwrap();
    let mut agent = admitted.creation().agent.clone();
    assert_eq!(agent.id, record.payload.workspaces[0].agents[0].id);
    assert_eq!(
        agent.work.warm_conversation_id.as_deref(),
        Some("prepared-session")
    );
    assert_ne!(agent.work.step, AgentWorkStep::WaitForWorkspace);
    assert_eq!(
        store.admit_agent(input).await.unwrap().creation().agent.id,
        agent.id
    );
    agent.work.step = AgentWorkStep::OpenConversation;
    agent.work.next_fence = 3;
    agent.work.lease = Some(AgentWorkLease {
        holder: "prepared-worker".into(),
        fence: 2,
        expires_at: layer2_core::rfc3339_at(
            std::time::SystemTime::now() + std::time::Duration::from_secs(300),
        ),
    });
    agent.revision += 1;
    store
        .save_agent(agent.revision - 1, agent.clone())
        .await
        .unwrap();
    let snapshot = store.agent_provenance_snapshot(&agent.id).await.unwrap();
    let open = layer3_adapter::AgentOpenRequest::new(
        format!("agent:{}:open:v1", agent.id),
        crate::sandbox::runtime_options(
            &agent,
            &parent,
            snapshot.resource.lineage.worktree.as_ref().unwrap(),
        ),
        None,
    );
    let runtime =
        layer3_adapter::RuntimeHandle(parent.runtime.as_ref().unwrap().runtime_id.clone());
    let encryption = crate::store::AuthEncryptionKey::from_base64(
        &base64::engine::general_purpose::STANDARD.encode([9; 32]),
    )
    .unwrap();
    let store = std::sync::Arc::new(store);
    let source = crate::store::agent_access_tools::MongoAgentAccessToolSource::new(
        store.clone(),
        "https://control.example.test",
        &encryption,
    )
    .unwrap();
    let binding = source.resolve(&runtime, &open, 2).await.unwrap().unwrap();
    assert!(source
        .confirm_open(&binding, "wrong-conversation")
        .await
        .is_err());
    source
        .confirm_open(&binding, "prepared-session")
        .await
        .unwrap();
    source
        .confirm_open(&binding, "prepared-session")
        .await
        .unwrap();
    // Independent recovery recomputes the ordinary retained receipt after successful handoff.
    let reopened = source.resolve(&runtime, &open, 2).await.unwrap().unwrap();
    assert_ne!(binding, reopened);
    source
        .confirm_open(&reopened, "prepared-session")
        .await
        .unwrap();
    let mut changed = snapshot.clone();
    changed
        .resource
        .lineage
        .agent
        .as_mut()
        .unwrap()
        .work
        .warm_conversation_id = Some("foreign".into());
    let mut session = client.start_session().await.unwrap();
    assert!(
        super::prepared_conversation(&store, &changed, &open, &mut session)
            .await
            .is_err()
    );
    changed = snapshot.clone();
    changed
        .resource
        .lineage
        .sandbox
        .runtime
        .as_mut()
        .unwrap()
        .boot_id = Some("c".repeat(32));
    assert!(
        super::prepared_conversation(&store, &changed, &open, &mut session)
            .await
            .unwrap()
            .is_none()
    );
    client.database(&name).drop().await.unwrap();
}
