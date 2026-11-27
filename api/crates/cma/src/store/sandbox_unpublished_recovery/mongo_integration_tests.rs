use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::{
    active_sandbox, database, insert_agent, ready_agent,
};
use crate::store::sandbox_mongo_integration_tests::work_lease_deadline;
use layer2_core::sandbox::store::{AgentStore, SandboxStore};
use layer2_core::sandbox::*;
use layer3_adapter::{
    HomeVolumeMode, HomeVolumePolicy, HomeVolumeReceipt, HomeVolumeScope, RuntimeImage,
    RUNTIME_CONTRACT_LABEL,
};
use mongodb::bson::doc;

const NOW: &str = "2026-09-05T00:30:00Z";

#[path = "retained_mongo_tests.rs"]
mod retained;

#[path = "platform_stop_mongo_tests.rs"]
mod platform_stop;

pub(in crate::store) async fn fixture(
    store: &MongoSandboxStore,
    id: &str,
) -> SandboxUnpublishedRecoveryRequest {
    let mut sandbox = active_sandbox(id);
    sandbox.status = SandboxStatus::Failed;
    sandbox.failure = Some(TypedFailure {
        class: "restore_failed".into(),
        detail: "native reopen incomplete".into(),
    });
    sandbox.lifecycle.home = Some(Box::new(home(&sandbox.id)));
    sandbox.lifecycle.runtime_generation_high_water = 2;
    let runtime = sandbox.runtime.as_mut().unwrap();
    runtime.runtime_id = "agent-sandbox:member:sandbox-uid".into();
    runtime.provision_generation = Some(2);
    runtime.boot_id = Some("b".repeat(32));
    sandbox.work.schedule(SandboxWorkStep::None, NOW.into());
    sandbox.validate().unwrap();
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let mut agent = ready_agent(&sandbox, &format!("agt_{id}"));
    agent.status = AgentStatus::Paused;
    agent.work.receipts.push(AgentWorkReceipt {
        step: AgentWorkStep::OpenConversation,
        operation_id: format!("agent:{}:open:v1", agent.id),
        request_sha256: "a".repeat(64),
        completed_at: NOW.into(),
    });
    agent
        .conversation
        .queued_messages
        .push(QueuedAgentMessage::new(2, "retained admitted input".into(), NOW.into()).unwrap());
    insert_agent(store, &agent, NOW).await;
    let snapshot = store
        .unpublished_runtime_recovery_snapshot(&sandbox.id)
        .await
        .unwrap();
    let proof = SandboxUnpublishedRecoveryProof {
        reserved_provision: None,
        observed_provision_generation: None,
        sandbox_id: sandbox.id.clone(),
        runtime_id: sandbox.runtime.as_ref().unwrap().runtime_id.clone(),
        runtime_generation: 2,
        home_identity: snapshot.home_identity().unwrap(),
        boot_id: sandbox.runtime.as_ref().unwrap().boot_id.clone(),
        runtime_process_absent: false,
        requires_native_quiescence: true,
        provider_plan: serde_json::json!({"inspection": "test-process-snapshot"}),
        provider_plan_sha256: SandboxUnpublishedRecoveryProof::provider_plan_digest(
            &serde_json::json!({"inspection": "test-process-snapshot"}),
        )
        .unwrap(),
        conversations: vec![SandboxRecoveryConversationProof {
            agent_id: agent.id.clone(),
            conversation_id: agent.conversation.conversation_id.clone().unwrap(),
            history_sha256: "c".repeat(64),
        }],
    };
    SandboxUnpublishedRecoveryRequest {
        snapshot,
        proof,
        target_runtime_version: "image-latest".into(),
        released_runtime: None,
        automatic: false,
        requested_at: NOW.into(),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mongo_unpublished_recovery_preserves_native_prefix_and_original_agent_record() {
    let (client, name, store) = database().await;
    let request = fixture(&store, "sbx_recovery_prefix").await;
    let agent = &request.snapshot.agents[0];
    let checkpoint = layer2_core::sandbox::store::AgentMessageCheckpoint {
        version: 1,
        update_id: uuid::Uuid::new_v4().to_string(),
        update_fingerprint: "a".repeat(64),
        native_conversation_id: agent.conversation.conversation_id.clone().unwrap(),
        runtime_generation: 2,
        active_turn: None,
        after_seq: agent.conversation.last_seq,
        assembly: AgentMessageAssembly::default(),
        outcome: AgentUpdateOutcome {
            appended: vec![],
            turn_closed: false,
        },
        output: Some(layer3_adapter::RuntimeOutputSource {
            native_conversation_id: agent.conversation.conversation_id.clone().unwrap(),
            stream_id: uuid::Uuid::new_v4().to_string(),
            cursor: 440,
            payload_sha256: "d".repeat(64),
        }),
    };
    checkpoint.validate().unwrap();
    let checkpoint = crate::store::sandbox_mongo::record_bson(&checkpoint).unwrap();
    store
        .agents
        .update_one(
            doc! { "_id": agent.id.as_str() },
            doc! { "$set": { "agent_message_checkpoint": checkpoint.clone() } },
        )
        .await
        .unwrap();
    let after = store
        .admit_unpublished_runtime_recovery(request.clone())
        .await
        .unwrap();
    assert_eq!(
        after.lifecycle.home,
        request.snapshot.sandbox.lifecycle.home
    );
    assert_eq!(
        after.lifecycle.suspended_runtime.as_deref(),
        request.snapshot.sandbox.runtime.as_ref()
    );
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap().as_ref(),
        Some(agent)
    );
    let row = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get("agent_message_checkpoint"), Some(&checkpoint));
    assert_eq!(
        store
            .agent_log
            .count_documents(doc! { "agent_id": agent.id.as_str() })
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .sandbox_events
            .count_documents(doc! { "sandbox_id": after.id.as_str() })
            .await
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mongo_unpublished_recovery_serializes_duplicate_admissions() {
    let (client, name, store) = database().await;
    let request = fixture(&store, "sbx_recovery_race").await;
    let (a, b) = tokio::join!(
        store.admit_unpublished_runtime_recovery(request.clone()),
        store.admit_unpublished_runtime_recovery(request.clone())
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert!(matches!(
        a.as_ref().err().or_else(|| b.as_ref().err()),
        Some(SandboxStoreError::Conflict)
    ));
    assert_eq!(
        store
            .sandbox_events
            .count_documents(doc! { "sandbox_id": request.snapshot.sandbox.id.as_str() })
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .get_agent(&request.snapshot.agents[0].id)
            .await
            .unwrap()
            .as_ref(),
        Some(&request.snapshot.agents[0])
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mongo_unpublished_recovery_refuses_a_concurrent_agent_revision() {
    let (client, name, store) = database().await;
    let request = fixture(&store, "sbx_recovery_stale").await;
    let agent = &request.snapshot.agents[0];
    store
        .agents
        .update_one(
            doc! { "_id": agent.id.as_str() },
            doc! { "$inc": { "revision": 1_i64, "record.revision": 1_i64 } },
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .admit_unpublished_runtime_recovery(request.clone())
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(
        store
            .get_sandbox(&request.snapshot.sandbox.id)
            .await
            .unwrap()
            .as_ref(),
        Some(&request.snapshot.sandbox)
    );
    assert_eq!(
        store
            .sandbox_events
            .count_documents(doc! { "sandbox_id": request.snapshot.sandbox.id.as_str() })
            .await
            .unwrap(),
        0
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mongo_unpublished_recovery_refuses_a_release_superseded_after_proof() {
    use crate::store::released_runtimes::MongoReleasedRuntimes;
    use layer2_core::released_runtime::{
        CandidateState, PromotionEvidence, ReleasedLane, ReleasedLaneRecord,
    };
    use layer3_adapter::{ReleasedLaneRevisions, ReleasedRuntime};

    let (client, name, mut store) = database().await;
    let lanes = MongoReleasedRuntimes::open(&client.database(&name), "recovery-test");
    lanes.initialize().await.unwrap();
    store.released_lanes = Some(lanes.clone());
    let image = format!("registry.example/runtime@sha256:{}", "a".repeat(64));
    let next = format!("registry.example/runtime@sha256:{}", "b".repeat(64));
    let first = ReleasedLaneRecord::bootstrap(ReleasedLane::Agent, image.clone(), NOW).unwrap();
    lanes.bootstrap(first.clone()).await.unwrap();
    let mut request = fixture(&store, "sbx_recovery_release").await;
    request.target_runtime_version = image.clone();
    request.released_runtime = Some(ReleasedRuntime {
        revision: first.active.revision,
        agent: image,
        machine: None,
        lane_revisions: Some(ReleasedLaneRevisions {
            agent: first.active.revision,
            machine: None,
        }),
    });

    let prepared = first
        .enable_following()
        .unwrap()
        .observe(next.clone(), "promotion".into(), NOW)
        .unwrap()
        .settle("promotion", CandidateState::Ready)
        .unwrap();
    lanes.replace(&first, prepared.clone(), None).await.unwrap();
    let evidence = PromotionEvidence {
        image: next,
        observation_id: "promotion".into(),
        lane_revision: prepared.revision,
    };
    let (promoted, transition) = prepared.promote(&evidence, NOW).unwrap();
    let mut session = lanes.session().await.unwrap();
    lanes
        .record_promotion(&prepared, promoted, transition, &mut session)
        .await
        .unwrap();
    session.commit_transaction().await.unwrap();

    let id = &request.snapshot.sandbox.id;
    assert!(matches!(
        store
            .admit_unpublished_runtime_recovery(request.clone())
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(
        store
            .unpublished_runtime_recovery_snapshot(id)
            .await
            .unwrap(),
        request.snapshot
    );
    client.database(&name).drop().await.unwrap();
}

pub(in crate::store) fn home(id: &layer2_core::SandboxId) -> SandboxHome {
    use layer3_adapter::agent_sandbox::*;
    let policy = HomeVolumePolicy {
        mode: HomeVolumeMode::NativeVctFullHomeV2,
        storage_class: "standard-rwo".into(),
        capacity_gib: 8,
        template_name: String::new(),
        template_generation: String::new(),
    };
    let mut home = SandboxHome::reserve(id, policy.clone()).unwrap();
    home.request.scope = Some(
        HomeVolumeScope::from_authority(
            "native-vct-empty-v1",
            &["attachment-test", "default", "runtime"],
        )
        .unwrap(),
    );
    let request = NativePoolClaimRequest {
        pool: NativePoolBinding {
            spec: NativePoolSpec {
                cold_canary: false,
                image: RuntimeImage {
                    uri: format!("registry.example/runtime@sha256:{}", "a".repeat(64)),
                    labels: std::collections::BTreeMap::from([
                        (RUNTIME_CONTRACT_LABEL.into(), "3".into()),
                        ("dev.chrono.cma.runtime-bootstrap".into(), "1".into()),
                        ("dev.chrono.cma.home-layout".into(), "2".into()),
                    ]),
                },
                cpu_millis: 1000,
                memory_mib: 2048,
                home: policy,
            },
            pod_policy: NativePoolPodPolicy {
                runtime_class: None,
                service_account: "runtime".into(),
                image_pull_secrets: vec![],
                template_network_policy: None,
                fs_group_change_policy: None,
            },
            name: "pool".into(),
            uid: "pool-uid".into(),
            template_name: "template".into(),
            template_uid: "template-uid".into(),
            config_sha256: "a".repeat(64),
        },
        home: home.request.clone(),
        runtime_generation: 1,
    };
    let claim = NativePoolClaim {
        request: request.clone(),
        name: "claim".into(),
        uid: "claim-uid".into(),
        resource_version: "1".into(),
        sandbox_name: Some("member".into()),
    };
    home.native_pool = Some(Box::new(SandboxNativePool {
        request,
        claim: Some(claim.clone()),
        cleanup: None,
        cleaned: false,
    }));
    home.bind(HomeVolumeReceipt {
        native_authority: Some(Box::new(NativePoolHomeOrigin {
            claim,
            sandbox_uid: "sandbox-uid".into(),
            pvc_uid: "pvc-uid".into(),
        })),
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "home-member".into(),
        storage_generation: "pvc-uid".into(),
    })
    .unwrap();
    home
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn mongo_no_agent_stalled_revival_keeps_retained_source_and_reserved_generation_floor() {
    use layer2_core::sandbox::store::{
        SandboxReconciliationStore, SandboxRuntimeLossRequest, SandboxWorkerStore,
    };
    let (client, name, store) = database().await;
    let mut sandbox = active_sandbox("sbx_empty_stalled_revival");
    sandbox.lifecycle.home = Some(Box::new(home(&sandbox.id)));
    let binding = sandbox.runtime.as_mut().unwrap();
    binding.runtime_id = "agent-sandbox:member:sandbox-uid".into();
    binding.boot_id = Some("b".repeat(32));
    sandbox.validate().unwrap();
    store.create_sandbox(sandbox.clone()).await.unwrap();
    store
        .recover_runtime_loss(SandboxRuntimeLossRequest {
            sandbox_id: sandbox.id.clone(),
            expected_revision: sandbox.revision,
            expected_runtime: sandbox.runtime.clone(),
            native_retirement_finality: false,
            retain_runtime: true,
            interruption_cause: Default::default(),
            observed_at: NOW.into(),
        })
        .await
        .unwrap();
    for phase in 0..3 {
        let claimed = store
            .claim_sandbox_work(&sandbox.id, "fixture", NOW.into(), work_lease_deadline())
            .await
            .unwrap()
            .unwrap();
        let fence = claimed.work.lease.as_ref().unwrap().fence;
        let mut next = claimed.clone();
        match phase {
            0 => next
                .complete_lifecycle_home_verification(
                    claimed.retained_home_recovery().unwrap(),
                    "fixture",
                    fence,
                    NOW.into(),
                )
                .unwrap(),
            1 => {
                next.reserve_lifecycle_runtime_generation(2, "fixture", fence, NOW.into())
                    .unwrap();
            }
            _ => next
                .retry_lifecycle(NOW.into(), "fixture", fence, NOW.into())
                .unwrap(),
        }
        store.save_sandbox(claimed.revision, next).await.unwrap();
    }
    let snapshot = store
        .unpublished_runtime_recovery_snapshot(&sandbox.id)
        .await
        .unwrap();
    snapshot.validate().unwrap();
    let provider_plan = serde_json::json!({"original": "nonexecuting-provider-proof"});
    let reservation = snapshot.recovery_provision_reservation().unwrap().cloned();
    let proof = SandboxUnpublishedRecoveryProof {
        reserved_provision: reservation.clone(),
        observed_provision_generation: None,
        sandbox_id: sandbox.id.clone(),
        runtime_id: sandbox.runtime.as_ref().unwrap().runtime_id.clone(),
        runtime_generation: 1,
        home_identity: snapshot.home_identity().unwrap(),
        boot_id: None,
        runtime_process_absent: true,
        requires_native_quiescence: true,
        provider_plan_sha256: SandboxUnpublishedRecoveryProof::provider_plan_digest(&provider_plan)
            .unwrap(),
        provider_plan,
        conversations: vec![],
    };
    let admitted = store
        .admit_unpublished_runtime_recovery(SandboxUnpublishedRecoveryRequest {
            snapshot,
            proof,
            target_runtime_version: "released-image".into(),
            released_runtime: None,
            automatic: false,
            requested_at: NOW.into(),
        })
        .await
        .unwrap();
    assert_eq!(
        admitted.lifecycle.suspended_runtime.as_deref(),
        sandbox.runtime.as_ref()
    );
    assert_eq!(admitted.lifecycle.home, sandbox.lifecycle.home);
    assert_eq!(admitted.lifecycle.runtime_generation_high_water, 2);
    assert_eq!(
        admitted.retained_home_recovery().unwrap().source_generation,
        1
    );
    assert!(admitted.lifecycle.runtime_provision.is_none());
    assert_eq!(
        admitted
            .lifecycle
            .unpublished_recovery
            .as_ref()
            .unwrap()
            .proof
            .reserved_provision,
        reservation
    );
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap().unwrap(),
        admitted
    );
    client.database(&name).drop().await.unwrap();
}

mod evidence_tests;
