//! Disposable Mongo fixtures only. No provider or production connection.

use layer2_core::sandbox::ledger_maintenance_admission::LedgerMaintenanceLeaseV1;
use layer2_core::sandbox::ledger_maintenance_plan::{
    LedgerMaintenancePlanArtifactV1, LedgerMaintenancePlanV1,
};
use layer2_core::sandbox::store::ledger_maintenance::{
    LedgerMaintenanceAdmission, LedgerMaintenanceStore,
};
use layer2_core::sandbox::*;
use layer2_core::{Authorization, Caller, Capability, WorkspaceProfileId};
use layer3_adapter::runtime_ledger_maintenance::{
    LedgerBindingV1, LedgerSourceFormatV1, LEDGER_BINDING_FORMAT,
};
use layer3_adapter::{HomeVolumePolicy, HomeVolumeReceipt};
use mongodb::bson::{doc, Document};

use super::ledger_maintenance_records::AUDITS;
use super::sandbox_maintenance::mongo_integration_tests::{active_sandbox, ready_agent};
use super::sandbox_mongo::MongoSandboxStore;

pub(super) const NOW: &str = "2026-09-05T00:30:02Z";
pub(super) const EXPIRED: &str = "2026-09-05T00:31:02Z";
pub(super) const NEXT: &str = "2026-09-05T00:32:02Z";

pub(super) fn lease(expires_at: &str) -> LedgerMaintenanceLeaseV1 {
    LedgerMaintenanceLeaseV1 {
        holder: "fixture-worker".into(),
        expires_at: expires_at.into(),
    }
}

pub(super) async fn fixture(store: &MongoSandboxStore, lane: bool) -> LedgerMaintenanceAdmission {
    let mut sandbox = active_sandbox("ledger-fixture-parent");
    sandbox.runtime.as_mut().unwrap().boot_id = Some("b".repeat(32));
    retain_home(&mut sandbox);
    let caller = Caller::new(
        sandbox.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let agent = ready_agent(&sandbox, "fixture-agent");
    store
        .sandboxes
        .insert_one(super::sandboxes::sandbox_document(&sandbox, 0).unwrap())
        .await
        .unwrap();
    insert_hierarchy(store, &sandbox, &agent).await;
    store
        .agents
        .insert_one(super::agents::agent_document(&agent).unwrap())
        .await
        .unwrap();
    if lane {
        store
            .agent_command_orders
            .insert_one(doc! {
                "_id":agent.id.as_str(), "document_schema_version":1_i64,
                "last_order":1_i64, "next_fence":2_i64, "lease":mongodb::bson::Bson::Null,
            })
            .await
            .unwrap();
    }
    let database = store.client.database(&store.sandboxes.namespace().db);
    if !database
        .list_collection_names()
        .await
        .unwrap()
        .iter()
        .any(|name| name == AUDITS)
    {
        database.create_collection(AUDITS).await.unwrap();
    }
    let inspection = store
        .inspect_ledger_maintenance(&caller, &sandbox.id)
        .await
        .unwrap();
    let plan = LedgerMaintenancePlanV1 {
        format: "ledger-maintenance-plan-v1".into(),
        binding: LedgerBindingV1 {
            format: LEDGER_BINDING_FORMAT.into(),
            sandbox_id: sandbox.id.to_string(),
            home_identity: sandbox
                .lifecycle
                .home
                .as_ref()
                .unwrap()
                .attachment()
                .unwrap()
                .identity()
                .unwrap(),
            generation: 1,
            boot_id: "b".repeat(32),
            roots_sha256: "a".repeat(64),
            source_format: LedgerSourceFormatV1::V1,
            archive_id: uuid::Uuid::from_u128(1).to_string(),
            maintenance_id: "fixture-operation".into(),
            maintenance_epoch: 1,
            sandbox_revision: sandbox.revision,
            candidate_sha256: "c".repeat(64),
            recovery_sha256: "d".repeat(64),
            exclusion_receipt_sha256: "e".repeat(64),
        },
        source: inspection.source,
        consent_receipt_sha256: "f".repeat(64),
        requested_at: NOW.into(),
    };
    LedgerMaintenanceAdmission {
        caller,
        accepted: artifact(&plan),
        lease: lease(EXPIRED),
        now: NOW.into(),
    }
}

/// Complete the actual Agent's hierarchy before maintenance inspection. Output
/// fixtures originally omitted these rows; a dangling Worktree must refuse.
pub(super) async fn insert_hierarchy(store: &MongoSandboxStore, sandbox: &Sandbox, agent: &Agent) {
    use layer2_core::profiles::*;
    assert_eq!(agent.sandbox_id, sandbox.id);
    assert_eq!(agent.owner, sandbox.owner);
    let workspace = Workspace {
        id: agent.workspace_id.clone(),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId("fixture-profile".into()),
            revision: 1,
            digest: "a".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: "fixture".into(),
                },
                name: "fixture".into(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: "fixture".into(),
        path: "/home/cma/fixture".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: Some("c".repeat(40)),
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into()),
        next_worktree_number: 2,
        created_at: NOW.into(),
        ready_at: Some(NOW.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    };
    let worktree = Worktree {
        id: agent.worktree_id.clone(),
        workspace_id: workspace.id.clone(),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        name: "wt-1".into(),
        path: "/home/cma/fixture/.worktrees/wt-1".into(),
        branch: "cma/wt-1".into(),
        base_commit: "c".repeat(40),
        status: WorktreeStatus::Ready,
        agent_id: Some(agent.id.clone()),
        revision: 1,
    };
    store
        .workspaces
        .insert_one(super::workspaces::workspace_document(&workspace).unwrap())
        .await
        .unwrap();
    store
        .worktrees
        .insert_one(super::worktrees::worktree_document(&worktree).unwrap())
        .await
        .unwrap();
}

pub(super) fn artifact(plan: &LedgerMaintenancePlanV1) -> LedgerMaintenancePlanArtifactV1 {
    // Deliberately preserve formatting that would change on reserialization.
    LedgerMaintenancePlanArtifactV1::parse(&serde_json::to_vec_pretty(plan).unwrap()).unwrap()
}

pub(super) async fn documents(store: &MongoSandboxStore) -> Vec<Vec<Document>> {
    use futures_util::TryStreamExt;
    let mut result = Vec::new();
    for collection in [
        &store.sandboxes,
        &store.workspaces,
        &store.worktrees,
        &store.agents,
        &store.agent_command_orders,
        &super::ledger_maintenance_transaction::audit_collection(store),
    ] {
        result.push(
            collection
                .find(doc! {})
                .sort(doc! {"_id":1})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap(),
        );
    }
    result
}

/// Synthetic retained volume for disposable persistence tests only.
pub(super) fn retain_home(sandbox: &mut Sandbox) {
    let mut home = SandboxHome::reserve(
        &sandbox.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "fixture-home".into(),
            capacity_gib: 8,
            template_name: "seed".into(),
            template_generation: "seed-generation".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "fixture-pvc".into(),
        storage_generation: "fixture-pvc-generation".into(),
    })
    .unwrap();
    sandbox.lifecycle.home = Some(Box::new(home));
}

/// Compare all fixture collections, including command, budget, audit and log
/// sidecars, so a refusal cannot pass after partially persisting another record.
pub(super) async fn all_rows(store: &MongoSandboxStore) -> Vec<(String, Vec<Document>)> {
    use futures_util::TryStreamExt;
    let database = store.client.database(&store.sandboxes.namespace().db);
    let mut names = database.list_collection_names().await.unwrap();
    names.sort();
    let mut result = Vec::new();
    for name in names {
        let rows = database
            .collection::<Document>(&name)
            .find(doc! {})
            .sort(doc! {"_id":1})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        result.push((name, rows));
    }
    result
}

/// A write-concern failpoint can return before the committed transaction reaches
/// the majority snapshot. Establish a read-only timestamp barrier before checking
/// its durable outcome; no sleep, retry of the mutation, or weaker read concern.
pub(super) async fn disable_failpoint_and_observe_majority(store: &MongoSandboxStore) {
    let stopped = store
        .client
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(stopped.get_timestamp("operationTime").unwrap())
        .await
        .unwrap();
    assert!(store
        .agents
        .find_one(doc! {})
        .projection(doc! {"_id":1})
        .read_concern(mongodb::options::ReadConcern::snapshot())
        .max_time(std::time::Duration::from_secs(10))
        .session(&mut session)
        .await
        .unwrap()
        .is_some());
}

/// A rejected commit command can leave the server transaction open until its
/// normal timeout. These tests exclusively own their disposable replica set and
/// run serially. Observe server cleanup before testing a new attempt; never kill
/// sessions, retry the failed mutation, or change the server's transaction limit.
pub(super) async fn observe_transaction_cleanup(store: &MongoSandboxStore) {
    tokio::time::timeout(std::time::Duration::from_secs(90), async {
        loop {
            let status = store
                .client
                .database("admin")
                .run_command(doc! {"serverStatus":1,"maxTimeMS":5000})
                .await
                .unwrap();
            let open = status
                .get_document("transactions")
                .unwrap()
                .get_i64("currentOpen")
                .unwrap();
            assert!(open >= 0);
            if open == 0 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    })
    .await
    .expect("disposable Mongo transaction cleanup exceeded its deadline");
}

/// Propose E1 from the current disposable database, before admission only.
/// This contains no execution binding and proves no provider/storage exclusion.
pub(super) async fn pre_exclusion(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    now: &str,
) -> (
    layer2_core::sandbox::ledger_maintenance_pre_exclusion_progress::LedgerPreExclusionPolicy,
    layer2_core::sandbox::ledger_maintenance_pre_exclusion_progress::LedgerPreExclusionAdmission,
) {
    use layer2_core::sandbox::agent_ledger_recovery::LedgerRecoveryDispositionV1;
    use layer2_core::sandbox::ledger_maintenance_pre_exclusion::*;
    use layer2_core::sandbox::ledger_maintenance_pre_exclusion_progress::{
        LedgerPreExclusionAdmission, LedgerPreExclusionPolicy,
    };
    use layer2_core::sandbox::store::ledger_maintenance_pre_exclusion::*;
    let caller = Caller::new(
        sandbox.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let source = store
        .inspect_pre_exclusion(&caller, &sandbox.id)
        .await
        .unwrap()
        .source;
    let runtime = source.sandbox.retained_runtime_binding().unwrap();
    let plan = LedgerPreExclusionPlanV1 {
        format: "ledger-pre-exclusion-plan-v1".into(),
        operation_id: "creation-fixture-maintenance".into(),
        sandbox_id: sandbox.id.clone(),
        home_identity: source
            .sandbox
            .lifecycle
            .home
            .as_ref()
            .unwrap()
            .attachment()
            .unwrap()
            .identity()
            .unwrap(),
        generation: runtime.provision_generation.unwrap(),
        boot_id: runtime.boot_id.clone().unwrap(),
        roots_sha256: "a".repeat(64),
        source,
        disposition: LedgerRecoveryDispositionV1::PreserveHeld,
        authorized_by: sandbox.owner.clone(),
        consent_receipt_sha256: "b".repeat(64),
        writer_barrier_epoch: 1,
        compatible_floor: LedgerMaintenanceFloorV1 {
            reader_manifest_sha256: "c".repeat(64),
            writer_manifest_sha256: "d".repeat(64),
        },
        requested_at: now.into(),
    };
    let policy = LedgerPreExclusionPolicy {
        enabled: true,
        qualified_floor: Some(plan.compatible_floor.clone()),
    };
    let expires = layer2_core::rfc3339_at(
        std::time::UNIX_EPOCH
            + std::time::Duration::from_secs(layer2_core::seconds_from_rfc3339(now).unwrap() + 60),
    );
    let input = LedgerPreExclusionAdmission {
        caller,
        accepted: LedgerPreExclusionArtifactV1::parse(&serde_json::to_vec_pretty(&plan).unwrap())
            .unwrap(),
        lease: lease(&expires),
        now: now.into(),
    };
    (policy, input)
}
