use super::*;
use layer2_core::sandbox::{
    store::{AgentRestartRequest, AgentRestartSource},
    RuntimeBinding, SandboxHome,
};
use layer2_core::{Authorization, Caller, Capability, OperationClaim};
use layer3_adapter::{
    AgentRestartRuntime, HomeVolumePolicy, HomeVolumeReceipt, RuntimeStatusRequest,
};
use std::time::{Duration, SystemTime};

mod cancellation;
mod publication_tests;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn owner_restart_admission_reserves_one_slot_and_replays_after_supersession() {
    let (client, name) = database("owner_restart_admission").await;
    crate::store::MongoOperationStore::open_for_test(&client, &name)
        .await
        .unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (stopped, source) = stopped_agent(&store).await;
    let first = request(&stopped, &source, "first-restart", [1; 32]);
    let competing = request(&stopped, &source, "second-restart", [1; 32]);

    let mut stale = first.clone();
    stale.observed_process.as_mut().unwrap().runtime.boot_id = "b".repeat(32);
    assert_eq!(
        store.schedule_agent_restart(stale).await,
        Err(AgentControlStoreError::Conflict)
    );

    let (left, right) = tokio::join!(
        store.schedule_agent_restart(first.clone()),
        store.schedule_agent_restart(competing.clone()),
    );
    let (accepted, mut replay) = match (left, right) {
        (Ok(accepted), Err(AgentControlStoreError::Conflict)) => (accepted, first),
        (Err(AgentControlStoreError::Conflict), Ok(accepted)) => (accepted, competing),
        outcomes => panic!("one restart must win the Agent CAS: {outcomes:?}"),
    };
    assert_eq!(accepted.source, source);
    assert_eq!(accepted.next_incarnation, 2);
    let starting = store.get_agent(&stopped.id).await.unwrap().unwrap();
    assert_eq!(starting.status, AgentStatus::Starting);
    assert_eq!(starting.work.step, AgentWorkStep::RestartConversation);
    assert_eq!(starting.revision, stopped.revision + 1);
    assert_eq!(starting.conversation, stopped.conversation);
    assert_eq!(starting.worktree_id, stopped.worktree_id);
    assert_eq!(starting.ready_at, stopped.ready_at);
    let row = store
        .agents
        .find_one(doc! {"_id": stopped.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_str("status").unwrap(), "starting");
    assert!(row.get_bool("work_pending").unwrap());
    assert!(row.get(STOP_RECEIPT_FIELD).is_none());
    assert!(read_control_receipts(&row, &starting).is_ok());

    let mut ready_again = starting;
    ready_again
        .work
        .schedule(AgentWorkStep::None, TWO_MINUTES.into());
    ready_again.conversation.runtime_generation = Some(2);
    ready_again
        .transition(AgentStatus::Ready, TWO_MINUTES.into(), None)
        .unwrap();
    replace_agent_fixture(&store, &ready_again).await;
    store
        .agents
        .update_one(
            doc! {"_id": ready_again.id.as_str()},
            doc! {"$unset": {"control_restart_active_v1": ""}},
        )
        .await
        .unwrap();
    replay.target = AgentControlTarget::from(&ready_again);
    replay.observed_process = None;
    assert_eq!(
        store.schedule_agent_restart(replay.clone()).await.unwrap(),
        accepted
    );

    let mut foreign = replay.clone();
    foreign.caller.subject = SubjectId("different-owner".into());
    assert_eq!(
        store.schedule_agent_restart(foreign).await,
        Err(AgentControlStoreError::NotFound)
    );
    let mut revoked = replay.clone();
    revoked.caller.authorization = Authorization::none();
    assert_eq!(
        store.schedule_agent_restart(revoked).await,
        Err(AgentControlStoreError::NotFound)
    );

    let mut conflict = replay.clone();
    conflict.claim = request(&ready_again, &source, replay.claim.key(), [2; 32]).claim;
    assert_eq!(
        store.schedule_agent_restart(conflict).await,
        Err(AgentControlStoreError::Conflict)
    );
    assert_eq!(
        store.get_agent(&ready_again.id).await.unwrap(),
        Some(ready_again)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn owner_restart_admission_respects_the_shared_creation_capacity() {
    let (client, name) = database("owner_restart_capacity").await;
    crate::store::MongoOperationStore::open_for_test(&client, &name)
        .await
        .unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (stopped, mut source) = stopped_agent(&store).await;
    let mut parent = store
        .get_sandbox(&stopped.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    parent.limits.max_agents_per_workspace = 1;
    parent.profile.content.max_agents_per_workspace = 1;
    parent.profile.digest = layer2_core::profiles::content_digest(&parent.profile.content).unwrap();
    parent.revision += 1;
    store
        .sandboxes
        .update_one(
            doc! {"_id": parent.id.as_str()},
            doc! {"$set": crate::store::sandboxes::sandbox_replacement_fields(&parent).unwrap()},
        )
        .await
        .unwrap();
    source.sandbox_revision = parent.revision;
    let mut workspace = store
        .get_workspace(&stopped.workspace_id)
        .await
        .unwrap()
        .unwrap();
    // The fixture inserted wt-1 directly, without advancing the allocator.
    workspace.next_worktree_number = 2;
    workspace.revision += 1;
    store
        .workspaces
        .update_one(
            doc! {"_id": workspace.id.as_str()},
            doc! {"$set": crate::store::workspaces::workspace_replacement_fields(&workspace).unwrap()},
        )
        .await
        .unwrap();
    source.workspace_revision = workspace.revision;
    let fresh_creation = AgentAdmission {
        binding: None,
        created_by: None,
        owner: stopped.owner.clone(),
        workspace_id: stopped.workspace_id.clone(),
        agent_id: AgentId::generate(),
        worktree_id: WorktreeId::generate(),
        profile: stopped.profile.clone(),
        capabilities: stopped.capabilities.clone(),
        title: None,
        first_message: None,
        origin: AgentOrigin::Manual,
        creation_key: "competing-creation".into(),
        event_at: TWO_MINUTES.into(),
    };

    // Validate ordinary creation in the same initial state, then roll it back
    // before racing the two admissions for the remaining slot.
    let mut session = client.start_session().await.unwrap();
    let mut scope =
        crate::store::agent_log_segments::transaction_scope::TransactionScope::start(&mut session)
            .await
            .unwrap();
    let control = crate::store::agent_log_segments::admission::apply(
        &store,
        &fresh_creation,
        false,
        None,
        &mut scope,
    )
    .await;
    assert!(matches!(
        control,
        Ok(crate::store::agent_admission_records::AdmissionReceipt::Created(_))
    ));
    scope.release().abort_transaction().await.unwrap();

    let (restarted, created) = tokio::join!(
        store.schedule_agent_restart(request(&stopped, &source, "restart", [3; 32])),
        store.admit_agent(fresh_creation),
    );
    match (restarted, created) {
        (Ok(_), Err(SandboxStoreError::AgentLimitReached { limit: 1, live: 1 })) => {}
        (
            Err(
                AgentControlStoreError::Conflict
                | AgentControlStoreError::LimitReached { limit: 1, live: 1 },
            ),
            Ok(_),
        ) => {
            source.sandbox_revision = store
                .get_sandbox(&parent.id)
                .await
                .unwrap()
                .unwrap()
                .revision;
            source.workspace_revision = store
                .get_workspace(&stopped.workspace_id)
                .await
                .unwrap()
                .unwrap()
                .revision;
            assert_eq!(
                store
                    .schedule_agent_restart(request(&stopped, &source, "restart", [3; 32]))
                    .await,
                Err(AgentControlStoreError::LimitReached { limit: 1, live: 1 }),
            );
        }
        (restarted, created) => panic!(
            "expected exactly one live slot reservation, restart {restarted:?}, create: {created:?}"
        ),
    }
    assert_eq!(
        store
            .agents
            .count_documents(doc! {
                "workspace_id": stopped.workspace_id.as_str(),
                "status": {"$in": crate::store::agents::LIVE_STATUSES},
            })
            .await
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn owner_restart_dispatch_is_durable_and_cannot_be_retargeted() {
    use layer2_core::sandbox::store::AgentRestartDispatch;
    use layer3_adapter::{AgentOpenRequest, AgentProcessAction};

    let (client, name) = database("owner_restart_dispatch").await;
    crate::store::MongoOperationStore::open_for_test(&client, &name)
        .await
        .unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (stopped, source) = stopped_agent(&store).await;
    let accepted = store
        .schedule_agent_restart(request(&stopped, &source, "dispatch", [1; 32]))
        .await
        .unwrap();
    let now = SystemTime::now();
    let timestamp = layer2_core::rfc3339_at(now);
    let claimed = store
        .claim_agent_work(
            &stopped.id,
            "restart-worker",
            timestamp.clone(),
            layer2_core::rfc3339_at(now + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap();
    let work = store.agent_restart_work(&claimed).await.unwrap();
    assert_eq!(work.acceptance, accepted);
    assert!(work.dispatched_open.is_none());
    let sandbox = store
        .get_sandbox(&claimed.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let worktree = store
        .get_worktree(&claimed.worktree_id)
        .await
        .unwrap()
        .unwrap();

    let mut open = AgentOpenRequest::new(
        AgentProcessAction::Open.operation_id(claimed.id.as_str(), accepted.next_incarnation),
        crate::sandbox::runtime_options(&claimed, &sandbox, &worktree),
        Some(source.conversation_id.clone()),
    );
    open.restart_runtime = Some(source.runtime.clone());
    let dispatch = AgentRestartDispatch {
        claimed: claimed.clone(),
        work,
        open,
        dispatched_at: timestamp,
    };
    let mut retargeted = dispatch.clone();
    retargeted.open.restart_runtime.as_mut().unwrap().boot_id = "b".repeat(32);
    assert_eq!(
        store.prepare_agent_restart_dispatch(retargeted).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(store.get_agent(&claimed.id).await.unwrap(), Some(claimed));

    let armed = store
        .prepare_agent_restart_dispatch(dispatch.clone())
        .await
        .unwrap();
    let recorded = store.agent_restart_work(&armed).await.unwrap();
    assert_eq!(recorded.acceptance, accepted);
    assert_eq!(recorded.dispatched_open.as_ref(), Some(&dispatch.open));
    assert_eq!(armed.work.step, AgentWorkStep::RestartConversation);
    let [pending] = armed.work.cleanup_debt.as_slice() else {
        panic!("exact successor Open must own cleanup coverage");
    };
    assert_eq!(pending.operation_id, dispatch.open.operation_id);
    assert!(armed.work.receipts.iter().all(|receipt| !matches!(
        receipt.step,
        AgentWorkStep::OpenConversation | AgentWorkStep::CloseProcess
    )));
    assert_eq!(
        store.prepare_agent_restart_dispatch(dispatch.clone()).await,
        Err(SandboxStoreError::Conflict)
    );
    let replay = AgentRestartDispatch {
        claimed: armed.clone(),
        work: recorded,
        ..dispatch
    };
    assert_eq!(
        store
            .prepare_agent_restart_dispatch(replay.clone())
            .await
            .unwrap(),
        armed
    );
    cancellation::exercise_dispatched_teardown(&store, &stopped, replay, &accepted, now).await;
    client.database(&name).drop().await.unwrap();
}

fn request(
    agent: &Agent,
    source: &AgentRestartSource,
    key: &str,
    fingerprint: [u8; 32],
) -> AgentRestartRequest {
    AgentRestartRequest {
        target: AgentControlTarget::from(agent),
        caller: Caller::new(
            agent.owner.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        claim: OperationClaim::new(
            agent.owner.clone(),
            format!("agent:owner-restart:{}", agent.id),
            key,
            fingerprint,
            SystemTime::UNIX_EPOCH,
            Duration::ZERO,
        ),
        observed_process: Some(source.clone()),
        requested_at: TWO_MINUTES.into(),
    }
}

async fn stopped_agent(store: &MongoSandboxStore) -> (Agent, AgentRestartSource) {
    let ready = ready_conversation_fixture(store, "restart-admission").await;
    stop_fixture(store, ready).await
}

async fn stop_fixture(store: &MongoSandboxStore, ready: Agent) -> (Agent, AgentRestartSource) {
    let mut sandbox = store.get_sandbox(&ready.sandbox_id).await.unwrap().unwrap();
    let mut home = SandboxHome::reserve(
        &sandbox.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "fixture-storage".into(),
            capacity_gib: 64,
            template_name: "fixture-seed".into(),
            template_generation: "fixture-generation".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "retained-home".into(),
        storage_generation: "retained-generation".into(),
    })
    .unwrap();
    let runtime = AgentRestartRuntime {
        runtime: RuntimeStatusRequest {
            sandbox_id: sandbox.id.to_string(),
            generation: 1,
            home_identity: home.attachment().unwrap().identity().unwrap(),
        },
        boot_id: "a".repeat(32),
    };
    sandbox.runtime = Some(RuntimeBinding {
        boot_id: Some(runtime.boot_id.clone()),
        runtime_id: "test-runtime".into(),
        runtime_version: "test-version".into(),
        provision_generation: Some(runtime.runtime.generation),
        bound_at: NOW.into(),
    });
    sandbox.lifecycle.home = Some(Box::new(home));
    sandbox.lifecycle.runtime_generation_high_water = runtime.runtime.generation;
    sandbox.revision += 1;
    store
        .sandboxes
        .update_one(
            doc! {"_id": sandbox.id.as_str()},
            doc! {"$set": crate::store::sandboxes::sandbox_replacement_fields(&sandbox).unwrap()},
        )
        .await
        .unwrap();

    let mut stopped = store
        .schedule_agent_stop(AgentStopRequest {
            target: AgentControlTarget::from(&ready),
            reason: AgentStopReason::Owner,
            requested_at: NOW.into(),
        })
        .await
        .unwrap()
        .into_agent();
    let open = stopped
        .work
        .receipts
        .iter()
        .find(|receipt| receipt.step == AgentWorkStep::OpenConversation)
        .unwrap()
        .clone();
    let close = stopped.close_process_request().unwrap();
    let closed = AgentWorkReceipt {
        step: AgentWorkStep::CloseProcess,
        operation_id: close.operation_id,
        request_sha256: close.request_sha256,
        completed_at: MINUTE.into(),
    };
    stopped.work.receipts.push(closed.clone());
    stopped.work.schedule(AgentWorkStep::None, MINUTE.into());
    stopped
        .transition(
            AgentStatus::Stopped,
            MINUTE.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    replace_agent_fixture(store, &stopped).await;
    let workspace = store
        .get_workspace(&stopped.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let worktree = store
        .get_worktree(&stopped.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let source = AgentRestartSource {
        conversation_id: stopped.conversation.conversation_id.clone().unwrap(),
        predecessor_open: open,
        predecessor_close: closed,
        runtime,
        worktree_revision: worktree.revision,
        workspace_revision: workspace.revision,
        sandbox_revision: sandbox.revision,
    };
    (stopped, source)
}
