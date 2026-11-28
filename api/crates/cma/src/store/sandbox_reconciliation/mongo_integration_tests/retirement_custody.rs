use super::*;
use crate::store::native_retirement_action::{
    fence_obligation, RetirementAction, RetirementObservation, RetirementPhase, RetirementRecords,
};
use layer2_core::sandbox::store::SandboxWorkerStore;
use layer2_core::sandbox::{AgentCleanupDebt, SandboxHome};
use layer3_adapter::owner_protocol::{TransportOwner, OWNER_PROTOCOL_VERSION};
use layer3_adapter::receiver_failure::RetirementReceiver;
use layer3_adapter::{HomeVolumePolicy, HomeVolumeReceipt, RetirementPod, RuntimeHandle};
use std::time::{Duration, SystemTime};

async fn candidate(
    store: &MongoSandboxStore,
    segmented: bool,
) -> (Sandbox, Vec<Agent>, RetirementAction) {
    let (mut sandbox, children) = retirement::retiring(store, segmented).await;
    let mut home = SandboxHome::reserve(
        &sandbox.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "local-test".into(),
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
        storage_reference: "synthetic-home".into(),
        storage_generation: "synthetic-home-uid".into(),
    })
    .unwrap();
    sandbox.lifecycle.home = Some(Box::new(home));
    let revision = sandbox.revision;
    sandbox.revision += 1;
    store.save_sandbox(revision, sandbox.clone()).await.unwrap();
    let now = SystemTime::now();
    let parent = store
        .claim_sandbox_work(
            &sandbox.id,
            "retirement-actor",
            layer2_core::rfc3339_at(now),
            layer2_core::rfc3339_at(now + Duration::from_secs(180)),
        )
        .await
        .unwrap()
        .unwrap();
    let binding = parent.runtime.clone().unwrap();
    let observation = RetirementObservation {
        owner: parent.owner.clone(),
        receiver: RetirementReceiver {
            runtime: RuntimeHandle::new(&binding.runtime_id),
            pod: RetirementPod {
                uid: "original-pod".into(),
                container_id: Some("original-container".into()),
                restart_count: Some(0),
            },
            owner: TransportOwner {
                protocol: OWNER_PROTOCOL_VERSION,
                agent_id: children[0].id.to_string(),
                instance_id: "synthetic-replica".into(),
                epoch: 1,
            },
        },
        binding,
    };
    store.note(&parent, &observation).await.unwrap();
    let (observation, fences) = store.observation(&parent.id).await.unwrap().unwrap();
    let action = RetirementAction::new(&parent, observation, &children, fences).unwrap();
    (parent, children, action)
}

fn external_debt(child: &Agent) -> Agent {
    let mut next = child.clone();
    next.revision += 1;
    next.work.cleanup_debt.push(AgentCleanupDebt {
        step: AgentWorkStep::DispatchFirstMessage,
        operation_id: "concurrent-external".into(),
        request_sha256: "c".repeat(64),
    });
    next
}

fn new_child(parent: &Sandbox) -> Agent {
    let mut child = ready_agent(parent, "agt_attached_after_enumeration");
    child.conversation.last_seq = 0;
    child
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn maintenance_preserves_the_native_retirement_receiver_observation() {
    let (client, name, store) = database().await;
    let (parent, _, _) = candidate(&store, false).await;
    let (original, _) = store.observation(&parent.id).await.unwrap().unwrap();
    let mut replacement = original.clone();
    replacement.receiver.pod.restart_count =
        Some(replacement.receiver.pod.restart_count.unwrap() + 1);
    let replica = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let parents = replica
        .database(&name)
        .collection::<mongodb::bson::Document>("sandboxes");
    let filter = doc! { "_id": parent.id.as_str() };
    for hold in [
        doc! { "phase": "requested" }.into(),
        mongodb::bson::Bson::Null,
    ] {
        parents
            .update_one(
                filter.clone(),
                doc! { "$set": { "ledger_maintenance": hold } },
            )
            .write_concern(mongodb::options::WriteConcern::majority())
            .await
            .unwrap();
        let held = parents.find_one(filter.clone()).await.unwrap().unwrap();
        assert_eq!(
            store.note(&parent, &replacement).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(parents.find_one(filter.clone()).await.unwrap(), Some(held));
    }
    parents
        .update_one(filter, doc! { "$unset": { "ledger_maintenance": "" } })
        .await
        .unwrap();
    store.note(&parent, &replacement).await.unwrap();
    assert_eq!(
        store.observation(&parent.id).await.unwrap().unwrap().0,
        replacement
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retirement_custody_fences_earlier_and_prestarted_child_writers() {
    for writer_wins in [true, false] {
        for attach in [true, false] {
            let (client, name, store) = database().await;
            let (parent, children, action) = candidate(&store, false).await;
            let later = new_child(&parent);
            let next = external_debt(&children[1]);
            if writer_wins {
                if attach {
                    store.create_agent(later.clone()).await.unwrap();
                } else {
                    store
                        .save_agent(children[1].revision, next.clone())
                        .await
                        .unwrap();
                }
                assert_eq!(
                    store.admit(&parent, &action).await,
                    Err(SandboxStoreError::Conflict)
                );
                assert_eq!(store.action(&parent.id).await.unwrap(), None);
            } else {
                // The other writer already started and pinned a snapshot without
                // custody. Its write must conflict and its retry must refuse.
                let mut writer = client.start_session().await.unwrap();
                start_transaction(&mut writer).await.unwrap();
                let row = store
                    .sandboxes
                    .find_one(doc! {"_id": parent.id.as_str()})
                    .session(&mut writer)
                    .await
                    .unwrap()
                    .unwrap();
                assert!(!row.contains_key(crate::store::native_retirement_action::FIELD));
                store.admit(&parent, &action).await.unwrap();
                let result = fence_obligation(&store, &parent.id, &mut writer).await;
                assert!(matches!(result, Err(error) if is_transient(&error)));
                let _ = writer.abort_transaction().await;
                if attach {
                    assert_eq!(
                        store.create_agent(later.clone()).await,
                        Err(SandboxStoreError::Conflict)
                    );
                    assert_eq!(store.get_agent(&later.id).await.unwrap(), None);
                } else {
                    assert_eq!(
                        store.save_agent(children[1].revision, next).await,
                        Err(SandboxStoreError::Conflict)
                    );
                    assert_eq!(
                        store.get_agent(&children[1].id).await.unwrap(),
                        Some(children[1].clone())
                    );
                }
                assert_eq!(store.action(&parent.id).await.unwrap(), Some(action));
            }
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn retirement_custody_reconciles_admission_and_atomically_releases_dispatched_action() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    for committed in [false, true] {
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let name = format!("retirement_custody_{suffix}");
        let app = format!("custody-{suffix}");
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        let client = Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (parent, children, mut action) = candidate(&store, committed).await;
        let mut data = doc! {"failCommands":["commitTransaction"],"appName":app};
        if committed {
            data.insert("writeConcernError", doc!{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"simulated lost custody acknowledgement"});
        } else {
            data.insert("errorCode", 50_i32);
            data.insert("errorLabels", ["UnknownTransactionCommitResult"]);
        }
        admin
            .database("admin")
            .run_command(
                doc! {"configureFailPoint":"failCommand","mode":{"times":5_i32},"data":data},
            )
            .await
            .unwrap();
        let admitted = store.admit(&parent, &action).await;
        disable_failpoint(&admin).await;
        if committed {
            assert_eq!(admitted, Ok(()));
        } else {
            assert!(matches!(admitted, Err(SandboxStoreError::Unavailable(_))));
            assert_eq!(store.action(&parent.id).await.unwrap(), None);
            store.admit(&parent, &action).await.unwrap();
        }
        let restarted = MongoSandboxStore::open(&client, &name).await.unwrap();
        assert_eq!(
            restarted.action(&parent.id).await.unwrap(),
            Some(action.clone())
        );
        let previous = action.clone();
        action.phase = RetirementPhase::Dispatching;
        restarted
            .transition(&parent, &previous, Some(&action))
            .await
            .unwrap();
        assert!(matches!(
            restarted
                .recover_runtime_loss(retirement::finality_loss(&parent))
                .await,
            Err(SandboxStoreError::Conflict)
        ));
        let mut proof = retirement::finality_loss(&parent);
        proof.observed_at = layer2_core::rfc3339_at(SystemTime::now());
        assert_eq!(
            restarted.settle(proof, &action).await.unwrap(),
            SandboxRuntimeLossOutcome::RecoveryScheduled
        );
        assert_eq!(restarted.action(&parent.id).await.unwrap(), None);
        let after = restarted.get_sandbox(&parent.id).await.unwrap().unwrap();
        assert!(after.runtime.is_none());
        assert_eq!(after.lifecycle.home, parent.lifecycle.home);
        assert_eq!(
            restarted
                .get_agent(&children[0].id)
                .await
                .unwrap()
                .unwrap()
                .status,
            AgentStatus::Stopped
        );
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn retirement_custody_does_not_retry_an_uncertain_transaction_body() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("retirement_mixed_labels_{suffix}");
    let app = format!("retirement-mixed-{suffix}");
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, children, action) = candidate(&store, false).await;
    let new_child = new_child(&parent);
    let child = &children[1];
    for operation in ["admit", "attach", "add_obligation"] {
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand",
                "mode": {"times": 1_i32},
                "data": {
                    "failCommands": ["update"],
                    "appName": &app,
                    "errorCode": 112_i32,
                    "errorLabels": ["UnknownTransactionCommitResult", "TransientTransactionError"],
                },
            })
            .await
            .unwrap();
        let result = match operation {
            "admit" => store.admit(&parent, &action).await,
            "attach" => store.create_agent(new_child.clone()).await,
            "add_obligation" => store.save_agent(child.revision, external_debt(child)).await,
            _ => unreachable!(),
        };
        disable_failpoint(&admin).await;
        assert!(
            matches!(result, Err(SandboxStoreError::Unavailable(_))),
            "{operation}: {result:?}"
        );
        assert_eq!(store.action(&parent.id).await.unwrap(), None);
        assert_eq!(store.get_agent(&new_child.id).await.unwrap(), None);
        assert_eq!(
            store.get_agent(&child.id).await.unwrap().as_ref(),
            Some(child)
        );
    }
    client.database(&name).drop().await.unwrap();
}
