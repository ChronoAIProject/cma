use super::*;
use crate::sandbox::ownership_retry_hint::{
    AdvisoryDecision, HintWriteResult, OwnershipRetryHints,
};
use crate::store::agent_failure_pacing::ownership::{record::TestObservation, FIELD as HINT};
use layer2_core::agent_ownership::{
    Authority, Member, OwnershipCoordinator, OwnershipStore, DEFAULT_MAX_OWNED_AGENTS,
};
use mongodb::bson::DateTime;

pub(super) fn member(id: &str) -> Member {
    Member {
        instance_id: id.into(),
        protocol: 1,
        relay_address: "http://fixture.invalid:8080".into(),
        expires_at_ms: 0,
        draining: false,
        owned_agents: 0,
        max_owned_agents: DEFAULT_MAX_OWNED_AGENTS,
    }
}

pub(super) async fn bind_runtime(
    store: &MongoSandboxStore,
    agent: &Agent,
    handle: &str,
) -> layer3_adapter::HomeVolumeAttachment {
    let mut parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let revision = parent.revision;
    parent.runtime = Some(layer2_core::sandbox::RuntimeBinding {
        runtime_id: handle.into(),
        boot_id: Some("a".repeat(32)),
        runtime_version: "image-v1".into(),
        provision_generation: Some(1),
        bound_at: NOW.into(),
    });
    parent.lifecycle.runtime_generation_high_water = 1;
    let mut home = layer2_core::sandbox::SandboxHome::reserve(
        &parent.id,
        layer3_adapter::HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "fixture".into(),
            capacity_gib: 64,
            template_name: "fixture-seed".into(),
            template_generation: "generation-one".into(),
        },
    )
    .unwrap();
    home.bind(layer3_adapter::HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "fixture-home".into(),
        storage_generation: "original".into(),
    })
    .unwrap();
    let attachment = home.attachment().unwrap();
    parent.lifecycle.home = Some(Box::new(home));
    parent.revision += 1;
    // Seed retained runtime state before enabling ownership; ordinary updates
    // deliberately cannot introduce a runtime binding outside lifecycle work.
    let fields = crate::store::sandboxes::sandbox_replacement_fields(&parent).unwrap();
    assert_eq!(
        store
            .sandboxes
            .update_one(
                doc! {"_id": parent.id.as_str(), "revision": revision as i64},
                doc! {"$set": fields}
            )
            .await
            .unwrap()
            .matched_count,
        1
    );
    assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(parent));
    attachment
}

pub(super) async fn fixture(
    suffix: &str,
) -> (
    Client,
    String,
    MongoSandboxStore,
    Agent,
    Arc<Authority>,
    TestObservation,
) {
    let (client, name) = database(suffix).await;
    let mut store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = ready_conversation_fixture(&store, suffix).await;
    bind_runtime(&store, &agent, "runtime").await;
    store = store
        .enable_agent_ownership("manager".into())
        .await
        .unwrap()
        .with_ownership_retry_hints(true);
    let manager = OwnershipCoordinator::register(
        store.agent_ownership.as_ref().unwrap().clone(),
        member("manager"),
    )
    .await
    .unwrap();
    let authority = manager.acquire(&agent.id).await.unwrap().unwrap();
    let now = SystemTime::now();
    let claimed = authority
        .run(store.claim_agent_work(
            &agent.id,
            "worker",
            layer2_core::rfc3339_at(now),
            layer2_core::rfc3339_at(now + Duration::from_secs(600)),
        ))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let captured = TestObservation::capture(&store, &claimed, &authority.token).await;
    (client, name, store, claimed, authority, captured)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ownership_retry_hint_survives_restart_and_epoch_change_without_sliding_deadline() {
    let (client, name, store, claimed, authority, observed) = fixture("retry_restart").await;
    assert_eq!(observed.write().await, HintWriteResult::Recorded);
    let before = store
        .agents
        .find_one(doc! {"_id": claimed.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed.write().await, HintWriteResult::AlreadyPresent);
    assert_eq!(
        store.get_agent(&claimed.id).await.unwrap().unwrap(),
        claimed
    );
    store
        .agent_ownership
        .as_ref()
        .unwrap()
        .release(&authority.token)
        .await
        .unwrap();
    drop(authority);
    let independent = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let mut restarted = MongoSandboxStore::open(&independent, &name)
        .await
        .unwrap()
        .enable_agent_ownership("successor".into())
        .await
        .unwrap()
        .with_ownership_retry_hints(true);
    assert_eq!(
        restarted.preflight(&claimed.id).await,
        AdvisoryDecision::Hold
    );
    assert_eq!(
        restarted
            .agents
            .find_one(doc! {"_id": claimed.id.as_str()})
            .await
            .unwrap()
            .unwrap()
            .get(HINT),
        before.get(HINT)
    );
    let until = *observed.document().get_datetime("until").unwrap();
    restarted.ownership_retry_hint_now = Some(until);
    assert_eq!(
        restarted.preflight(&claimed.id).await,
        AdvisoryDecision::Proceed
    );
    assert_eq!(observed.write().await, HintWriteResult::Stale);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ownership_retry_hint_wakes_for_binding_admission_and_stop_but_not_bookkeeping() {
    let (client, name, store, agent, _, observed) = fixture("retry_wakes").await;
    assert_eq!(observed.write().await, HintWriteResult::Recorded);
    for (field, value, holds) in [
        (
            "record.revision",
            Bson::Int64(agent.revision as i64 + 1),
            true,
        ),
        ("agent_command_admission_fence", Bson::Int64(1), false),
        (
            "record.requested_stop_reason",
            Bson::String("sandbox_retired".into()),
            false,
        ),
        ("record.work.attempts", Bson::Int32(1), false),
    ] {
        let original = store
            .agents
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        let mut fields = doc! {field: value.clone()};
        if field == "record.revision" {
            fields.insert("revision", value);
        }
        store
            .agents
            .update_one(doc! {"_id": agent.id.as_str()}, doc! {"$set": fields})
            .await
            .unwrap();
        assert_eq!(
            store.preflight(&agent.id).await,
            if holds {
                AdvisoryDecision::Hold
            } else {
                AdvisoryDecision::Proceed
            },
            "{field}"
        );
        store
            .agents
            .replace_one(doc! {"_id": agent.id.as_str()}, original)
            .await
            .unwrap();
    }
    for (field, value) in [
        ("record.runtime.boot_id", Bson::String("b".repeat(32))),
        (
            "record.runtime.runtime_version",
            Bson::String("replacement".into()),
        ),
        ("record.runtime.provision_generation", Bson::Int64(2)),
        (
            "record.runtime.bound_at",
            Bson::String("2026-09-06T00:00:00Z".into()),
        ),
        (
            "record.lifecycle.runtime_generation_high_water",
            Bson::Int64(2),
        ),
    ] {
        let original = store
            .sandboxes
            .find_one(doc! {"_id": agent.sandbox_id.as_str()})
            .await
            .unwrap()
            .unwrap();
        let mut fields = doc! {field: value.clone()};
        if field == "record.runtime.runtime_version" {
            fields.insert("runtime_version", value);
        } else if field == "record.runtime.provision_generation" {
            fields.insert("record.lifecycle.runtime_generation_high_water", value);
        }
        store
            .sandboxes
            .update_one(
                doc! {"_id": agent.sandbox_id.as_str()},
                doc! {"$set": fields},
            )
            .await
            .unwrap();
        assert_eq!(
            store.preflight(&agent.id).await,
            AdvisoryDecision::Proceed,
            "{field}"
        );
        store
            .sandboxes
            .replace_one(doc! {"_id": agent.sandbox_id.as_str()}, original)
            .await
            .unwrap();
    }
    let mut expired = store.clone();
    expired.ownership_retry_hint_now = Some(DateTime::from_millis(
        observed
            .document()
            .get_datetime("until")
            .unwrap()
            .timestamp_millis(),
    ));
    assert_eq!(
        expired.preflight(&agent.id).await,
        AdvisoryDecision::Proceed
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ownership_retry_hint_capture_rejects_later_agent_parent_and_owner_changes() {
    for changed in ["admission", "parent", "owner", "drain", "marker"] {
        let (client, name, store, agent, authority, observed) =
            fixture(&format!("retry_stale_{changed}")).await;
        match changed {
            "admission" => {
                store
                    .agents
                    .update_one(
                        doc! {"_id": agent.id.as_str()},
                        doc! {"$inc": {"agent_command_admission_fence": 1_i64}},
                    )
                    .await
                    .unwrap();
            }
            "parent" => {
                store
                    .sandboxes
                    .update_one(
                        doc! {"_id": agent.sandbox_id.as_str()},
                        doc! {"$inc": {"revision": 1_i64, "record.revision": 1_i64}},
                    )
                    .await
                    .unwrap();
            }
            "owner" => {
                store
                    .agent_ownership
                    .as_ref()
                    .unwrap()
                    .release(&authority.token)
                    .await
                    .unwrap();
            }
            "drain" => {
                store
                    .agent_ownership
                    .as_ref()
                    .unwrap()
                    .heartbeat("manager", true)
                    .await
                    .unwrap();
            }
            "marker" => {
                store
                    .sandboxes
                    .update_one(
                        doc! {"_id": agent.sandbox_id.as_str()},
                        doc! {"$set": {"runtime_writer_claim_fence": i64::MAX}},
                    )
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert_eq!(observed.write().await, HintWriteResult::Stale, "{changed}");
        assert!(store
            .agents
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .unwrap()
            .unwrap()
            .get(HINT)
            .is_none());
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ownership_retry_hint_new_command_wakes_both_commit_orderings_and_replay_retains_hold() {
    use crate::store::sandbox_mongo_integration_tests::command_ledger_tests::request;
    use layer2_core::sandbox::agent_io::{AgentCommandStore, CommandReservationResult};
    for hint_first in [false, true] {
        let (client, name, store, agent, authority, mut observed) =
            fixture("retry_command_race").await;
        let parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
        let mut command = request(&agent, &parent, "new-command");
        command.admitted_at = layer2_core::rfc3339_at(SystemTime::now());
        command.queue_deadline =
            layer2_core::rfc3339_at(SystemTime::now() + Duration::from_secs(600));
        let pending = if hint_first {
            assert_eq!(observed.write().await, HintWriteResult::Recorded);
            None
        } else {
            let gate = observed.pause_at("parent_read");
            let writer = tokio::spawn(async move { observed.write().await });
            tokio::time::timeout(Duration::from_secs(1), gate.arrived.notified())
                .await
                .unwrap();
            Some((gate, writer))
        };
        assert!(matches!(
            store.reserve_command(command.clone()).await.unwrap(),
            CommandReservationResult::Created(_)
        ));
        if let Some((gate, writer)) = pending {
            gate.resume.notify_one();
            assert_eq!(writer.await.unwrap(), HintWriteResult::Stale);
        }
        assert_eq!(store.preflight(&agent.id).await, AdvisoryDecision::Proceed);
        assert_eq!(
            store.get_agent(&agent.id).await.unwrap(),
            Some(agent.clone())
        );
        let captured = TestObservation::capture(&store, &agent, &authority.token).await;
        assert_eq!(captured.write().await, HintWriteResult::Recorded);
        let hint = store
            .agents
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .unwrap()
            .unwrap()
            .get(HINT)
            .cloned();
        assert!(matches!(
            store.reserve_command(command).await.unwrap(),
            CommandReservationResult::Existing(_)
        ));
        assert_eq!(store.preflight(&agent.id).await, AdvisoryDecision::Hold);
        assert_eq!(
            store
                .agents
                .find_one(doc! {"_id": agent.id.as_str()})
                .await
                .unwrap()
                .unwrap()
                .get(HINT),
            hint.as_ref()
        );
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ownership_retry_hint_expired_observation_never_renews_or_overwrites_newer_failure() {
    let (client, name, mut store, agent, authority, mut observed) =
        fixture("retry_fixed_interval").await;
    assert_eq!(observed.write().await, HintWriteResult::Recorded);
    let until = *observed.document().get_datetime("until").unwrap();
    observed.policy_time(until);
    assert_eq!(observed.write().await, HintWriteResult::Stale);
    store.ownership_retry_hint_now = Some(until);
    let fresh = TestObservation::capture(&store, &agent, &authority.token).await;
    assert_eq!(fresh.write().await, HintWriteResult::Recorded);
    assert_eq!(
        fresh
            .document()
            .get_datetime("until")
            .unwrap()
            .timestamp_millis(),
        until.timestamp_millis() + 60_000
    );
    assert_eq!(observed.write().await, HintWriteResult::Stale);
    assert_eq!(fresh.write().await, HintWriteResult::AlreadyPresent);
    assert_eq!(
        store
            .agents
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .unwrap()
            .unwrap()
            .get_document(HINT)
            .unwrap(),
        &fresh.document()
    );
    client.database(&name).drop().await.unwrap();
}
