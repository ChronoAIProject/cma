use super::super::super::retirement_case6::{mongo_parent_worker, run_mongo_parent_due};
use super::*;
use crate::store::{
    native_retirement_action::{
        RetirementAction, RetirementObservation, RetirementPhase, RetirementRecords,
    },
    MongoSandboxStore,
};
use layer2_core::sandbox::store::{WorktreeStore, MAX_PAGE_SIZE};
use layer2_core::sandbox::TypedFailure;
use layer3_adapter::owner_protocol::{TransportOwner, OWNER_PROTOCOL_VERSION};
use layer3_adapter::receiver_failure::RetirementReceiver;
use mongodb::{bson::doc, Client};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Boundary {
    Disabled,
    Live,
    NextPage,
    Custody,
    ReplacedUid,
    UnknownDelete,
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn failed_child_retirement_requires_credentials_and_exact_provider_finality() {
    use Boundary::*;
    for boundary in [
        Disabled,
        Live,
        NextPage,
        Custody,
        ReplacedUid,
        UnknownDelete,
    ] {
        Box::pin(exercise(boundary)).await;
    }
}

async fn exercise(boundary: Boundary) {
    use SandboxWorkStep::{MarkRetired, RevokeRetirementCredentials, StopAgents, TeardownRuntime};

    let (mut f, mut parent, now) = Box::pin(active_fixture()).await;
    let mut child = lifecycle_queue_tests::ready_agent(&parent, now);
    let native = fixture::NativeFixture::new(&mut parent).await;
    native.transport.replaced_boot();
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let name = format!("failed_child_{}", uuid::Uuid::new_v4().simple());
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    parent.revision = 1;
    store.create_sandbox(parent.clone()).await.unwrap();

    child.status = AgentStatus::Failed;
    child.failure = Some(TypedFailure {
        class: "fixture_failure".into(),
        detail: "synthetic child failure".into(),
    });
    child.conversation.runtime_generation = Some(1);
    for index in 0..match boundary {
        Boundary::Live => 0,
        Boundary::NextPage => MAX_PAGE_SIZE,
        _ => 1,
    } {
        child.id.0 = format!("agent-fixture-{index:04}");
        child.worktree_id = layer2_core::WorktreeId::generate();
        child.creation_key = child.id.to_string();
        store.create_agent(child.clone()).await.unwrap();
    }
    if matches!(boundary, Boundary::Live | Boundary::NextPage) {
        child = lifecycle_queue_tests::ready_agent(&parent, now);
        child.id.0 = "zz-live-child".into();
        child.conversation.runtime_generation = Some(1);
        let workspace = lifecycle_recovery_tests::workspace_for(&child, now);
        store
            .create_worktree(layer2_core::sandbox::Worktree {
                id: child.worktree_id.clone(),
                workspace_id: workspace.id.clone(),
                sandbox_id: parent.id.clone(),
                owner: parent.owner.clone(),
                name: "wt-1".into(),
                path: format!("{}/.worktrees/wt-1", workspace.path),
                branch: "cma/wt-1".into(),
                base_commit: "a".repeat(40),
                status: layer2_core::sandbox::WorktreeStatus::Ready,
                agent_id: Some(child.id.clone()),
                revision: 1,
            })
            .await
            .unwrap();
        store.create_workspace(workspace).await.unwrap();
        store.create_agent(child.clone()).await.unwrap();
    }
    parent
        .begin_retirement(rfc3339_at(SystemTime::now()))
        .unwrap();
    store.save_sandbox(1, parent.clone()).await.unwrap();
    let binding = parent.runtime.as_ref().unwrap();

    if boundary == Boundary::Custody {
        let observation = RetirementObservation {
            owner: parent.owner.clone(),
            binding: binding.clone(),
            receiver: RetirementReceiver {
                runtime: RuntimeHandle::new(&binding.runtime_id),
                pod: layer3_adapter::RetirementPod {
                    uid: "original-pod".into(),
                    container_id: Some("containerd://original".into()),
                    restart_count: Some(0),
                },
                owner: TransportOwner {
                    protocol: OWNER_PROTOCOL_VERSION,
                    agent_id: child.id.to_string(),
                    instance_id: "fixture-owner".into(),
                    epoch: 1,
                },
            },
        };
        let mut action = RetirementAction::new(
            &parent,
            observation,
            std::slice::from_ref(&child),
            [None; 2],
        )
        .unwrap();
        action.phase = RetirementPhase::Dispatching;
        client.database(&name).collection::<mongodb::bson::Document>("sandboxes")
            .update_one(doc! {"_id": parent.id.as_str()}, doc! {"$set": {
                "native_retirement_action_v1": mongodb::bson::serialize_to_bson(&action).unwrap()
            }})
            .await.unwrap();
    }

    f.harness.adapter = native.adapter.clone();
    if boundary == Boundary::Disabled {
        f = f.without_retirement_admission();
    }
    let worker = mongo_parent_worker(&f, store.clone(), "failed-child-parent", None);
    let credentials = &f.harness.credentials;
    let id = &parent.id;
    let read = || async { store.get_sandbox(id).await.unwrap().unwrap() };

    let capacity = || async { store.owner_capacity(&parent.owner).await.unwrap().counts };
    let deletes = || {
        native
            .api
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter_map(|(method, path)| (method == "DELETE").then_some(path.clone()))
            .collect::<Vec<_>>()
    };
    credentials.issue(&parent, 2, now).await.unwrap();
    let deletion_state = (store.clone(), parent.clone(), Arc::clone(credentials));
    native.api.state.lock().unwrap().before_delete = Some(Box::new(move || {
        let (store, parent, credentials) = deletion_state.clone();
        Box::pin(async move {
            let persisted = store.get_sandbox(&parent.id).await.unwrap().unwrap();
            assert_eq!(persisted.work.step, TeardownRuntime);
            assert_eq!(persisted.runtime, parent.runtime);
            assert!(
                persisted.credential.current.is_none() && persisted.credential.pending.is_none()
            );
            assert!(!persisted.credential.delivered && persisted.credential.revoke_debt.is_empty());
            assert!(credentials.live_keys.lock().unwrap().is_empty());
            assert!(credentials.custodied(&parent.id).await.unwrap().is_empty());
        })
    }));

    let action = store.action(id).await.unwrap();
    assert_eq!(capacity().await, [(SandboxStatus::Retiring, 1)]);
    run_mongo_parent_due(&worker, id).await;
    let stopped = read().await;
    assert_eq!(stopped.runtime.as_ref(), Some(binding));
    assert!(deletes().is_empty());
    assert!(native.transport.responses.load(Ordering::SeqCst) > 0);
    if matches!(boundary, Boundary::ReplacedUid | Boundary::UnknownDelete) {
        assert_eq!(stopped.work.step, RevokeRetirementCredentials);
        run_mongo_parent_due(&worker, id).await;
        assert_eq!(read().await.work.step, TeardownRuntime);
        assert!(deletes().is_empty());
        {
            let mut api = native.api.state.lock().unwrap();
            if boundary == Boundary::ReplacedUid {
                api.rows.get_mut(api::MEMBER).unwrap()["metadata"]["uid"] = json!("replacement");
            } else {
                api.lose_member_delete = true;
            }
        }
        run_mongo_parent_due(&worker, id).await;
        let held = read().await;
        assert_eq!(held.work.step, TeardownRuntime, "{:?}", held.failure);
        assert_eq!(held.runtime.as_ref(), Some(binding));
        assert_eq!(held.lifecycle.home, parent.lifecycle.home);
        assert_eq!(capacity().await, [(SandboxStatus::Retiring, 1)]);
        if boundary == Boundary::ReplacedUid {
            assert!(deletes().is_empty());
            let state = native.api.state.lock().unwrap();
            assert_eq!(state.rows[api::MEMBER]["metadata"]["uid"], "replacement");
        } else {
            let expected = [native.claim_path.as_str(), api::MEMBER];
            assert_eq!(deletes(), expected);
            run_mongo_parent_due(&worker, id).await;
            assert_eq!(read().await.work.step, MarkRetired);
            assert_eq!(capacity().await, [(SandboxStatus::Retiring, 1)]);
            run_mongo_parent_due(&worker, id).await;
            let retired = read().await;
            assert_eq!(retired.status, SandboxStatus::Retired);
            assert!(retired.lifecycle.home.unwrap().released);
            assert!(!worker.run_one(id, SystemTime::now()).await.unwrap());
            assert_eq!(read().await.retired_at, retired.retired_at);
            assert!(capacity().await.is_empty());
            assert_eq!(deletes(), expected);
            let api = native.api.state.lock().unwrap();
            assert!(!api.rows.contains_key(api::POD) && !api.rows.contains_key(api::VOLUME));
        }
    } else {
        assert_eq!(stopped.work.step, StopAgents, "{boundary:?}");
        assert_eq!(stopped.credential, parent.credential);
        if child.status.is_live() {
            let live = store.get_agent(&child.id).await.unwrap().unwrap();
            assert!(live.requested_stop_reason.is_some());
        }
    }
    assert_eq!(store.action(id).await.unwrap(), action);
    client.database(&name).drop().await.unwrap();
}
