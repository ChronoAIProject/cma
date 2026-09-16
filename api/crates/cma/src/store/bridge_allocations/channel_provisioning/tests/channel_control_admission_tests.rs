use super::*;
use crate::store::sandbox_mongo::record_bson;
use layer2_core::bridge::channel_control::*;
use layer2_core::bridge::invoke::*;
use mongodb::bson::Document;

#[path = "channel_control_admission_tests/fixture.rs"]
pub(super) mod fixture;
#[path = "channel_control_admission_tests/http.rs"]
mod http;
#[path = "channel_control_admission_tests/v2_tests.rs"]
mod v2;
#[path = "channel_control_admission_tests/worker.rs"]
mod worker;
use fixture::ControlFixture;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_admission_holds_cancel_and_steer_in_common_ledger_without_message_authority(
) {
    let c = ControlFixture::new().await;
    let before =
        c.f.resources
            .get_command(&c.f.command.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
    let first_request = c.request("cancel-source", 77, false).await;
    assert!(c
        .f
        .resources
        .reserve_command(first_request.command().clone())
        .await
        .is_err());
    let first = c.store.reserve(first_request).await.unwrap();
    let second = c
        .store
        .reserve(c.request("steer-source", 78, true).await)
        .await
        .unwrap();
    for result in [&first, &second] {
        result.validate().unwrap();
        assert_eq!(result.command.status, AgentResponseStatus::WaitingAction);
        assert_eq!(result.command.work, CommandWorkState::default());
        assert!(result.command.receipt.response_id.is_none());
        assert!(!result.command.awaiting_claim(&layer2_core::now_rfc3339()));
        assert!(!result.command.holds_message_order());
        let row =
            c.f.client
                .database(&c.f.database)
                .collection::<Document>("agent_commands")
                .find_one(doc! {"_id":result.sidecar.command_id.as_str()})
                .await
                .unwrap()
                .unwrap();
        assert!(!row.contains_key("channel_admission"));
        assert!(!row.contains_key("channel_source"));
        let old = crate::store::agent_commands::read_command(&row).unwrap();
        assert_eq!(old, result.command);
    }
    assert_eq!(
        second.command.receipt.agent_order,
        first.command.receipt.agent_order + 1
    );
    worker::prove_old_worker_holds(&c, &first).await;
    worker::prove_old_worker_holds(&c, &second).await;
    assert_eq!(
        c.f.resources
            .get_command(&c.f.command.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert_eq!(c.f.items().await.len(), 1);
    let session = c.f.tools.authenticate(&c.f.credential).await.unwrap();
    assert_eq!(
        c.f.replies.control_context(&session).await.unwrap(),
        Some(c.reference.reference.clone())
    );
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_admission_reference_fence_accepts_only_the_named_nonnegative_i64() {
    let c = ControlFixture::new().await;
    c.reference
        .binding()
        .require_allocation(&c.f.allocation)
        .unwrap();
    let mut changed_placement = c.f.allocation.clone();
    let AllocationPlacement::NewSandbox { profiles, .. } = &mut changed_placement.placement else {
        panic!("fixture must create its sandbox");
    };
    profiles.approved_by = layer2_core::resource_access::ResourceActor::Human {
        subject: layer2_core::ids::SubjectId("different-profile-approver".into()),
    };
    changed_placement.validate().unwrap();
    assert_eq!(
        c.reference.binding().require_allocation(&changed_placement),
        Err(layer2_core::bridge::outbox::OutboxError::SourceMismatch)
    );
    c.store
        .reserve(c.request("reference-fence", 84, false).await)
        .await
        .unwrap();
    let session = c.f.tools.authenticate(&c.f.credential).await.unwrap();
    let rows =
        c.f.client
            .database(&c.f.database)
            .collection::<Document>("bridge_channel_control_references");
    let original = rows
        .find_one(doc! {"_id":&c.reference.reference})
        .await
        .unwrap()
        .unwrap();
    assert!(original.get_i64("control_admission_fence").unwrap() >= 1);
    for value in [
        mongodb::bson::Bson::Int32(1),
        mongodb::bson::Bson::Int64(-1),
        mongodb::bson::Bson::String("1".into()),
    ] {
        rows.update_one(
            doc! {"_id":&c.reference.reference},
            doc! {"$set":{"control_admission_fence":value}},
        )
        .await
        .unwrap();
        assert!(c.f.replies.control_context(&session).await.is_err());
        rows.replace_one(doc! {"_id":&c.reference.reference}, original.clone())
            .await
            .unwrap();
    }
    assert_eq!(
        c.f.replies.control_context(&session).await.unwrap(),
        Some(c.reference.reference.clone())
    );
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_admission_held_controls_count_against_common_queue_capacity() {
    let c = ControlFixture::new().await;
    let first = c.request("capacity-0", 90, false).await;
    let capacity = first.command().queue_capacity;
    assert!((1..=64).contains(&capacity));
    c.store.reserve(first).await.unwrap();
    for index in 1..capacity {
        c.store
            .reserve(
                c.request(&format!("capacity-{index}"), 90 + u64::from(index), false)
                    .await,
            )
            .await
            .unwrap();
    }
    assert!(matches!(
        c.store
            .reserve(c.request("capacity-full", 190, false).await)
            .await,
        Err(ChannelControlError::Conflict)
    ));
    let rows =
        c.f.client
            .database(&c.f.database)
            .collection::<Document>("bridge_channel_control_intents");
    assert_eq!(
        rows.count_documents(doc! {}).await.unwrap(),
        u64::from(capacity)
    );
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable failpoint-enabled MongoDB replica contract runner"]
async fn channel_control_admission_unknown_commit_retains_original_command_intent_and_order() {
    let c = ControlFixture::new().await;
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let mut options = mongodb::options::ClientOptions::parse(&uri).await.unwrap();
    let application = format!("control-admission-{}", uuid::Uuid::new_v4().simple());
    options.app_name = Some(application.clone());
    let client = mongodb::Client::with_options(options).unwrap();
    let resources = Arc::new(
        crate::store::MongoSandboxStore::open(&client, &c.f.database)
            .await
            .unwrap(),
    );
    let bridges = Arc::new(
        crate::store::MongoBridgeStore::open(
            &client,
            &c.f.database,
            "fixture-deployment",
            &custody_key(),
        )
        .await
        .unwrap(),
    );
    let invokes = Arc::new(
        crate::store::MongoBridgeInvokeStore::open(bridges.clone(), resources, &c.f.database)
            .await
            .unwrap(),
    );
    let callbacks = Arc::new(
        crate::store::MongoCallbackInbox::open(
            &bridges,
            &c.f.database,
            "fixture-deployment",
            &custody_key(),
        )
        .await
        .unwrap(),
    );
    let store = crate::store::bridge_invokes::channel_controls::MongoChannelControlStore::open(
        invokes, callbacks,
    )
    .await
    .unwrap();
    let request = ChannelControlReservation::prepare(
        c.proof(
            c.write_with_lease(
                "ambiguous-admission",
                85,
                12345,
                true,
                false,
                Duration::from_secs(1),
            )
            .await,
        )
        .await
        .unwrap(),
    )
    .unwrap();
    let intent = request.proof().intent().clone();
    c.f.client.database("admin").run_command(doc! {"configureFailPoint":"failCommand","mode":"alwaysOn",
        "data":{"failCommands":["commitTransaction"],"appName":&application,"blockConnection":true,"blockTimeMS":300_i32,
        "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"unknown control admission"}}}).await.unwrap();
    let result = store.reserve(request).await;
    c.f.client
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(matches!(result, Err(ChannelControlError::Unavailable)));
    let recovered = c
        .store
        .recover(
            &c.write("ambiguous-admission-recovery", 85, 12345, true, false)
                .await,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(recovered.sidecar.intent == intent);
    assert_eq!(
        c.f.client
            .database(&c.f.database)
            .collection::<Document>("bridge_channel_control_intents")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        c.f.client
            .database(&c.f.database)
            .collection::<Document>("agent_commands")
            .count_documents(doc! {"channel_control_admission":{"$exists":true}})
            .await
            .unwrap(),
        1
    );
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_admission_requires_original_sender_group_reply_and_consistent_retained_identity(
) {
    let c = ControlFixture::new().await;
    for (sender, reply) in [(98765, true), (12345, false)] {
        let write = c.write("foreign-control", 80, sender, reply, false).await;
        assert!(matches!(
            c.proof(write).await,
            Err(ChannelControlError::SourceUnavailable)
        ));
    }
    let request = c.request("valid-control", 81, false).await;
    let intent = request.proof().intent();
    let mut inconsistent = intent.clone();
    inconsistent.callback.bot_id = "another-bot".into();
    assert!(inconsistent.validate().is_err());
    let mut inconsistent = intent.clone();
    inconsistent.callback.pin.config_revision += 1;
    assert!(inconsistent.validate().is_err());
    let mut refreshed = intent.clone();
    refreshed.expected_agent_revision += 1;
    assert_eq!(
        request.proof().require_intent(&refreshed),
        Err(ChannelControlError::Conflict)
    );
    assert_eq!(
        c.f.client
            .database(&c.f.database)
            .collection::<Document>("bridge_channel_control_intents")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_admission_revocation_after_proof_rolls_back_order_and_intent() {
    let c = ControlFixture::new().await;
    let request = c.request("revoked-control", 82, false).await;
    let orders =
        c.f.client
            .database(&c.f.database)
            .collection::<Document>("agent_command_orders");
    let before = orders
        .find_one(doc! {"_id":c.f.command.receipt.envelope.agent_id.as_str()})
        .await
        .unwrap();
    let bridge =
        c.f.bridges
            .bridge(&c.f.inbox.pin.bridge_id)
            .await
            .unwrap()
            .unwrap();
    BridgeSourceControlService::new(c.f.invokes.clone(), c.invokes.clone())
        .submit(
            &caller(),
            None,
            &c.f.grant.id,
            "revoke-control",
            SourceControlRequest {
                expected_bridge_revision: bridge.revision,
                expected_grant_revision: 1,
                expected_policy_revision: 1,
                action: SourceControlAction::RevokeSenderControls,
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        c.store.reserve(request).await,
        Err(ChannelControlError::AuthorityUnavailable)
    ));
    assert_eq!(
        orders
            .find_one(doc! {"_id":c.f.command.receipt.envelope.agent_id.as_str()})
            .await
            .unwrap(),
        before
    );
    for name in [
        "bridge_channel_control_intents",
        "bridge_channel_control_aliases",
    ] {
        assert_eq!(
            c.f.client
                .database(&c.f.database)
                .collection::<Document>(name)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    }
    c.f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_control_admission_provider_alias_recovers_original_after_source_finishes() {
    let c = ControlFixture::new().await;
    let original = c
        .store
        .reserve(c.request("first-control", 83, false).await)
        .await
        .unwrap();
    c.f.finish().await;
    let alias = c.write("new-nyxid-callback", 83, 12345, true, false).await;
    let recovered = c.store.recover(&alias).await.unwrap().unwrap();
    assert!(recovered == original);
    assert_eq!(
        c.f.client
            .database(&c.f.database)
            .collection::<Document>("bridge_channel_control_intents")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        c.f.client
            .database(&c.f.database)
            .collection::<Document>("bridge_channel_control_aliases")
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        recovered.sidecar.intent.expected_agent_revision,
        original.sidecar.intent.expected_agent_revision
    );
    assert_eq!(
        recovered.sidecar.intent.captured_at,
        original.sidecar.intent.captured_at
    );
    c.f.close().await;
}
