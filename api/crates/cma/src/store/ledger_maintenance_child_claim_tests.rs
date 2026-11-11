//! Exercise the real worker ports, with A's transactional claim hook applied.

use super::*;
use layer2_core::sandbox::store::{
    AgentStore, AgentWorkerStore, SandboxStoreError, WorkspaceStore, WorkspaceWorkerStore,
};
use layer2_core::sandbox::{
    AgentWorkState, AgentWorkStep, WorkspaceStatus, WorkspaceWorkState, WorkspaceWorkStep,
};

#[derive(Clone, Copy)]
enum Kind {
    Agent,
    Workspace,
}

async fn pending(store: &MongoSandboxStore, kind: Kind) -> LedgerMaintenanceAdmission {
    let mut request = fixture(store, false).await;
    let mut plan = request.accepted.plan().clone();
    match kind {
        Kind::Agent => {
            let mut agent = plan.source.agents[0].clone();
            let revision = agent.revision;
            agent.work = AgentWorkState::new(AgentWorkStep::CloseProcess, NOW.into());
            agent.revision += 1;
            store.save_agent(revision, agent).await.unwrap();
        }
        Kind::Workspace => {
            let mut workspace = plan.source.workspaces[0].clone();
            let revision = workspace.revision;
            workspace.status = WorkspaceStatus::Removing;
            workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::Remove, NOW.into());
            workspace.revision += 1;
            store.save_workspace(revision, workspace).await.unwrap();
        }
    }
    plan.source = store
        .inspect_ledger_maintenance(&request.caller, &plan.source.sandbox.id)
        .await
        .unwrap()
        .source;
    request.accepted = artifact(&plan);
    request
}

async fn claim(
    store: &MongoSandboxStore,
    request: &LedgerMaintenanceAdmission,
    kind: Kind,
    now: &str,
) -> Result<bool, SandboxStoreError> {
    let source = &request.accepted.plan().source;
    match kind {
        Kind::Agent => store
            .claim_agent_work(
                &source.agents[0].id,
                "fixture-claim",
                now.into(),
                NEXT.into(),
            )
            .await
            .map(|row| row.is_some()),
        Kind::Workspace => store
            .claim_workspace_work(
                &source.workspaces[0].id,
                "fixture-claim",
                now.into(),
                NEXT.into(),
            )
            .await
            .map(|row| row.is_some()),
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and the shared claim hook"]
async fn real_child_claims_succeed_without_a_hold_and_invalidate_the_old_plan() {
    for kind in [Kind::Agent, Kind::Workspace] {
        let (client, name, store) = database().await;
        let request = pending(&store, kind).await;
        // Fixture preparation itself can use a fenced generic child save.
        // The claim must add exactly one admission to that observed baseline.
        let prior_fence = documents(&store).await[0][0]
            .get("ledger_writer_admission_fence_v1")
            .map(|value| value.as_i64().unwrap())
            .unwrap_or(0);
        let other = MongoSandboxStore::open(&client, &name).await.unwrap();
        assert!(claim(&other, &request, kind, NOW).await.unwrap());
        let before = documents(&store).await;
        assert_eq!(
            before[0][0]
                .get_i64("ledger_writer_admission_fence_v1")
                .unwrap(),
            prior_fence + 1
        );
        assert_eq!(
            store.admit_ledger_maintenance(ENABLED, request).await,
            Err(Domain::Refused(Refusal::Conflict))
        );
        assert_eq!(documents(&store).await, before);
        assert!(!before[0][0].contains_key(FIELD));
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and the shared claim hook"]
async fn real_child_claims_refuse_held_work_after_restart_and_maintenance_lease_expiry() {
    for kind in [Kind::Agent, Kind::Workspace] {
        let (client, name, store) = database().await;
        let request = pending(&store, kind).await;
        store
            .admit_ledger_maintenance(ENABLED, request.clone())
            .await
            .unwrap();
        let before = documents(&store).await;
        let restarted = MongoSandboxStore::open(&client, &name).await.unwrap();
        for at in [NOW, EXPIRED] {
            assert!(!claim(&restarted, &request, kind, at).await.unwrap());
            assert_eq!(documents(&store).await, before);
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and the shared claim hook"]
async fn real_child_claim_conflicts_with_an_inflight_maintenance_snapshot() {
    for kind in [Kind::Agent, Kind::Workspace] {
        let (client, name, store) = database().await;
        let request = pending(&store, kind).await;
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        super::super::super::ledger_maintenance_snapshot::load(
            &store,
            &request.caller,
            &request.accepted.plan().source.sandbox.id,
            &mut session,
        )
        .await
        .unwrap_or_else(|_| panic!("fixture snapshot failed"));
        let other = MongoSandboxStore::open(&client, &name).await.unwrap();
        assert!(claim(&other, &request, kind, NOW).await.unwrap());
        let before = documents(&store).await;
        let result =
            transaction::apply(&store, &Change::Admit(ENABLED, request), &mut session).await;
        assert!(matches!(result, Err(ApplyError::Driver(error)) if is_transient(&error)));
        let _ = session.abort_transaction().await;
        assert_eq!(documents(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and the shared claim hook"]
async fn malformed_hold_refuses_real_child_claims_without_partial_parent_or_child_writes() {
    for kind in [Kind::Agent, Kind::Workspace] {
        let (client, name, store) = database().await;
        let request = pending(&store, kind).await;
        store
            .sandboxes
            .update_one(doc! {}, doc! {"$set":{FIELD:"invalid-fixture"}})
            .await
            .unwrap();
        let before = documents(&store).await;
        assert!(matches!(
            claim(&store, &request, kind, NOW).await,
            Err(SandboxStoreError::Corrupt(_))
        ));
        assert_eq!(documents(&store).await, before);
        client.database(&name).drop().await.unwrap();
    }
}
