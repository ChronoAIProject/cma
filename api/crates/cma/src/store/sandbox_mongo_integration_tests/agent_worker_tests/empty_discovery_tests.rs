use super::*;
mod measurement;
use crate::store::{
    empty_discovery::{Kind, Policy},
    MongoProfileStore, MongoTriggerDefinitionStore,
};
use layer2_core::sandbox::{
    agent_io::AgentCommandWorkerStore, binding::BindingStore, control::SandboxControlStore,
};
use layer2_core::trigger_domain::LaunchStore;

async fn fixture(
    label: &str,
) -> (
    Client,
    String,
    MongoSandboxStore,
    MongoTriggerDefinitionStore,
) {
    let (client, name) = database(label).await;
    crate::store::migrations::prepare_bindings_for_test(&client.database(&name)).await;
    let mut store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let profiles = Arc::new(MongoProfileStore::open(&client, &name).await.unwrap());
    let mut launches = MongoTriggerDefinitionStore::open(&client, &name, profiles)
        .await
        .unwrap();
    let policy = Policy::new(true);
    store.empty_discovery = policy.clone();
    launches.empty_discovery = policy;
    (client, name, store, launches)
}
async fn empty_sweep(store: &MongoSandboxStore, launches: &MongoTriggerDefinitionStore) {
    let page = PageRequest::new(10, None).unwrap();
    let (
        commands,
        controls,
        agents,
        sandboxes,
        workspaces,
        bindings,
        conversations,
        access,
        launches,
    ) = tokio::join!(
        store.command_work_page(&page),
        store.queued_sandbox_controls(&page),
        store.due_agent_work(NOW, 10),
        store.due_sandbox_work(NOW, 10),
        store.due_workspace_work(NOW, 10),
        store.pending_bindings(None, 10),
        store.pending_binding_conversations(None, 10),
        store.claim_access_request("probe-test"),
        launches.due_launches(NOW, 10),
    );
    assert!(commands.unwrap().items.is_empty());
    assert!(controls.unwrap().items.is_empty());
    assert!(agents.unwrap().is_empty());
    assert!(sandboxes.unwrap().is_empty());
    assert!(workspaces.unwrap().is_empty());
    assert!(bindings.unwrap().is_empty());
    assert!(conversations.unwrap().is_empty());
    assert!(access.unwrap().is_none());
    assert!(launches.unwrap().is_empty());
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn all_empty_boundaries_coalesce_without_reusing_a_completed_absence() {
    let (client, name, store, launches) = fixture("empty_discovery").await;
    for _ in 0..4 {
        empty_sweep(&store, &launches).await;
    }
    let (batches, skipped) = store.empty_discovery.counts();
    assert!(
        batches > 0 && skipped >= 9,
        "fresh overlapping empty requests must coalesce: {batches}/{skipped}"
    );
    // A later insertion from a separate store must be discovered on the very
    // next request even though this process received no local wake.
    let remote = MongoSandboxStore::open(&client, &name).await.unwrap();
    remote
        .sandbox_controls
        .insert_one(doc! {"_id":"invalid-control","queued":true})
        .await
        .unwrap();
    let page = PageRequest::new(10, None).unwrap();
    let (result, other) = tokio::join!(
        store.queued_sandbox_controls(&page),
        store.due_workspace_work(NOW, 10)
    );
    assert!(
        result.is_err(),
        "a new malformed row must reach ordinary validation"
    );
    assert!(other.unwrap().is_empty());
    assert!(store.queued_sandbox_controls(&page).await.is_err());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn future_due_and_live_leases_coalesce_without_delaying_new_work() {
    let (client, name, store, launches) = fixture("future_discovery").await;
    empty_sweep(&store, &launches).await;
    // Future work and live leases share an empty probe. The next request
    // evaluates its own cutoff without an insertion or a local notification.
    store
        .workspaces
        .insert_one(
            doc! {"_id":"wsp_future","document_schema_version":DOCUMENT_SCHEMA_VERSION,
            "work_pending":true,"work_ready_at":MINUTE,"work_lease_expires_at":TWO_MINUTES},
        )
        .await
        .unwrap();
    store
        .sandboxes
        .insert_one(
            doc! {"_id":"sbx_future","document_schema_version":DOCUMENT_SCHEMA_VERSION,
            "work_pending":true,"work_ready_at":MINUTE,"work_lease_expires_at":TWO_MINUTES},
        )
        .await
        .unwrap();
    let (_, skipped_before) = store.empty_discovery.counts();
    let (workspace_before, sandbox_before) = tokio::join!(
        store.due_workspace_work(NOW, 10),
        store.due_sandbox_work(NOW, 10)
    );
    assert!(workspace_before.unwrap().is_empty());
    assert!(sandbox_before.unwrap().is_empty());
    assert_eq!(store.empty_discovery.counts().1 - skipped_before, 2);
    let (workspace_held, sandbox_held) = tokio::join!(
        store.due_workspace_work(MINUTE, 10),
        store.due_sandbox_work(MINUTE, 10)
    );
    assert!(workspace_held.unwrap().is_empty());
    assert!(sandbox_held.unwrap().is_empty());
    let (workspace_due, sandbox_due) = tokio::join!(
        store.due_workspace_work(TWO_MINUTES, 10),
        store.due_sandbox_work(TWO_MINUTES, 10)
    );
    assert_eq!(
        workspace_due.unwrap(),
        vec![WorkspaceId("wsp_future".into())]
    );
    assert_eq!(sandbox_due.unwrap(), vec![SandboxId("sbx_future".into())]);
    // Cursor continuation cannot inherit a global empty page. Both malformed
    // lookahead and first-page rows remain validation failures.
    store
        .sandbox_controls
        .insert_many([
            doc! {"_id":"a","queued":true,"scope":"a","key_sha256":"a","sandbox_id":"sbx_a"},
            doc! {"_id":"z","queued":true,"scope":"z","key_sha256":"z","sandbox_id":"sbx_z"},
        ])
        .await
        .unwrap();
    assert!(store
        .queued_sandbox_controls(&PageRequest::new(1, Some("a".into())).unwrap())
        .await
        .is_err());
    assert!(PageRequest::new(0, None).is_err());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn cancelled_and_two_hundred_overlapping_requests_remain_bounded_and_fresh() {
    let (client, name, store, _launches) = fixture("bounded_discovery").await;
    let store = Arc::new(store);
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..200 {
        let store = store.clone();
        tasks.spawn(async move { store.due_workspace_work(NOW, 10).await.unwrap() });
    }
    while let Some(result) = tasks.join_next().await {
        assert!(result.unwrap().is_empty());
    }
    let policy = Policy::new(true);
    {
        let cancelled = policy.empty(
            Kind::Workspaces,
            &store.workspaces,
            doc! {"work_pending":true},
        );
        tokio::pin!(cancelled);
        assert!(futures_util::poll!(&mut cancelled).is_pending());
    }
    store
        .workspaces
        .insert_one(
            doc! {"_id":"wsp_admitted","document_schema_version":DOCUMENT_SCHEMA_VERSION,
            "work_pending":true,"work_ready_at":NOW,"work_lease_expires_at":Bson::Null},
        )
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        store.due_workspace_work(NOW, 10),
        store.due_agent_work(NOW, 10)
    );
    assert_eq!(a.unwrap(), vec![WorkspaceId("wsp_admitted".into())]);
    assert!(b.unwrap().is_empty());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn probe_failure_falls_through_and_late_admission_never_joins_an_inflight_absence() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let name = format!("cma_discovery_race_{}", uuid::Uuid::new_v4().simple());
    let started = Arc::new(tokio::sync::Notify::new());
    let notify = started.clone();
    let app = format!("discovery-race-{}", uuid::Uuid::new_v4().simple());
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if matches!(event, CommandEvent::Started(event) if event.command_name == "aggregate") {
            notify.notify_one();
        }
    }));
    let client = Client::with_options(options).unwrap();
    let control = Client::with_uri_str(&uri).await.unwrap();
    let mut store = MongoSandboxStore::open(&client, &name).await.unwrap();
    store.empty_discovery = Policy::new(true);
    let store = Arc::new(store);
    control
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},
        "data":{"failCommands":["aggregate"],"appName":&app,"errorCode":11600}})
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        store.due_workspace_work(NOW, 10),
        store.due_sandbox_work(NOW, 10)
    );
    assert!(a.unwrap().is_empty());
    assert!(b.unwrap().is_empty());
    assert_eq!(
        store.empty_discovery.counts(),
        (1, 0),
        "an errored probe is not absence"
    );
    // Use a new coordinator so the deliberate failure's probe backoff does not
    // prevent this second independent race from reaching the driver.
    let mut race = (*store).clone();
    race.empty_discovery = Policy::new(true);
    let race = Arc::new(race);
    control.database("admin").run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},
        "data":{"failCommands":["aggregate"],"appName":&app,"blockConnection":true,"blockTimeMS":150}}).await.unwrap();
    // Drain the earlier command notification, then register before spawning.
    started.notified().await;
    let ready = started.notified();
    tokio::pin!(ready);
    ready.as_mut().enable();
    let first = race.clone();
    let task = tokio::spawn(async move {
        tokio::join!(
            first.due_workspace_work(NOW, 10),
            first.due_sandbox_work(NOW, 10)
        )
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), ready)
        .await
        .unwrap();
    control
        .database(&name)
        .collection::<Document>("workspaces")
        .insert_one(doc! {
        "_id":"wsp_raced","document_schema_version":DOCUMENT_SCHEMA_VERSION,"work_pending":true,
        "work_ready_at":NOW,"work_lease_expires_at":Bson::Null})
        .await
        .unwrap();
    let (late, other) = tokio::join!(
        race.due_workspace_work(NOW, 10),
        race.due_sandbox_work(NOW, 10)
    );
    assert_eq!(late.unwrap(), vec![WorkspaceId("wsp_raced".into())]);
    assert!(other.unwrap().is_empty());
    let (a, b) = task.await.unwrap();
    a.unwrap();
    b.unwrap();
    control
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn missing_collection_and_required_index_fall_through_to_ordinary_reads() {
    let (client, name, mut store, _launches) = fixture("probe_metadata").await;
    store
        .agent_commands
        .drop_index("agent_command_work_scan")
        .await
        .unwrap();
    let page = PageRequest::new(10, None).unwrap();
    let (command, workspace) = tokio::join!(
        store.command_work_page(&page),
        store.due_workspace_work(NOW, 10)
    );
    assert!(
        command.is_err(),
        "missing required hint must remain an ordinary query error"
    );
    assert!(workspace.unwrap().is_empty());
    assert_eq!(store.empty_discovery.counts(), (1, 0));
    store.empty_discovery = Policy::new(true);
    store.bindings.conversations.drop().await.unwrap();
    let (conversation, workspace) = tokio::join!(
        store.pending_binding_conversations(None, 10),
        store.due_workspace_work(NOW, 10)
    );
    assert!(conversation.unwrap().is_empty());
    assert!(workspace.unwrap().is_empty());
    assert_eq!(
        store.empty_discovery.counts(),
        (1, 0),
        "missing collection cannot supply a coalesced absence"
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn positive_populations_suppress_only_extra_probes_and_never_grant_empty() {
    let (client, name, store, _launches) = fixture("probe_positive").await;
    let specs = [
        (Kind::Commands, "agent_commands", doc! {"status":"queued"}),
        (Kind::Controls, "sandbox_controls", doc! {"queued":true}),
        (Kind::Agents, "agents", doc! {"work_pending":true}),
        (Kind::Sandboxes, "sandboxes", doc! {"work_pending":true}),
        (Kind::Workspaces, "workspaces", doc! {"work_pending":true}),
        (Kind::Bindings, "provider_bindings", doc! {"pending":true}),
        (
            Kind::Conversations,
            "binding_conversations",
            doc! {"pending":true},
        ),
        (
            Kind::Access,
            "agent_access_requests",
            doc! {"record.phase":"pending"},
        ),
        (
            Kind::Launches,
            "trigger_launches",
            doc! {"work_step":"open"},
        ),
    ];
    // These rows test the probe boundary only: no parser or durable effect is
    // called. Every possible candidate must force the ordinary path.
    let mut inputs = Vec::new();
    for (kind, collection, filter) in specs {
        let collection = client.database(&name).collection::<Document>(collection);
        let mut row = filter.clone();
        row.insert("_id", "pending");
        if row.remove("record.phase").is_some() {
            row.insert("record", doc! {"phase":"pending"});
        }
        collection.insert_one(row).await.unwrap();
        inputs.push((kind, collection, filter));
    }
    let policy = store.empty_discovery.clone();
    let values = futures_util::future::join_all(
        inputs
            .iter()
            .map(|(kind, c, filter)| policy.empty(*kind, c, filter.clone())),
    )
    .await;
    assert!(values.iter().all(|empty| !empty));
    assert_eq!(policy.counts(), (1, 0));
    let before = policy.counts();
    for _ in 0..20 {
        for (kind, _, _) in &inputs {
            policy.observed(*kind, true);
        }
        let values = futures_util::future::join_all(
            inputs
                .iter()
                .map(|(kind, c, filter)| policy.empty(*kind, c, filter.clone())),
        )
        .await;
        assert!(values.iter().all(|empty| !empty));
    }
    assert_eq!(
        policy.counts(),
        before,
        "a due-now empty result cannot add repeated probes to a known pending population"
    );
    println!(
        "EMPTY_DISCOVERY_POSITIVE requests=180 extra_probe_commands=0 skipped_ordinary_queries=0"
    );
    client.database(&name).drop().await.unwrap();
}
