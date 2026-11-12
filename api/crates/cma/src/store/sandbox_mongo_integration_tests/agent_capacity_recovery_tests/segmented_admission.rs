use super::*;
use crate::store::agent_capacity_recovery::{records, RecoveryService};
use crate::store::agent_log_segments::{schema, writer_gate};
use layer2_core::sandbox::agent_capacity_recovery::{
    ports::CommitOutcome,
    service::CapacityRecoveryFeatures,
    state::{RecoveryPhase, RecoveryRequest},
};
use layer2_core::sandbox::store::{AgentLogPublicationIdentity, AgentLogStore};
use layer2_core::sandbox::{RuntimeBinding, SandboxHome};
use layer3_adapter::{HomeVolumePolicy, HomeVolumeReceipt};

fn service(store: &MongoSandboxStore, enabled: bool) -> RecoveryService {
    RecoveryService::new(store.clone(), transactions::access(store))
        .with_features(CapacityRecoveryFeatures { enabled })
}

fn request(agent: &Agent) -> RecoveryRequest {
    RecoveryRequest {
        agent_id: agent.id.clone(),
        creation_key: agent.creation_key.clone(),
        creation_fingerprint: agent.creation_fingerprint.clone(),
        failure_seq: agent.conversation.last_seq,
        requested_at: layer2_core::now_rfc3339(),
    }
}

async fn set_gate(
    store: &MongoSandboxStore,
    name: writer_gate::GateName,
    policy: writer_gate::Policy,
) {
    let db = store.client.database(&store.agents.namespace().db);
    let mut session = store.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let current = writer_gate::read(&db, name, &mut session)
        .await
        .unwrap()
        .expect("fixture gate");
    writer_gate::replace(&db, name, current.revision, policy, &mut session)
        .await
        .unwrap();
    session.commit_transaction().await.unwrap();
}

async fn segmented_capacity_fixture(store: &MongoSandboxStore, suffix: &str) -> Agent {
    let (_, mut worktree, claimed) = claimed_agent_work_fixture(store, suffix).await;
    store
        .append_agent_log(
            &claimed.id,
            NOW.into(),
            AgentLogKind::Created {
                workspace_id: claimed.workspace_id.clone(),
                worktree_id: claimed.worktree_id.clone(),
            },
        )
        .await
        .unwrap();
    let current = store.get_agent(&claimed.id).await.unwrap().unwrap();
    let conversation_id = format!("segmented-recovery-{suffix}");
    let mut claimed = ready_publication_claim(current, &conversation_id);
    claimed.revision += 1;
    replace_agent_fixture(store, &claimed).await;
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut ready = claimed.clone();
    ready
        .work
        .advance(
            "worker",
            fence,
            AgentWorkStep::PublishConnection,
            NOW.into(),
        )
        .unwrap();
    ready
        .transition(AgentStatus::Ready, NOW.into(), None)
        .unwrap();
    ready.conversation.last_seq += 2;
    let logs = vec![
        AgentLogKind::ConversationOpened {
            conversation_id,
            runtime_generation: 1,
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        },
    ];
    let expected_revision = worktree.revision;
    worktree.transition(WorktreeStatus::Ready).unwrap();
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "worker".into(),
            fence,
            agent: ready.clone(),
            worktree: Some(VersionedWorktree {
                expected_revision,
                worktree,
            }),
            appended_at: NOW.into(),
            sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                agent_id: ready.id.clone(),
                from: AgentStatus::Starting,
                to: AgentStatus::Ready,
            }),
            agent_logs: logs,
        })
        .await
        .unwrap();

    let mut sandbox = store.get_sandbox(&ready.sandbox_id).await.unwrap().unwrap();
    sandbox.runtime = Some(RuntimeBinding {
        runtime_id: "segmented-runtime".into(),
        runtime_version: "fixture".into(),
        boot_id: Some("b".repeat(32)),
        provision_generation: Some(1),
        bound_at: NOW.into(),
    });
    sandbox.lifecycle.runtime_generation_high_water = 1;
    let mut home = SandboxHome::reserve(
        &sandbox.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "storage".into(),
            capacity_gib: 64,
            template_name: "empty".into(),
            template_generation: "seed".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "segmented-home".into(),
        storage_generation: "original".into(),
    })
    .unwrap();
    sandbox.lifecycle.home = Some(Box::new(home));
    sandbox.revision += 1;
    store
        .sandboxes
        .update_one(
            doc! {"_id": sandbox.id.as_str()},
            doc! {"$set": crate::store::sandboxes::sandbox_replacement_fields(&sandbox).unwrap()},
        )
        .await
        .unwrap();
    let workspace = store
        .get_workspace(&ready.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let mut worktree = store
        .get_worktree(&ready.worktree_id)
        .await
        .unwrap()
        .unwrap();
    worktree.path = format!("{}/.worktrees/{}", workspace.path, worktree.name);
    worktree.base_commit = workspace.resolved_commit.clone().unwrap();
    worktree.revision += 1;
    replace_worktree_fixture(store, &worktree).await;

    let db = store.client.database(&store.agents.namespace().db);
    schema::migrate(&db).await.unwrap();
    for name in [
        writer_gate::GateName::Segments,
        writer_gate::GateName::CapacityRecovery,
    ] {
        let mut session = store.client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        writer_gate::replace(
            &db,
            name,
            0,
            writer_gate::Policy {
                enabled: true,
                agents: vec![ready.id.to_string()],
                ..Default::default()
            },
            &mut session,
        )
        .await
        .unwrap();
        session.commit_transaction().await.unwrap();
    }
    let identity = AgentLogPublicationIdentity {
        key: "segmented-capacity-adoption".into(),
        fingerprint: [17; 32],
        appended_at: NOW.into(),
    };
    store
        .append_agent_log_keyed(
            &ready.id,
            &identity,
            AgentLogKind::ConversationGap { frames: 1 },
        )
        .await
        .unwrap();
    let mut failed = store.get_agent(&ready.id).await.unwrap().unwrap();
    failed.status = AgentStatus::Failed;
    failed.work.step = AgentWorkStep::None;
    failed.work.lease = None;
    failed.failure = Some(TypedFailure {
        class: "agent_log_capacity_exceeded".into(),
        detail: "segmented fixture".into(),
    });
    failed.revision += 1;
    replace_agent_fixture(store, &failed).await;
    store
        .append_agent_log(
            &failed.id,
            NOW.into(),
            AgentLogKind::WorkStepFailed {
                step: AgentWorkStep::PublishConnection,
                failure: failed.failure.clone().unwrap(),
            },
        )
        .await
        .unwrap();
    let failed = store.get_agent(&failed.id).await.unwrap().unwrap();
    let row = store
        .agents
        .find_one(doc! {"_id": failed.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_i64("document_schema_version").unwrap(), 2);
    assert!(crate::store::agent_log_segments::read_format(&row)
        .unwrap()
        .is_some());
    failed
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    agent: Agent,
    head: Document,
    projection: Document,
    original_publication: Document,
    carrier: Option<Document>,
    publications: u64,
    admissions: u64,
    log_rows: u64,
}

async fn snapshot(store: &MongoSandboxStore, agent: &Agent, request: &RecoveryRequest) -> Snapshot {
    Snapshot {
        agent: store.get_agent(&agent.id).await.unwrap().unwrap(),
        head: store
            .segment_logs
            .heads
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        projection: store
            .segment_logs
            .projections
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        original_publication: store
            .segment_logs
            .publications
            .find_one(doc! {"publication_key": "segmented-capacity-adoption"})
            .await
            .unwrap()
            .unwrap(),
        carrier: records::collection(store)
            .find_one(doc! {"_id": request.key().unwrap()})
            .await
            .unwrap(),
        publications: store
            .segment_logs
            .publications
            .count_documents(doc! {})
            .await
            .unwrap(),
        admissions: store
            .client
            .database(&store.agents.namespace().db)
            .collection::<Document>("agent_log_admissions")
            .count_documents(doc! {})
            .await
            .unwrap(),
        log_rows: store.agent_log.count_documents(doc! {}).await.unwrap(),
    }
}

async fn failpoint(admin: &Client, app: &str, mode: &str) {
    let data = match mode {
        "abort" => doc! {"failCommands": ["update"], "errorCode": 2_i32},
        "unknown" => doc! {
            "failCommands": ["commitTransaction"],
            "errorCode": 50_i32,
            "errorLabels": ["UnknownTransactionCommitResult"]
        },
        _ => panic!("unknown failpoint mode"),
    };
    let mut data = data;
    data.insert("appName", app);
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "alwaysOn", "data": data})
        .await
        .unwrap();
}

async fn disable_failpoint(admin: &Client) {
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
}

async fn failpoint_clients(prefix: &str) -> (Client, Client, String, String) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("cma_{prefix}_{suffix}");
    let app = format!("{prefix}-{suffix}");
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    (admin, Client::with_options(options).unwrap(), name, app)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn segmented_capacity_admission_publishes_successor_and_preserves_witness() {
    let (client, name) = database("capacity_segmented_admit").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = segmented_capacity_fixture(&store, "success").await;
    let caller = Caller::new(
        agent.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let request = request(&agent);
    let before = snapshot(&store, &agent, &request).await;
    let CommitOutcome::Recorded(work) = service(&store, true)
        .admit(&caller, &request)
        .await
        .unwrap()
    else {
        panic!("segmented capacity admission must commit");
    };
    assert_eq!(work.phase, RecoveryPhase::Admitted);
    assert_eq!(work.request, request);
    let after = snapshot(&store, &agent, &request).await;
    assert_eq!(
        after.agent.conversation.last_seq,
        before.agent.conversation.last_seq
    );
    assert_eq!(after.agent.revision, before.agent.revision + 1);
    assert_eq!(
        after.head.get_i64("revision").unwrap(),
        before.head.get_i64("revision").unwrap() + 1
    );
    assert_eq!(
        after.projection.get_i64("through_seq"),
        before.projection.get_i64("through_seq")
    );
    assert_eq!(after.publications, before.publications + 1);
    assert_eq!(after.admissions, before.admissions + 1);
    assert_eq!(after.log_rows, before.log_rows);
    assert_eq!(after.original_publication, before.original_publication);
    let carrier = after.carrier.as_ref().expect("capacity carrier");
    assert_eq!(
        carrier
            .get_document("work")
            .unwrap()
            .get_document("request")
            .unwrap()
            .get_str("requested_at")
            .unwrap(),
        request.requested_at
    );
    assert_eq!(carrier.get_str("agent_id").unwrap(), agent.id.as_str());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn segmented_capacity_admission_abort_keeps_source_and_carrier_atomic() {
    let (admin, client, name, app) = failpoint_clients("capacity-segmented-abort").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = segmented_capacity_fixture(&store, "abort").await;
    let caller = Caller::new(
        agent.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let request = request(&agent);
    let before = snapshot(&store, &agent, &request).await;
    failpoint(&admin, &app, "abort").await;
    let outcome = service(&store, true).admit(&caller, &request).await;
    disable_failpoint(&admin).await;
    assert!(
        outcome.is_err(),
        "an aborted admission cannot be acknowledged"
    );
    assert_eq!(snapshot(&store, &agent, &request).await, before);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn segmented_capacity_admission_unknown_reopens_and_retries_exact_request() {
    let (admin, client, name, app) = failpoint_clients("capacity-segmented-unknown").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = segmented_capacity_fixture(&store, "unknown").await;
    let caller = Caller::new(
        agent.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let request = request(&agent);
    let before = snapshot(&store, &agent, &request).await;
    failpoint(&admin, &app, "unknown").await;
    let first = service(&store, true)
        .admit(&caller, &request)
        .await
        .unwrap();
    disable_failpoint(&admin).await;
    assert_eq!(first, CommitOutcome::Unknown);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    let CommitOutcome::Recorded(work) = service(&reopened, true)
        .admit(&caller, &request)
        .await
        .unwrap()
    else {
        panic!("restart must reconcile or complete the exact request");
    };
    assert_eq!(work.phase, RecoveryPhase::Admitted);
    let after = snapshot(&reopened, &agent, &request).await;
    assert_eq!(
        after.agent.conversation.last_seq,
        before.agent.conversation.last_seq
    );
    assert_eq!(after.publications, before.publications + 1);
    assert_eq!(
        after.carrier.as_ref().unwrap().get_str("agent_id").unwrap(),
        agent.id.as_str()
    );
    assert_eq!(
        service(&reopened, false)
            .admit(&caller, &request)
            .await
            .unwrap(),
        CommitOutcome::Recorded(work)
    );
    assert_eq!(snapshot(&reopened, &agent, &request).await, after);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn segmented_capacity_admission_requires_enabled_matching_gates() {
    let (client, name) = database("capacity_segmented_gates").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = segmented_capacity_fixture(&store, "gates").await;
    let caller = Caller::new(
        agent.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let request = request(&agent);
    let before = snapshot(&store, &agent, &request).await;
    assert_eq!(
        service(&store, false).admit(&caller, &request).await,
        Err(layer2_core::sandbox::agent_capacity_recovery::ports::RecoveryError::Disabled)
    );
    set_gate(
        &store,
        writer_gate::GateName::Segments,
        writer_gate::Policy::default(),
    )
    .await;
    assert!(service(&store, true)
        .admit(&caller, &request)
        .await
        .is_err());
    set_gate(
        &store,
        writer_gate::GateName::Segments,
        writer_gate::Policy {
            enabled: true,
            agents: vec!["other-agent".into()],
            ..Default::default()
        },
    )
    .await;
    assert!(service(&store, true)
        .admit(&caller, &request)
        .await
        .is_err());
    assert_eq!(snapshot(&store, &agent, &request).await, before);
    client.database(&name).drop().await.unwrap();
}
