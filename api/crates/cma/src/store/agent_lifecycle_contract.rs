//! Actual restricted Mongo proves recovery, ownership and stale-worker fences.
use crate::store::{agent_lifecycle_operations::COLLECTION, *};
use layer1_public::rest::AgentLifecycleCreateRequest;
use layer2_core::{Capability, SubjectId};
use mongodb::{
    bson::{doc, DateTime, Document},
    Database,
};

pub(in crate::store) async fn verify(db: &Database) {
    let store = MongoAgentLifecycleStore::open(db).await.unwrap();
    let owner = SubjectId::new("lifecycle-owner");
    let request: AgentLifecycleCreateRequest = serde_json::from_value(serde_json::json!({
        "name": "Research", "image": {"id": "latest-image"}
    }))
    .unwrap();
    let input = LifecycleInput {
        behavior: LifecycleBehavior::PromptRepository,
        managed_agent_instructions: Some(crate::agent_lifecycle::guidance::GITHUB_API_V1.into()),
        setup_message: Some("Original admitted setup message".into()),
        actor: Some(layer2_core::resource_access::ResourceActor::Api {
            subject: owner.clone(),
            delegation_id: Some("original-grant".into()),
        }),
        request: request.clone(),
        resolved_image: Some("pinned-first".into()),
        capacity: Some(layer3_adapter::SandboxCapacity::default()),
        sandbox_availability: Default::default(),
        capabilities: vec![Capability::UseOwnSandboxes],
    };
    let CreateResult::Created(id) = store
        .create(&owner, "exact-intent", input.clone())
        .await
        .unwrap()
    else {
        panic!("new operation")
    };
    // Replay is based on the submitted request, never new catalog resolutions
    // or the ordering/content of capabilities captured by another HTTP attempt.
    let mut changed = input.clone();
    changed.resolved_image = Some("latest-now-points-elsewhere".into());
    changed.capacity = Some(layer3_adapter::SandboxCapacity {
        cpu_millis: 2000,
        memory_mib: 4096,
    });
    changed.capabilities.clear();
    changed.actor = None;
    changed.behavior = LifecycleBehavior::Legacy;
    changed.managed_agent_instructions =
        Some(crate::agent_lifecycle::guidance::managed_v2().into());
    changed.setup_message = Some("A later deployment's setup message".into());
    let CreateResult::Existing(replayed) =
        store.create(&owner, "exact-intent", changed).await.unwrap()
    else {
        panic!("same intent")
    };
    assert_eq!(replayed.id, id);
    assert_eq!(replayed.input.resolved_image, input.resolved_image);
    assert_eq!(replayed.input.capabilities, input.capabilities);
    assert_eq!(replayed.input.capacity, input.capacity);
    assert_eq!(replayed.input.actor, input.actor);
    assert_eq!(replayed.input.behavior, input.behavior);
    assert_eq!(
        replayed.input.managed_agent_instructions,
        input.managed_agent_instructions
    );
    assert_eq!(replayed.input.setup_message, input.setup_message);
    assert_eq!(
        store
            .replay(&owner, "exact-intent", &request)
            .await
            .unwrap()
            .unwrap()
            .id,
        id
    );
    let mut other_request = request.clone();
    other_request.name = "Different".into();
    assert!(store
        .replay(&owner, "exact-intent", &other_request)
        .await
        .is_err());
    let mut contextual = input.clone();
    contextual.request.image = None;
    contextual.request.workspace_id = Some("wk_context".into());
    contextual.resolved_image = None;
    let CreateResult::Created(context_id) = store
        .create(&owner, "contextual-intent", contextual.clone())
        .await
        .unwrap()
    else {
        panic!("new contextual intent")
    };
    let restarted_context = MongoAgentLifecycleStore::open(db).await.unwrap();
    let context_record = restarted_context
        .replay(&owner, "contextual-intent", &contextual.request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(context_record.id, context_id);
    assert_eq!(context_record.input.setup_message, contextual.setup_message);
    assert_eq!(
        context_record.input.request.workspace_id.as_deref(),
        Some("wk_context")
    );
    let mut changed_context = contextual.request.clone();
    changed_context.workspace_id = Some("wk_another".into());
    assert!(restarted_context
        .replay(&owner, "contextual-intent", &changed_context)
        .await
        .is_err());
    changed_context.workspace_id = None;
    assert!(restarted_context
        .replay(&owner, "contextual-intent", &changed_context)
        .await
        .is_err());
    let other_owner = SubjectId::new("another-owner");
    assert!(store
        .read_for_owner(&other_owner, &id)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .replay(&other_owner, "exact-intent", &request)
        .await
        .unwrap()
        .is_none());

    let first = store.claim(&id).await.unwrap().unwrap().lease().unwrap();
    assert!(store.claim(&id).await.unwrap().is_none());
    assert!(!store.pending().await.unwrap().contains(&id));
    store
        .append_event(&first, 1, "sandbox_ready", "running", "Creating Sandbox")
        .await
        .unwrap();
    store
        .mark_sandbox(&first, "original-sandbox")
        .await
        .unwrap();
    store
        .append_event(&first, 2, "sandbox_ready", "ready", "Sandbox ready")
        .await
        .unwrap();
    let phase = store.read_for_owner(&owner, &id).await.unwrap().unwrap();
    assert_eq!(
        phase.status, "running",
        "phase ready is not operation completion"
    );
    assert_eq!(phase.events[1].status, "ready");
    assert!(phase.agent_id.is_none());

    let rows = db.collection::<Document>(COLLECTION);
    rows.update_one(
        doc! {"_id": &id},
        doc! {"$set": {"lease_expires_at": DateTime::from_millis(i64::MAX / 2)}},
    )
    .await
    .unwrap();
    store.renew(&first).await.unwrap();
    let renewed = store.read_for_owner(&owner, &id).await.unwrap().unwrap();
    assert!(
        renewed.lease_expires_at.unwrap().timestamp_millis()
            < DateTime::now().timestamp_millis() + 100_000
    );

    // A restarted process initially observes a live lease. Later scans reclaim
    // it after expiry without another process restart or client resubmission.
    let restarted = MongoAgentLifecycleStore::open(db).await.unwrap();
    assert!(!restarted.pending().await.unwrap().contains(&id));
    rows.update_one(
        doc! {"_id": &id},
        doc! {"$set": {"status": "ready", "lease_expires_at": DateTime::from_millis(0)}},
    )
    .await
    .unwrap();
    assert!(
        store.renew(&first).await.is_err(),
        "expired owners cannot revive a claim"
    );
    assert!(restarted.pending().await.unwrap().contains(&id));
    let second_record = restarted.claim(&id).await.unwrap().unwrap();
    let second = second_record.lease().unwrap();
    assert_eq!(second_record.input.capacity, input.capacity);
    assert_ne!(first.token, second.token);
    assert_eq!(
        second_record.sandbox_id.as_deref(),
        Some("original-sandbox")
    );
    assert!(store
        .append_event(&first, 3, "failed", "failed", "stale")
        .await
        .is_err());
    assert!(store.mark_sandbox(&first, "wrong-sandbox").await.is_err());
    restarted
        .mark_sandbox(&second, "original-sandbox")
        .await
        .unwrap();
    assert!(matches!(
        restarted.mark_sandbox(&second, "wrong-sandbox").await,
        Err(layer2_core::StoreError::RevisionConflict { .. })
    ));
    assert!(store.mark_agent(&first, "wrong-agent").await.is_err());
    assert!(store.fail(&first, "stale failure").await.is_err());
    assert!(store.succeed(&first, "wrong-agent").await.is_err());
    assert!(store.renew(&first).await.is_err());
    restarted
        .append_event(&second, 1, "sandbox_ready", "running", "replay")
        .await
        .unwrap();
    let replayed = restarted
        .read_for_owner(&owner, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replayed.sandbox_id.as_deref(), Some("original-sandbox"));
    assert_eq!(
        replayed.events.len(),
        2,
        "completed phase gates are not duplicated"
    );
    assert_eq!(replayed.events[1].status, "ready");
    // New workers extend a legacy two-event operation without colliding with
    // fixed sequence numbers or resetting completed gates during recovery.
    for (phase, status) in [
        ("sandbox_ready", "running"),
        ("sandbox_ready", "ready"),
        ("workspace_runtime", "running"),
        ("workspace_runtime", "ready"),
        ("key_scope_ready", "running"),
        ("key_scope_ready", "ready"),
        ("agent_startup", "running"),
        ("agent_startup", "ready"),
    ] {
        restarted
            .record_progress(&second, phase, status, "Observed progress")
            .await
            .unwrap();
    }
    let progressed = restarted
        .read_for_owner(&owner, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(progressed.events.len(), 8);
    assert_eq!(progressed.phase, "agent_startup");
    assert_eq!(
        progressed.events[0].occurred_at,
        replayed.events[0].occurred_at
    );
    assert_eq!(
        progressed
            .events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        (1..=8).collect::<Vec<_>>()
    );
    restarted
        .record_progress(&second, "workspace_runtime", "running", "Replay")
        .await
        .unwrap();
    restarted
        .record_progress(&second, "agent_startup", "ready", "Replay")
        .await
        .unwrap();
    let unchanged = restarted
        .read_for_owner(&owner, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.events.len(), 8);
    assert_eq!(unchanged.phase, "agent_startup");
    assert_eq!(
        unchanged.events[7].occurred_at,
        progressed.events[7].occurred_at
    );
    assert!(store
        .record_progress(&first, "agent_startup", "ready", "Stale worker")
        .await
        .is_err());
    restarted
        .mark_agent(&second, "original-agent")
        .await
        .unwrap();
    restarted.succeed(&second, "original-agent").await.unwrap();
    assert!(restarted.claim(&id).await.unwrap().is_none());
    assert!(restarted.fail(&second, "late failure").await.is_err());
    let finished = restarted
        .read_for_owner(&owner, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(finished.status, "succeeded");
    assert_eq!(finished.agent_id.as_deref(), Some("original-agent"));
    assert!(finished.lease_token.is_none());
    assert!(!restarted.pending().await.unwrap().contains(&id));

    let CreateResult::Created(retry_id) = store
        .create(&owner, "transient-retry", input.clone())
        .await
        .unwrap()
    else {
        panic!("new retry intent")
    };
    let claimed = store.claim(&retry_id).await.unwrap().unwrap();
    assert_eq!(claimed.attempts, 1);
    let lease = claimed.lease().unwrap();
    store.mark_sandbox(&lease, "stable-sandbox").await.unwrap();
    store.mark_agent(&lease, "stable-agent").await.unwrap();
    store.retry(&lease, claimed.attempts).await.unwrap();
    assert!(store.fail(&lease, "stale outcome").await.is_err());
    assert!(store.claim(&retry_id).await.unwrap().is_none());
    assert!(!store.pending().await.unwrap().contains(&retry_id));
    let queued = store
        .read_for_owner(&owner, &retry_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(queued.status, "queued");
    assert!(queued.retry_at.unwrap() > DateTime::now());
    db.collection::<Document>(COLLECTION)
        .update_one(
            doc! {"_id": &retry_id},
            doc! {"$set":{"retry_at":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    let resumed_store = MongoAgentLifecycleStore::open(db).await.unwrap();
    assert!(resumed_store.pending().await.unwrap().contains(&retry_id));
    let next = resumed_store.claim(&retry_id).await.unwrap().unwrap();
    assert_eq!(next.attempts, 2);
    assert_eq!(next.sandbox_id.as_deref(), Some("stable-sandbox"));
    assert_eq!(next.agent_id.as_deref(), Some("stable-agent"));
    assert_eq!(next.input.actor, input.actor);
    resumed_store
        .succeed(&next.lease().unwrap(), "stable-agent")
        .await
        .unwrap();

    let CreateResult::Created(waiting_id) = store
        .create(&owner, "connect-intent", input.clone())
        .await
        .unwrap()
    else {
        panic!("new connection intent")
    };
    let waiting_lease = store
        .claim(&waiting_id)
        .await
        .unwrap()
        .unwrap()
        .lease()
        .unwrap();
    store
        .mark_sandbox(&waiting_lease, "retained-sandbox")
        .await
        .unwrap();
    let target = serde_json::json!({"type":"workspace","id":"owned-workspace"});
    store
        .wait_for_connection(
            &waiting_lease,
            vec!["api-github".into()],
            target.clone(),
            None,
        )
        .await
        .unwrap();
    let waiting = store
        .read_for_owner(&owner, &waiting_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(waiting.status, "waiting_for_connection");
    assert_eq!(waiting.view().connection.unwrap().target, target);
    assert_eq!(waiting.connection.unwrap().services, vec!["api-github"]);
    assert!(!store.pending().await.unwrap().contains(&waiting_id));
    assert!(store.claim(&waiting_id).await.unwrap().is_none());
    store
        .resume_connection(&other_owner, &waiting_id)
        .await
        .unwrap();
    assert!(store.claim(&waiting_id).await.unwrap().is_none());
    store.resume_connection(&owner, &waiting_id).await.unwrap();
    let resumed = store.claim(&waiting_id).await.unwrap().unwrap();
    assert_eq!(resumed.sandbox_id.as_deref(), Some("retained-sandbox"));
    assert!(resumed.connection.is_none());
    assert_eq!(resumed.input.resolved_image, input.resolved_image);
    store
        .succeed(&resumed.lease().unwrap(), "connected-agent")
        .await
        .unwrap();

    // Discovery never materializes an unbounded backlog into worker tasks.
    for index in 0..20 {
        store
            .create(&owner, &format!("backlog-{index}"), input.clone())
            .await
            .unwrap();
    }
    assert_eq!(store.pending().await.unwrap().len(), DISCOVERY_BATCH);
}
